//! HTML tokenizer -> inert plain text (legacy content_md storage name). Original HTML is retained by the adapter.
//! No DOM execution, image fetching, local links or embedded objects.
use html5ever::{
    tendril::StrTendril,
    tokenizer::{
        BufferQueue, StartTag, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
    },
};
use std::{cell::RefCell, collections::BTreeSet};

#[derive(Default)]
struct Output {
    text: String,
    suppressed: Vec<String>,
    links: Vec<Option<String>>,
    lists: Vec<usize>,
    warnings: BTreeSet<String>,
}
impl Output {
    fn line(&mut self, blank: bool) {
        while self.text.ends_with(' ') {
            self.text.pop();
        }
        let n = if blank { 2 } else { 1 };
        let existing = self.text.chars().rev().take_while(|c| *c == '\n').count();
        if !self.text.is_empty() {
            for _ in existing..n {
                self.text.push('\n');
            }
        }
    }
}
#[derive(Default)]
struct Sink(RefCell<Output>);
impl TokenSink for Sink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut o = self.0.borrow_mut();
        match token {
            Token::CharacterTokens(s) if o.suppressed.is_empty() => {
                for c in s.chars() {
                    if c.is_whitespace() {
                        if !o.text.ends_with(char::is_whitespace) && !o.text.is_empty() {
                            o.text.push(' ');
                        }
                    } else {
                        o.text.push(c);
                    }
                }
            }
            Token::TagToken(tag) => {
                let start = tag.kind == StartTag;
                let name = tag.name.as_ref();
                if !o.suppressed.is_empty() {
                    if !start && o.suppressed.last().is_some_and(|n| n == name) {
                        o.suppressed.pop();
                    } else if start
                        && matches!(
                            name,
                            "script" | "style" | "iframe" | "object" | "svg" | "math"
                        )
                    {
                        o.suppressed.push(name.into());
                    }
                    return TokenSinkResult::Continue;
                }
                match name {
                    "script" | "style" | "iframe" | "object" | "svg" | "math" if start => {
                        o.suppressed.push(name.into());
                        o.warnings.insert(
                            "埋込コンテンツを表示対象から除外しました。原 HTML は保存済みです"
                                .into(),
                        );
                    }
                    "p" | "div" | "blockquote" | "section" => o.line(true),
                    "br" => o.line(false),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        o.line(true);
                    }
                    "ul" | "ol" => {
                        o.line(true);
                        if start {
                            o.lists.push(if name == "ol" { 1 } else { 0 });
                        } else {
                            o.lists.pop();
                        }
                    }
                    "li" => {
                        o.line(false);
                        if start {
                            let depth = o.lists.len().saturating_sub(1);
                            o.text.push_str(&"  ".repeat(depth));
                            let n = o.lists.last().copied().unwrap_or(0);
                            if n == 0 {
                                o.text.push_str("- ");
                            } else {
                                o.text.push_str(&format!("{n}. "));
                                if let Some(last) = o.lists.last_mut() {
                                    *last += 1;
                                }
                            }
                        }
                    }
                    "strong" | "b" | "em" | "i" => {}
                    "a" if start => {
                        let href = tag
                            .attrs
                            .iter()
                            .find(|a| a.name.local.as_ref() == "href")
                            .map(|a| a.value.to_string());
                        let safe = href
                            .and_then(|s| url::Url::parse(&s).ok())
                            .filter(|u| matches!(u.scheme(), "https" | "http" | "mailto"))
                            .map(|u| u.to_string());
                        if safe.is_none() {
                            o.warnings
                                .insert("内部リンク・未対応リンクは本文のみ保持しました".into());
                        }
                        o.links.push(safe);
                    }
                    "a" => {
                        if let Some(Some(href)) = o.links.pop() {
                            o.text.push_str(" (");
                            o.text
                                .push_str(&href.replace('(', "%28").replace(')', "%29"));
                            o.text.push(')');
                        }
                    }
                    "img" if start => {
                        if let Some(alt) = tag.attrs.iter().find(|a| a.name.local.as_ref() == "alt")
                        {
                            o.text.push_str(&alt.value);
                        }
                        o.warnings.insert(
                            "ノート内画像は対象外です。原 HTML とファイル参照は保存済みです".into(),
                        );
                    }
                    "table" | "tr" | "td" | "th" => {
                        o.line(false);
                        o.warnings.insert(
                            "表はテキストへ変換しました。元の配置は原 HTML に保持しています".into(),
                        );
                    }
                    "span" | "html" | "body" => {}
                    _ if start => {
                        o.warnings.insert(format!(
                            "HTML 要素 {name} の書式は未反映です。本文と原 HTML は保持しています"
                        ));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}
pub(super) fn to_markdown(html: &str) -> (String, Vec<String>) {
    let input = BufferQueue::default();
    input.push_back(StrTendril::from(html));
    let tokenizer = Tokenizer::new(Sink::default(), TokenizerOpts::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let output = tokenizer.sink.0.into_inner();
    (
        output.text.trim().into(),
        output.warnings.into_iter().collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paragraphs_lists_entities_and_inert_links() {
        let (md,warnings)=to_markdown("<p>A &amp; 日本語</p><ol><li>First<li>Second</ol><p><a href='https://example.org/a(b)'>Safe</a> <a href='javascript:alert(1)'>Text</a><img src='file:///private/a' alt='image'><script>secret()</script></p>");
        assert!(md.contains("A & 日本語"));
        assert!(md.contains("1. First\n2. Second"));
        assert!(md.contains("Safe (https://example.org/a%28b%29)"));
        assert!(!md.contains("javascript"));
        assert!(!md.contains("secret"));
        assert!(!md.contains("file:"));
        assert_eq!(warnings.len(), 3);
    }
}
