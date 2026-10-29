use crate::Result;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// Reject DB/sidecar aliases before opening a non-SQLite descriptor. Closing
/// even a read-only alias can release this process's POSIX SQLite locks.
/// This stat check does not protect against a concurrent path replacement.
pub fn reject_database_file(path: &Path, db: &Path) -> Result<()> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for suffix in ["", "-wal", "-shm"] {
        let mut name = db.as_os_str().to_owned();
        name.push(suffix);
        let other = match std::fs::metadata(Path::new(&name)) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        #[cfg(unix)]
        let same = {
            use std::os::unix::fs::MetadataExt;
            meta.dev() == other.dev() && meta.ino() == other.ino()
        };
        #[cfg(not(unix))]
        let same = { let _ = (&meta, &other); path.canonicalize()? == Path::new(&name).canonicalize()? };
        if same {
            return Err(crate::CoreError::BadInput(
                "DB・WAL・SHM は添付や結果保存先に指定できません".into(),
            ));
        }
    }
    Ok(())
}

pub fn reject_connection_file(conn: &Connection, path: &Path) -> Result<()> {
    // Includes attached databases, if a caller uses them.
    let mut stmt = conn.prepare("PRAGMA database_list")?;
    for db in stmt.query_map([], |r| r.get::<_, String>(2))? {
        let db = db?;
        if !db.is_empty() {
            reject_database_file(path, Path::new(&db))?;
        }
    }
    Ok(())
}

pub struct Db {
    pub conn: Connection,
    /// DB ファイルパス（in-memory は None）。managed storage の位置決めに使う。
    pub path: Option<std::path::PathBuf>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?, Some(path.to_path_buf()))
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    /// DB の置き場（Application Support/OpenRef）。storage/・certs/・styles/ がこの下
    pub fn app_dir(&self) -> Option<std::path::PathBuf> {
        Some(self.path.as_ref()?.parent()?.to_path_buf())
    }

    /// managed 添付の保存先（DB と同階層の `storage/`）
    pub fn storage_root(&self) -> Option<std::path::PathBuf> {
        Some(self.app_dir()?.join("storage"))
    }

    /// settings テーブルの読み出し（未設定は None）
    pub fn setting(&self, key: &str) -> Option<String> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// PDF 添付の書込先モード。'drive' = Drive 互換レイアウト / それ以外 = managed storage
    pub fn pdf_write_mode(&self) -> String {
        self.setting("pdf_write_mode").unwrap_or_else(|| "managed".into())
    }

    fn init(conn: Connection, path: Option<std::path::PathBuf>) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current > SCHEMA_VERSION {
            return Err(crate::CoreError::BadInput(
                "この DB は新しい OpenRef で作成されています。対応する版を使ってください".into(),
            ));
        }
        let db = Db { conn, path };
        // Preserve the old schema before any journal-mode or schema changes.
        if current > 0 && current < MIGRATIONS.len() as i64 {
            crate::backup::snapshot(&db, "pre-schema-", 5)?;
        }
        let Db { conn, path } = db;
        // journal_mode は行を返すので query で実行する（pragma_update は不可）
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut db = Db { conn, path };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let current: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let v = (i + 1) as i64;
            if v > current {
                let tx = self.conn.transaction()?;
                tx.execute_batch(sql)?;
                if v == 2 {
                    backfill_fts_v2(&tx)?;
                }
                if v == 6 {
                    backfill_journal_keys_v6(&tx)?;
                }
                if v == 7 {
                    backfill_fulltext_fts_v7(&tx)?;
                }
                if v == 10 {
                    crate::import::plan::create_identity_indexes(&tx)?;
                }
                tx.pragma_update(None, "user_version", v)?;
                tx.commit()?;
            }
        }
        Ok(())
    }
}

/// v2: FTS 再構築後の backfill（cjk 列は SQL では計算できないため Rust 側で全 item を再索引）
fn backfill_fts_v2(conn: &rusqlite::Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id, csl FROM items")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    for (id, csl_s) in rows {
        if let Ok(csl) = serde_json::from_str::<serde_json::Value>(&csl_s) {
            crate::ingest::index_fts(conn, id, &csl)?;
        }
    }
    Ok(())
}

/// v6: 誌名識別キー（journal_norm は Rust 側なので全 item を Rust で索引し直す）
fn backfill_journal_keys_v6(conn: &rusqlite::Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id, csl FROM items")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    for (id, csl_s) in rows {
        if let Ok(csl) = serde_json::from_str::<serde_json::Value>(&csl_s) {
            crate::ingest::index_journal(conn, id, &csl)?;
        }
    }
    Ok(())
}

const MIGRATIONS: &[&str] = &[
    SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8,
    SCHEMA_V9, SCHEMA_V10,
];

/// Import provenance stays outside CSL/FTS. Target keys deliberately have no FK:
/// a purged target leaves a tombstone, preventing accidental resurrection.
const SCHEMA_V10: &str = r#"
ALTER TABLE tags ADD COLUMN import_key TEXT;
CREATE UNIQUE INDEX idx_tags_import_key ON tags(import_key) WHERE import_key IS NOT NULL;
CREATE TABLE import_sources (
  id TEXT PRIMARY KEY,
  format TEXT NOT NULL,
  date_added TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE import_runs (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES import_sources(id),
  input_sha256 TEXT NOT NULL,
  adapter_version TEXT NOT NULL,
  attachment_mode TEXT NOT NULL CHECK (attachment_mode IN ('copy','link')),
  state TEXT NOT NULL CHECK (state IN ('running','finished','cancelled','interrupted','failed')),
  input_json TEXT NOT NULL CHECK (json_valid(input_json)),
  report_json TEXT,
  backup_path TEXT,
  date_added TEXT NOT NULL DEFAULT (datetime('now')),
  date_finished TEXT
);
CREATE TABLE import_objects (
  source_id TEXT NOT NULL REFERENCES import_sources(id),
  kind TEXT NOT NULL,
  scope TEXT NOT NULL,
  object_id TEXT NOT NULL,
  target_key TEXT NOT NULL,
  raw_json TEXT NOT NULL CHECK (json_valid(raw_json)),
  content_sha256 TEXT NOT NULL,
  PRIMARY KEY (source_id,kind,scope,object_id)
);
CREATE INDEX idx_import_objects_target ON import_objects(kind,target_key);
CREATE TABLE import_results (
  run_id TEXT NOT NULL REFERENCES import_runs(id),
  ordinal INTEGER NOT NULL,
  result_json TEXT NOT NULL CHECK (json_valid(result_json)),
  PRIMARY KEY (run_id,ordinal)
);
CREATE TABLE import_files (
  run_id TEXT NOT NULL REFERENCES import_runs(id),
  key TEXT PRIMARY KEY,
  path TEXT NOT NULL,
  staging_path TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  owns_staging INTEGER NOT NULL DEFAULT 0,
  file_device TEXT,
  file_inode TEXT,
  state TEXT NOT NULL CHECK (state IN ('planned','ready','attached','cleaned'))
);
"#;

/// v9: `attachments.doi_check` — 自動取得（pdffetch の DOI 起点連鎖）した PDF の同一性照合の結果を添付行に永続化する。
/// 'match' = 冒頭 2 頁に目的 DOI があった / 'unknown' = DOI で照合を試みたが本文に目的 DOI も別 DOI も無く、未確認のまま添付
/// （出版社が宣言した所在・`/capture/attachment`）/ NULL = 未照合（手動ドロップ・旧添付・item に DOI が無い・本文を抽出できない・
/// PMC 経路の Unknown = PMCID + json 照合で同一性が担保される）。GUI は 'unknown' のときだけ「DOI 未確認」を出す。
/// 既存の未使用列 `edition` は版種（author manuscript / 出版社版）用に予約されており意味が別なので流用しない。
/// 旧バイナリは列を参照しないので共存可（ALTER TABLE ADD COLUMN のみ）。
const SCHEMA_V9: &str = r#"
ALTER TABLE attachments ADD COLUMN doi_check TEXT;
"#;

/// v8: `INSERT OR REPLACE INTO fulltext`（v7 より前の MCP バイナリの get_or_extract が行う）への防御。
/// REPLACE が既存行を暗黙に消すとき AFTER DELETE トリガーは発火しない（recursive_triggers 既定 off）ので、
/// external content の索引に旧本文が残って壊れる（integrity-check が malformed を返す）。
/// BEFORE INSERT で「同じ attachment_id の行が既にあれば先に索引から消す」。素の INSERT が UNIQUE で失敗する場合は
/// statement ごと巻き戻るので整合は保たれる。v7 と分けたのは v7 まで適用済みの DB にも当てるため
const SCHEMA_V8: &str = r#"
CREATE TRIGGER fulltext_bi BEFORE INSERT ON fulltext
WHEN EXISTS (SELECT 1 FROM fulltext WHERE attachment_id = new.attachment_id)
BEGIN
  INSERT INTO fulltext_fts(fulltext_fts, rowid, text, cjk)
    SELECT 'delete', attachment_id, text, cjk FROM fulltext WHERE attachment_id = new.attachment_id;
END;
"#;

/// v7: 既存 fulltext 行の cjk 列を Rust で埋め（cjk_spaced は SQL では計算できない）、索引を rebuild してから
/// 同期トリガーを張る（トリガーを先に張ると UPDATE の 'delete' 命令が索引に無い旧値を消そうとして壊れる）
fn backfill_fulltext_fts_v7(conn: &rusqlite::Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT attachment_id, text FROM fulltext")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    for (id, text) in rows {
        if crate::model::has_cjk(&text) {
            conn.execute(
                "UPDATE fulltext SET cjk = ?1 WHERE attachment_id = ?2",
                rusqlite::params![crate::model::cjk_spaced(&text), id],
            )?;
        }
    }
    conn.execute_batch("INSERT INTO fulltext_fts(fulltext_fts) VALUES('rebuild');")?;
    conn.execute_batch(TRIGGERS_V7)?;
    Ok(())
}

/// v7: PDF 本文の全文検索（Paperpile の full-text search 相当）。
/// `fulltext_fts` は **external content**（content='fulltext'）: contentless だと `snippet()` が空を返す
/// （sqlite3 3.51 で実測）ため、抜粋を出すには本文の実体を FTS 側から読めなければならない。
/// 本文は既存の `fulltext.text` に 1 回だけ持ち、cjk 列（per-char 分かち書き。和文 PDF のみ非空）を同表に足す。
/// 索引の同期はトリガー（INSERT / DELETE / UPDATE。v8 で BEFORE INSERT も）で行うので、purge の FK CASCADE や
/// 旧バイナリの INSERT（OR REPLACE 含む・v8 以降）でも索引は追随する（旧バイナリは fulltext_fts を参照しないだけで共存可）。
/// md5 / status / detail = 再抽出スキップ判定と「空（スキャン PDF）/ 失敗」の記録（二重抽出を避ける）。
const SCHEMA_V7: &str = r#"
ALTER TABLE fulltext ADD COLUMN cjk    TEXT NOT NULL DEFAULT '';
ALTER TABLE fulltext ADD COLUMN md5    TEXT;
ALTER TABLE fulltext ADD COLUMN status TEXT NOT NULL DEFAULT 'ok';
ALTER TABLE fulltext ADD COLUMN detail TEXT;
CREATE VIRTUAL TABLE fulltext_fts USING fts5(
  text, cjk,
  content='fulltext', content_rowid='attachment_id',
  tokenize="unicode61 remove_diacritics 2"
);
"#;

const TRIGGERS_V7: &str = r#"
CREATE TRIGGER fulltext_ai AFTER INSERT ON fulltext BEGIN
  INSERT INTO fulltext_fts(rowid, text, cjk) VALUES (new.attachment_id, new.text, new.cjk);
END;
CREATE TRIGGER fulltext_ad AFTER DELETE ON fulltext BEGIN
  INSERT INTO fulltext_fts(fulltext_fts, rowid, text, cjk) VALUES ('delete', old.attachment_id, old.text, old.cjk);
END;
CREATE TRIGGER fulltext_au AFTER UPDATE ON fulltext BEGIN
  INSERT INTO fulltext_fts(fulltext_fts, rowid, text, cjk) VALUES ('delete', old.attachment_id, old.text, old.cjk);
  INSERT INTO fulltext_fts(rowid, text, cjk) VALUES (new.attachment_id, new.text, new.cjk);
END;
"#;

/// v6: 誌名識別キー `journal_keys`（item ごとに issn:… / t:<正規化誌名> を複数行）。
/// 誌名チップは「クリックした誌名のキーと同じキーを持つ item → その item の他のキー → …」と推移的に束ねて引く
/// （Paperpile の "Abdom. Radiol. (NY)" と PubMed の "Abdom Radiol (NY)"、print/electronic ISSN の違いを吸収）。
/// 旧バイナリ（v5 まで）はこのテーブルを参照しないので共存可
const SCHEMA_V6: &str = r#"
CREATE TABLE journal_keys (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  key     TEXT NOT NULL,
  PRIMARY KEY (item_id, key)
);
CREATE INDEX idx_journal_keys_key ON journal_keys(key);
"#;

/// v5: settings（`pdf_write_mode` 等。旧バイナリはこのテーブルを参照しないだけで共存可）
const SCHEMA_V5: &str = r#"
CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;

/// v2: items_fts に cjk 列を追加（CJK per-char 分かち書き列。和文タイトル・和文著者の部分一致用）
const SCHEMA_V2: &str = r#"
DROP TABLE IF EXISTS items_fts;
CREATE VIRTUAL TABLE items_fts USING fts5(
  title, abstract, creators, container_title, keywords, cjk,
  content='', contentless_delete=1,
  tokenize="unicode61 remove_diacritics 2"
);
"#;

/// v3: tags に色と表示順を追加（Paperpile LABELS UX）
const SCHEMA_V3: &str = r#"
ALTER TABLE tags ADD COLUMN color TEXT;
ALTER TABLE tags ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
"#;

/// v4: annotations.kind の CHECK に 'strikethrough' を追加（注釈 v2）。
/// SQLite は CHECK 制約の変更不可 → 新テーブル作成 → INSERT SELECT（全列明示）→ 旧 DROP → RENAME。
/// annotations への FK は attachments 向きの一方向のみで、annotations を参照する他テーブル・トリガー・ビューは無い
/// （idx_annotations_attachment インデックスのみ再作成が必要）。
const SCHEMA_V4: &str = r#"
CREATE TABLE annotations_v4 (
  id            INTEGER PRIMARY KEY,
  key           TEXT NOT NULL UNIQUE,
  attachment_id INTEGER NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
  author        TEXT NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('highlight','note','underline','strikethrough')),
  color         TEXT,
  comment       TEXT,
  page          INTEGER,
  quote         TEXT,
  prefix        TEXT,
  suffix        TEXT,
  rects         TEXT,
  sort_key      TEXT,
  date_added    TEXT NOT NULL DEFAULT (datetime('now')),
  date_modified TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO annotations_v4 (id, key, attachment_id, author, kind, color, comment, page, quote, prefix, suffix, rects, sort_key, date_added, date_modified)
  SELECT id, key, attachment_id, author, kind, color, comment, page, quote, prefix, suffix, rects, sort_key, date_added, date_modified
  FROM annotations;
DROP TABLE annotations;
ALTER TABLE annotations_v4 RENAME TO annotations;
CREATE INDEX idx_annotations_attachment ON annotations(attachment_id);
"#;

/// v1: CSL-JSON 正準 + 生成列 + FTS5（contentless）。
const SCHEMA_V1: &str = r#"
CREATE TABLE items (
  id              INTEGER PRIMARY KEY,
  key             TEXT NOT NULL UNIQUE,
  csl             TEXT NOT NULL CHECK (json_valid(csl)),
  item_type       TEXT    GENERATED ALWAYS AS (json_extract(csl, '$.type')) STORED,
  title           TEXT    GENERATED ALWAYS AS (json_extract(csl, '$.title')) STORED,
  year            INTEGER GENERATED ALWAYS AS (json_extract(csl, '$.issued."date-parts"[0][0]')) STORED,
  doi             TEXT    GENERATED ALWAYS AS (lower(json_extract(csl, '$.DOI'))) STORED,
  pmid            TEXT    GENERATED ALWAYS AS (json_extract(csl, '$.PMID')) STORED,
  pmcid           TEXT    GENERATED ALWAYS AS (json_extract(csl, '$.PMCID')) STORED,
  container_title TEXT    GENERATED ALWAYS AS (json_extract(csl, '$."container-title"')) STORED,
  starred         INTEGER NOT NULL DEFAULT 0,
  date_added      TEXT NOT NULL DEFAULT (datetime('now')),
  date_modified   TEXT NOT NULL DEFAULT (datetime('now')),
  deleted_at      TEXT
);
CREATE INDEX idx_items_doi  ON items(doi)  WHERE doi  IS NOT NULL;
CREATE INDEX idx_items_pmid ON items(pmid) WHERE pmid IS NOT NULL;
CREATE INDEX idx_items_year ON items(year);
CREATE INDEX idx_items_type ON items(item_type);

CREATE TABLE creators_index (
  item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  role     TEXT NOT NULL DEFAULT 'author',
  position INTEGER NOT NULL,
  family   TEXT,
  given    TEXT,
  literal  TEXT,
  PRIMARY KEY (item_id, role, position)
);
CREATE INDEX idx_creators_family ON creators_index(family);

CREATE TABLE collections (
  id         INTEGER PRIMARY KEY,
  key        TEXT NOT NULL UNIQUE,
  name       TEXT NOT NULL,
  parent_id  INTEGER REFERENCES collections(id),
  deleted_at TEXT
);
CREATE TABLE collection_items (
  collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  item_id       INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  PRIMARY KEY (collection_id, item_id)
);

CREATE TABLE tags (
  id   INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE item_tags (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (item_id, tag_id)
);

CREATE TABLE attachments (
  id         INTEGER PRIMARY KEY,
  key        TEXT NOT NULL UNIQUE,
  item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL DEFAULT 'pdf',
  mode       TEXT NOT NULL CHECK (mode IN ('managed','linked')),
  path       TEXT NOT NULL,
  md5        TEXT,
  page_count INTEGER,
  edition    TEXT,
  date_added TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_attachments_item ON attachments(item_id);

CREATE TABLE annotations (
  id            INTEGER PRIMARY KEY,
  key           TEXT NOT NULL UNIQUE,
  attachment_id INTEGER NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
  author        TEXT NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('highlight','note','underline')),
  color         TEXT,
  comment       TEXT,
  page          INTEGER,
  quote         TEXT,
  prefix        TEXT,
  suffix        TEXT,
  rects         TEXT,
  sort_key      TEXT,
  date_added    TEXT NOT NULL DEFAULT (datetime('now')),
  date_modified TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_annotations_attachment ON annotations(attachment_id);

CREATE TABLE notes (
  id            INTEGER PRIMARY KEY,
  key           TEXT NOT NULL UNIQUE,
  item_id       INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  author        TEXT NOT NULL,
  content_md    TEXT NOT NULL,
  date_added    TEXT NOT NULL DEFAULT (datetime('now')),
  date_modified TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_notes_item ON notes(item_id);

CREATE TABLE saved_searches (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,
  query_json TEXT NOT NULL
);

CREATE TABLE fulltext (
  attachment_id INTEGER PRIMARY KEY REFERENCES attachments(id) ON DELETE CASCADE,
  text          TEXT NOT NULL,
  extracted_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE retraction_flags (
  item_id    INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
  status     TEXT NOT NULL,
  detail     TEXT,
  source     TEXT,
  checked_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE VIRTUAL TABLE items_fts USING fts5(
  title, abstract, creators, container_title, keywords,
  content='', contentless_delete=1,
  tokenize="unicode61 remove_diacritics 2"
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// v8 までの schema（旧バイナリが作った既存 DB の形）から開くと v9 で attachments.doi_check が増え、旧列はそのまま
    #[test]
    fn migration_v8_to_v9_adds_doi_check() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for sql in &MIGRATIONS[..8] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 8).unwrap();
        let cols = |c: &Connection| -> Vec<String> {
            let mut st = c.prepare("PRAGMA table_info(attachments)").unwrap();
            st.query_map([], |r| r.get::<_, String>(1)).unwrap().filter_map(|x| x.ok()).collect()
        };
        assert!(!cols(&conn).iter().any(|c| c == "doi_check"), "v8 には無い列");
        let db = Db::init(conn, None).unwrap();
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 10);
        let c = cols(&db.conn);
        assert!(c.iter().any(|x| x == "doi_check"));
        for keep in ["id", "key", "item_id", "kind", "mode", "path", "md5", "page_count", "edition", "date_added"] {
            assert!(c.iter().any(|x| x == keep), "{keep} が残っている");
        }
        // 既定は NULL
        db.conn
            .execute(
                "INSERT INTO items (key, csl) VALUES ('K1', '{\"title\":\"t\"}')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO attachments (key, item_id, kind, mode, path) VALUES ('A1', 1, 'pdf', 'managed', '/x.pdf')",
                [],
            )
            .unwrap();
        let mark: Option<String> = db
            .conn
            .query_row("SELECT doi_check FROM attachments WHERE key = 'A1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mark, None);
    }
}
