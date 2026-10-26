//! CSL-JSON → BibTeX の決定論変換（export 用・最小十分）と、item から引用用の 1 件（BibTeX・引用キー・DOI）を作る `entries`。
//! citekey は Paperpile の citekey を優先し、なければ FamilyYYYY-<item key 下4桁> を生成。

use crate::{model, search, Db, Result};
use serde_json::Value;

/// item 1 件の BibTeX・引用キー・DOI（GUI の引用メニューと MCP の bibtex_entries）
#[derive(Debug, serde::Serialize)]
pub struct Entry {
    pub key: String,
    pub citekey: String,
    pub bibtex: String,
    pub doi: Option<String>,
}

/// keys の順に返す（見つからない key・ゴミ箱の item は飛ばす）
pub fn entries(db: &Db, keys: &[String]) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for k in keys {
        if let Some(item) = search::get_by_key(db, k)? {
            out.push(Entry {
                citekey: citekey(&item.csl, k),
                bibtex: item_to_bibtex(&item.csl, k),
                doi: model::s(&item.csl, "DOI").map(str::to_string),
                key: k.clone(),
            });
        }
    }
    Ok(out)
}

/// 引用キー（BibTeX の citekey・LaTeX の \cite{} に使う）
pub fn citekey(csl: &Value, item_key: &str) -> String {
    csl
        .pointer("/custom/paperpile/citekey")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let fam: String = model::first_author_family(csl)
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            let fam = if fam.is_empty() { "Ref".to_string() } else { fam };
            let year = model::year(csl).map(|y| y.to_string()).unwrap_or_default();
            let tail: String = item_key
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{fam}{year}-{}", tail.to_lowercase())
        })
}

pub fn item_to_bibtex(csl: &Value, item_key: &str) -> String {
    let citekey = citekey(csl, item_key);

    let entry_type = match model::s(csl, "type").unwrap_or("article-journal") {
        "article-journal" | "article" => "article",
        "book" => "book",
        "chapter" => "incollection",
        "paper-conference" => "inproceedings",
        "thesis" => "phdthesis",
        "report" => "techreport",
        _ => "misc",
    };

    let mut fields: Vec<(&str, String)> = Vec::new();
    let authors = csl
        .get("author")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .map(|a| {
                    if let Some(l) = a.get("literal").and_then(|v| v.as_str()) {
                        format!("{{{l}}}")
                    } else {
                        let fam = a.get("family").and_then(|v| v.as_str()).unwrap_or("");
                        let giv = a.get("given").and_then(|v| v.as_str()).unwrap_or("");
                        if giv.is_empty() {
                            fam.to_string()
                        } else {
                            format!("{fam}, {giv}")
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join(" and ")
        })
        .unwrap_or_default();
    if !authors.is_empty() {
        fields.push(("author", esc(&authors)));
    }
    if let Some(t) = model::s(csl, "title") {
        fields.push(("title", esc(t)));
    }
    if let Some(j) = model::s(csl, "container-title") {
        fields.push(("journal", esc(j)));
    }
    if let Some(y) = model::year(csl) {
        fields.push(("year", y.to_string()));
    }
    for (bib, key) in [("volume", "volume"), ("number", "issue"), ("pages", "page"), ("issn", "ISSN")] {
        if let Some(v) = model::s(csl, key) {
            fields.push((bib, esc(v)));
        }
    }
    if let Some(d) = model::s(csl, "DOI") {
        fields.push(("doi", d.to_string()));
    }
    if let Some(p) = model::s(csl, "PMID") {
        fields.push(("pmid", p.to_string()));
    }
    if let Some(u) = model::s(csl, "URL") {
        fields.push(("url", u.to_string()));
    }

    let body = fields
        .iter()
        .map(|(k, v)| format!("  {k} = {{{v}}}"))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("@{entry_type}{{{citekey},\n{body}\n}}")
}

/// BibTeX フィールド内の最小エスケープ（& % # _。波括弧は素通し）
fn esc(s: &str) -> String {
    s.replace('&', "\\&")
        .replace('%', "\\%")
        .replace('#', "\\#")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bibtex_shape() {
        let csl = json!({
            "type": "article-journal",
            "title": "Peritoneum & Beyond",
            "author": [{"family": "Gaballah", "given": "A H"}, {"literal": "Some Group"}],
            "container-title": "RadioGraphics",
            "issued": {"date-parts": [[2024]]},
            "volume": "44", "issue": "8", "page": "e230216",
            "DOI": "10.1148/rg.230216", "PMID": "39088361",
            "custom": {"paperpile": {"citekey": "Gaballah2024-xy"}}
        });
        let b = item_to_bibtex(&csl, "ABCD2345");
        assert!(b.starts_with("@article{Gaballah2024-xy,"));
        assert!(b.contains("author = {Gaballah, A H and {Some Group}}"));
        assert!(b.contains("title = {Peritoneum \\& Beyond}"));
        assert!(b.contains("journal = {RadioGraphics}"));
        assert!(b.contains("year = {2024}"));
        assert!(b.contains("doi = {10.1148/rg.230216}"));
        // citekey なし → 生成
        let csl2 = json!({"type": "article-journal", "title": "T",
            "author": [{"family": "Kano"}], "issued": {"date-parts": [[2026]]}});
        let b2 = item_to_bibtex(&csl2, "WXYZ6789");
        assert!(b2.starts_with("@article{Kano2026-6789,"), "{b2}");
    }
}
