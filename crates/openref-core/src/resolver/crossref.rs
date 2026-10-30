//! doi.org content negotiation（Accept: CSL-JSON）。
//! Crossref / DataCite 両 agency をこの 1 経路でカバーする（PubMed 非収載 DOI の fallback）。

use super::pubmed::Throttle;
use super::Resolved;
use crate::http::{agent, call_with_retry_paced};
use crate::Result;
use serde_json::{json, Value};
use std::time::Duration;

/// Crossref への要求どうしの最小間隔。匿名（mailto なし）の公開プールは応答ヘッダで `x-rate-limit-limit: 1`・
/// `x-rate-limit-interval: 1s`・`x-concurrency-limit: 1` を返す → 同時 1 本（前の応答を受け取るまで次を送らない）・
/// 前の送信から間隔を空ける。間隔は 1 s ちょうどにしない: 要求ごとに接続を張り直すので TLS / DNS の揺らぎで Crossref 側の
/// 到着間隔は送信間隔より縮みうる（EUTILS_INTERVAL の 333 → 400 ms と同じ理由）→ 余裕を取って 1.1 s
const CROSSREF_INTERVAL: Duration = Duration::from_millis(1100);

/// Crossref の門（**プロセス単位の static**・同時 1 本 + `CROSSREF_INTERVAL`）。api.crossref.org の書誌検索・
/// works/{doi}（pdffetch の link[]）と、doi.org の content negotiation（Crossref 登録の DOI は api.crossref.org の
/// transform へ転送され、同じ公開プールの上限に数えられる）が通る。DB ロックの外で並行に呼ばれる connectd /
/// Tauri の取込でも直列に並ぶ。app・connectd・MCP をまたぐ協調は無い（EUTILS_THROTTLE と同じ）
static CROSSREF_GATE: Throttle = Throttle::new(CROSSREF_INTERVAL);

/// Crossref の門を通して `f` を 1 本だけ走らせる。`f` は各送信の直前に渡された `pace` を呼ぶ
pub(crate) fn with_crossref_gate<T>(f: impl FnOnce(&dyn Fn()) -> T) -> T {
    CROSSREF_GATE.exclusive(f)
}

pub fn fetch_by_doi(doi: &str) -> Result<Resolved> {
    let url = format!("https://doi.org/{doi}");
    let req = agent()
        .get(&url)
        .set("Accept", "application/vnd.citationstyles.csl+json");
    let what = format!("doi.org negotiation ({doi})");
    let body = with_crossref_gate(|pace| call_with_retry_paced(req, &what, pace))?;
    let v: Value = serde_json::from_str(&body)?;
    Ok(Resolved {
        csl: normalize(v),
        retracted: None,
        source: "doi.org",
    })
}

/// Crossref 書誌検索の候補（照合は pdfingest::pick_match が本文テキストに対して行う）
#[derive(Debug, Clone)]
pub struct Candidate {
    pub doi: String,
    pub title: String,
    pub year: Option<i64>,
    pub first_author_family: Option<String>,
    /// Crossref の type（journal-article / posted-content / book-chapter …）。同題複数時の優先に使う
    pub kind: Option<String>,
}

/// Crossref `works?query.bibliographic=` でタイトル推定文字列から候補を引く（識別子なし PDF 用）。
pub fn search_bibliographic(q: &str, rows: usize) -> Result<Vec<Candidate>> {
    let req = agent()
        .get("https://api.crossref.org/works")
        .query("query.bibliographic", q)
        .query("rows", &rows.to_string())
        .query("select", "DOI,title,author,issued,container-title,type");
    let body = with_crossref_gate(|pace| call_with_retry_paced(req, "crossref search", pace))?;
    let v: Value = serde_json::from_str(&body)?;
    let items = v
        .pointer("/message/items")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(items
        .iter()
        .filter_map(|it| {
            let doi = it.get("DOI")?.as_str()?.to_lowercase();
            let title = crate::model::strip_markup(
                it.get("title")
                    .and_then(|t| t.as_array())
                    .and_then(|a| a.first())
                    .and_then(|t| t.as_str())?,
            );
            let year = it.pointer("/issued/date-parts/0/0").and_then(|y| y.as_i64());
            let first_author_family = it
                .pointer("/author/0/family")
                .and_then(|f| f.as_str())
                .map(|s| s.to_string());
            let kind = it.get("type").and_then(|t| t.as_str()).map(|s| s.to_string());
            Some(Candidate { doi, title, year, first_author_family, kind })
        })
        .collect())
}

/// Crossref 由来の CSL の揺れを正す:
/// title/container-title の配列 → 文字列、journal-article → article-journal、DOI 小文字化。
/// JATS マークアップ（`<i>` `<sup>` `<sub>` `<scp>`）は**剥がさない**（citeproc が書式として使う。
/// 比較・表示では `model::strip_markup` を使う）
fn normalize(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        for k in ["title", "container-title", "container-title-short"] {
            let joined = o.get(k).and_then(|x| x.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            });
            if let Some(s) = joined {
                o.insert(k.into(), json!(s));
            }
        }
        if o.get("type").and_then(|t| t.as_str()) == Some("journal-article") {
            o.insert("type".into(), json!("article-journal"));
        }
        if let Some(d) = o.get("DOI").and_then(|d| d.as_str()).map(|d| d.to_lowercase()) {
            o.insert("DOI".into(), json!(d));
        }
        // 医書.jp の DOI negotiation は type を省略し、号を number で返す。
        // CSL の issue に写し、誌名・巻・頁がある同社の DOI だけ雑誌論文と補完する。
        if !o.contains_key("issue") {
            if let Some(number) = o.get("number").cloned() {
                o.insert("issue".into(), number);
            }
        }
        if !o.contains_key("type")
            && o.get("DOI").and_then(Value::as_str).is_some_and(|d| d.starts_with("10.18885/"))
            && ["container-title", "volume", "page"].iter().all(|k| o.get(*k).and_then(Value::as_str).is_some_and(|s| !s.is_empty()))
        {
            o.insert("type".into(), json!("article-journal"));
        }
        o.remove("id"); // citeproc 用の一時 id は保持しない
        match o.get_mut("custom").and_then(|c| c.as_object_mut()) {
            Some(c) => {
                c.insert("source".into(), json!("doi.org"));
            }
            None => {
                o.insert("custom".into(), json!({ "source": "doi.org" }));
            }
        }
    }
    v
}

#[cfg(test)]
mod markup_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn csl_title_keeps_markup_but_strip_helper_cleans_it() {
        // 実例: medRxiv 10.1101/2021.10.26.21265520 の Crossref 題名
        let raw = "Protocol for\n <sup>18</sup>\n F-PSMA PET imaging in staging";
        let v = json!({ "title": [raw], "container-title": ["<scp>JMRI</scp> &amp; Friends"], "DOI": "10.1101/X", "type": "posted-content" });
        let n = normalize(v);
        // 保存する CSL はそのまま（citeproc の書式用。main と同じ）
        assert_eq!(n["title"], raw);
        assert_eq!(n["container-title"], "<scp>JMRI</scp> &amp; Friends");
        let v = json!({ "title": raw, "DOI": "10.1101/X" });
        assert_eq!(normalize(v)["title"], raw, "doi.org の文字列題名もそのまま");
        // 比較・表示用ヘルパは剥がす（上付きの後の整形改行は大文字に続くとき詰める）
        assert_eq!(crate::model::strip_markup(raw), "Protocol for 18F-PSMA PET imaging in staging");
        assert_eq!(crate::model::strip_markup("CO<sub>2</sub>\n uptake in the lung"), "CO2 uptake in the lung");
        assert_eq!(crate::model::strip_markup("<scp>MRI</scp> of the <i>in vivo</i> liver"), "MRI of the in vivo liver");
        assert_eq!(crate::model::strip_markup("A &#8211; B &#x2013; C &amp; D"), "A – B – C & D");
        assert_eq!(crate::model::strip_markup("&lt;i&gt;x&lt;/i&gt;"), "<i>x</i>", "復号はタグ除去の後");
        assert_eq!(crate::model::strip_markup("plain title"), "plain title");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_isho_metadata() {
        let v = json!({"DOI":"10.18885/CI.0000000612", "title":"頭蓋底の正常変異と側頭骨領域の先天奇形",
            "author":[{"family":"安藤","given":"知広"},{"family":"加藤","given":"博基"},{"family":"松尾","given":"政之"}],
            "container-title":"臨床画像", "issued":{"date-parts":[[2021,4,1]]},
            "volume":"37", "number":"13", "page":"28-41"});
        let n = normalize(v.clone());
        assert_eq!(n["type"], "article-journal");
        assert_eq!(n["issue"], "13");
        assert_eq!(n["author"], v["author"]);
        assert_eq!(n["issued"], v["issued"]);
        assert_eq!(n["DOI"], "10.18885/ci.0000000612");
        let mut existing = v.clone();
        existing["type"] = json!("book-chapter");
        existing["issue"] = json!("special");
        let n = normalize(existing);
        assert_eq!(n["type"], "book-chapter");
        assert_eq!(n["issue"], "special");
        assert!(normalize(json!({"DOI":"10.18885/other"})).get("type").is_none());
    }

    #[test]
    fn normalize_crossref_quirks() {
        let v = json!({
            "id": "temp",
            "type": "journal-article",
            "title": ["A Title"],
            "container-title": ["Radiology"],
            "DOI": "10.1148/RG.230216"
        });
        let n = normalize(v);
        assert_eq!(n["type"], "article-journal");
        assert_eq!(n["title"], "A Title");
        assert_eq!(n["container-title"], "Radiology");
        assert_eq!(n["DOI"], "10.1148/rg.230216");
        assert!(n.get("id").is_none());
        assert_eq!(n["custom"]["source"], "doi.org");
    }
}
