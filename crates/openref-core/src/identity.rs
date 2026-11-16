//! 重複の判定と識別子の正規化(DOI・PMID・書誌の指紋)と、照合用の SQL 索引。
//! 登録・Capture・PDF 取込(`ingest`)・schema の migration v2(`db`)・ライブラリ移行(`import`)が共有する。
//! 2026-11-14 に `import/plan.rs` から移した(関数の中身は同じ = 移行のモジュールを外しても core の重複判定が立つ)。
use crate::{model, Result};
use rusqlite::Connection;
use serde_json::Value;

pub(crate) fn doi(csl: &Value) -> Option<String> {
    let d = model::normalize_doi(model::s(csl, "DOI")?);
    let (prefix, suffix) = d.split_once('/')?;
    let registrant = prefix.strip_prefix("10.")?;
    let valid = registrant.len() >= 4
        && registrant.bytes().all(|c| c.is_ascii_digit())
        && !suffix.is_empty()
        && !d.chars().any(char::is_whitespace);
    valid.then_some(d)
}

pub(crate) fn pmid(csl: &Value) -> Option<String> {
    if let Some(number) = csl.get("PMID").and_then(Value::as_u64) {
        return (number > 0).then(|| number.to_string());
    }
    let raw = model::s(csl, "PMID")?.trim();
    // IDs are strings; do not silently match invalid values or overflow integers.
    if raw.is_empty() || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let normalized = raw.trim_start_matches('0');
    (!normalized.is_empty()).then(|| normalized.to_string())
}

/// Native SQLite expressions keep old binaries able to maintain these indexes.
/// Unicode identifiers/whitespace use the small unusual-identifiers index and
/// the same Rust normalizer as preview; SQLite's lower() is ASCII-only.
pub(crate) fn identifier_sql() -> (String, String) {
    let whitespace = "char(9)||char(10)||char(11)||char(12)||char(13)||char(32)";
    let raw = format!("lower(trim(doi,{whitespace}))");
    let mut expression = "CASE".to_string();
    for prefix in model::DOI_PREFIXES {
        expression.push_str(&format!(
            " WHEN {raw} LIKE '{prefix}%' THEN trim(substr({raw},{}),{whitespace})",
            prefix.len() + 1
        ));
    }
    expression.push_str(&format!(" ELSE {raw} END"));
    (expression, format!("ltrim(trim(pmid,{whitespace}),'0')"))
}

pub(crate) fn create_identity_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_import_legacy_paperpile ON items(json_extract(csl,'$.custom.paperpile.id'));")?;
    let (doi, pmid) = identifier_sql();
    conn.execute_batch(&format!("CREATE INDEX idx_import_doi ON items({doi}) WHERE doi IS NOT NULL; CREATE INDEX idx_import_pmid ON items({pmid}) WHERE pmid IS NOT NULL; CREATE INDEX idx_import_unusual_ids ON items(key) WHERE doi GLOB '*[^ -~]*' OR pmid GLOB '*[^ -~]*';"))?;
    conn.execute_batch("CREATE TABLE import_match_keys(item_id INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,fingerprint TEXT,valid INTEGER NOT NULL DEFAULT 0); CREATE INDEX idx_import_match_keys ON import_match_keys(fingerprint) WHERE valid=1; CREATE INDEX idx_import_match_pending ON import_match_keys(item_id) WHERE valid=0; CREATE TRIGGER import_match_insert AFTER INSERT ON items BEGIN INSERT INTO import_match_keys(item_id,valid) VALUES(new.id,0) ON CONFLICT(item_id) DO UPDATE SET valid=0,fingerprint=NULL; END; CREATE TRIGGER import_match_invalidate AFTER UPDATE OF csl ON items BEGIN INSERT INTO import_match_keys(item_id,valid) VALUES(new.id,0) ON CONFLICT(item_id) DO UPDATE SET valid=0,fingerprint=NULL; END;")?;
    let rows: Vec<(i64, String)> = conn
        .prepare("SELECT id,csl FROM items")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for (id, csl) in rows {
        index_descriptive(conn, id, &serde_json::from_str(&csl)?)?;
    }
    Ok(())
}

pub(crate) fn index_descriptive(conn: &Connection, id: i64, csl: &Value) -> rusqlite::Result<()> {
    // During v2/v6 backfill the derived table does not exist yet.
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='import_match_keys')",
        [],
        |r| r.get(0),
    )?;
    if exists {
        let fingerprint = descriptive(csl).map(|s| crate::import::files::digest(s.as_bytes()));
        conn.execute("INSERT INTO import_match_keys(item_id,fingerprint,valid) VALUES(?1,?2,1) ON CONFLICT(item_id) DO UPDATE SET fingerprint=excluded.fingerprint,valid=1",rusqlite::params![id,fingerprint])?;
    }
    Ok(())
}

pub(crate) fn descriptive(csl: &Value) -> Option<String> {
    let title = model::journal_norm(model::s(csl, "title")?);
    let authors = model::authors(csl);
    let year = model::year(csl)?;
    if title.is_empty() || authors.is_empty() {
        return None;
    }
    Some(
        serde_json::to_string(&(
            title,
            authors
                .iter()
                .map(|s| model::journal_norm(s))
                .collect::<Vec<_>>(),
            year,
        ))
        .unwrap(),
    )
}

pub(crate) fn conflicts(a: &Value, b: &Value) -> bool {
    [(doi(a), doi(b)), (pmid(a), pmid(b))]
        .into_iter()
        .any(|(a, b)| matches!((a, b), (Some(a), Some(b)) if a != b))
}

/// Shared by migration and automatic ingestion, under their writer transaction.
pub(crate) fn identifier_candidates(
    conn: &Connection,
    csl: &Value,
    mapped: Option<&str>,
) -> Result<Vec<(String, Value, bool)>> {
    let doi = self::doi(csl);
    let pmid = self::pmid(csl);
    if mapped.is_none() && doi.is_none() && pmid.is_none() {
        return Ok(Vec::new());
    }
    let (doi_sql, pmid_sql) = identifier_sql();
    let sql=format!("SELECT key,csl,deleted_at IS NOT NULL FROM items WHERE key=?1 OR (doi IS NOT NULL AND {doi_sql}=?2) OR (pmid IS NOT NULL AND {pmid_sql}=?3)");
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut candidates: Vec<(String, Value, bool)> = stmt
        .query_map(rusqlite::params![mapped, doi, pmid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?
        .map(|r| {
            let (key, csl, deleted) = r?;
            Ok((key, serde_json::from_str(&csl)?, deleted))
        })
        .collect::<Result<_>>()?;
    let mut unusual=conn.prepare_cached("SELECT key,csl,deleted_at IS NOT NULL FROM items WHERE doi GLOB '*[^ -~]*' OR pmid GLOB '*[^ -~]*'")?;
    for row in unusual.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, bool>(2)?,
        ))
    })? {
        let (key, csl, deleted) = row?;
        let csl: Value = serde_json::from_str(&csl)?;
        if !candidates.iter().any(|c| c.0 == key)
            && ((doi.is_some() && doi == self::doi(&csl))
                || (pmid.is_some() && pmid == self::pmid(&csl)))
        {
            candidates.push((key, csl, deleted));
        }
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SQL の索引の式（items.doi = 生成列で小文字済み）と Rust の正規化が同じ値を出す
    #[test]
    fn identifier_sql_matches_rust_normalizer() {
        let conn = Connection::open_in_memory().unwrap();
        let (doi_sql, pmid_sql) = identifier_sql();
        for raw in [
            "10.1148/rg.230216",
            " 10.1148/RG.230216 ",
            "https://doi.org/10.1148/rg.230216",
            "HTTP://DOI.ORG/10.1148/rg.230216",
            "https://dx.doi.org/ 10.1148/rg.230216",
            "http://dx.doi.org/10.1148/rg.230216",
            "doi: 10.1148/rg.230216",
            "https://doi.org/doi:10.1/x",
            "https://www.doi.org/10.1/x",
        ] {
            let sql: String = conn
                .query_row(&format!("SELECT {doi_sql} FROM (SELECT lower(?1) AS doi)"), [raw], |r| r.get(0))
                .unwrap();
            assert_eq!(sql, model::normalize_doi(raw), "{raw:?}");
        }
        for (raw, want) in [("00123", "123"), (" 123 ", "123")] {
            let sql: String = conn
                .query_row(&format!("SELECT {pmid_sql} FROM (SELECT ?1 AS pmid)"), [raw], |r| r.get(0))
                .unwrap();
            assert_eq!(sql, want);
            assert_eq!(pmid(&serde_json::json!({ "PMID": raw })).as_deref(), Some(want));
        }
    }
}
