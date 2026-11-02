//! html ページの解析: `<title>` = 題名・`<meta name="wiki:tags" content="a, b">`・
//! `<meta name="wiki:items" content="KEY, KEY">`・リンク = `<a href>` `<img src>` `<video src>` `<audio src>` `<source src>`
//! （相対は kind = html）。`<script>` `<style>` `<template>` の中は見ない。html5ever の tokenizer だけを使う（DOM は作らない）

use super::links::{classify, openref_key, LinkKind, RawLink};
use super::parse_md::{dedup_nocase, Parsed};
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{BufferQueue, EndTag, StartTag, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};
use std::cell::RefCell;

#[derive(Default)]
struct Out {
    title: Option<String>,
    in_title: bool,
    title_buf: String,
    /// 中を見ない要素の入れ子（script / style / template）
    skip: Vec<String>,
    tags: Vec<String>,
    items: Vec<String>,
    links: Vec<RawLink>,
    /// タグを除いたテキスト（全文検索用）
    text: String,
}

#[derive(Default)]
struct Sink(RefCell<Out>);

impl TokenSink for Sink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut o = self.0.borrow_mut();
        match token {
            Token::CharacterTokens(s) => {
                if o.in_title && o.skip.is_empty() {
                    o.title_buf.push_str(&s);
                } else if o.skip.is_empty() {
                    o.text.push_str(&s);
                }
            }
            Token::TagToken(tag) => {
                let name = tag.name.as_ref().to_string();
                // タグの境で語がくっつかないように空白 1 つ（`<td>a</td><td>b</td>` → "a b"）
                if o.skip.is_empty() && !o.text.ends_with(char::is_whitespace) && !o.text.is_empty() {
                    o.text.push(' ');
                }
                if !o.skip.is_empty() {
                    if tag.kind == EndTag && o.skip.last() == Some(&name) {
                        o.skip.pop();
                    }
                    return TokenSinkResult::Continue;
                }
                let attr = |k: &str| tag.attrs.iter().find(|a| a.name.local.as_ref() == k).map(|a| a.value.to_string());
                if tag.kind == StartTag {
                    match name.as_str() {
                        // script / style / title の中は生のテキスト（tokenizer の状態を切り替える = 中の `<a>` をタグにしない）
                        "script" if !tag.self_closing => {
                            o.skip.push(name.clone());
                            return TokenSinkResult::RawData(RawKind::ScriptData);
                        }
                        "style" if !tag.self_closing => {
                            o.skip.push(name.clone());
                            return TokenSinkResult::RawData(RawKind::Rawtext);
                        }
                        "template" if !tag.self_closing => o.skip.push(name.clone()),
                        "title" => {
                            if o.title.is_none() {
                                o.in_title = true;
                                o.title_buf.clear();
                            }
                            return TokenSinkResult::RawData(RawKind::Rcdata);
                        }
                        "meta" => {
                            let n = attr("name").unwrap_or_default().to_ascii_lowercase();
                            let content = attr("content").unwrap_or_default();
                            let list = || content.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                            match n.as_str() {
                                "wiki:tags" => {
                                    let t: Vec<String> = list().map(|s| s.trim_start_matches('#').to_string()).collect();
                                    o.tags.extend(t);
                                }
                                "wiki:items" => {
                                    let t: Vec<String> = list().collect();
                                    o.items.extend(t);
                                }
                                _ => {}
                            }
                        }
                        "a" | "img" | "video" | "audio" | "source" => {
                            let k = if name == "a" { "href" } else { "src" };
                            if let Some(l) = attr(k).and_then(|v| classify(&v, LinkKind::Html)) {
                                o.links.push(l);
                            }
                        }
                        _ => {}
                    }
                } else if name == "title" && o.in_title {
                    o.in_title = false;
                    let t = o.title_buf.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !t.is_empty() {
                        o.title = Some(t);
                    }
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

pub fn parse(html: &str) -> Parsed {
    let input = BufferQueue::default();
    input.push_back(StrTendril::from(html));
    let tok = Tokenizer::new(Sink::default(), TokenizerOpts::default());
    let _ = tok.feed(&input);
    tok.end();
    let mut o = tok.sink.0.into_inner();
    for l in &o.links {
        if l.kind == LinkKind::Openref {
            if let Some(k) = openref_key(&l.target) {
                o.items.push(k);
            }
        }
    }
    Parsed {
        title: o.title,
        tags: dedup_nocase(o.tags),
        items: dedup_nocase(o.items),
        links: o.links,
        frontmatter: None,
        text: o.text,
    }
}
