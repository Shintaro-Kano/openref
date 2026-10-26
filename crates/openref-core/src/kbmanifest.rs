//! 下流の知識ベース（外部ツール）との連携: 書誌確定済み manifest の供給。
//! 知識ベース側の論文取り込みの前段（PubMed 確定工程）を短絡するための構造化レコードを吐く。
//! 取り込み側のアダプタは知識ベース側で別途定義する（一方通行: OpenRef → 知識ベース）。

use crate::model;
use serde_json::{json, Value};

pub fn manifest_record(key: &str, csl: &Value, date_added: &str) -> Value {
    let authors: Vec<Value> = csl
        .get("author")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .map(|a| {
                    json!({
                        "family": a.get("family").and_then(|v| v.as_str()),
                        "given": a.get("given").and_then(|v| v.as_str()),
                        "literal": a.get("literal").and_then(|v| v.as_str()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mesh: Vec<&str> = csl
        .pointer("/custom/mesh")
        .and_then(|m| m.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    json!({
        // 知識ベース連携の契約のためキー名は改名前のまま据え置き（変えると取り込み側の改訂が要る）
        "refract_key": key,
        "pmid": model::s(csl, "PMID"),
        "doi": model::s(csl, "DOI"),
        "pmcid": model::s(csl, "PMCID"),
        "title": model::s(csl, "title"),
        "journal": model::s(csl, "container-title"),
        "journal_abbrev": model::s(csl, "container-title-short"),
        "issn": model::s(csl, "ISSN"),
        "volume": model::s(csl, "volume"),
        "issue": model::s(csl, "issue"),
        "pages": model::s(csl, "page"),
        "year": model::year(csl),
        "abstract": model::s(csl, "abstract"),
        "keywords": model::s(csl, "keyword"),
        "mesh": mesh,
        "authors": authors,
        "paperpile_filename": csl.pointer("/custom/paperpile/filename").and_then(|v| v.as_str()),
        "date_added": date_added,
    })
}
