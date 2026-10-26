//! PubMed E-utilities による書誌機械確定。
//! esearch: DOI → PMID / efetch: PMID → PubmedArticle XML → CSL-JSON。

use super::Resolved;
use crate::{CoreError, Result};
use roxmltree::{Document, Node};
use serde_json::{json, Map, Value};

const EUTILS: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";

/// resolver 共通の HTTP agent（timeout 30s・UA 固定。crossref.rs も使う）
pub(super) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("openref/0.1 (personal reference manager)")
        .build()
}

/// 429（E-utilities は 3 req/s 超・Crossref も混雑時）なら 1s, 2s 待って最大 2 回再試行。成功したら本文を文字列で返す。
/// PDF 一括取込では 1 件あたり esearch+efetch の 2 連打になり、素のままでは実測で 429 が出た。
pub(super) fn call_with_retry(req: ureq::Request, what: &str) -> Result<String> {
    let mut attempt: u64 = 0;
    loop {
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
    let body = call_with_retry(req, "esearch")?;
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
    let xml = call_with_retry(req, "efetch")?;
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
