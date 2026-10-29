//! 整理系の書き込み操作（star / trash / tags / collections / 添付）。
//! すべて key（8桁）を入口にし、UI・MCP・CLI から共用する。

use crate::{ingest, keys, model, types, CoreError, Db, Result};
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::path::Path;

/// 手動入力・編集の共通サニタイズ: 型の妥当性 → 未知フィールド除去 → 空値除去 → DOI 正規化。
/// 「型カタログで検証する」の実行点。
fn sanitize(csl: &Value) -> Result<Value> {
    let type_id = model::s(csl, "type").unwrap_or("document").to_string();
    if types::type_def(&type_id).is_none() {
        return Err(CoreError::BadInput(format!("未知の参照タイプ: {type_id}")));
    }
    let obj = csl
        .as_object()
        .ok_or_else(|| CoreError::BadInput("CSL がオブジェクトではありません".into()))?;
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        if !types::is_valid_field(&type_id, k) {
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
            ("DOI", Value::String(s)) => Value::String(
                s.trim()
                    .trim_start_matches("https://doi.org/")
                    .trim_start_matches("http://dx.doi.org/")
                    .to_lowercase(),
            ),
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
    let pmid = model::s(&csl, "PMID").map(str::to_string);
    let doi = model::s(&csl, "DOI").map(str::to_string);
    if let Some((_, existing)) = crate::search::find_by_ids(db, pmid.as_deref(), doi.as_deref())? {
        return Err(CoreError::Duplicate {
            existing_key: existing,
            field: if pmid.is_some() { "PMID" } else { "DOI" },
        });
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
    let mut new = sanitize(csl)?;
    // custom（取込元・paperpile 情報・MeSH 等）は UI に出さないので既存から引き継ぐ
    if let Ok(old) = serde_json::from_str::<Value>(&old_s) {
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
    tx.execute(
        "UPDATE items SET csl = ?1, date_modified = datetime('now') WHERE id = ?2",
        rusqlite::params![new.to_string(), id],
    )?;
    ingest::reindex_item(&tx, id, &new)?;
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
    let id = item_id(db, key)?;
    db.conn
        .execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
    let tag_id: i64 = db
        .conn
        .query_row("SELECT id FROM tags WHERE name = ?1", [tag], |r| r.get(0))?;
    db.conn.execute(
        "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
        rusqlite::params![id, tag_id],
    )?;
    Ok(())
}

/// 名前は add_tag と同じく前後の空白を落とし、空なら同じ BadInput。
/// 呼び手は登録表（commands.rs の remove_tag / set_tags）だけで、そちらも trim して渡す（GUI は detach_tag = id・CLI は呼ばない）
pub fn remove_tag(db: &Db, key: &str, tag: &str) -> Result<()> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Err(CoreError::BadInput("空のタグ".into()));
    }
    let id = item_id(db, key)?;
    db.conn.execute(
        "DELETE FROM item_tags WHERE item_id = ?1 AND tag_id IN (SELECT id FROM tags WHERE name = ?2)",
        rusqlite::params![id, tag],
    )?;
    Ok(())
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
/// 既存判定は NOCASE（MCP resolve_collection と同基準。大小文字違いの重複を作らない）。
/// 返り値 = (最終セグメントの collection id, 新規作成したセグメント名の列)。
pub fn ensure_collection_path(db: &Db, path: &str) -> Result<(i64, Vec<String>)> {
    let mut parent: Option<i64> = None;
    let mut created = Vec::new();
    for seg in path.split('/').map(str::trim).filter(|s| !s.is_empty()) {
        let id = match crate::collections::child_id(&db.conn, parent, seg)? {
            Some(id) => id,
            None => {
                let id = crate::collections::insert_child(&db.conn, parent, seg)?;
                created.push(seg.to_string());
                id
            }
        };
        parent = Some(id);
    }
    parent
        .map(|id| (id, created))
        .ok_or_else(|| CoreError::BadInput("フォルダ名が空です".into()))
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
    let (id, placement) = if db.pdf_write_mode() != "drive" {
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
}
