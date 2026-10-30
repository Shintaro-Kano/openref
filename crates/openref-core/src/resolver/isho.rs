//! 医書.jp の DOI がまだ doi.org で解決できない場合の公式 RIS 書誌。
//! 対象は確認済みの臨床画像 DOI 形式のみ。取得先は固定し、返却 DOI の一致を必須にする。
use super::Resolved;
use crate::{CoreError, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) fn canonical_doi(doi: &str) -> Option<String> {
    let lower = doi.to_ascii_lowercase();
    let suffix = lower.strip_prefix("10.18885/ci.")?;
    if suffix.len() != 10 || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("10.18885/CI.{suffix}"))
}

pub(super) fn fetch(doi: &str) -> Result<Resolved> {
    let doi = canonical_doi(doi)
        .ok_or_else(|| CoreError::BadInput("医書.jp の対象 DOI ではありません".into()))?;
    let body = super::pubmed::agent()
        .post("https://webview.isho.jp/bib/download")
        .send_form(&[("doi", doi.as_str()), ("format", "0")])
        .map_err(|e| CoreError::Http(format!("医書.jp RIS: {e}")))?
        .into_string()
        .map_err(|e| CoreError::Http(e.to_string()))?;
    parse(&body, &doi)
}

fn parse(body: &str, doi: &str) -> Result<Resolved> {
    let bad = || CoreError::Parse("医書.jp RIS の書誌が不完全、または DOI が一致しません".into());
    let mut fields: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut ended = false;
    let mut last = None;
    for line in body.trim_start_matches('\u{feff}').lines() {
        if let Some((tag, value)) = line
            .split_once("  - ")
            .or_else(|| line.strip_suffix("  -").map(|t| (t, "")))
        {
            if ended {
                return Err(bad());
            } // 複数レコードを混ぜない
            if tag == "ER" {
                ended = true;
                continue;
            }
            if tag == "TY" && fields.contains_key("TY") {
                return Err(bad());
            }
            fields.entry(tag).or_default().push(value.trim().into());
            last = Some(tag);
        } else if !line.trim().is_empty() {
            if ended {
                return Err(bad());
            }
            if let Some(value) = last
                .and_then(|t| fields.get_mut(t))
                .and_then(|v| v.last_mut())
            {
                value.push(' ');
                value.push_str(line.trim());
            } else {
                return Err(bad());
            }
        }
    }
    let one = |key| {
        fields
            .get(key)
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    };
    if !ended
        || one("TY") != Some("JOUR")
        || !one("DO").is_some_and(|d| d.eq_ignore_ascii_case(doi))
    {
        return Err(bad());
    }
    let title = one("T1").ok_or_else(bad)?;
    let journal = one("JF").or_else(|| one("T2")).ok_or_else(bad)?;
    let authors: Vec<Value> = fields
        .get("AU")
        .ok_or_else(bad)?
        .iter()
        .map(|name| {
            // 出版社の和名は「姓 名」。分割不能な著者名は literal として保つ。
            let parts: Vec<_> = name.split_whitespace().collect();
            if parts.len() == 2 {
                json!({"family":parts[0], "given":parts[1]})
            } else {
                json!({"literal":name})
            }
        })
        .collect();
    if authors.is_empty() || fields["AU"].iter().any(|s| s.is_empty()) {
        return Err(bad());
    }
    let date: Vec<u32> = one("DA")
        .or_else(|| one("Y1"))
        .or_else(|| one("PY"))
        .ok_or_else(bad)?
        .split('/')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| bad())?;
    if date.is_empty()
        || date.len() > 3
        || !(1800..=2200).contains(&date[0])
        || date.get(1).is_some_and(|m| !(1..=12).contains(m))
        || date.get(2).is_some_and(|d| !(1..=31).contains(d))
    {
        return Err(bad());
    }
    let mut csl = json!({"type":"article-journal", "DOI":doi.to_ascii_lowercase(), "title":title,
        "author":authors, "container-title":journal, "issued":{"date-parts":[date]},
        "URL":format!("https://webview.isho.jp/journal/detail/abs/{doi}"),
        "custom":{"source":"isho-ris"}});
    for (tag, key) in [
        ("VL", "volume"),
        ("IS", "issue"),
        ("PB", "publisher"),
        ("SN", "ISSN"),
    ] {
        if let Some(v) = one(tag) {
            csl[key] = json!(v);
        }
    }
    if let Some(start) = one("SP") {
        csl["page"] = json!(match one("EP") {
            Some(end) if end != start => format!("{start}-{end}"),
            _ => start.to_string(),
        });
    }
    Ok(Resolved {
        csl,
        retracted: None,
        source: "isho-ris",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const RIS: &str = include_str!("fixtures/isho-ci3574.ris");
    #[test]
    fn official_ris() {
        let c = parse(RIS, "10.18885/ci.0000003574").unwrap().csl;
        assert_eq!(c["title"], "肺腫瘍に対するアブレーション治療");
        assert_eq!(
            c["author"],
            json!([{"family":"宗友","given":"一晃"},{"family":"松井","given":"裕輔"},{"family":"平木","given":"隆夫"}])
        );
        assert_eq!(c["issued"], json!({"date-parts":[[2026,11,26]]}));
        assert_eq!(c["container-title"], "臨床画像");
        assert_eq!(c["volume"], "42");
        assert_eq!(c["issue"], "11");
        assert_eq!(c["page"], "1324-1331");
    }
    #[test]
    fn rejects_wrong_or_missing_metadata() {
        let doi = "10.18885/ci.0000003574";
        assert!(parse(RIS, "10.18885/ci.0000000612").is_err());
        assert!(parse("<html>Login</html>", doi).is_err());
        assert!(parse(&RIS.replace("ER  -", "XX  -"), doi).is_err());
        assert!(parse(&format!("{RIS}{RIS}"), doi).is_err());
        assert!(parse(&RIS.replace("T1  -", "XX  -"), doi).is_err());
    }
    #[test]
    fn restricts_publisher_doi() {
        assert_eq!(
            canonical_doi("10.18885/ci.0000003574").as_deref(),
            Some("10.18885/CI.0000003574")
        );
        for d in [
            "10.9999/CI.0000003574",
            "10.18885/ci.123",
            "10.18885/ci.0000003574?x",
            "10.18885/ci.0000003574/other",
        ] {
            assert!(canonical_doi(d).is_none());
        }
    }
}
