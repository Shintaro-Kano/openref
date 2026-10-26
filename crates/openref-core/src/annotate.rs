//! 注釈・ノート。user と LLM の共同注釈を author provenance 付きで保持する。
//! anchor は TextQuoteSelector（page + quote + prefix/suffix）＝LLM が座標なしで指せる形式。

use crate::{keys, CoreError, Db, Result};
use rusqlite::OptionalExtension;

#[derive(Debug, Default)]
pub struct NewAnnotation {
    /// highlight / note / underline / strikethrough
    pub kind: String,
    pub color: Option<String>,
    pub comment: Option<String>,
    pub page: Option<i64>,
    pub quote: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    /// 'user' | 'claude' | 'claude:<model-id>' など
    pub author: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationRow {
    pub key: String,
    pub author: String,
    pub kind: String,
    pub color: Option<String>,
    pub comment: Option<String>,
    pub page: Option<i64>,
    pub quote: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub date_added: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRow {
    pub key: String,
    pub author: String,
    pub content_md: String,
    pub date_added: String,
    pub date_modified: String,
}

fn first_pdf_attachment(db: &Db, item_key: &str) -> Result<Option<i64>> {
    Ok(db
        .conn
        .query_row(
            "SELECT a.id FROM attachments a JOIN items i ON i.id = a.item_id
             WHERE i.key = ?1 AND a.kind = 'pdf' ORDER BY a.id LIMIT 1",
            [item_key],
            |r| r.get::<_, i64>(0),
        )
        .optional()?)
}

/// 注釈の kind の許可表（DB の CHECK と同じ）
const KINDS: [&str; 4] = ["highlight", "note", "underline", "strikethrough"];

fn check_kind(kind: &str) -> Result<()> {
    if KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(CoreError::BadInput(format!("kind は highlight / note / underline / strikethrough のいずれか: {kind}")))
    }
}

pub fn add_annotation(db: &Db, item_key: &str, na: &NewAnnotation) -> Result<String> {
    check_kind(&na.kind)?;
    if na.author.trim().is_empty() {
        return Err(CoreError::BadInput("author 必須".into()));
    }
    if na.quote.as_deref().unwrap_or("").trim().is_empty()
        && na.comment.as_deref().unwrap_or("").trim().is_empty()
    {
        return Err(CoreError::BadInput("quote か comment のどちらかは必須".into()));
    }
    let att = first_pdf_attachment(db, item_key)?.ok_or_else(|| {
        CoreError::BadInput(format!(
            "item {item_key} に PDF 添付がないため注釈を付けられません（item 単位のメモは add_note を使ってください）"
        ))
    })?;
    let key = keys::new_key();
    db.conn.execute(
        "INSERT INTO annotations (key, attachment_id, author, kind, color, comment, page, quote, prefix, suffix)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            key,
            att,
            na.author,
            na.kind,
            na.color,
            na.comment,
            na.page,
            na.quote,
            na.prefix,
            na.suffix,
        ],
    )?;
    Ok(key)
}

pub fn list_annotations(db: &Db, item_key: &str) -> Result<Vec<AnnotationRow>> {
    let mut stmt = db.conn.prepare(
        "SELECT an.key, an.author, an.kind, an.color, an.comment, an.page, an.quote, an.prefix, an.suffix, an.date_added
         FROM annotations an
         JOIN attachments a ON a.id = an.attachment_id
         JOIN items i ON i.id = a.item_id
         WHERE i.key = ?1
         ORDER BY COALESCE(an.page, 0), an.id",
    )?;
    let rows = stmt.query_map([item_key], |r| {
        Ok(AnnotationRow {
            key: r.get(0)?,
            author: r.get(1)?,
            kind: r.get(2)?,
            color: r.get(3)?,
            comment: r.get(4)?,
            page: r.get(5)?,
            quote: r.get(6)?,
            prefix: r.get(7)?,
            suffix: r.get(8)?,
            date_added: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// 既存注釈の部分更新パッチ。フィールドごとに指定しなければ不変更。
/// comment / color は DB 上 NULL 許容のため三値表現（Option<Option<String>>）:
///   - `None`          … 不変更
///   - `Some(None)`     … NULL へ消去
///   - `Some(Some(s))`  … 値を `s` に設定（空文字も可。消去とは区別する）
/// kind は NOT NULL のため二値（`None` = 不変更 / `Some(k)` = allowlist 検証の上で変更）。
/// quote/page/anchor（prefix/suffix）は対象外（位置の付替えは非対応）。
#[derive(Debug, Default)]
pub struct AnnotationPatch {
    pub comment: Option<Option<String>>,
    pub color: Option<Option<String>>,
    pub kind: Option<String>,
}

pub fn update_annotation(db: &Db, key: &str, patch: &AnnotationPatch) -> Result<()> {
    if let Some(k) = &patch.kind {
        check_kind(k)?;
    }
    if patch.comment.is_none() && patch.color.is_none() && patch.kind.is_none() {
        return Err(CoreError::BadInput(
            "update_annotation: comment / color / kind のいずれかを指定してください".into(),
        ));
    }
    // 指定の無い列は自分の値のまま（?1 / ?3 = 指定ありか。NULL への消去と区別する）
    let n = db.conn.execute(
        "UPDATE annotations SET date_modified = datetime('now'),
           comment = CASE WHEN ?1 THEN ?2 ELSE comment END,
           color = CASE WHEN ?3 THEN ?4 ELSE color END,
           kind = COALESCE(?5, kind)
         WHERE key = ?6",
        rusqlite::params![
            patch.comment.is_some(),
            patch.comment.as_ref().and_then(|c| c.as_deref()),
            patch.color.is_some(),
            patch.color.as_ref().and_then(|c| c.as_deref()),
            patch.kind,
            key,
        ],
    )?;
    if n == 0 {
        return Err(CoreError::NotFound(format!("annotation {key}")));
    }
    Ok(())
}

pub fn delete_annotation(db: &Db, key: &str) -> Result<()> {
    let n = db.conn.execute("DELETE FROM annotations WHERE key = ?1", [key])?;
    if n == 0 {
        return Err(CoreError::NotFound(format!("annotation {key}")));
    }
    Ok(())
}

pub fn delete_note(db: &Db, key: &str) -> Result<()> {
    let n = db.conn.execute("DELETE FROM notes WHERE key = ?1", [key])?;
    if n == 0 {
        return Err(CoreError::NotFound(format!("note {key}")));
    }
    Ok(())
}

pub fn add_note(db: &Db, item_key: &str, author: &str, content_md: &str) -> Result<String> {
    if content_md.trim().is_empty() {
        return Err(CoreError::BadInput("content_md が空です".into()));
    }
    let item_id = crate::edit::item_id(db, item_key)?;
    let key = keys::new_key();
    db.conn.execute(
        "INSERT INTO notes (key, item_id, author, content_md) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![key, item_id, author, content_md],
    )?;
    Ok(key)
}

/// メモ本文を差し替える（UI の 1 item = 1 メモ編集用。空文字は BadInput — 消すなら delete_note）
pub fn update_note(db: &Db, key: &str, content_md: &str) -> Result<()> {
    if content_md.trim().is_empty() {
        return Err(CoreError::BadInput("content_md が空です（削除は delete_note）".into()));
    }
    let n = db.conn.execute(
        "UPDATE notes SET content_md = ?1, date_modified = datetime('now') WHERE key = ?2",
        rusqlite::params![content_md, key],
    )?;
    if n == 0 {
        return Err(CoreError::NotFound(format!("note {key}")));
    }
    Ok(())
}

pub fn list_notes(db: &Db, item_key: &str) -> Result<Vec<NoteRow>> {
    let mut stmt = db.conn.prepare(
        "SELECT n.key, n.author, n.content_md, n.date_added, n.date_modified
         FROM notes n JOIN items i ON i.id = n.item_id
         WHERE i.key = ?1 ORDER BY n.id",
    )?;
    let rows = stmt.query_map([item_key], |r| {
        Ok(NoteRow {
            key: r.get(0)?,
            author: r.get(1)?,
            content_md: r.get(2)?,
            date_added: r.get(3)?,
            date_modified: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
