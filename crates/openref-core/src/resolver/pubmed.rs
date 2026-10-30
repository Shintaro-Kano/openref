//! PubMed E-utilities による書誌機械確定。
//! esearch: DOI → PMID / efetch: PMID → PubmedArticle XML → CSL-JSON。

use super::Resolved;
use crate::{CoreError, Result};
use roxmltree::{Document, Node};
use serde_json::{json, Map, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const EUTILS: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";

/// E-utilities の要求どうしの最小間隔（NCBI の上限 3 req/s = 333.3 ms）。要求ごとに agent を作り直すので
/// TLS / DNS の揺らぎで NCBI 側の到着間隔は送信間隔より縮みうる → 余裕を取って 400 ms。
/// 1 件の DOI 解決 = esearch + efetch の 2 要求で、その間も空ける。
/// Crossref（api.crossref.org・doi.org の content negotiation）は別の門 `crossref::CROSSREF_GATE`（同時 1 本 + 1.1 s 間隔）。
/// PMC の S3 には掛けない（NCBI の 3 req/s とは別の基盤）。
const EUTILS_INTERVAL: Duration = Duration::from_millis(400);

/// E-utilities 全要求（429 の再試行を含む）が通る間隔待ち。入口（CLI・GUI・connectd・MCP）側で待たない。
/// **プロセス単位の static** = 同じプロセスのスレッドどうしは直列に並ぶが、app・connectd・MCP をまたぐ協調は無い。
static EUTILS_THROTTLE: Throttle = Throttle::new(EUTILS_INTERVAL);

/// 「前回の通過から `interval` 経つまで待つ」門。Mutex を握ったまま眠るので、並行に呼ばれても 1 本ずつ間隔を空けて通る。
pub(super) struct Throttle {
    interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl Throttle {
    pub(super) const fn new(interval: Duration) -> Self {
        Self { interval, last: Mutex::new(None) }
    }

    /// 間隔だけ空ける（要求そのものは並行してよい相手 = E-utilities）
    fn wait(&self) {
        // 他スレッドが panic しても時刻 1 つの状態は壊れない → poison は無視して続ける
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        Self::pace(self.interval, &mut last);
    }

    /// 同時 1 本: `f` の間も門を閉じたままにする（Crossref = 匿名の同時接続 1）。`f` は各送信の直前に渡された `pace` を呼ぶ
    /// （前回の送信から `interval` 経つまで眠る）。記録するのは送信の時刻だけ = 次の呼び手は前の `f` の終了（応答の受信）を
    /// 待ってから、前の送信から `interval` 以上後に送る（応答が `interval` より遅ければ待たずに送る）。`f` が一度も
    /// `pace` を呼ばなければ（送らずに返った）前回の時刻のまま
    pub(super) fn exclusive<T>(&self, f: impl FnOnce(&dyn Fn()) -> T) -> T {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        let cell = std::cell::Cell::new(*last);
        let pace = || {
            let mut l = cell.get();
            Self::pace(self.interval, &mut l);
            cell.set(l);
        };
        let r = f(&pace);
        *last = cell.get();
        r
    }

    fn pace(interval: Duration, last: &mut Option<Instant>) {
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }
}

/// resolver 共通の HTTP agent（timeout 30s・UA 固定。crossref.rs も使う）
pub(super) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("openref/0.1 (personal reference manager)")
        .build()
}

/// 429（E-utilities は 3 req/s 超・Crossref も混雑時）なら 1s, 2s 待って最大 2 回再試行。成功したら本文を文字列で返す。
/// PDF 一括取込では 1 件あたり esearch+efetch の 2 連打になり、素のままでは実測で 429 が出た（今は EUTILS_THROTTLE が要求ごとに空ける）。
/// `pace` = 各送信（再試行を含む）の直前に呼ぶ待ち（E-utilities は `EUTILS_THROTTLE`・Crossref は `CROSSREF_GATE` が渡す）。
pub(super) fn call_with_retry_paced(req: ureq::Request, what: &str, pace: impl Fn()) -> Result<String> {
    let mut attempt: u64 = 0;
    loop {
        pace();
        match req.clone().call() {
            Ok(r) => return r.into_string().map_err(|e| CoreError::Http(e.to_string())),
            Err(ureq::Error::Status(429, _)) if attempt < 2 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(1000 * attempt));
            }
            Err(e) => return Err(CoreError::Http(format!("{what}: {e}"))),
        }
    }
}

pub fn find_pmid_by_doi(doi: &str) -> Result<Option<String>> {
    let req = agent()
        .get(&format!("{EUTILS}/esearch.fcgi"))
        .query("db", "pubmed")
        .query("term", &format!("\"{doi}\"[DOI]"))
        .query("retmode", "json")
        .query("tool", "openref");
    let body = call_with_retry_paced(req, "esearch", || EUTILS_THROTTLE.wait())?;
    let v: Value = serde_json::from_str(&body)?;
    Ok(v.pointer("/esearchresult/idlist")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.as_str())
        .map(String::from))
}

pub fn fetch_by_pmid(pmid: &str) -> Result<Resolved> {
    let req = agent()
        .get(&format!("{EUTILS}/efetch.fcgi"))
        .query("db", "pubmed")
        .query("id", pmid)
        .query("retmode", "xml")
        .query("tool", "openref");
    let xml = call_with_retry_paced(req, "efetch", || EUTILS_THROTTLE.wait())?;
    parse_pubmed_xml(&xml)
}

/// PubmedArticle XML → CSL-JSON。撤回（PublicationType / RetractionIn）も検出する。
pub fn parse_pubmed_xml(xml: &str) -> Result<Resolved> {
    // PubMed の efetch XML は DOCTYPE 宣言つき（roxmltree は既定で DTD 拒否）
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = Document::parse_with_options(xml, opts)
        .map_err(|e| CoreError::Parse(format!("PubMed XML: {e}")))?;
    let article = doc
        .descendants()
        .find(|n| n.has_tag_name("PubmedArticle"))
        .ok_or_else(|| {
            CoreError::Parse("PubmedArticle が見つかりません（PMID 不正の可能性）".into())
        })?;
    let citation = child(&article, "MedlineCitation")
        .ok_or_else(|| CoreError::Parse("MedlineCitation なし".into()))?;
    let art = citation
        .descendants()
        .find(|n| n.has_tag_name("Article"))
        .ok_or_else(|| CoreError::Parse("Article なし".into()))?;

    let mut m = Map::new();
    m.insert("type".into(), json!("article-journal"));

    // PMID は MedlineCitation 直下（参考文献内の PMID を拾わない）
    if let Some(p) = child(&citation, "PMID") {
        m.insert("PMID".into(), json!(deep_text(&p)));
    }
    if let Some(t) = art.descendants().find(|n| n.has_tag_name("ArticleTitle")) {
        m.insert("title".into(), json!(deep_text(&t)));
    }
    if let Some(j) = art.descendants().find(|n| n.has_tag_name("Journal")) {
        if let Some(t) = j.descendants().find(|n| n.has_tag_name("Title")) {
            m.insert("container-title".into(), json!(deep_text(&t)));
        }
        if let Some(t) = j.descendants().find(|n| n.has_tag_name("ISOAbbreviation")) {
            m.insert("container-title-short".into(), json!(deep_text(&t)));
        }
        if let Some(t) = j.descendants().find(|n| n.has_tag_name("ISSN")) {
            m.insert("ISSN".into(), json!(deep_text(&t)));
        }
        if let Some(v) = j.descendants().find(|n| n.has_tag_name("Volume")) {
            m.insert("volume".into(), json!(deep_text(&v)));
        }
        if let Some(i) = j.descendants().find(|n| n.has_tag_name("Issue")) {
            m.insert("issue".into(), json!(deep_text(&i)));
        }
        if let Some(pd) = j.descendants().find(|n| n.has_tag_name("PubDate")) {
            if let Some(dp) = date_parts(&pd) {
                m.insert("issued".into(), json!({ "date-parts": [dp] }));
            }
        }
    }
    if let Some(p) = art.descendants().find(|n| n.has_tag_name("MedlinePgn")) {
        let pg = deep_text(&p);
        if !pg.is_empty() {
            m.insert("page".into(), json!(pg));
        }
    }

    let abs_parts: Vec<String> = art
        .descendants()
        .filter(|n| n.has_tag_name("AbstractText"))
        .map(|n| match n.attribute("Label") {
            Some(l) => format!("{}: {}", l, deep_text(&n)),
            None => deep_text(&n),
        })
        .filter(|s| !s.is_empty())
        .collect();
    if !abs_parts.is_empty() {
        m.insert("abstract".into(), json!(abs_parts.join("\n")));
    }

    let mut authors = Vec::new();
    if let Some(al) = art.descendants().find(|n| n.has_tag_name("AuthorList")) {
        for a in al.children().filter(|n| n.has_tag_name("Author")) {
            if let Some(cn) = child(&a, "CollectiveName") {
                authors.push(json!({ "literal": deep_text(&cn) }));
            } else if let Some(f) = child(&a, "LastName") {
                let mut o = Map::new();
                o.insert("family".into(), json!(deep_text(&f)));
                if let Some(g) = child(&a, "ForeName") {
                    o.insert("given".into(), json!(deep_text(&g)));
                }
                authors.push(Value::Object(o));
            }
        }
    }
    if !authors.is_empty() {
        m.insert("author".into(), Value::Array(authors));
    }
    if let Some(l) = art.descendants().find(|n| n.has_tag_name("Language")) {
        m.insert("language".into(), json!(deep_text(&l)));
    }

    // DOI / PMCID は PubmedData/ArticleIdList 直下のみ（参考文献の ArticleId を拾わない）
    let mut doi: Option<String> = None;
    let mut pmcid: Option<String> = None;
    if let Some(pd) = child(&article, "PubmedData") {
        if let Some(ail) = child(&pd, "ArticleIdList") {
            for aid in ail.children().filter(|n| n.has_tag_name("ArticleId")) {
                match aid.attribute("IdType") {
                    Some("doi") if doi.is_none() => doi = Some(deep_text(&aid).to_lowercase()),
                    Some("pmc") if pmcid.is_none() => pmcid = Some(deep_text(&aid)),
                    _ => {}
                }
            }
        }
    }
    if doi.is_none() {
        doi = art
            .descendants()
            .find(|n| n.has_tag_name("ELocationID") && n.attribute("EIdType") == Some("doi"))
            .map(|n| deep_text(&n).to_lowercase());
    }
    if let Some(d) = doi.filter(|d| !d.is_empty()) {
        m.insert("DOI".into(), json!(d));
    }
    if let Some(p) = pmcid.filter(|p| !p.is_empty()) {
        m.insert("PMCID".into(), json!(p));
    }

    let kws: Vec<String> = citation
        .descendants()
        .filter(|n| n.has_tag_name("Keyword"))
        .map(|n| deep_text(&n))
        .filter(|s| !s.is_empty())
        .collect();
    if !kws.is_empty() {
        m.insert("keyword".into(), json!(kws.join(", ")));
    }

    let mesh: Vec<String> = citation
        .descendants()
        .filter(|n| n.has_tag_name("DescriptorName"))
        .map(|n| deep_text(&n))
        .collect();
    let pubtypes: Vec<String> = art
        .descendants()
        .filter(|n| n.has_tag_name("PublicationType"))
        .map(|n| deep_text(&n))
        .collect();

    let mut custom = Map::new();
    if !mesh.is_empty() {
        custom.insert("mesh".into(), json!(mesh));
    }
    if !pubtypes.is_empty() {
        custom.insert("publication-types".into(), json!(pubtypes));
    }
    custom.insert("source".into(), json!("pubmed"));
    m.insert("custom".into(), Value::Object(custom));

    // 撤回検出（Zotero retractedItems の思想を ingest 時に前倒し）
    let mut retracted = None;
    if pubtypes.iter().any(|t| t == "Retracted Publication") {
        retracted = Some("PubMed PublicationType: Retracted Publication".to_string());
    }
    for cc in citation
        .descendants()
        .filter(|n| n.has_tag_name("CommentsCorrections"))
    {
        if cc.attribute("RefType") == Some("RetractionIn") {
            let src = cc
                .descendants()
                .find(|n| n.has_tag_name("RefSource"))
                .map(|n| deep_text(&n))
                .unwrap_or_default();
            retracted = Some(format!("RetractionIn: {src}"));
        }
    }

    Ok(Resolved {
        csl: Value::Object(m),
        retracted,
        source: "pubmed",
    })
}

fn child<'a, 'i>(n: &Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

/// 子孫 text ノードを連結（<i> 等のインラインマークアップを透過）
fn deep_text(n: &Node) -> String {
    let mut out = String::new();
    for d in n.descendants() {
        if d.is_text() {
            if let Some(t) = d.text() {
                out.push_str(t);
            }
        }
    }
    out.trim().to_string()
}

fn date_parts(pubdate: &Node) -> Option<Vec<i64>> {
    if let Some(y) = child(pubdate, "Year")
        .map(|n| deep_text(&n))
        .and_then(|y| y.parse::<i64>().ok())
    {
        let mut parts = vec![y];
        if let Some(mn) = child(pubdate, "Month")
            .map(|n| deep_text(&n))
            .and_then(|m| month_num(&m))
        {
            parts.push(mn);
            if let Some(d) = child(pubdate, "Day")
                .map(|n| deep_text(&n))
                .and_then(|d| d.parse::<i64>().ok())
            {
                parts.push(d);
            }
        }
        return Some(parts);
    }
    // MedlineDate 形式（例 "2023 Jan-Feb"）→ 年のみ
    let md = child(pubdate, "MedlineDate").map(|n| deep_text(&n))?;
    let y: String = md.chars().take_while(|c| c.is_ascii_digit()).collect();
    y.parse::<i64>().ok().map(|y| vec![y])
}

fn month_num(m: &str) -> Option<i64> {
    if let Ok(n) = m.parse::<i64>() {
        if (1..=12).contains(&n) {
            return Some(n);
        }
    }
    let ml = m.to_ascii_lowercase();
    let names = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    names.iter().position(|x| ml.starts_with(x)).map(|i| (i + 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 連続 3 回: 1 回目は待たず、以降は前回の通過から EUTILS_INTERVAL 以上空く（実ネット不要）
    #[test]
    fn throttle_spaces_sequential_calls() {
        assert!(EUTILS_INTERVAL >= Duration::from_millis(400), "3 req/s に対する余裕が足りない間隔");
        let t = Throttle::new(EUTILS_INTERVAL);
        let start = Instant::now();
        let mut at = Vec::new();
        for _ in 0..3 {
            t.wait();
            at.push(Instant::now());
        }
        assert!(at[0] - start < Duration::from_millis(50), "初回は待たない");
        for w in at.windows(2) {
            assert!(w[1] - w[0] >= EUTILS_INTERVAL, "間隔 {:?}", w[1] - w[0]);
        }
    }

    /// 並行に呼ばれても（DB ロックを外して呼ぶ connectd を想定）通過時刻は互いに EUTILS_INTERVAL 以上空く
    #[test]
    fn throttle_serializes_concurrent_callers() {
        let t = std::sync::Arc::new(Throttle::new(EUTILS_INTERVAL));
        let start = Instant::now();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let t = t.clone();
                std::thread::spawn(move || {
                    t.wait();
                    Instant::now()
                })
            })
            .collect();
        let mut at: Vec<Instant> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        at.sort();
        // 4 本 = 間隔 3 つ分は必ずかかる
        assert!(at[3] - start >= EUTILS_INTERVAL * 3, "全体 {:?}", at[3] - start);
        for w in at.windows(2) {
            // 通過の記録（wait 内）と Instant::now()（wait 後）の差でわずかに縮みうる → 5ms の許容
            assert!(w[1] - w[0] + Duration::from_millis(5) >= EUTILS_INTERVAL, "間隔 {:?}", w[1] - w[0]);
        }
    }

    /// exclusive（Crossref の門）: 並行に呼ばれても `f` は同時に 1 本だけ走り（次の送信は前の `f` の終了の後）、
    /// 前の送信から次の送信（pace の後）まで interval 以上空く。f の中の 2 回目の pace（429 の再試行）も前の送信から
    /// interval 空ける（実ネット不要）
    #[test]
    fn exclusive_is_one_at_a_time_and_spaced() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let iv = Duration::from_millis(60);
        let t = Arc::new(Throttle::new(iv));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        // (最初の送信, 最後の送信, 終了)
        let spans = Arc::new(Mutex::new(Vec::<(Instant, Instant, Instant)>::new()));
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let (t, active, peak, spans) = (t.clone(), active.clone(), peak.clone(), spans.clone());
                std::thread::spawn(move || {
                    t.exclusive(|pace| {
                        pace();
                        let sent = Instant::now();
                        let mut last_sent = sent;
                        let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(n, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(15));
                        if i == 0 {
                            // 再試行の 2 回目の送信も前の送信から interval 空く
                            pace();
                            last_sent = Instant::now();
                            assert!(sent.elapsed() >= iv, "再試行の間隔 {:?}", sent.elapsed());
                        }
                        active.fetch_sub(1, Ordering::SeqCst);
                        spans.lock().unwrap().push((sent, last_sent, Instant::now()));
                    })
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1, "同時に 2 本走った");
        let mut s = spans.lock().unwrap().clone();
        s.sort();
        for w in s.windows(2) {
            assert!(w[1].0 >= w[0].2, "重なった");
            // 送信の記録（pace 内）は test 側の Instant::now() より前 = 差は interval よりわずかに短くなりうる → 5ms の許容
            assert!(w[1].0 - w[0].1 + Duration::from_millis(5) >= iv, "前の送信 → 次の送信 {:?}", w[1].0 - w[0].1);
        }
    }

    /// 間隔は「前の送信」から数える: 前の応答（`f` の終了）が interval より遅ければ、次の呼び手は終了の直後に送る
    /// （旧 = 終了から数え直して、直列でも 1 要求ごとに interval が上乗せされていた）
    #[test]
    fn exclusive_counts_from_previous_send() {
        let iv = Duration::from_millis(60);
        let t = Throttle::new(iv);
        let ended = t.exclusive(|pace| {
            pace();
            std::thread::sleep(Duration::from_millis(100)); // 応答が interval より遅い
            Instant::now()
        });
        let sent = t.exclusive(|pace| {
            pace();
            Instant::now()
        });
        assert!(sent - ended < Duration::from_millis(30), "終了 → 次の送信 {:?}（前の送信から数えていない）", sent - ended);
        // 応答が速ければ、前の送信から interval 待つ
        let first = t.exclusive(|pace| {
            pace();
            Instant::now()
        });
        let second = t.exclusive(|pace| {
            pace();
            Instant::now()
        });
        assert!(second - first + Duration::from_millis(5) >= iv, "{:?}", second - first);
        // 送らずに返った呼び手（pace を呼ばない）は時刻を進めない
        t.exclusive(|_| ());
        let third = t.exclusive(|pace| {
            pace();
            Instant::now()
        });
        assert!(third - second + Duration::from_millis(5) >= iv, "{:?}", third - second);
    }

    /// 間隔より後に来た呼び出しは待たない
    #[test]
    fn throttle_does_not_wait_after_interval() {
        let t = Throttle::new(Duration::from_millis(20));
        t.wait();
        std::thread::sleep(Duration::from_millis(30));
        let s = Instant::now();
        t.wait();
        assert!(s.elapsed() < Duration::from_millis(15));
    }

    const FIXTURE: &str = r#"<?xml version="1.0" ?>
<!DOCTYPE PubmedArticleSet PUBLIC "-//NLM//DTD PubMedArticle, 1st January 2023//EN" "https://dtd.nlm.nih.gov/ncbi/pubmed/out/pubmed_230101.dtd">
<PubmedArticleSet>
<PubmedArticle>
  <MedlineCitation Status="MEDLINE" Owner="NLM">
    <PMID Version="1">12345678</PMID>
    <Article PubModel="Print">
      <Journal>
        <ISSN IssnType="Print">0016-5085</ISSN>
        <JournalIssue CitedMedium="Internet">
          <Volume>164</Volume>
          <Issue>5</Issue>
          <PubDate><Year>2023</Year><Month>Feb</Month><Day>18</Day></PubDate>
        </JournalIssue>
        <Title>Gastroenterology</Title>
        <ISOAbbreviation>Gastroenterology</ISOAbbreviation>
      </Journal>
      <ArticleTitle>Test <i>Title</i> With Markup.</ArticleTitle>
      <Pagination><MedlinePgn>752-765</MedlinePgn></Pagination>
      <ELocationID EIdType="doi" ValidYN="Y">10.1000/TEST.DOI</ELocationID>
      <Abstract>
        <AbstractText Label="BACKGROUND">Part one.</AbstractText>
        <AbstractText Label="RESULTS">Part two.</AbstractText>
      </Abstract>
      <AuthorList CompleteYN="Y">
        <Author ValidYN="Y"><LastName>Stoffel</LastName><ForeName>Elena M</ForeName><Initials>EM</Initials></Author>
        <Author ValidYN="Y"><CollectiveName>Some Working Group</CollectiveName></Author>
      </AuthorList>
      <Language>eng</Language>
      <PublicationTypeList>
        <PublicationType UI="D016428">Journal Article</PublicationType>
      </PublicationTypeList>
    </Article>
    <MeshHeadingList>
      <MeshHeading><DescriptorName UI="D010190" MajorTopicYN="N">Pancreatic Neoplasms</DescriptorName></MeshHeading>
    </MeshHeadingList>
    <KeywordList Owner="NOTNLM">
      <Keyword MajorTopicYN="N">Pancreatic Cancer</Keyword>
    </KeywordList>
  </MedlineCitation>
  <PubmedData>
    <ArticleIdList>
      <ArticleId IdType="pubmed">12345678</ArticleId>
      <ArticleId IdType="doi">10.1000/TEST.DOI</ArticleId>
      <ArticleId IdType="pmc">PMC7654321</ArticleId>
    </ArticleIdList>
  </PubmedData>
</PubmedArticle>
</PubmedArticleSet>"#;

    #[test]
    fn parse_fixture() {
        let r = parse_pubmed_xml(FIXTURE).unwrap();
        let c = &r.csl;
        assert_eq!(c["title"], "Test Title With Markup.");
        assert_eq!(c["PMID"], "12345678");
        assert_eq!(c["DOI"], "10.1000/test.doi"); // 小文字正規化
        assert_eq!(c["PMCID"], "PMC7654321");
        assert_eq!(c["container-title"], "Gastroenterology");
        assert_eq!(c["volume"], "164");
        assert_eq!(c["issue"], "5");
        assert_eq!(c["page"], "752-765");
        assert_eq!(c["issued"]["date-parts"][0][0], 2023);
        assert_eq!(c["issued"]["date-parts"][0][1], 2);
        assert_eq!(c["issued"]["date-parts"][0][2], 18);
        assert_eq!(c["author"][0]["family"], "Stoffel");
        assert_eq!(c["author"][0]["given"], "Elena M");
        assert_eq!(c["author"][1]["literal"], "Some Working Group");
        assert!(c["abstract"].as_str().unwrap().contains("BACKGROUND: Part one."));
        assert_eq!(c["custom"]["mesh"][0], "Pancreatic Neoplasms");
        assert_eq!(c["keyword"], "Pancreatic Cancer");
        assert_eq!(c["ISSN"], "0016-5085");
        assert!(r.retracted.is_none());
        assert_eq!(r.source, "pubmed");
    }

    #[test]
    fn retraction_detected() {
        let typed = FIXTURE.replace(
            r#"<PublicationType UI="D016428">Journal Article</PublicationType>"#,
            r#"<PublicationType UI="D016428">Journal Article</PublicationType><PublicationType UI="D016441">Retracted Publication</PublicationType>"#,
        );
        let r = parse_pubmed_xml(&typed).unwrap();
        assert_eq!(r.retracted.as_deref(), Some("PubMed PublicationType: Retracted Publication"));
        assert_eq!(r.csl["custom"]["publication-types"][1], "Retracted Publication");
        // RetractionIn があればそちらの出典で上書きする
        let both = typed.replace(
            "<MeshHeadingList>",
            r#"<CommentsCorrectionsList><CommentsCorrections RefType="RetractionIn"><RefSource>Gastroenterology. 2024;166:1.</RefSource></CommentsCorrections></CommentsCorrectionsList><MeshHeadingList>"#,
        );
        assert_eq!(parse_pubmed_xml(&both).unwrap().retracted.as_deref(), Some("RetractionIn: Gastroenterology. 2024;166:1."));
    }
}
