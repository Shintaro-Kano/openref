//! 整理系の書き込み操作（star / trash / tags / collections / 添付）。
//! すべて key（8桁）を入口にし、UI・MCP・CLI から共用する。

use crate::{ingest, keys, model, types, CoreError, Db, Result};
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::path::Path;

/// 手動入力・編集の共通サニタイズ: 型の妥当性 → 未知フィールド除去 → 空値除去 → DOI 正規化。
/// 「型カタログで検証する」の実行点。
fn sanitize(csl: &Value) -> Result<Value> {
    sanitize_with(csl, &[])
}

/// sanitize の本体。`carried` は型カタログの外でも通すキー（update_csl が「既存 item の、旧い型のカタログにも新しい型のカタログにも無いキー」を渡す）
fn sanitize_with(csl: &Value, carried: &[String]) -> Result<Value> {
    let type_id = types::canonical_type(model::s(csl, "type").unwrap_or("document")).to_string();
    if types::type_def(&type_id).is_none() {
        return Err(CoreError::BadInput(format!("未知の参照タイプ: {type_id}")));
    }
    let obj = csl
        .as_object()
        .ok_or_else(|| CoreError::BadInput("CSL がオブジェクトではありません".into()))?;
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        if !types::is_valid_field(&type_id, k) && !carried.contains(k) {
            continue; // 型に無いフィールドは黙って落とす（混入防止）
        }
        let keep = match v {
            Value::Null => false,
            Value::String(s) => !s.trim().is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
            _ => true,
        };
        if !keep {
            continue;
        }
        let v = match (k.as_str(), v) {
            ("DOI", Value::String(s)) => match model::normalize_doi_for_storage(s) {
                d if d.is_empty() => continue, // 接頭辞だけ（`doi:` 等）は空値と同じ
                d => Value::String(d),
            },
            (_, Value::String(s)) => Value::String(s.trim().to_string()),
            _ => v.clone(),
        };
        out.insert(k.clone(), v);
    }
    out.insert("type".into(), Value::String(type_id));
    if model::s(&Value::Object(out.clone()), "title").is_none() {
        return Err(CoreError::BadInput("タイトルは必須です".into()));
    }
    Ok(Value::Object(out))
}

/// 手動作成（Create new reference）。DOI/PMID が既存と衝突する場合は拒否。
pub fn create_manual(db: &mut Db, csl: &Value) -> Result<String> {
    let csl = sanitize(csl)?;
    if let Some((existing_key, field)) = ingest::find_duplicate(&db.conn, &csl)? {
        return Err(CoreError::Duplicate { existing_key, field });
    }
    let key = keys::new_key();
    let tx = db.conn.transaction()?;
    tx.execute(
        "INSERT INTO items (key, csl) VALUES (?1, ?2)",
        rusqlite::params![key, csl.to_string()],
    )?;
    let id = tx.last_insert_rowid();
    ingest::index_item(&tx, id, &csl)?;
    tx.commit()?;
    Ok(key)
}

/// 既存 item の書誌を差し替える（編集）。custom は既存を温存する。
///
/// 型カタログの外のキー（取込が書く `accessed`・Zotero の `citation-key` 等）は編集の画面に出ないので、
/// 「旧 CSL のうち、**旧い型**のカタログにも**新しい型**のカタログにも無いキー」は次の扱いにする（それ以外は sanitize のとおり）:
/// - 入力に無い → 旧い値を引き継ぐ（画面に出せないものを、出さなかったことを理由に消さない）
/// - 入力にあり空（null・空文字・空配列・空オブジェクト）→ 消す（明示の削除）
/// - 入力にあり値がある → その値にする（sanitize と同じ trim / DOI 正規化）
///
/// 旧い型のカタログに**あった**キーは、入力に無ければ消える（画面で空にした = 削除）。型を変えたとき、
/// 旧型の欄が新型の CSL に残らないのはこのため（sanitize の混入防止）。旧い型がカタログに無い型なら、
/// 旧 CSL の全キーが「旧い型のカタログに無かったキー」になる。ただし旧い型 `article` は例外で、document として扱う
/// （`types::canonical_type`）= document のカタログにある欄（genre・number 等）は「旧い型にあったキー」なので温存しない。
/// 新しい型のカタログに**ある**キーも温存しない: 新しい型では欄として見えている（エディタは型に関係なく
/// CSL の全キーを読む）ので、入力が正（空で保存すれば消える）。型を変えない編集では旧い型 = 新しい型なので、この条件は効かない。
pub fn update_csl(db: &mut Db, key: &str, csl: &Value) -> Result<()> {
    let (id, old_s): (i64, String) = db
        .conn
        .query_row(
            "SELECT id, csl FROM items WHERE key = ?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("item {key}")))?;
    let old = serde_json::from_str::<Value>(&old_s).ok();
    // 新しい型の検証は sanitize_with がする（未知の型なら BadInput）。ここでは温存の判定にだけ使う
    let new_type = types::canonical_type(model::s(csl, "type").unwrap_or("document"));
    let carried: Vec<(String, Value)> = match old.as_ref().and_then(Value::as_object) {
        Some(o) => {
            let old_type = types::canonical_type(o.get("type").and_then(Value::as_str).unwrap_or("document"));
            o.iter()
                .filter(|(k, _)| !types::is_valid_field(old_type, k) && !types::is_valid_field(new_type, k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        }
        None => Vec::new(),
    };
    let carried_keys: Vec<String> = carried.iter().map(|(k, _)| k.clone()).collect();
    let mut new = sanitize_with(csl, &carried_keys)?;
    if let (Some(o), Some(input)) = (new.as_object_mut(), csl.as_object()) {
        for (k, v) in carried {
            if !input.contains_key(&k) {
                o.insert(k, v);
            }
        }
    }
    // custom（取込元・paperpile 情報・MeSH 等）は UI に出さないので既存から引き継ぐ
    if let Some(old) = old {
        if let (Some(o), Some(custom)) = (new.as_object_mut(), old.get("custom")) {
            let merged = match o.get("custom") {
                Some(Value::Object(edited)) => {
                    let mut base = custom.as_object().cloned().unwrap_or_default();
                    for (k, v) in edited {
                        base.insert(k.clone(), v.clone());
                    }
                    Value::Object(base)
                }
                _ => custom.clone(),
            };
            o.insert("custom".into(), merged);
        }
    }
    let tx = db.conn.transaction()?;
    ingest::replace_csl(&tx, id, &new)?;
    tx.commit()?;
    Ok(())
}

/// item key → id（ゴミ箱の item も引く）。無ければ NotFound("item {key}")
pub(crate) fn item_id(db: &Db, key: &str) -> Result<i64> {
    db.conn
        .query_row("SELECT id FROM items WHERE key = ?1", [key], |r| r.get(0))
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("item {key}")))
}

pub fn set_starred(db: &Db, key: &str, starred: bool) -> Result<()> {
    let id = item_id(db, key)?;
    db.conn.execute(
        "UPDATE items SET starred = ?1, date_modified = datetime('now') WHERE id = ?2",
        rusqlite::params![starred as i64, id],
    )?;
    Ok(())
}

pub fn trash(db: &Db, key: &str) -> Result<()> {
    let id = item_id(db, key)?;
    db.conn.execute(
        "UPDATE items SET deleted_at = datetime('now') WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

pub fn restore(db: &Db, key: &str) -> Result<()> {
    let id = item_id(db, key)?;
    db.conn
        .execute("UPDATE items SET deleted_at = NULL WHERE id = ?1", [id])?;
    Ok(())
}

/// 完全削除の結果（UI の toast・ログ用）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Purged {
    pub key: String,
    /// managed storage（OpenRef 所有）から削除したファイル数
    pub files_removed: usize,
    /// linked（Paperpile の Drive 等・OpenRef 所有でない）のため残したファイル数
    pub files_kept: usize,
    /// managed だが削除に失敗したファイル数（DB 行は消えている = 孤児。stderr に経路を出す）
    pub files_failed: usize,
}

/// ゴミ箱内の item を**完全削除**する（復元不可）。Paperpile の Delete permanently に相当。
/// - ゴミ箱に無い item は拒否する（MCP/CLI からの誤爆防止。必ず trash を経由させる）
/// - DB: `items` 行の DELETE で creators_index / collection_items / item_tags / notes / retraction_flags /
///   attachments（→ annotations・fulltext）が FK CASCADE で消える（`PRAGMA foreign_keys = ON`）。
///   `items_fts` は contentless_delete=1 なので rowid 指定で明示 DELETE
/// - ファイル: mode='managed'（DB 同階層 storage/ 配下）のみ削除し、空になった `storage/<添付key>/` も畳む。
///   mode='linked'（Paperpile の Drive フォルダ等）は**触らない**（cutover 前は Paperpile が SSOT）
pub fn purge(db: &Db, key: &str) -> Result<Purged> {
    let (id, deleted_at): (i64, Option<String>) = db
        .conn
        .query_row("SELECT id, deleted_at FROM items WHERE key = ?1", [key], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("item {key}")))?;
    if deleted_at.is_none() {
        return Err(CoreError::BadInput(format!(
            "item {key} はゴミ箱に無いので完全削除できません（先にゴミ箱へ移動してください）"
        )));
    }
    // 添付の所在は DB 行を消す前に控える
    let atts: Vec<(String, String)> = {
        let mut stmt = db
            .conn
            .prepare("SELECT mode, path FROM attachments WHERE item_id = ?1")?;
        let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let tx = db.conn.unchecked_transaction()?;
    tx.execute("DELETE FROM items_fts WHERE rowid = ?1", [id])?;
    tx.execute("DELETE FROM items WHERE id = ?1", [id])?;
    tx.commit()?;

    // DB が真。ファイル削除の失敗で DB を巻き戻さない（孤児として報告する）
    let root = db.storage_root();
    let mut out = Purged { key: key.to_string(), files_removed: 0, files_kept: 0, files_failed: 0 };
    for (mode, path) in atts {
        let p = Path::new(&path);
        let ours = mode == "managed" && root.as_deref().is_some_and(|r| p.starts_with(r));
        if !ours {
            out.files_kept += 1;
            continue;
        }
        match std::fs::remove_file(p) {
            Ok(()) => {
                out.files_removed += 1;
                if let Some(dir) = p.parent() {
                    let _ = std::fs::remove_dir(dir); // 空でなければ失敗して残る（それで良い）
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => out.files_removed += 1,
            Err(e) => {
                eprintln!("[openref] purge {key}: managed ファイル削除失敗 {}: {e}", p.display());
                out.files_failed += 1;
            }
        }
    }
    Ok(out)
}

pub fn add_tag(db: &Db, key: &str, tag: &str) -> Result<()> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Err(CoreError::BadInput("空のタグ".into()));
    }
    item_id(db, key)?; // 無い key ならラベルを作らずに NotFound
    let tag_id = crate::tags::ensure_tag(db, tag, None)?;
    crate::tags::attach_tag(db, key, tag_id)
}

/// 名前は add_tag と同じく前後の空白を落とし、空なら同じ BadInput。
/// 呼び手は登録表（commands.rs の remove_tag / set_tags）だけで、そちらも trim して渡す（GUI は detach_tag = id・CLI は呼ばない）
pub fn remove_tag(db: &Db, key: &str, tag: &str) -> Result<()> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Err(CoreError::BadInput("空のタグ".into()));
    }
    item_id(db, key)?; // 無い key は NotFound（無い名前は no-op）
    match crate::tags::tag_by_name(&db.conn, tag)? {
        Some((tag_id, _)) => crate::tags::detach_tag(db, key, tag_id),
        None => Ok(()),
    }
}

pub fn add_to_collection(db: &Db, key: &str, collection_id: i64) -> Result<()> {
    let id = item_id(db, key)?;
    let exists: Option<i64> = db
        .conn
        .query_row(
            "SELECT id FROM collections WHERE id = ?1 AND deleted_at IS NULL",
            [collection_id],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(CoreError::NotFound(format!("collection {collection_id}")));
    }
    db.conn.execute(
        "INSERT OR IGNORE INTO collection_items (collection_id, item_id) VALUES (?1, ?2)",
        rusqlite::params![collection_id, id],
    )?;
    Ok(())
}

pub fn remove_from_collection(db: &Db, key: &str, collection_id: i64) -> Result<()> {
    let id = item_id(db, key)?;
    db.conn.execute(
        "DELETE FROM collection_items WHERE collection_id = ?1 AND item_id = ?2",
        rusqlite::params![collection_id, id],
    )?;
    Ok(())
}

/// フォルダをパスで確保する（冪等）。'Parent/Child' 可・途中の親も無ければ作る。
/// 段の照合は `collections::walk`（MCP のフォルダ指定と同じ基準 = 大小無視。大小文字違いの重複を作らない）。
/// 返り値 = (最終セグメントの collection id, 新規作成したセグメント名の列)。
pub fn ensure_collection_path(db: &Db, path: &str) -> Result<(i64, Vec<String>)> {
    crate::collections::ensure_path(&db.conn, None, path)
}

/// 添付書込の実際の置き場所（attach_auto の戻り値。呼び手はログ・診断表示に使う）
#[derive(Debug)]
pub enum Placement {
    /// Drive 互換レイアウトへ書いた（pdf_write_mode=drive の正常経路）
    Drive(std::path::PathBuf),
    /// managed storage へ書いた（cutover 前の既定）
    Managed(std::path::PathBuf),
    /// drive モードだが書けず managed へ降格した（Drive 障害時も添付は失敗させない）
    ManagedFallback { path: std::path::PathBuf, reason: String },
}

/// 添付 1 本の結果: 行 id・実体の md5・置き場所。呼び手は同じバイト列の md5 を取り直さず（重複判定・照合印）、
/// 照合印は id で書く（`pdffetch::mark_doi_check`）
#[derive(Debug)]
pub struct Attached {
    pub id: i64,
    pub md5: String,
    pub placement: Placement,
}

/// 添付の統一入口: `pdf_write_mode` 設定に従い Drive 互換レイアウトまたは managed storage へ書く。
/// drive モードで書けない場合は managed へ降格して理由を返す（捕獲・OA 取得を止めない）。ファイルは 1 度だけ読む
pub fn attach_auto(db: &Db, key: &str, src: &Path) -> Result<Attached> {
    if !src.exists() {
        return Err(CoreError::NotFound(format!("file {}", src.display())));
    }
    let fname = src
        .file_name()
        .ok_or_else(|| CoreError::BadInput(format!("ファイル名が取れません: {}", src.display())))?
        .to_string_lossy()
        .to_string();
    crate::db::reject_connection_file(&db.conn, src)?;
    let bytes = std::fs::read(src)?;
    attach_auto_bytes(db, key, &fname, &bytes)
}

pub fn attach_auto_bytes(db: &Db, key: &str, filename: &str, bytes: &[u8]) -> Result<Attached> {
    attach_auto_with_md5(db, key, filename, bytes, &crate::pdffetch::md5_hex(bytes))
}

/// md5 を計算済みの呼び手（取込の重複判定・捕獲）用: 同じバイト列の md5 を取り直さない。`md5` = `bytes` の 16 進 md5
pub fn attach_auto_with_md5(db: &Db, key: &str, filename: &str, bytes: &[u8], md5: &str) -> Result<Attached> {
    if bytes.is_empty() {
        return Err(CoreError::BadInput("空のファイル".into()));
    }
    let (id, placement) = if db.pdf_write_mode()? != "drive" {
        let (att_id, path) = write_managed(db, key, filename, bytes, md5)?;
        (att_id, Placement::Managed(path))
    } else {
        let item = item_id(db, key)?;
        let csl_s: String = db
            .conn
            .query_row("SELECT csl FROM items WHERE id = ?1", [item], |r| r.get(0))?;
        let csl: Value = serde_json::from_str(&csl_s)?;
        // Drive に置けなければ（mount 未検出・書込失敗）理由を添えて managed へ降格
        let placed = match crate::mount::detect() {
            None => Err("Drive mount 未検出（cutover モードだが managed へ降格）".to_string()),
            Some(mount) => crate::drive_layout::place_bytes(&mount, &csl, bytes)
                .map_err(|e| format!("Drive 書込失敗のため managed へ降格: {e}")),
        };
        match placed {
            Ok(dest) => {
                db.conn.execute(
                    "INSERT INTO attachments (key, item_id, kind, mode, path, md5) VALUES (?1, ?2, 'pdf', 'linked', ?3, ?4)",
                    rusqlite::params![keys::new_key(), item, dest.to_string_lossy(), md5],
                )?;
                let att_id = db.conn.last_insert_rowid();
                // 本文の全文索引（失敗しても添付は成立させる）。手元のバイト列から作る = 書いたファイルを読み直さない
                crate::fulltext::index_bytes_quietly(&db.conn, att_id, bytes, &dest, md5);
                (att_id, Placement::Drive(dest))
            }
            Err(reason) => {
                let (att_id, path) = write_managed(db, key, filename, bytes, md5)?;
                (att_id, Placement::ManagedFallback { path, reason })
            }
        }
    };
    Ok(Attached { id, md5: md5.to_string(), placement })
}

/// managed 添付の本体: storage/<添付key>/<filename> へ書き、行を入れ、手元のバイト列から本文を索引する。
/// 戻り値 = (添付行 id, 書込先)
fn write_managed(db: &Db, key: &str, filename: &str, bytes: &[u8], md5: &str) -> Result<(i64, std::path::PathBuf)> {
    let id = item_id(db, key)?;
    let root = db
        .storage_root()
        .ok_or_else(|| CoreError::BadInput("in-memory DB は managed 添付不可".into()))?;
    let att_key = keys::new_key();
    let dir = root.join(&att_key);
    std::fs::create_dir_all(&dir)?;
    let safe = if filename.trim().is_empty() { "file.pdf" } else { filename.trim() };
    let dest = dir.join(safe);
    std::fs::write(&dest, bytes)?;
    db.conn.execute(
        "INSERT INTO attachments (key, item_id, kind, mode, path, md5) VALUES (?1, ?2, 'pdf', 'managed', ?3, ?4)",
        rusqlite::params![att_key, id, dest.to_string_lossy(), md5],
    )?;
    let att_id = db.conn.last_insert_rowid();
    // 本文の全文索引（失敗しても添付は成立させる）。手元のバイト列から作る = 書いたファイルを読み直さない
    crate::fulltext::index_bytes_quietly(&db.conn, att_id, bytes, &dest, md5);
    Ok((att_id, dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ラベル名の空白: add_tag / remove_tag とも前後を落とし、空なら両方 BadInput（remove_tag が黙って no-op だった非対称を揃えた）
    #[test]
    fn tag_name_blank_and_trim_are_symmetric() {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .execute_batch("INSERT INTO items (key, csl) VALUES ('ITEM0001', '{\"title\":\"x\",\"type\":\"article-journal\"}')")
            .unwrap();
        assert!(matches!(add_tag(&db, "ITEM0001", "   "), Err(CoreError::BadInput(_))));
        assert!(matches!(remove_tag(&db, "ITEM0001", "   "), Err(CoreError::BadInput(_))));
        let names = |db: &Db| -> Vec<String> {
            crate::search::item_detail(db, "ITEM0001").unwrap().unwrap().tags.into_iter().map(|t| t.name).collect()
        };
        add_tag(&db, "ITEM0001", "  liver ").unwrap();
        assert_eq!(names(&db), ["liver"]);
        remove_tag(&db, "ITEM0001", " liver  ").unwrap();
        assert!(names(&db).is_empty());
        // 付いていない / 存在しない名前は従来どおり no-op（エラーにしない）
        remove_tag(&db, "ITEM0001", "nope").unwrap();
    }

    /// 旧い型のカタログに無かったキー（取込由来の accessed・Zotero の citation-key）は編集で消えない。
    /// カタログ内のキーは入力から外せば消え、型を変えれば旧型の欄は残らない
    #[test]
    fn update_keeps_keys_outside_old_type_catalog() {
        use serde_json::json;
        let mut db = Db::open_in_memory().unwrap();
        let old = json!({
            "type": "article-journal", "title": "T", "author": [{"family": "Kano", "given": "S"}],
            "container-title": "Radiology", "volume": "1", "PMID": "123",
            "issued": {"date-parts": [[2020]]},
            "accessed": {"date-parts": [[2024, 5, 1]]}, "citation-key": "kano2020liver",
            "custom": {"source": "zotero"}
        });
        db.conn
            .execute("INSERT INTO items (key, csl) VALUES ('ITEM0001', ?1)", [old.to_string()])
            .unwrap();
        let get = |db: &Db| -> Value {
            let s: String = db.conn.query_row("SELECT csl FROM items WHERE key = 'ITEM0001'", [], |r| r.get(0)).unwrap();
            serde_json::from_str(&s).unwrap()
        };
        let bibkey = |db: &Db| crate::bibtex::citekey(&get(db), "ITEM0001");

        // GUI の形（その型のカタログの欄だけ・空欄は送らない）: volume を空にし、PMID を外し、title を直す
        let before_key = bibkey(&db);
        update_csl(&mut db, "ITEM0001", &json!({
            "type": "article-journal", "title": "T2", "author": [{"family": "Kano", "given": "S"}],
            "container-title": "Radiology", "issued": {"date-parts": [[2020]]}, "custom": {"subtype": "review"}
        })).unwrap();
        let c = get(&db);
        assert_eq!(c["title"], "T2");
        assert_eq!(c["accessed"], json!({"date-parts": [[2024, 5, 1]]}), "accessed は残る");
        assert_eq!(c["citation-key"], "kano2020liver", "citation-key は残る");
        assert!(c.get("volume").is_none() && c.get("PMID").is_none(), "カタログ内のキーは入力から外すと消える");
        assert_eq!(c["custom"], json!({"source": "zotero", "subtype": "review"}));
        assert_eq!(before_key, "kano2020liver");
        assert_eq!(bibkey(&db), before_key, "BibTeX の引用キーが変わらない");

        // MCP の形（get_item の csl を直して渡す）: 値を変えれば変わり、空にすれば消える
        let mut m = get(&db);
        m["citation-key"] = json!("  kano2020b ");
        m["accessed"] = json!(null);
        update_csl(&mut db, "ITEM0001", &m).unwrap();
        let c = get(&db);
        assert_eq!(c["citation-key"], "kano2020b");
        assert!(c.get("accessed").is_none(), "空値は明示の削除");

        // 型を変える: 旧型（article-journal）の欄で新型（webpage）に無いものは残らない。カタログ外の citation-key は残る
        update_csl(&mut db, "ITEM0001", &json!({
            "type": "webpage", "title": "T2", "URL": "https://example.org/x",
            "container-title": "Radiology", "issued": {"date-parts": [[2020]]}
        })).unwrap();
        let c = get(&db);
        assert_eq!(c["type"], "webpage");
        assert!(c.get("author").is_none(), "入力に無いカタログ内のキーは消える");
        assert_eq!(c["citation-key"], "kano2020b");
        // webpage の accessed（カタログ内）を入れてから article-journal へ戻すと、旧型の欄なので残らない
        let mut w = get(&db);
        w["accessed"] = json!({"date-parts": [[2025, 1, 2]]});
        update_csl(&mut db, "ITEM0001", &w).unwrap();
        assert!(get(&db).get("accessed").is_some());
        update_csl(&mut db, "ITEM0001", &json!({"type": "article-journal", "title": "T2"})).unwrap();
        let c = get(&db);
        assert!(c.get("accessed").is_none() && c.get("URL").is_none(), "旧型の欄は新型に混ざらない");
        assert_eq!(c["citation-key"], "kano2020b");
        // カタログ外でも、元の CSL に無いキーは入れられない（混入防止は従来どおり）
        update_csl(&mut db, "ITEM0001", &json!({"type": "article-journal", "title": "T2", "bogus": "x"})).unwrap();
        assert!(get(&db).get("bogus").is_none());
    }

    /// 温存は「旧い型のカタログにも新しい型のカタログにも無いキー」だけ。新しい型で欄として見えるキーは入力が正
    /// （エディタで型を変え、見えている欄を空で保存したら消える）。型を変えない編集は温存する
    #[test]
    fn update_carries_only_keys_outside_both_catalogs() {
        use serde_json::json;
        let mut db = Db::open_in_memory().unwrap();
        // article-journal のカタログに accessed は無く、webpage のカタログには有る（どちらにも citation-key は無い）
        assert!(!types::is_valid_field("article-journal", "accessed") && types::is_valid_field("webpage", "accessed"));
        assert!(!types::is_valid_field("webpage", "citation-key") && !types::is_valid_field("article-journal", "citation-key"));
        let old = json!({
            "type": "article-journal", "title": "T", "URL": "https://example.org/a",
            "accessed": {"date-parts": [[2024, 5, 1]]}, "citation-key": "kano2020liver"
        });
        let put = |db: &Db, v: &Value| {
            db.conn.execute("DELETE FROM items WHERE key = 'ITEM0001'", []).unwrap();
            db.conn.execute("INSERT INTO items (key, csl) VALUES ('ITEM0001', ?1)", [v.to_string()]).unwrap();
        };
        let get = |db: &Db| -> Value {
            let s: String = db.conn.query_row("SELECT csl FROM items WHERE key = 'ITEM0001'", [], |r| r.get(0)).unwrap();
            serde_json::from_str(&s).unwrap()
        };

        // 型を変えない編集（GUI の形 = カタログの欄だけ）: accessed も citation-key も温存
        put(&db, &old);
        update_csl(&mut db, "ITEM0001", &json!({"type": "article-journal", "title": "T2"})).unwrap();
        let c = get(&db);
        assert_eq!(c["accessed"], json!({"date-parts": [[2024, 5, 1]]}));
        assert_eq!(c["citation-key"], "kano2020liver");

        // 型を変えて、新しい型の欄（accessed）を空で保存（= 入力に無い）→ 消える。どちらの型にも無い citation-key は温存
        put(&db, &old);
        update_csl(&mut db, "ITEM0001", &json!({"type": "webpage", "title": "T", "URL": "https://example.org/a"})).unwrap();
        let c = get(&db);
        assert_eq!(c["type"], "webpage");
        assert!(c.get("accessed").is_none(), "新しい型で見えている欄は入力が正 = 空なら消える");
        assert_eq!(c["citation-key"], "kano2020liver", "新しい型にも無いキーは温存");

        // 型を変えて accessed を入力に残す（エディタが旧値を欄に読んだまま保存）→ その値で残る
        put(&db, &old);
        update_csl(&mut db, "ITEM0001", &json!({
            "type": "webpage", "title": "T", "accessed": {"date-parts": [[2024, 5, 1]]}
        })).unwrap();
        let c = get(&db);
        assert_eq!(c["accessed"], json!({"date-parts": [[2024, 5, 1]]}));
        assert_eq!(c["citation-key"], "kano2020liver");
    }

    /// 型カタログに無い `article`（1.1.0 までの GUI 取込 stub・Zotero 移行の preprint）は document として編集・保存できる
    #[test]
    fn legacy_article_type_is_edited_as_document() {
        use serde_json::json;
        let mut db = Db::open_in_memory().unwrap();
        let old = json!({
            "type": "article", "title": "Preprint", "publisher": "medRxiv", "genre": "Preprint",
            "number": "2024.01.01.1", "citation-key": "kano2024",
            "custom": {"source": "zotero"}
        });
        db.conn
            .execute("INSERT INTO items (key, csl) VALUES ('ITEM0001', ?1)", [old.to_string()])
            .unwrap();
        let get = |db: &Db| -> Value {
            let s: String = db.conn.query_row("SELECT csl FROM items WHERE key = 'ITEM0001'", [], |r| r.get(0)).unwrap();
            serde_json::from_str(&s).unwrap()
        };
        // 型が article のまま送られても通り、document として保存される（document の欄 = publisher・genre・number はそのまま）
        update_csl(&mut db, "ITEM0001", &json!({
            "type": "article", "title": "Preprint 2", "publisher": "medRxiv", "genre": "Preprint", "number": "2024.01.01.1"
        })).unwrap();
        let c = get(&db);
        assert_eq!(c["type"], "document");
        assert_eq!(c["title"], "Preprint 2");
        assert_eq!((c["publisher"].as_str(), c["genre"].as_str(), c["number"].as_str()), (Some("medRxiv"), Some("Preprint"), Some("2024.01.01.1")));
        assert_eq!(c["citation-key"], "kano2024", "どちらの型にも無いキーは温存");
        assert_eq!(c["custom"], json!({"source": "zotero"}));

        // 旧い型が article の item を document で保存し直しても、document の欄を空にすれば消える（旧型 = document 扱い）
        update_csl(&mut db, "ITEM0001", &json!({"type": "document", "title": "Preprint 2", "publisher": "medRxiv"})).unwrap();
        let c = get(&db);
        assert!(c.get("genre").is_none() && c.get("number").is_none(), "document の欄は入力が正");

        // 温存の判定: 旧 article の item を article-journal に型を変えて保存すると、document のカタログにある genre・number は
        // 旧型の欄（article を document として扱う）なので残らない。どちらの型にも無い citation-key は温存。
        // （article をカタログに無い型として扱うと全キーが温存対象になり、genre・number も残ってしまう）
        db.conn.execute("DELETE FROM items WHERE key = 'ITEM0001'", []).unwrap();
        db.conn.execute("INSERT INTO items (key, csl) VALUES ('ITEM0001', ?1)", [old.to_string()]).unwrap();
        update_csl(&mut db, "ITEM0001", &json!({"type": "article-journal", "title": "Preprint 3"})).unwrap();
        let c = get(&db);
        assert_eq!(c["type"], "article-journal");
        assert!(c.get("genre").is_none() && c.get("number").is_none() && c.get("publisher").is_none());
        assert_eq!(c["citation-key"], "kano2024");

        // 手動作成・sanitize も同じ
        let created = create_manual(&mut db, &json!({"type": "article", "title": "New"})).unwrap();
        let s: String = db.conn.query_row("SELECT csl FROM items WHERE key = ?1", [created], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap()["type"], "document");
        // 未知の型は従来どおり Err
        assert!(matches!(
            update_csl(&mut db, "ITEM0001", &json!({"type": "post-weblog", "title": "x"})),
            Err(CoreError::BadInput(_))
        ));
    }
}
