//! Paperpile JSON export の一括 import（初回 seed）。
//! API-free 決定論: export 内の書誌をそのまま CSL-JSON 化する（PubMed 再確定はしない）。
//! 既存 item と PMID/DOI が一致した場合は skip でなく **merge**（folder/添付/由来を既存へ付与）。

use crate::{ingest, keys, model, CoreError, Db, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Normalize without opening a database or contacting any network service.
/// Current array/attachment shape verified with a 2026-10-27 Paperpile export.
/// A second observed export includes labels/labelsNamed. Folder hierarchy and
/// notes remain unverified rather than guessed from the old importer.
pub fn read_input(json_path: &Path, source: super::ImportSource) -> Result<super::ImportInput> {
    use std::collections::BTreeMap;
    if source.format != "paperpile" {
        return Err(CoreError::BadInput(
            "Paperpile adapter の移行元形式が一致しません".into(),
        ));
    }
    let bytes = std::fs::read(json_path)?;
    let data: Value = serde_json::from_slice(&bytes)?;
    let records = data
        .as_array()
        .ok_or_else(|| CoreError::Parse("Paperpile JSON: 配列ではありません".into()))?;
    let mut folders = BTreeMap::new();
    let mut items = Vec::with_capacity(records.len());
    for (position, rec) in records.iter().enumerate() {
        if !rec.is_object() {
            return Err(CoreError::Parse(format!(
                "Paperpile JSON: レコード {} がオブジェクトではありません",
                position + 1
            )));
        }
        let mut warnings = Vec::new();
        let (csl, folder_paths) = match record_to_csl(rec) {
            Some((csl, extras)) => (csl, extras.folders),
            None => {
                warnings.push(
                    "題名のない元レコードを document として保持します。書誌は補っていません".into(),
                );
                (json!({"type":"document"}), Vec::new())
            }
        };
        let mut folder_ids = Vec::new();
        // Only the explicitly named legacy path field is split. Current opaque
        // folder IDs and Zotero collection names must never be split on '/'.
        for path in folder_paths {
            let mut parent_id = None;
            let mut current = String::new();
            for name in path.split('/').map(str::trim).filter(|s| !s.is_empty()) {
                if !current.is_empty() {
                    current.push('/');
                }
                current.push_str(name);
                let id = format!("legacy-path:{current}");
                folders
                    .entry(id.clone())
                    .or_insert_with(|| super::ImportFolder {
                        id: id.clone(),
                        stable_id: false,
                        name: name.into(),
                        parent_id: parent_id.clone(),
                        raw: json!({"foldersNamed": path}),
                    });
                parent_id = Some(id);
            }
            if let Some(id) = parent_id {
                folder_ids.push(id);
            }
        }
        for field in ["folders", "notes"] {
            if rec.get(field).is_some_and(|v| {
                !v.is_null() && v != &json!([]) && v != &json!({}) && v != &json!("")
            }) {
                warnings.push(format!(
                    "{field}: 実出力の形式が未検証のため未変換です。元レコードに保持しています"
                ));
            }
        }
        let mut tags = Vec::new();
        let label_names = rec.get("labelsNamed").and_then(Value::as_array);
        let label_ids = rec.get("labels").and_then(Value::as_array);
        if matches!((label_names,label_ids),(Some(names),Some(ids)) if names.len()!=ids.len()) {
            warnings.push("labels と labelsNamed の件数が異なるため、名前でタグを保持し、ID 対応は原情報に残します".into());
        }
        if let Some(names) = label_names {
            for (n, name) in names.iter().enumerate() {
                let Some(name) = name.as_str().filter(|s| !s.trim().is_empty()) else {
                    warnings.push("空または未対応の labelsNamed を元レコードに保持しました".into());
                    continue;
                };
                let original_id = label_ids
                    .filter(|ids| ids.len() == names.len())
                    .and_then(|ids| ids[n].as_str());
                tags.push(super::ImportTag {
                    id: original_id.map(str::to_string).unwrap_or_else(|| {
                        format!("label-name:{}", super::files::digest(name.as_bytes()))
                    }),
                    stable_id: false,
                    name: name.into(),
                    color: None,
                    raw: json!({"id":original_id,"name":name}),
                });
            }
        }
        if label_ids.is_some_and(|ids| !ids.is_empty()) && tags.is_empty() {
            warnings.push(
                "labels: 名前を取得できないため未変換です。元レコードに保持しています".into(),
            );
        }
        let handled = [
            "_id",
            "title",
            "abstract",
            "author",
            "journalfull",
            "journal",
            "issn",
            "volume",
            "issue",
            "pages",
            "publisher",
            "language",
            "pmid",
            "pmc",
            "keywords",
            "doi",
            "url",
            "published",
            "accessed",
            "foldersNamed",
            "folders",
            "notes",
            "labels",
            "labelsNamed",
            "attachments",
            "created",
            "citekey",
            "sha1",
            "kind",
            "pubtype",
        ];
        let internal = ["owner", "dup_sha1", "incomplete", "pdf_restricted"];
        let unmapped: Vec<_> = rec
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, value)| {
                !handled.contains(&key.as_str())
                    && !internal.contains(&key.as_str())
                    && !value.is_null()
                    && **value != json!("")
                    && **value != json!([])
            })
            .map(|(key, _)| key.as_str())
            .collect();
        if !unmapped.is_empty() {
            warnings.push(format!(
                "UI/引用に未反映のフィールドを元レコードに保持: {}",
                unmapped.join(", ")
            ));
        }
        if rec
            .get("url")
            .and_then(Value::as_array)
            .is_some_and(|urls| urls.len() > 1)
        {
            warnings.push("URL は先頭を引用用に採用し、残りを元レコードに保持します".into());
        }
        if !matches!(
            rec.get("pubtype").and_then(Value::as_str),
            None | Some("PP_ARTICLE")
        ) {
            warnings.push("論文以外の Paperpile 種別は実標本が未取得です。未反映の書誌項目は元レコードに保持しています".into());
        }
        let mut attachments = Vec::new();
        if let Some(atts) = rec.get("attachments") {
            let atts = atts.as_array().ok_or_else(|| {
                CoreError::Parse(format!(
                    "レコード {} の attachments が配列ではありません",
                    position + 1
                ))
            })?;
            for (n, att) in atts.iter().enumerate() {
                let filename = att.get("filename").and_then(Value::as_str).unwrap_or("");
                // Verified relationship in the sample: JSON All Papers/K/...,
                // ZIP Paperpile files/K/.... Keep the complete relative suffix.
                let alternative_paths = zip_layout_paths(filename);
                attachments.push(super::ImportAttachment {
                    stable_id: false,
                    id: att
                        .get("_id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("attachment:{n}")),
                    path: filename.into(),
                    alternative_paths,
                    media_type: att
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    raw: att.clone(),
                });
            }
        }
        items.push(super::ImportItem {
            id: rec
                .get("_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("record:{position}")),
            // A single export proves presence, not stability across re-exports.
            stable_id: false,
            csl,
            date_added: rec
                .get("created")
                .and_then(Value::as_f64)
                .and_then(unix_to_sqlite),
            raw: rec.clone(),
            folder_ids,
            tags,
            notes: Vec::new(),
            attachments,
            warnings,
        });
    }
    let has_tags = items.iter().any(|item| !item.tags.is_empty());
    let mut input=super::ImportInput {
        source, adopt_legacy_paperpile_ids:false, input_sha256: super::files::digest(&bytes), adapter_version: "paperpile-array-v1".into(),
        items, folders: folders.into_values().collect(),
        warnings: vec![
            "Paperpile の現行配列形式と labelsNamed のタグ名を確認済み。フォルダ階層・ノート・空フォルダ・独立添付は未検証です".into(),
            "元レコードと内部 CSL は別々に保持します。この事前確認では DB への保存は行いません".into(),
            "内容変更後の再書出しにおける元 ID の安定性は未検証です。同一入力ダイジェスト内だけで照合します".into(),
        ],
    };
    if has_tags {
        input.warnings.push(
            "元のタグ色は JSON に含まれないため、OpenRef 側の既定色または既存の色を使用します"
                .into(),
        );
    }
    Ok(input)
}

#[derive(Debug)]
pub struct ImportReport {
    pub total: usize,
    pub inserted: usize,
    pub merged: usize,
    pub skipped_bad: usize,
    pub without_ids: usize,
    pub collections_created: usize,
    pub pdf_linked: usize,
    pub pdf_missing: Vec<String>,
    /// リンクした PDF のうち本文を索引できた本数（dry-run は 0）
    pub fulltext_indexed: usize,
    /// リンクしたが本文を索引しなかった本数（テキスト層なし / 抽出失敗 / 未ダウンロード。`openref fulltext index` で再試行可）
    pub fulltext_unindexed: usize,
    pub dry_run: bool,
}

pub struct Extras {
    pub folders: Vec<String>,
    pub filename: Option<String>,
    pub created_sqlite: Option<String>,
}

pub fn import(
    db: &mut Db,
    json_path: &Path,
    mount: Option<PathBuf>,
    dry_run: bool,
) -> Result<ImportReport> {
    let raw = std::fs::read_to_string(json_path)?;
    let data: Value = serde_json::from_str(&raw)?;
    let records = data
        .as_array()
        .ok_or_else(|| CoreError::Parse("Paperpile JSON: 配列ではありません".into()))?;

    let mut report = ImportReport {
        total: records.len(),
        inserted: 0,
        merged: 0,
        skipped_bad: 0,
        without_ids: 0,
        collections_created: 0,
        pdf_linked: 0,
        pdf_missing: Vec::new(),
        fulltext_indexed: 0,
        fulltext_unindexed: 0,
        dry_run,
    };

    let tx = db.conn.transaction()?;
    let mut col_cache: HashMap<String, i64> = HashMap::new();
    // 本文索引は commit 後に 1 本ずつ（抽出中に書込ロックを握り続けて connectd / GUI を database is locked にしない）
    let mut pending_fulltext: Vec<(i64, PathBuf)> = Vec::new();

    for rec in records {
        let Some((csl, extras)) = record_to_csl(rec) else {
            report.skipped_bad += 1;
            continue;
        };
        let pmid = model::s(&csl, "PMID").map(str::to_string);
        let doi = model::s(&csl, "DOI").map(|d| d.to_lowercase());
        let pp_id = csl
            .pointer("/custom/paperpile/id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if pmid.is_none() && doi.is_none() {
            report.without_ids += 1;
        }

        let item_id = match find_existing(&tx, pmid.as_deref(), doi.as_deref(), pp_id.as_deref())? {
            Some((id, csl_s)) => {
                if let Ok(mut existing) = serde_json::from_str::<Value>(&csl_s) {
                    if let Some(pp) = csl.pointer("/custom/paperpile") {
                        merge_custom_paperpile(&mut existing, pp.clone());
                        tx.execute(
                            "UPDATE items SET csl = ?1, date_modified = datetime('now') WHERE id = ?2",
                            rusqlite::params![existing.to_string(), id],
                        )?;
                    }
                }
                report.merged += 1;
                id
            }
            None => {
                let key = keys::new_key();
                match &extras.created_sqlite {
                    Some(ts) => tx.execute(
                        "INSERT INTO items (key, csl, date_added, date_modified) VALUES (?1, ?2, ?3, ?3)",
                        rusqlite::params![key, csl.to_string(), ts],
                    )?,
                    None => tx.execute(
                        "INSERT INTO items (key, csl) VALUES (?1, ?2)",
                        rusqlite::params![key, csl.to_string()],
                    )?,
                };
                let id = tx.last_insert_rowid();
                ingest::index_item(&tx, id, &csl)?;
                report.inserted += 1;
                id
            }
        };

        for folder in &extras.folders {
            let cid = ensure_collection_path(&tx, folder, &mut col_cache, &mut report.collections_created)?;
            tx.execute(
                "INSERT OR IGNORE INTO collection_items (collection_id, item_id) VALUES (?1, ?2)",
                rusqlite::params![cid, item_id],
            )?;
        }

        // 添付リンク（item がまだ添付を持たない場合のみ）
        let has_att: i64 = tx.query_row(
            "SELECT COUNT(*) FROM attachments WHERE item_id = ?1",
            [item_id],
            |r| r.get(0),
        )?;
        if has_att == 0 {
            if let Some(f) = extras.filename.as_deref() {
                match mount.as_deref().and_then(|m| resolve_in_mount(m, f)) {
                    Some(abs) => {
                        tx.execute(
                            "INSERT INTO attachments (key, item_id, kind, mode, path) VALUES (?1, ?2, 'pdf', 'linked', ?3)",
                            rusqlite::params![keys::new_key(), item_id, abs.to_string_lossy()],
                        )?;
                        report.pdf_linked += 1;
                        pending_fulltext.push((tx.last_insert_rowid(), abs));
                    }
                    None => report.pdf_missing.push(f.to_string()),
                }
            }
        }
    }

    if dry_run {
        tx.rollback()?;
    } else {
        tx.commit()?;
        // 本文の全文索引（dry-run では抽出しない。失敗・空・未ダウンロードは件数だけ数える。1 本ごとに autocommit）
        for (att_id, abs) in pending_fulltext {
            match crate::fulltext::index_attachment(&db.conn, att_id) {
                Ok(crate::fulltext::IndexOutcome::Indexed { .. }) => report.fulltext_indexed += 1,
                Ok(_) => report.fulltext_unindexed += 1,
                Err(e) => {
                    eprintln!("fulltext: {} の本文索引でエラー: {e}", abs.display());
                    report.fulltext_unindexed += 1;
                }
            }
        }
    }
    Ok(report)
}

/// Paperpile record → (CSL-JSON, extras)。title 欠落のみ不採用。
pub fn record_to_csl(rec: &Value) -> Option<(Value, Extras)> {
    let title = rec.get("title").and_then(|v| v.as_str())?.trim();
    if title.is_empty() {
        return None;
    }
    let mut m = Map::new();
    m.insert(
        "type".into(),
        json!(map_pubtype(rec.get("pubtype").and_then(|v| v.as_str()))),
    );
    m.insert("title".into(), json!(title));

    if let Some(a) = rec.get("abstract").and_then(|v| v.as_str()) {
        m.insert("abstract".into(), json!(a));
    }

    let mut authors = Vec::new();
    if let Some(arr) = rec.get("author").and_then(|v| v.as_array()) {
        for a in arr {
            if let Some(last) = a.get("last").and_then(|v| v.as_str()) {
                let mut o = Map::new();
                o.insert("family".into(), json!(last));
                if let Some(first) = a.get("first").and_then(|v| v.as_str()) {
                    o.insert("given".into(), json!(first));
                }
                authors.push(Value::Object(o));
            } else if let Some(lit) = a
                .get("collective")
                .and_then(|v| v.as_str())
                .or_else(|| a.get("formatted").and_then(|v| v.as_str()))
            {
                authors.push(json!({ "literal": lit }));
            }
        }
    }
    if !authors.is_empty() {
        m.insert("author".into(), Value::Array(authors));
    }

    let journal_full = rec.get("journalfull").and_then(|v| v.as_str());
    let journal_abbr = rec.get("journal").and_then(|v| v.as_str());
    if let Some(j) = journal_full.or(journal_abbr) {
        m.insert("container-title".into(), json!(j));
    }
    if let (Some(_), Some(abbr)) = (journal_full, journal_abbr) {
        m.insert("container-title-short".into(), json!(abbr));
    }

    for (csl_key, pp_key) in [
        ("ISSN", "issn"),
        ("volume", "volume"),
        ("issue", "issue"),
        ("page", "pages"),
        ("publisher", "publisher"),
        ("language", "language"),
        ("PMID", "pmid"),
        ("PMCID", "pmc"),
        ("keyword", "keywords"),
    ] {
        if let Some(v) = rec.get(pp_key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                m.insert(csl_key.into(), json!(v));
            }
        }
    }
    if let Some(d) = rec.get("doi").and_then(|v| v.as_str()) {
        if !d.is_empty() {
            m.insert("DOI".into(), json!(d.to_lowercase()));
        }
    }
    if let Some(u) = rec
        .get("url")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
    {
        m.insert("URL".into(), json!(u));
    }
    if let Some(d) = date_from(rec, "published") {
        m.insert("issued".into(), d);
    }
    if let Some(d) = date_from(rec, "accessed") {
        m.insert("accessed".into(), d);
    }

    let folders: Vec<String> = rec
        .get("foldersNamed")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let filename = rec.get("attachments").and_then(|v| v.as_array()).and_then(|atts| {
        atts.iter()
            .find(|a| a.get("article_pdf").and_then(|x| x.as_i64()) == Some(1))
            .or_else(|| atts.first())
            .and_then(|a| a.get("filename"))
            .and_then(|f| f.as_str())
            .map(String::from)
    });

    let mut pp = Map::new();
    for (k, src) in [("id", "_id"), ("citekey", "citekey"), ("sha1", "sha1")] {
        if let Some(v) = rec.get(src).and_then(|v| v.as_str()) {
            pp.insert(k.into(), json!(v));
        }
    }
    if let Some(f) = &filename {
        pp.insert("filename".into(), json!(f));
    }
    if !folders.is_empty() {
        pp.insert("folders".into(), json!(folders));
    }

    let mut custom = Map::new();
    custom.insert("source".into(), json!("paperpile"));
    if let Some(k) = rec.get("kind").and_then(|v| v.as_str()) {
        custom.insert("kind".into(), json!(k));
    }
    custom.insert("paperpile".into(), Value::Object(pp));
    m.insert("custom".into(), Value::Object(custom));

    let created_sqlite = rec
        .get("created")
        .and_then(|v| v.as_f64())
        .and_then(unix_to_sqlite);

    Some((
        Value::Object(m),
        Extras {
            folders,
            filename,
            created_sqlite,
        },
    ))
}

fn map_pubtype(pt: Option<&str>) -> &'static str {
    match pt {
        Some("PP_ARTICLE") | None => "article-journal",
        Some("PP_BOOK") => "book",
        Some("PP_CHAPTER") => "chapter",
        Some("PP_CONFERENCE") | Some("PP_PROCEEDINGS") => "paper-conference",
        Some("PP_THESIS") => "thesis",
        Some("PP_REPORT") => "report",
        Some("PP_WEBSITE") | Some("PP_WEBPAGE") => "webpage",
        Some("PP_PATENT") => "patent",
        _ => "document",
    }
}

fn date_from(rec: &Value, field: &str) -> Option<Value> {
    let o = rec.get(field)?.as_object()?;
    let y = o.get("year")?.as_str()?.parse::<i64>().ok()?;
    let mut parts = vec![y];
    if let Some(mo) = o
        .get("month")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
    {
        parts.push(mo);
        if let Some(d) = o
            .get("day")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
        {
            parts.push(d);
        }
    }
    Some(json!({ "date-parts": [parts] }))
}

fn unix_to_sqlite(ts: f64) -> Option<String> {
    if !(0.0..=4102444800.0).contains(&ts) {
        return None;
    }
    let odt = time::OffsetDateTime::from_unix_timestamp(ts as i64).ok()?;
    let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    odt.format(&fmt).ok()
}

/// Verified from the supplied JSON + ZIP pair; never a basename search.
fn zip_layout_paths(filename: &str) -> Vec<String> {
    filename
        .strip_prefix("All Papers/")
        .map(|suffix| vec![format!("Paperpile files/{suffix}"), suffix.to_string()])
        .unwrap_or_default()
}

/// Legacy CLI still links PDFs, but uses the same explicit-root safety checks
/// and the observed ZIP layout. Ambiguous/unsafe paths remain unresolved.
fn resolve_in_mount(mount: &Path, rel: &str) -> Option<PathBuf> {
    let roots = super::files::AttachmentRoots::new(&[mount.to_path_buf()]).ok()?;
    let alternatives = zip_layout_paths(rel);
    let paths: Vec<_> = std::iter::once(rel)
        .chain(alternatives.iter().map(String::as_str))
        .collect();
    match roots.resolve_candidates(&paths).ok()? {
        super::files::ResolvedPath::Found { path } => Some(path),
        _ => None,
    }
}

/// dedup 三段キー: PMID → DOI → Paperpile _id（DOI・PMID を持たない和文誌等の再 import 重複防止）
fn find_existing(
    conn: &Connection,
    pmid: Option<&str>,
    doi: Option<&str>,
    pp_id: Option<&str>,
) -> Result<Option<(i64, String)>> {
    for (val, sql) in [
        (pmid, "SELECT id, csl FROM items WHERE deleted_at IS NULL AND pmid = ?1"),
        (doi, "SELECT id, csl FROM items WHERE deleted_at IS NULL AND doi = ?1"),
        (
            pp_id,
            "SELECT id, csl FROM items WHERE deleted_at IS NULL AND json_extract(csl, '$.custom.paperpile.id') = ?1",
        ),
    ] {
        if let Some(v) = val {
            let row = conn
                .query_row(sql, [v], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .optional()?;
            if row.is_some() {
                return Ok(row);
            }
        }
    }
    Ok(None)
}

fn merge_custom_paperpile(existing: &mut Value, pp: Value) {
    let Some(o) = existing.as_object_mut() else {
        return;
    };
    let custom = o.entry("custom").or_insert_with(|| json!({}));
    if let Some(c) = custom.as_object_mut() {
        c.insert("paperpile".into(), pp);
        c.entry("source").or_insert(json!("paperpile"));
    }
}

fn ensure_collection_path(
    conn: &Connection,
    path: &str,
    cache: &mut HashMap<String, i64>,
    created: &mut usize,
) -> Result<i64> {
    let mut parent: Option<i64> = None;
    let mut acc = String::new();
    for seg in path.split('/').map(str::trim).filter(|s| !s.is_empty()) {
        acc = if acc.is_empty() {
            seg.to_string()
        } else {
            format!("{acc}/{seg}")
        };
        if let Some(&id) = cache.get(&acc) {
            parent = Some(id);
            continue;
        }
        // 兄弟の照合は大小無視（edit::ensure_collection_path・collections::create_collection と同じ基準）。
        // GUI で "Liver" → "liver" と改名した後に再 import しても同じ親の下に "Liver" をもう 1 つ作らない
        let id = match crate::collections::child_id(conn, parent, seg)? {
            Some(id) => id,
            None => {
                let id = crate::collections::insert_child(conn, parent, seg)?;
                *created += 1;
                id
            }
        };
        cache.insert(acc.clone(), id);
        parent = Some(id);
    }
    parent.ok_or_else(|| CoreError::Parse(format!("空のフォルダパス: {path}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_record_basics() {
        let rec = json!({
            "_id": "abc", "citekey": "Test2026-xy", "pubtype": "PP_ARTICLE",
            "title": "A Mapping Test",
            "author": [
                {"first": "Taro", "last": "Test"},
                {"collective": "Some Working Group", "formatted": "Some Working Group"}
            ],
            "journal": "Test J", "journalfull": "Test Journal",
            "volume": "10", "issue": "2", "pages": "1-9",
            "doi": "10.5555/UPPER.Case", "pmid": "12345",
            "published": {"year": "2026"},
            "keywords": "a; b",
            "foldersNamed": ["Parent/Child"],
            "attachments": [{"article_pdf": 1, "filename": "All Papers/T/Test 2026 - A.pdf"}],
            "created": 1750000000.0
        });
        let (csl, ex) = record_to_csl(&rec).unwrap();
        assert_eq!(csl["type"], "article-journal");
        assert_eq!(csl["DOI"], "10.5555/upper.case");
        assert_eq!(csl["container-title"], "Test Journal");
        assert_eq!(csl["container-title-short"], "Test J");
        assert_eq!(csl["author"][0]["family"], "Test");
        assert_eq!(csl["author"][1]["literal"], "Some Working Group");
        assert_eq!(csl["issued"]["date-parts"][0][0], 2026);
        assert_eq!(csl["custom"]["paperpile"]["citekey"], "Test2026-xy");
        assert_eq!(ex.folders, vec!["Parent/Child"]);
        assert_eq!(ex.filename.as_deref(), Some("All Papers/T/Test 2026 - A.pdf"));
        // 1750000000 = 2025-06-15 UTC
        assert!(ex.created_sqlite.unwrap().starts_with("2025-06-15"));
    }

    #[test]
    fn record_without_title_rejected() {
        assert!(record_to_csl(&json!({"pubtype": "PP_ARTICLE"})).is_none());
    }
}
