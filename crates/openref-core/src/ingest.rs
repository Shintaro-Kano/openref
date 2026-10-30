use crate::{keys, model, resolver::Resolved, CoreError, Db, Result};

#[cfg(test)]
mod tests {
    use super::titles_match;

    #[test]
    fn title_identity_guard() {
        // 同一（副題・句読点差）
        assert!(titles_match(
            "The Peritoneum: Anatomy, Pathologic Findings, and Patterns of Disease Spread",
            "The peritoneum - anatomy, pathologic findings and patterns of disease spread"
        ));
        // 部分タイトル（connector が副題を落とす例）
        assert!(titles_match(
            "Imaging Spectrum of Cavernous Sinus Lesions",
            "Imaging Spectrum of Cavernous Sinus Lesions with Histopathologic Correlation"
        ));
        // 別論文（live 検証で実際に混入したケース）
        assert!(!titles_match(
            "Imaging Spectrum of Cavernous Sinus Lesions with Histopathologic Correlation",
            "COVID-19 Vaccination-related Axillary Lymphadenopathy Mimicking Metastasis"
        ));
        // 空は判定不能 → 拒否
        assert!(!titles_match("", "Anything"));
    }
}

#[derive(Debug)]
pub struct Added {
    pub id: i64,
    pub key: String,
}

/// item 登録の一気通貫: dedup 照合 → items + creators_index + FTS + 撤回フラグ（単一 tx）。
pub fn add_item(db: &mut Db, resolved: &Resolved) -> Result<Added> {
    let csl = &resolved.csl;

    // Match and insert under the same writer reservation as library migration.
    let tx = db
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some((existing_key, field)) = find_duplicate(&tx, csl)? {
        return Err(CoreError::Duplicate {
            existing_key,
            field,
        });
    }

    let key = keys::new_key();

    tx.execute(
        "INSERT INTO items (key, csl) VALUES (?1, ?2)",
        rusqlite::params![key, csl.to_string()],
    )?;
    let id = tx.last_insert_rowid();
    index_item(&tx, id, csl)?;

    if let Some(detail) = &resolved.retracted {
        upsert_retraction(&tx, id, detail, resolved.source)?;
    }

    tx.commit()?;
    Ok(Added { id, key })
}

/// 撤回の印を書く（resolver が撤回を返したときだけ呼ぶ）。既にあれば detail / source / checked_at を今回の値に更新する。
/// 戻り値 = 印が新しく付いた・中身が変わった。
///
/// resolver の `retracted: None` の意味は経路で違う: PMID のある item（source = pubmed）なら「PubMed は撤回と言っていない」だが、
/// DOI だけの item は `resolver::resolve` が PubMed の照合で該当なし（非収載）のときと、照合そのものに失敗したとき（通信失敗。
/// 失敗は `Resolved.warnings` と stderr に残る）に Crossref / 医書.jp へ落ちる
/// （どちらも撤回を返さない = 常に None）ので、None が「撤回を見ていない」を意味しうる。
/// → None では既存の印を消さない（撤回の警告を黙って外す方が危ない）。**印は自動では一度も消えない。消えるのは item を
/// 削除したときの ON DELETE CASCADE だけ**。印を付けるのは取込（add_item）と enrich（merge_enrichment = refresh・connector の補完）
fn upsert_retraction(
    conn: &rusqlite::Connection,
    id: i64,
    detail: &str,
    source: &str,
) -> rusqlite::Result<bool> {
    use rusqlite::OptionalExtension;
    let before: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT status, detail, source FROM retraction_flags WHERE item_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    conn.execute(
        "INSERT INTO retraction_flags (item_id, status, detail, source) VALUES (?1, 'retracted', ?2, ?3)
         ON CONFLICT(item_id) DO UPDATE SET status = excluded.status, detail = excluded.detail,
           source = excluded.source, checked_at = datetime('now')",
        rusqlite::params![id, detail, source],
    )?;
    Ok(before != Some(("retracted".to_string(), Some(detail.to_string()), Some(source.to_string()))))
}

/// creators_index + items_fts への索引付け（insert 専用。update は delete+insert で行う）
pub(crate) fn index_item(
    conn: &rusqlite::Connection,
    id: i64,
    csl: &serde_json::Value,
) -> rusqlite::Result<()> {
    if let Some(arr) = csl.get("author").and_then(|a| a.as_array()) {
        let mut stmt = conn.prepare(
            "INSERT INTO creators_index (item_id, role, position, family, given, literal)
             VALUES (?1, 'author', ?2, ?3, ?4, ?5)",
        )?;
        for (i, a) in arr.iter().enumerate() {
            stmt.execute(rusqlite::params![
                id,
                i as i64,
                a.get("family").and_then(|v| v.as_str()),
                a.get("given").and_then(|v| v.as_str()),
                a.get("literal").and_then(|v| v.as_str()),
            ])?;
        }
    }
    index_journal(conn, id, csl)?;
    index_fts(conn, id, csl)?;
    Ok(())
}

/// journal_keys を貼り替える（ISSN / 正規化誌名。CSL を書き換える経路は必ずここを通す: add / update / enrich / migration v6）
pub(crate) fn index_journal(
    conn: &rusqlite::Connection,
    id: i64,
    csl: &serde_json::Value,
) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM journal_keys WHERE item_id = ?1", [id])?;
    let mut stmt = conn.prepare("INSERT OR IGNORE INTO journal_keys (item_id, key) VALUES (?1, ?2)")?;
    for k in crate::model::journal_keys(csl) {
        stmt.execute(rusqlite::params![id, k])?;
    }
    Ok(())
}

/// items_fts への 1 行挿入。cjk 列 = CJK per-char 分かち書き（和文の部分一致検索用）。
pub(crate) fn index_fts(
    conn: &rusqlite::Connection,
    id: i64,
    csl: &serde_json::Value,
) -> rusqlite::Result<()> {
    crate::import::plan::index_descriptive(conn, id, csl)?;
    let creators_txt = model::authors(csl).join("; ");
    let mut kw: Vec<String> = Vec::new();
    if let Some(k) = model::s(csl, "keyword") {
        kw.push(k.to_string());
    }
    if let Some(mesh) = csl.pointer("/custom/mesh").and_then(|m| m.as_array()) {
        kw.extend(mesh.iter().filter_map(|x| x.as_str().map(String::from)));
    }
    let title = model::s(csl, "title").unwrap_or("");
    let container = model::s(csl, "container-title").unwrap_or("");
    let kw_joined = kw.join("; ");
    let cjk_src = [title, &creators_txt, container, &kw_joined].join(" ");
    let cjk = if model::has_cjk(&cjk_src) {
        model::cjk_spaced(&cjk_src)
    } else {
        String::new()
    };
    conn.execute(
        "INSERT INTO items_fts (rowid, title, abstract, creators, container_title, keywords, cjk)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            id,
            title,
            model::s(csl, "abstract").unwrap_or(""),
            creators_txt,
            container,
            kw_joined,
            cjk,
        ],
    )?;
    Ok(())
}

#[derive(Debug)]
pub enum RefreshOutcome {
    Updated,
    Unchanged,
    /// PMID も DOI も無く、権威データを引く手掛かりがない
    NoIdentifier,
    /// 識別子の指す論文が別物だった（同一性ガード発動）
    Rejected(String),
}

/// 既存 item の書誌を PubMed / doi.org で引き直して是正する（connector 由来の壊れた著者などの修復）。
pub fn refresh_item(db: &mut Db, key: &str) -> Result<RefreshOutcome> {
    use rusqlite::OptionalExtension;
    let row: Option<(i64, String)> = db
        .conn
        .query_row(
            "SELECT id, csl FROM items WHERE key = ?1 AND deleted_at IS NULL",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (id, csl_s) = row.ok_or_else(|| CoreError::NotFound(format!("item {key}")))?;
    let csl: serde_json::Value = serde_json::from_str(&csl_s)?;

    let input = match model::s(&csl, "PMID") {
        Some(p) if !p.is_empty() => crate::resolver::AddInput::Pmid(p.to_string()),
        _ => match model::s(&csl, "DOI") {
            Some(d) if !d.is_empty() => crate::resolver::AddInput::Doi(d.to_string()),
            _ => return Ok(RefreshOutcome::NoIdentifier),
        },
    };
    let resolved = crate::resolver::resolve(&input)?;
    match merge_enrichment(db, id, &resolved) {
        Ok(true) => Ok(RefreshOutcome::Updated),
        Ok(false) => Ok(RefreshOutcome::Unchanged),
        Err(CoreError::BadInput(msg)) => Ok(RefreshOutcome::Rejected(msg)),
        Err(e) => Err(e),
    }
}

/// title の同一性判定: 英数小文字の語集合の Jaccard 係数で照合（部分タイトル・句読点差を吸収）。
/// どちらかが空なら判定不能として false（安全側）。
pub fn titles_match(a: &str, b: &str) -> bool {
    use std::collections::HashSet;
    let norm = |s: &str| -> HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(String::from)
            .collect()
    };
    let (sa, sb) = (norm(a), norm(b));
    if sa.is_empty() || sb.is_empty() {
        return false;
    }
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    // 片方が他方の部分集合に近い（副題の有無等）ケースも許容
    let containment = inter / sa.len().min(sb.len()) as f64;
    inter / union >= 0.5 || containment >= 0.8
}

/// 索引（creators_index / journal_keys / FTS / import_match_keys）を今の CSL から丸ごと貼り替える。
/// CSL を書き換える経路はこれを直接呼ばず、必ず `replace_csl` を通す。直接呼ぶのは CSL を変えない修復（`reindex_keys`）だけ
pub fn reindex_item(
    conn: &rusqlite::Connection,
    id: i64,
    csl: &serde_json::Value,
) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM creators_index WHERE item_id = ?1", [id])?;
    conn.execute("DELETE FROM items_fts WHERE rowid = ?1", [id])?;
    index_item(conn, id, csl) // creators_index / journal_keys / FTS（+ import_match_keys）
}

/// 索引（creators_index / journal_keys / FTS / import_match_keys）を今の CSL から貼り直す（CSL・date_modified は触らない）。
/// 索引がずれた item の修復用（`openref reindex`）。1 tx・知らない key が 1 つでもあれば何も書かない。
/// 戻り値 = 貼り直した item の数（同じ key を重ねて渡しても 1 件）
pub fn reindex_keys(db: &mut Db, keys: &[String]) -> Result<usize> {
    use rusqlite::OptionalExtension;
    let keys: std::collections::BTreeSet<&String> = keys.iter().collect();
    let tx = db.conn.transaction()?;
    for key in &keys {
        let (id, csl_s): (i64, String) = tx
            .query_row("SELECT id, csl FROM items WHERE key = ?1", [key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("item {key}")))?;
        reindex_item(&tx, id, &serde_json::from_str(&csl_s)?)?;
    }
    tx.commit()?;
    Ok(keys.len())
}

/// 既存 item の CSL を書き換え、索引（creators_index / journal_keys / FTS / import_match_keys）を丸ごと貼り替える。
/// CSL を書き換える経路（編集・enrich・取込の merge）は必ずここを通す（索引の貼り忘れを構造で防ぐ）
pub fn replace_csl(
    conn: &rusqlite::Connection,
    id: i64,
    csl: &serde_json::Value,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE items SET csl = ?1, date_modified = datetime('now') WHERE id = ?2",
        rusqlite::params![csl.to_string(), id],
    )?;
    reindex_item(conn, id, csl)
}

/// レジストリ（PubMed / Crossref）が権威を持つフィールド。
/// connector 由来のデータは誌面 HTML のスクレイピングなので、著者の姓名分割が壊れる
/// （例: "Houshmand S" → family="S", given="Houshmand"／そもそも著者ゼロ）。
/// 構造化レジストリの値が取れたなら、それで**置き換える**のが正しい。
const AUTHORITATIVE_FIELDS: &[&str] = &[
    "author",
    "container-title",
    "container-title-short",
    "volume",
    "issue",
    "page",
    "issued",
    "ISSN",
    "DOI",
    "PMID",
    "PMCID",
    "abstract",
    "keyword",
    "language",
];

/// 既存 item を resolver（PubMed / doi.org）の権威データで是正する（connector 保存後の非同期 enrich・`openref refresh`）。
/// 権威フィールドは上書き、それ以外（title・URL 等）は既存を保持。変更時は `replace_csl` で 4 索引（creators_index / journal_keys / FTS / 取込照合の import_match_keys）をすべて貼り替える。
/// resolver が撤回を返していれば撤回の印も書く（`upsert_retraction`。撤回の照会のために要求を増やさない = 同じ応答を使う）。
/// 戻り値 = CSL か撤回の印のどちらかが変わった。
///
/// **同一性ガード（必須）**: connector が渡す DOI/PMID が誤っていると、別論文の書誌が
/// 混入した「キメラレコード」が黙って生まれる（live 検証で実際に発生）。title 類似度で照合し、
/// 一致しない場合は enrich を拒否する（捏造データを作らない = 誠実性ポリシー）。
pub fn merge_enrichment(db: &mut Db, id: i64, resolved: &Resolved) -> Result<bool> {
    let richer = &resolved.csl;
    let csl_s: String = db
        .conn
        .query_row("SELECT csl FROM items WHERE id = ?1", [id], |r| r.get(0))?;
    let mut csl: serde_json::Value = serde_json::from_str(&csl_s)?;

    let existing_title = model::s(&csl, "title").unwrap_or("");
    let richer_title = model::s(richer, "title").unwrap_or("");
    if !titles_match(existing_title, richer_title) {
        return Err(CoreError::BadInput(format!(
            "enrich 中止: 識別子の指す論文が一致しません（既存「{}」/ 取得「{}」）。connector 由来の DOI/PMID が誤っている可能性があります",
            model::truncate_chars(existing_title, 50),
            model::truncate_chars(richer_title, 50)
        )));
    }

    let mut changed = false;
    {
        let Some(obj) = csl.as_object_mut() else {
            return Ok(false);
        };
        for k in AUTHORITATIVE_FIELDS {
            let Some(v) = richer.get(*k) else { continue };
            if v.is_null() {
                continue;
            }
            if obj.get(*k) != Some(v) {
                obj.insert(k.to_string(), v.clone());
                changed = true;
            }
        }
        // type もレジストリに従う: connector 由来の type は誌面の着地ページの種別を写しただけ（RSNA の epdf ページ →
        // webpage）で書誌の種類ではない（PubMed で補完された後も webpage のまま残さない）。PubMed は常に article-journal、Crossref は journal-article → article-journal に正規化済み。
        // Crossref のそれ以外は生の Crossref 名（book-chapter 等）のまま来るので、types の既知の id だけ受ける
        if let Some(t) = richer.get("type").and_then(|t| t.as_str()) {
            if crate::types::type_def(t).is_some() && obj.get("type").and_then(|x| x.as_str()) != Some(t) {
                obj.insert("type".to_string(), serde_json::json!(t));
                changed = true;
            }
        }
        let custom = obj
            .entry("custom")
            .or_insert_with(|| serde_json::json!({}));
        if let (Some(co), Some(rc)) = (custom.as_object_mut(), richer.get("custom").and_then(|c| c.as_object())) {
            for k in ["mesh", "publication-types"] {
                if co.get(k).is_none() {
                    if let Some(v) = rc.get(k) {
                        co.insert(k.to_string(), v.clone());
                        changed = true;
                    }
                }
            }
        }
    }
    let flagged = resolved.retracted.is_some();
    if changed || flagged {
        let tx = db.conn.transaction()?;
        if changed {
            replace_csl(&tx, id, &csl)?; // enrich は author / container-title / ISSN も差し替えうる
        }
        if let Some(detail) = &resolved.retracted {
            changed |= upsert_retraction(&tx, id, detail, resolved.source)?;
        }
        tx.commit()?;
    }
    Ok(changed)
}

pub(crate) fn find_duplicate(
    conn: &rusqlite::Connection,
    csl: &serde_json::Value,
) -> Result<Option<(String, &'static str)>> {
    let candidates = crate::import::plan::identifier_candidates(conn, csl, None)?;
    let pmid = crate::import::plan::pmid(csl);
    let doi = crate::import::plan::doi(csl);
    for (field, value) in [("PMID", pmid), ("DOI", doi)] {
        if let Some(value) = value {
            if let Some((key, _, _)) = candidates.iter().find(|(_, csl, deleted)| {
                !*deleted
                    && (if field == "PMID" {
                        crate::import::plan::pmid(csl)
                    } else {
                        crate::import::plan::doi(csl)
                    })
                    .as_deref()
                        == Some(&value)
            }) {
                return Ok(Some((key.clone(), field)));
            }
        }
    }
    Ok(None)
}
