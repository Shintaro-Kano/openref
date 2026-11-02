//! OpenNote のリンク抽出・解決の規則（tests/fixtures/wiki-links.json）。描画側の node test も同じ fixture を読む

use openref_core::wiki::links::{LinkKind, Resolver};
use openref_core::wiki::{parse_html, parse_md};
use serde_json::{json, Value};
use std::path::Path;

fn fixture() -> Value {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wiki-links.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn links_json(links: &[openref_core::wiki::links::RawLink]) -> Value {
    Value::Array(links.iter().map(|l| json!({"kind": l.kind.as_str(), "target": l.target})).collect())
}

#[test]
fn wiki_md_extraction_matches_fixture() {
    let f = fixture();
    let cases = f["md"].as_array().unwrap();
    assert!(cases.len() >= 10);
    for c in cases {
        let p = parse_md::parse(c["md"].as_str().unwrap());
        assert_eq!(links_json(&p.links), c["links"], "{}", c["name"]);
    }
}

/// GUI の改名の書換えが使う link_spans は parse と同じリンクを出す（frontmatter を持たない例で全件一致）。
/// 範囲は wiki / embed = 名前の部分（target の `#` の前）・md = dest そのもの
#[test]
fn wiki_md_spans_agree_with_parse() {
    let f = fixture();
    for c in f["md"].as_array().unwrap() {
        let md = c["md"].as_str().unwrap();
        let p = parse_md::parse(md);
        if p.frontmatter.is_some() {
            continue;
        }
        let spans = parse_md::link_spans(md);
        let got: Vec<openref_core::wiki::links::RawLink> =
            spans.iter().map(|s| openref_core::wiki::links::RawLink { kind: s.kind, target: s.target.clone() }).collect();
        assert_eq!(links_json(&got), links_json(&p.links), "{}", c["name"]);
        for s in &spans {
            let text = &md[s.range.clone()];
            match s.kind {
                LinkKind::Wiki | LinkKind::Embed => assert_eq!(text, s.target.split('#').next().unwrap().trim(), "{}", c["name"]),
                _ => assert_eq!(text, s.target.trim(), "{}", c["name"]),
            }
        }
    }
}

#[test]
fn wiki_html_extraction_matches_fixture() {
    let f = fixture();
    for c in f["html"].as_array().unwrap() {
        let p = parse_html::parse(c["html"].as_str().unwrap());
        assert_eq!(links_json(&p.links), c["links"], "{}", c["name"]);
        assert_eq!(json!(p.title), c["title"], "{}", c["name"]);
        assert_eq!(json!(p.tags), c["tags"], "{}", c["name"]);
        assert_eq!(json!(p.items), c["items"], "{}", c["name"]);
    }
}

#[test]
fn wiki_resolution_matches_fixture() {
    let f = fixture();
    let r = &f["resolve"];
    let files: Vec<&str> = r["files"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let res = Resolver::new(Path::new(r["root"].as_str().unwrap()), files.iter().copied());
    for c in r["cases"].as_array().unwrap() {
        let kind = LinkKind::parse(c["kind"].as_str().unwrap()).unwrap();
        let got = res.resolve(c["src"].as_str().unwrap(), kind, c["target"].as_str().unwrap());
        assert_eq!((json!(got.path), json!(got.ambiguous)), (c["path"].clone(), c["ambiguous"].clone()), "{c}");
    }
}

/// 実 vault の計測（受入 a・c）。OPENREF_WIKI_VAULT=<vault> OPENREF_WIKI_INDEX=<vault の外の索引パス>
/// cargo test -p openref-core --release --test wiki_links_fixture -- --ignored --nocapture real_vault
#[test]
#[ignore]
fn real_vault() {
    use openref_core::wiki::Index;
    let vault = std::fs::canonicalize(std::env::var("OPENREF_WIKI_VAULT").unwrap()).unwrap();
    let ip = std::path::PathBuf::from(std::env::var("OPENREF_WIKI_INDEX").unwrap());
    assert!(!ip.starts_with(&vault), "索引を vault の中に作らない");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", ip.display()));
    }
    let mut ix = Index::open(&ip, &vault).unwrap();
    let t = std::time::Instant::now();
    ix.refresh().unwrap();
    let first = t.elapsed();
    let s1 = ix.stats().unwrap();
    let t = std::time::Instant::now();
    ix.refresh().unwrap();
    let second = t.elapsed();
    let s2 = ix.stats().unwrap();
    let t = std::time::Instant::now();
    ix.rebuild().unwrap();
    let third = t.elapsed();
    let s3 = ix.stats().unwrap();
    let size = std::fs::metadata(&ip).unwrap().len() + std::fs::metadata(format!("{}-wal", ip.display())).map(|m| m.len()).unwrap_or(0);
    println!("初回の全走査 {:.2}s / 2 回目（変更なし）{:.3}s / 作り直し {:.2}s / 索引 {size} バイト", first.as_secs_f64(), second.as_secs_f64(), third.as_secs_f64());
    println!("{}", serde_json::to_string_pretty(&s1).unwrap());
    for k in ["pages", "links", "unresolved", "ambiguous"] {
        assert_eq!(s1[k], s2[k], "{k}");
        assert_eq!(s1[k], s3[k], "{k}");
    }
    println!("--- 未解決（先頭 40）");
    for l in ix.unresolved(40).unwrap() {
        println!("{} | {} | {}", l.src, l.kind, l.target);
    }
    println!("--- ambiguous（先頭 20）");
    for l in ix.ambiguous(20).unwrap() {
        println!("{} | {} | {} -> {:?}", l.src, l.kind, l.target, l.path);
    }
}
