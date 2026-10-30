mod crossref;
mod isho;
mod pubmed;

pub use crossref::{search_bibliographic, Candidate};
pub(crate) use crossref::with_crossref_gate;

use crate::{CoreError, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum AddInput {
    Pmid(String),
    Doi(String),
}

/// resolver の出力 = 正準 CSL-JSON + 撤回情報 + 由来 + 警告
pub struct Resolved {
    pub csl: Value,
    pub retracted: Option<String>,
    /// 書誌の取得元（resolver = pubmed / doi.org / isho-ris。resolver の外で作るものは connector・capture・pdf-import 等）
    pub source: &'static str,
    /// 書誌は取れたが経路に欠けがあった（普段は空）。今は「DOI → PubMed の照合が失敗して doi.org で代用した」だけ
    /// = PMID・抄録・MeSH・撤回を見ていない。入口（MCP・CLI・GUI・connectd・PDF 取込）が利用者へ出す
    pub warnings: Vec<String>,
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

/// `resolve` の取得先（本番 = `NET`。テストはネットを使わない関数に差し替える = 分岐は同じ `resolve_with` を通る）
struct Sources {
    find_pmid_by_doi: fn(&str) -> Result<Option<String>>,
    fetch_by_pmid: fn(&str) -> Result<Resolved>,
    fetch_by_doi: fn(&str) -> Result<Resolved>,
    fetch_isho: fn(&str) -> Result<Resolved>,
}

const NET: Sources = Sources {
    find_pmid_by_doi: pubmed::find_pmid_by_doi,
    fetch_by_pmid: pubmed::fetch_by_pmid,
    fetch_by_doi: crossref::fetch_by_doi,
    fetch_isho: isho::fetch,
};

/// 書誌解決: PMID → PubMed efetch。DOI → PubMed 照合を先行（abstract/MeSH が濃い）、
/// 非収載なら doi.org content negotiation（Crossref/DataCite。医書.jp は公式 RIS）へ fallback。
/// PubMed の照合そのものが失敗した（429 の再試行切れ・NCBI 障害）ときも doi.org へ落ちるが、非収載とは分けて
/// `warnings` に理由を残し（stderr にも出す）、doi.org も失敗したら最終エラー文に PubMed の理由を連結する
pub fn resolve(input: &AddInput) -> Result<Resolved> {
    resolve_with(&NET, input)
}

fn resolve_with(net: &Sources, input: &AddInput) -> Result<Resolved> {
    let d = match input {
        AddInput::Pmid(p) => return (net.fetch_by_pmid)(p),
        AddInput::Doi(d) => d,
    };
    // Ok(None) = PubMed 非収載（警告なし）。Err = 照合の失敗 = 収載かどうか分からない
    let pubmed_err = match (net.find_pmid_by_doi)(d) {
        Ok(Some(pmid)) => return (net.fetch_by_pmid)(&pmid),
        Ok(None) => None,
        Err(e) => {
            // connectd のログ・GUI の stderr・enrich / refresh の裏経路にも残す（呼び手が warnings を見なくても辿れる）
            eprintln!("resolver: DOI {d}: PubMed の照合に失敗 → doi.org へ: {e}");
            Some(e)
        }
    };
    let fallback = match (net.fetch_by_doi)(d) {
        Ok(resolved) => Ok(resolved),
        Err(primary) if isho::canonical_doi(d).is_some() => (net.fetch_isho)(d)
            .map_err(|fallback| CoreError::Http(format!("{primary} / {fallback}"))),
        Err(error) => Err(error),
    };
    let Some(pe) = pubmed_err else { return fallback };
    match fallback {
        Ok(mut r) => {
            let by = if r.source == "isho-ris" { "医書.jp" } else { "doi.org" };
            // 利用者向けの 1 文なので理由は短く（URL を含む全文は上の eprintln だけ）。再取得の入口は今は CLI だけ
            r.warnings.push(format!(
                "PubMed の照合に失敗（{}）→ {by} の書誌で代用（PMID・抄録・MeSH・撤回は未確認。後で再取得 = CLI の openref refresh）",
                short_reason(&pe)
            ));
            Ok(r)
        }
        Err(e) => Err(CoreError::Http(format!("PubMed の照合に失敗（{pe}）/ doi.org でも解決できません（{e}）"))),
    }
}

/// 通信の失敗を利用者向けの短い理由にする（ureq のエラー文 = 「<what>: <URL>: status code N」「<what>: <URL>: Connection Failed: …」）
fn short_reason(e: &CoreError) -> String {
    let CoreError::Http(m) = e else {
        return match e {
            CoreError::Json(_) | CoreError::Parse(_) => "応答を読めない".into(),
            other => other.to_string(),
        };
    };
    if let Some(code) = regex::Regex::new(r"status code (\d{3})").unwrap().captures(m) {
        return match &code[1] {
            "429" => "HTTP 429・再試行切れ".into(),
            c => format!("HTTP {c}"),
        };
    }
    for (needle, why) in [("Dns Failed", "名前解決できない"), ("timed out", "時間切れ"), ("Connection Failed", "接続できない")] {
        if m.contains(needle) {
            return why.into();
        }
    }
    // 想定外の形: URL だけ落として短く
    let s = regex::Regex::new(r"https?://\S*[^\s:]").unwrap().replace_all(m, "…");
    crate::model::truncate_chars(s.trim(), 80)
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

    // ---- resolve_with: DOI 経路の分岐（ネットなし）----

    fn fake(source: &'static str) -> Result<Resolved> {
        Ok(Resolved { csl: serde_json::json!({ "title": source }), retracted: None, source, warnings: vec![] })
    }
    fn pubmed_ok(_: &str) -> Result<Resolved> {
        fake("pubmed")
    }
    fn doi_ok(_: &str) -> Result<Resolved> {
        fake("doi.org")
    }
    fn doi_down(_: &str) -> Result<Resolved> {
        Err(CoreError::Http("doi.org negotiation (x): status code 503".into()))
    }
    fn isho_ok(_: &str) -> Result<Resolved> {
        fake("isho-ris")
    }
    fn not_called(d: &str) -> Result<Resolved> {
        panic!("呼ばれないはず: {d}")
    }
    fn esearch_429(_: &str) -> Result<Option<String>> {
        Err(CoreError::Http("esearch: status code 429".into()))
    }
    const DOI: &str = "10.1148/rg.230216";

    fn net(
        find: fn(&str) -> Result<Option<String>>,
        by_pmid: fn(&str) -> Result<Resolved>,
        by_doi: fn(&str) -> Result<Resolved>,
    ) -> Sources {
        Sources { find_pmid_by_doi: find, fetch_by_pmid: by_pmid, fetch_by_doi: by_doi, fetch_isho: not_called }
    }

    #[test]
    fn doi_on_pubmed_has_no_warning() {
        let n = net(|_| Ok(Some("36804602".into())), pubmed_ok, not_called);
        let r = resolve_with(&n, &AddInput::Doi(DOI.into())).unwrap();
        assert_eq!((r.source, r.warnings.len()), ("pubmed", 0));
    }

    #[test]
    fn doi_not_on_pubmed_falls_back_without_warning() {
        let n = net(|_| Ok(None), not_called, doi_ok);
        let r = resolve_with(&n, &AddInput::Doi(DOI.into())).unwrap();
        assert_eq!(r.source, "doi.org");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn pubmed_failure_falls_back_with_warning() {
        let n = net(esearch_429, not_called, doi_ok);
        let r = resolve_with(&n, &AddInput::Doi(DOI.into())).unwrap();
        assert_eq!(r.source, "doi.org");
        assert_eq!(
            r.warnings,
            vec!["PubMed の照合に失敗（HTTP 429・再試行切れ）→ doi.org の書誌で代用（PMID・抄録・MeSH・撤回は未確認。後で再取得 = CLI の openref refresh）".to_string()]
        );
    }

    #[test]
    fn pubmed_failure_with_isho_fallback_names_isho() {
        let n = Sources { find_pmid_by_doi: esearch_429, fetch_by_pmid: not_called, fetch_by_doi: doi_down, fetch_isho: isho_ok };
        let r = resolve_with(&n, &AddInput::Doi("10.18885/CI.0000001234".into())).unwrap();
        assert_eq!(r.source, "isho-ris");
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("→ 医書.jp の書誌で代用"), "{:?}", r.warnings);
    }

    #[test]
    fn both_failures_are_in_the_error() {
        let n = net(esearch_429, not_called, doi_down);
        let e = resolve_with(&n, &AddInput::Doi(DOI.into())).err().unwrap().to_string();
        assert_eq!(
            e,
            "HTTP error: PubMed の照合に失敗（HTTP error: esearch: status code 429）/ doi.org でも解決できません（HTTP error: doi.org negotiation (x): status code 503）"
        );
        // 非収載 + doi.org の失敗は今までどおり doi.org のエラーだけ
        let n = net(|_| Ok(None), not_called, doi_down);
        let e = resolve_with(&n, &AddInput::Doi(DOI.into())).err().unwrap().to_string();
        assert_eq!(e, "HTTP error: doi.org negotiation (x): status code 503");
    }

    /// 警告の理由は URL を含まない短い語（実物の ureq のエラー文で）
    #[test]
    fn short_reason_drops_urls() {
        let url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi?db=pubmed&term=%2210.1148%2Fx%22%5BDOI%5D&retmode=json&tool=openref";
        let http = |m: String| CoreError::Http(m);
        assert_eq!(short_reason(&http(format!("esearch: {url}: status code 429"))), "HTTP 429・再試行切れ");
        assert_eq!(short_reason(&http(format!("esearch: {url}: status code 503"))), "HTTP 503");
        assert_eq!(
            short_reason(&http(format!("esearch: {url}: Connection Failed: Connect error: Operation not permitted (os error 1)"))),
            "接続できない"
        );
        assert_eq!(short_reason(&http(format!("esearch: {url}: Dns Failed: resolve dns name"))), "名前解決できない");
        assert_eq!(short_reason(&http(format!("esearch: {url}: Network Error: timed out reading response"))), "時間切れ");
        assert_eq!(short_reason(&http(format!("esearch: {url}: Too Many Redirects"))), "esearch: …: Too Many Redirects");
        let json = serde_json::from_str::<Value>("{").unwrap_err();
        assert_eq!(short_reason(&CoreError::Json(json)), "応答を読めない");
    }

    #[test]
    fn pmid_goes_straight_to_pubmed() {
        let n = net(|_| panic!("esearch は呼ばない"), pubmed_ok, not_called);
        let r = resolve_with(&n, &AddInput::Pmid("1".into())).unwrap();
        assert_eq!((r.source, r.warnings.len()), ("pubmed", 0));
    }
}
