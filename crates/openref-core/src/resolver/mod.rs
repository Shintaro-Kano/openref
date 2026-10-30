mod crossref;
mod isho;
mod pubmed;

pub use crossref::{search_bibliographic, Candidate};

use crate::{CoreError, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum AddInput {
    Pmid(String),
    Doi(String),
}

/// resolver の出力 = 正準 CSL-JSON + 撤回情報 + 由来
pub struct Resolved {
    pub csl: Value,
    pub retracted: Option<String>,
    pub source: &'static str,
}

/// PMID / DOI / PubMed URL / doi.org URL を解釈する。
/// 誌面 URL の meta タグ抽出と title 検索は未対応（誠実にエラーで案内）。
pub fn parse_input(raw: &str) -> Result<AddInput> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(CoreError::BadInput("空入力".into()));
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        return Ok(AddInput::Pmid(t.to_string()));
    }
    if let Some(rest) = t.strip_prefix("PMID:").or_else(|| t.strip_prefix("pmid:")) {
        let d = rest.trim();
        if d.chars().all(|c| c.is_ascii_digit()) && !d.is_empty() {
            return Ok(AddInput::Pmid(d.into()));
        }
    }
    if t.starts_with("10.") && t.contains('/') {
        return Ok(AddInput::Doi(t.to_string()));
    }
    if t.starts_with("http://") || t.starts_with("https://") {
        let no_scheme = t.splitn(2, "://").nth(1).unwrap_or("");
        if let Some(path) = no_scheme.strip_prefix("pubmed.ncbi.nlm.nih.gov/") {
            let digits: String = path.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return Ok(AddInput::Pmid(digits));
            }
        }
        if let Some(idx) = no_scheme.find("doi.org/") {
            let doi = no_scheme[idx + "doi.org/".len()..].trim_end_matches('/');
            if doi.starts_with("10.") {
                return Ok(AddInput::Doi(doi.to_string()));
            }
        }
        return Err(CoreError::BadInput(format!(
            "誌面 URL からの抽出は未対応です。PMID / DOI / PubMed URL / doi.org URL を入力してください: {t}"
        )));
    }
    Err(CoreError::BadInput(format!(
        "PMID / DOI / PubMed URL / doi.org URL として解釈できません: {t}"
    )))
}

/// 書誌解決: PMID → PubMed efetch。DOI → PubMed 照合を先行（abstract/MeSH が濃い）、
/// 見つからなければ doi.org content negotiation（Crossref/DataCite）へ fallback。
pub fn resolve(input: &AddInput) -> Result<Resolved> {
    match input {
        AddInput::Pmid(p) => pubmed::fetch_by_pmid(p),
        AddInput::Doi(d) => match pubmed::find_pmid_by_doi(d) {
            Ok(Some(pmid)) => pubmed::fetch_by_pmid(&pmid),
            _ => match crossref::fetch_by_doi(d) {
                Ok(resolved) => Ok(resolved),
                Err(primary) if isho::canonical_doi(d).is_some() => isho::fetch(d)
                    .map_err(|fallback| CoreError::Http(format!("{primary} / {fallback}"))),
                Err(error) => Err(error),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs() {
        assert_eq!(
            parse_input("36804602").unwrap(),
            AddInput::Pmid("36804602".into())
        );
        assert_eq!(
            parse_input("https://pubmed.ncbi.nlm.nih.gov/36804602/").unwrap(),
            AddInput::Pmid("36804602".into())
        );
        assert_eq!(
            parse_input("PMID: 12345").unwrap(),
            AddInput::Pmid("12345".into())
        );
        assert_eq!(
            parse_input("10.1053/j.gastro.2023.02.012").unwrap(),
            AddInput::Doi("10.1053/j.gastro.2023.02.012".into())
        );
        assert_eq!(
            parse_input("https://doi.org/10.1148/rg.230216").unwrap(),
            AddInput::Doi("10.1148/rg.230216".into())
        );
        assert!(parse_input("hello world").is_err());
        // 誌面 URL は未対応（doi.org を含まない URL）
        assert!(parse_input("https://www.ajronline.org/doi/10.2214/AJR.24.31234").is_err());
    }
}
