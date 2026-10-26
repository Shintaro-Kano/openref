//! Zotero Connector が POST してくる item JSON（Zotero API 形式）→ CSL-JSON 変換。
//! translator 750 本の出力をそのまま受けるための互換層。

use serde_json::{json, Map, Value};

pub struct ConnectorItem {
    pub csl: Value,
    /// connector 側の一時 id（saveAttachment の parentItemID 突合に使う）
    pub connector_id: Option<String>,
    /// 添付予定として宣言された PDF URL 群（バイナリ POST が来ない場合の fallback 用）
    pub pdf_urls: Vec<String>,
}

pub fn zotero_to_csl(z: &Value) -> Option<ConnectorItem> {
    let title = z.get("title").and_then(|v| v.as_str())?.trim();
    if title.is_empty() {
        return None;
    }
    let mut m = Map::new();
    m.insert(
        "type".into(),
        json!(map_item_type(z.get("itemType").and_then(|v| v.as_str()).unwrap_or(""))),
    );
    m.insert("title".into(), json!(title));

    for (csl_key, z_key) in [
        ("container-title", "publicationTitle"),
        ("container-title-short", "journalAbbreviation"),
        ("volume", "volume"),
        ("issue", "issue"),
        ("page", "pages"),
        ("ISSN", "ISSN"),
        ("abstract", "abstractNote"),
        ("URL", "url"),
        ("language", "language"),
        ("publisher", "publisher"),
    ] {
        if let Some(v) = z.get(z_key).and_then(|v| v.as_str()) {
            if !v.trim().is_empty() {
                m.insert(csl_key.into(), json!(v.trim()));
            }
        }
    }
    if let Some(d) = z.get("DOI").and_then(|v| v.as_str()) {
        let d = d.trim().trim_start_matches("https://doi.org/");
        if !d.is_empty() {
            m.insert("DOI".into(), json!(d.to_lowercase()));
        }
    }

    // creators（author のみ採用）
    let mut authors = Vec::new();
    if let Some(arr) = z.get("creators").and_then(|v| v.as_array()) {
        for c in arr {
            let ctype = c.get("creatorType").and_then(|v| v.as_str()).unwrap_or("author");
            if ctype != "author" {
                continue;
            }
            if let Some(last) = c.get("lastName").and_then(|v| v.as_str()) {
                let mut o = Map::new();
                o.insert("family".into(), json!(last));
                if let Some(first) = c.get("firstName").and_then(|v| v.as_str()) {
                    if !first.is_empty() {
                        o.insert("given".into(), json!(first));
                    }
                }
                authors.push(Value::Object(o));
            } else if let Some(name) = c.get("name").and_then(|v| v.as_str()) {
                authors.push(json!({ "literal": name }));
            }
        }
    }
    if !authors.is_empty() {
        m.insert("author".into(), Value::Array(authors));
    }

    if let Some(dp) = z.get("date").and_then(|v| v.as_str()).and_then(parse_date) {
        m.insert("issued".into(), json!({ "date-parts": [dp] }));
    }

    // extra 欄から PMID / PMCID / DOI を拾う（PubMed translator が書き込む）
    if let Some(extra) = z.get("extra").and_then(|v| v.as_str()) {
        for line in extra.lines() {
            let line = line.trim();
            if let Some(v) = strip_label(line, "PMID") {
                if v.chars().all(|c| c.is_ascii_digit()) && !v.is_empty() {
                    m.entry("PMID").or_insert(json!(v));
                }
            } else if let Some(v) = strip_label(line, "PMCID") {
                if !v.is_empty() {
                    m.entry("PMCID").or_insert(json!(v));
                }
            } else if let Some(v) = strip_label(line, "DOI") {
                if v.starts_with("10.") {
                    m.entry("DOI").or_insert(json!(v.to_lowercase()));
                }
            }
        }
    }

    // tags → keyword
    let tags: Vec<&str> = z
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("tag").and_then(|x| x.as_str()).or_else(|| t.as_str()))
                .collect()
        })
        .unwrap_or_default();
    if !tags.is_empty() {
        m.insert("keyword".into(), json!(tags.join(", ")));
    }

    let mut custom = Map::new();
    custom.insert("source".into(), json!("connector"));
    if let Some(lib_catalog) = z.get("libraryCatalog").and_then(|v| v.as_str()) {
        custom.insert("library-catalog".into(), json!(lib_catalog));
    }
    m.insert("custom".into(), Value::Object(custom));

    let pdf_urls = z
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|a| {
                    a.get("mimeType").and_then(|x| x.as_str()) == Some("application/pdf")
                })
                .filter_map(|a| a.get("url").and_then(|x| x.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Some(ConnectorItem {
        csl: Value::Object(m),
        connector_id: z
            .get("id")
            .and_then(|v| v.as_str().map(String::from).or_else(|| v.as_i64().map(|n| n.to_string()))),
        pdf_urls,
    })
}

fn strip_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(label)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    Some(rest.trim())
}

fn map_item_type(t: &str) -> &'static str {
    match t {
        "journalArticle" | "" => "article-journal",
        "preprint" => "article",
        "magazineArticle" => "article-magazine",
        "newspaperArticle" => "article-newspaper",
        "book" => "book",
        "bookSection" => "chapter",
        "conferencePaper" => "paper-conference",
        "thesis" => "thesis",
        "report" => "report",
        "webpage" => "webpage",
        _ => "document",
    }
}

/// Zotero の date 文字列（"2024-08-01" / "August 2024" / "2024 Aug 1" / "2024"）→ date-parts
fn parse_date(s: &str) -> Option<Vec<i64>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // 年 = 最初の 4 桁数値
    let mut year: Option<i64> = None;
    let mut nums: Vec<i64> = Vec::new();
    for tok in s.split(|c: char| !c.is_ascii_alphanumeric()) {
        if tok.len() == 4 {
            if let Ok(n) = tok.parse::<i64>() {
                if (1500..=2100).contains(&n) && year.is_none() {
                    year = Some(n);
                    continue;
                }
            }
        }
        if let Ok(n) = tok.parse::<i64>() {
            nums.push(n);
        }
    }
    let year = year?;
    let month = month_from_name(s).or_else(|| nums.iter().find(|n| (1..=12).contains(*n)).copied());
    let mut parts = vec![year];
    if let Some(mo) = month {
        parts.push(mo);
        if let Some(day) = nums.iter().find(|n| (1..=31).contains(*n) && Some(**n) != Some(mo)) {
            parts.push(*day);
        }
    }
    Some(parts)
}

fn month_from_name(s: &str) -> Option<i64> {
    let ls = s.to_ascii_lowercase();
    let names = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    names
        .iter()
        .position(|m| ls.contains(m))
        .map(|i| (i + 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zotero_journal_article() {
        let z = json!({
            "id": "abc123",
            "itemType": "journalArticle",
            "title": "Connector Test Article",
            "creators": [
                {"creatorType": "author", "firstName": "Elena M", "lastName": "Stoffel"},
                {"creatorType": "editor", "firstName": "X", "lastName": "NotIncluded"},
                {"creatorType": "author", "name": "Some Group"}
            ],
            "publicationTitle": "Gastroenterology",
            "journalAbbreviation": "Gastroenterology",
            "volume": "164", "issue": "5", "pages": "752-765",
            "date": "2023-02-18",
            "DOI": "10.1053/J.GASTRO.2023.02.012",
            "abstractNote": "An abstract.",
            "extra": "PMID: 36804602\nPMCID: PMC10243302",
            "tags": [{"tag": "Pancreatic Cancer"}],
            "attachments": [
                {"title": "Full Text PDF", "mimeType": "application/pdf", "url": "https://example.org/x.pdf"},
                {"title": "Snapshot", "mimeType": "text/html", "url": "https://example.org/"}
            ]
        });
        let ci = zotero_to_csl(&z).unwrap();
        let c = &ci.csl;
        assert_eq!(c["type"], "article-journal");
        assert_eq!(c["DOI"], "10.1053/j.gastro.2023.02.012");
        assert_eq!(c["PMID"], "36804602");
        assert_eq!(c["PMCID"], "PMC10243302");
        assert_eq!(c["author"].as_array().unwrap().len(), 2); // editor は除外
        assert_eq!(c["author"][1]["literal"], "Some Group");
        assert_eq!(c["issued"]["date-parts"][0], json!([2023, 2, 18]));
        assert_eq!(ci.connector_id.as_deref(), Some("abc123"));
        assert_eq!(ci.pdf_urls, vec!["https://example.org/x.pdf"]);
    }

    #[test]
    fn date_variants() {
        assert_eq!(parse_date("August 2024"), Some(vec![2024, 8]));
        assert_eq!(parse_date("2024"), Some(vec![2024]));
        assert_eq!(parse_date("2024 Aug 3"), Some(vec![2024, 8, 3]));
        assert_eq!(parse_date(""), None);
    }
}
