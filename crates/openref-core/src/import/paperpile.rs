//! Paperpile JSON export の adapter。`read_input` が export を `ImportInput`（正規化済みの中間表現）に変えるだけで、
//! DB 書込・突合・添付の取り込みは共通の移行経路（`apply`）が行う。
//! API-free 決定論: export 内の書誌をそのまま CSL-JSON 化する（PubMed 再確定はしない）。

use crate::{CoreError, Result};
use serde_json::{json, Map, Value};
use std::path::Path;

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
            Some((csl, folders)) => (csl, folders),
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

/// Paperpile record → (CSL-JSON, foldersNamed のパス)。title 欠落のみ不採用。
pub fn record_to_csl(rec: &Value) -> Option<(Value, Vec<String>)> {
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

    Some((Value::Object(m), folders))
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
        let (csl, folders) = record_to_csl(&rec).unwrap();
        assert_eq!(csl["type"], "article-journal");
        assert_eq!(csl["DOI"], "10.5555/upper.case");
        assert_eq!(csl["container-title"], "Test Journal");
        assert_eq!(csl["container-title-short"], "Test J");
        assert_eq!(csl["author"][0]["family"], "Test");
        assert_eq!(csl["author"][1]["literal"], "Some Working Group");
        assert_eq!(csl["issued"]["date-parts"][0][0], 2026);
        assert_eq!(csl["custom"]["paperpile"]["citekey"], "Test2026-xy");
        assert_eq!(folders, vec!["Parent/Child"]);
        assert_eq!(
            csl["custom"]["paperpile"]["filename"],
            "All Papers/T/Test 2026 - A.pdf"
        );
    }

    #[test]
    fn record_without_title_rejected() {
        assert!(record_to_csl(&json!({"pubtype": "PP_ARTICLE"})).is_none());
    }
}
