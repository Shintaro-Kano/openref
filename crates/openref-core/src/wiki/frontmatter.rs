//! frontmatter。YAML の部分集合を自前で解く（serde_yaml は使わない）。
//! 解くもの: スカラー（素・引用符 ' "）・フロー（`[a, b]` `{k: v}`・入れ子可）・ブロックの列（`- a`・`- k: v`）・
//! 入れ子のマッピング・ブロックスカラー（`|` `>`）・コメント。
//! それ以外（アンカー `&` / 別名 `*` / タグ `!!` / 複雑なキー `?` / タブの字下げ / 閉じない引用符 等）はそのキーだけ
//! **生のまま**（`raw`）に落とす。他のキーは解く（1 つの失敗で全体を捨てない）。

use serde_json::{json, Map, Value};

/// 入れ子の上限（フロー・ブロックとも）。これを超えるキーは生のまま（深い再帰で小さなスタックのスレッドを落とさない）
pub const MAX_DEPTH: usize = 64;

/// frontmatter の 1 項目（並びは原文の順）
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    pub value: Value,
    /// 解けなかったときの原文（value は同じ文字列）。解けたら None
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frontmatter {
    pub fields: Vec<Field>,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.iter().find(|f| f.key == key && f.raw.is_none()).map(|f| &f.value)
    }
    /// 生のままに落ちた項目の数
    pub fn raw_count(&self) -> usize {
        self.fields.iter().filter(|f| f.raw.is_some()).count()
    }
    /// 応答用: `[{key, value}]`（生のままは `raw: true` を足す）
    pub fn to_json(&self) -> Value {
        Value::Array(
            self.fields
                .iter()
                .map(|f| {
                    let mut o = json!({"key": f.key, "value": f.value});
                    if f.raw.is_some() {
                        o["raw"] = json!(true);
                    }
                    o
                })
                .collect(),
        )
    }
}

/// 本文の先頭の frontmatter を切り出す: (frontmatter の中身, 本文の開始 byte)。
/// 1 行目が `---` で、後に `---` か `...` だけの行があるときだけ（Obsidian と同じ）。無ければ (None, 0)
pub fn split(text: &str) -> (Option<&str>, usize) {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = text.len() - t.len();
    let first_end = match t.find('\n') {
        Some(i) => i,
        None => return (None, 0),
    };
    if t[..first_end].trim_end() != "---" {
        return (None, 0);
    }
    let mut pos = first_end + 1;
    while pos <= t.len() {
        let end = t[pos..].find('\n').map(|i| pos + i).unwrap_or(t.len());
        let line = t[pos..end].trim_end();
        if line == "---" || line == "..." {
            let body = (end + 1).min(t.len());
            return (Some(&t[first_end + 1..pos]), bom + body);
        }
        if end == t.len() {
            break;
        }
        pos = end + 1;
    }
    (None, 0)
}

/// frontmatter の中身を解く
pub fn parse(src: &str) -> Frontmatter {
    let lines: Vec<&str> = src.lines().collect();
    let mut fields = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if is_blank_or_comment(line) {
            i += 1;
            continue;
        }
        // 項目の範囲 = この行 + 字下げされた行・空行・字下げ 0 の `- `（`key:\n- a` の形）
        let mut j = i + 1;
        while j < lines.len() {
            let l = lines[j];
            if is_blank_or_comment(l) || indent(l) > 0 || l.starts_with("- ") || l == "-" {
                j += 1;
            } else {
                break;
            }
        }
        let block = &lines[i..j];
        let raw_text = block.join("\n");
        match top_field(block) {
            Some((key, value)) => fields.push(Field { key, value, raw: None }),
            None => {
                let key = line.split(':').next().unwrap_or(line).trim().to_string();
                fields.push(Field { key, value: Value::String(raw_text.clone()), raw: Some(raw_text) });
            }
        }
        i = j;
    }
    Frontmatter { fields }
}

fn is_blank_or_comment(l: &str) -> bool {
    let t = l.trim_start();
    t.is_empty() || t.starts_with('#')
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start_matches(' ').len()
}

fn top_field(block: &[&str]) -> Option<(String, Value)> {
    if block.iter().any(|l| l.starts_with('\t') || l.trim_start_matches(' ').starts_with('\t')) {
        return None;
    }
    let (key, rest) = split_key(block[0])?;
    let lines: Vec<(usize, &str)> = block[1..].iter().filter(|l| !is_blank_or_comment(l)).map(|l| (indent(l), *l)).collect();
    // ブロックスカラーは空行も中身なので、空行込みの原文を渡す
    let value = value_of(rest, &lines, &block[1..], 0)?;
    Some((key, value))
}

/// `key: rest` を分ける（キーは素か引用符）。`key:` の後は空白か行末
fn split_key(line: &str) -> Option<(String, &str)> {
    let t = line.trim_start();
    if t.starts_with('?') || t.starts_with('-') || t.starts_with('&') || t.starts_with('*') || t.starts_with('!') {
        return None;
    }
    let (key, after) = if t.starts_with('"') || t.starts_with('\'') {
        let (k, n) = quoted(t)?;
        let after = t[n..].trim_start();
        (k, after.strip_prefix(':')?)
    } else {
        let mut idx = None;
        let b = t.as_bytes();
        for (i, &c) in b.iter().enumerate() {
            if c == b':' && (i + 1 == b.len() || b[i + 1] == b' ') {
                idx = Some(i);
                break;
            }
        }
        let i = idx?;
        let k = t[..i].trim();
        if k.is_empty() || k.contains(['{', '}', '[', ']']) {
            return None;
        }
        (k.to_string(), &t[i + 1..])
    };
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    Some((key, after.trim()))
}

/// 値: 同じ行の残り `inline` と、それより字下げの深い行 `lines`（空行・コメントを除いた (字下げ, 行)）。
/// `raw_lines` はブロックスカラー用の空行込みの原文
fn value_of(inline: &str, lines: &[(usize, &str)], raw_lines: &[&str], depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    let inline = strip_comment(inline);
    if inline.is_empty() {
        if lines.is_empty() {
            return Some(Value::Null);
        }
        let first = lines[0].1.trim_start();
        if first == "-" || first.starts_with("- ") {
            return sequence(lines, depth + 1);
        }
        return mapping(lines, depth + 1);
    }
    let c = inline.as_bytes()[0];
    if c == b'&' || c == b'*' || c == b'!' {
        return None;
    }
    if c == b'|' || c == b'>' {
        if !inline[1..].chars().all(|ch| matches!(ch, '-' | '+' | '1'..='9')) {
            return None;
        }
        return Some(block_scalar(c == b'|', &inline[1..], raw_lines));
    }
    if c == b'[' || c == b'{' {
        // 閉じていなければ続く行を足す（複数行のフロー）
        let mut joined = inline.to_string();
        for (_, l) in lines {
            joined.push(' ');
            joined.push_str(l.trim());
        }
        let mut p = Flow { s: joined.as_bytes(), src: &joined, i: 0, depth };
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() && !joined[p.i..].trim_start().starts_with('#') {
            return None;
        }
        return Some(v);
    }
    if c == b'"' || c == b'\'' {
        let mut joined = inline.to_string();
        for (_, l) in lines {
            joined.push(' ');
            joined.push_str(l.trim());
        }
        let (s, n) = quoted(&joined)?;
        let rest = strip_comment(&joined[n..]);
        if !rest.is_empty() {
            return None;
        }
        return Some(Value::String(s));
    }
    // 素のスカラー（続く字下げ行は空白で連結 = 複数行の素のスカラー）
    let mut s = inline.to_string();
    for (_, l) in lines {
        let t = l.trim();
        if t.starts_with("- ") || split_key(t).is_some() {
            // `key: value` の下に入れ子がある等 = 部分集合の外
            return None;
        }
        s.push(' ');
        s.push_str(strip_comment(t));
    }
    // 素のスカラーの中の `: `（`title: A: B`）は YAML では不可だが、表示のために文字列として受ける
    Some(plain_scalar(&s))
}

fn sequence(lines: &[(usize, &str)], depth: usize) -> Option<Value> {
    let base = lines[0].0;
    let mut out = Vec::new();
    let mut k = 0;
    while k < lines.len() {
        let (ind, l) = lines[k];
        if ind != base {
            return None;
        }
        let t = l.trim_start();
        let item = if t == "-" { "" } else { t.strip_prefix("- ")? };
        let mut e = k + 1;
        while e < lines.len() && lines[e].0 > base {
            e += 1;
        }
        let sub = &lines[k + 1..e];
        let item_t = item.trim();
        let v = if !item_t.starts_with(['[', '{', '"', '\'']) && split_key(item_t).is_some() {
            // `- k: v` = 列の要素がマッピング。1 行目のキーの字下げ = base + 2 + （- の後の空白）
            let first_ind = base + (t.len() - item.trim_start().len());
            let mut m: Vec<(usize, &str)> = vec![(first_ind, item.trim_start())];
            m.extend_from_slice(sub);
            mapping_at(&m, first_ind, depth + 1)?
        } else {
            let raw: Vec<&str> = sub.iter().map(|(_, l)| *l).collect();
            value_of(item_t, sub, &raw, depth + 1)?
        };
        out.push(v);
        k = e;
    }
    Some(Value::Array(out))
}

fn mapping(lines: &[(usize, &str)], depth: usize) -> Option<Value> {
    mapping_at(lines, lines[0].0, depth)
}

/// マッピング（キーの字下げ = base）。`lines[0]` は base の字下げで始まる（列の要素の 1 行目は字下げを持たない文字列）
fn mapping_at(lines: &[(usize, &str)], base: usize, depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut m = Map::new();
    let mut k = 0;
    while k < lines.len() {
        let (ind, l) = lines[k];
        if ind != base && k != 0 {
            return None;
        }
        let (key, rest) = split_key(l)?;
        let mut e = k + 1;
        while e < lines.len() && (lines[e].0 > base || (lines[e].0 == base && lines[e].1.trim_start().starts_with("- "))) {
            e += 1;
        }
        let sub = &lines[k + 1..e];
        let raw: Vec<&str> = sub.iter().map(|(_, l)| *l).collect();
        m.insert(key, value_of(rest, sub, &raw, depth + 1)?);
        k = e;
    }
    Some(Value::Object(m))
}

fn block_scalar(literal: bool, chomp: &str, raw_lines: &[&str]) -> Value {
    let ind = raw_lines.iter().filter(|l| !l.trim().is_empty()).map(|l| indent(l)).min().unwrap_or(0);
    // 先頭の ASCII 空白を最大 ind 個だけ外す（全角空白・NBSP だけの行で byte の切り位置が文字の中に入らない）
    let body: Vec<&str> = raw_lines.iter().map(|l| &l[indent(l).min(ind)..]).collect();
    let mut s = if literal {
        body.join("\n")
    } else {
        // 折り畳み: 空行は改行・それ以外の改行は空白
        let mut out = String::new();
        for (n, l) in body.iter().enumerate() {
            if l.is_empty() {
                out.push('\n');
            } else {
                if n > 0 && !out.ends_with('\n') && !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(l);
            }
        }
        out
    };
    let trimmed = s.trim_end_matches('\n').len();
    s.truncate(trimmed);
    if !chomp.contains('-') {
        s.push('\n');
    }
    Value::String(s)
}

/// 行末のコメント（` #` 以降）を除く。引用符の中は見ない（引用符で始まる値は quoted が処理する）
fn strip_comment(s: &str) -> &str {
    let s = s.trim();
    if s.starts_with('"') || s.starts_with('\'') {
        return s;
    }
    match s.find(" #") {
        Some(i) => s[..i].trim_end(),
        None => s,
    }
}

/// 引用符のスカラー。(中身, 閉じ引用符の次の byte)
fn quoted(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let q = b[0];
    let mut out = String::new();
    let mut chars = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if q == b'\'' {
            if c == '\'' {
                if matches!(chars.peek(), Some((_, '\''))) {
                    chars.next();
                    out.push('\'');
                    continue;
                }
                return Some((out, i + 1));
            }
            out.push(c);
        } else {
            match c {
                '"' => return Some((out, i + 1)),
                '\\' => {
                    let (_, e) = chars.next()?;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        '0' => out.push('\0'),
                        'u' => {
                            let hex: String = (0..4).filter_map(|_| chars.next().map(|(_, h)| h)).collect();
                            out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                        }
                        _ => return None,
                    }
                }
                _ => out.push(c),
            }
        }
    }
    None
}

/// 素のスカラーの型: true / false / null / ~ / 整数 / 小数。それ以外（日付も）は文字列
fn plain_scalar(s: &str) -> Value {
    let s = s.trim();
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    let digits = s.strip_prefix('-').unwrap_or(s);
    let leading_zero = digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0.");
    if !leading_zero && !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) {
        if let Ok(n) = s.parse::<i64>() {
            return json!(n);
        }
    }
    if !leading_zero
        && digits.bytes().filter(|&c| c == b'.').count() == 1
        && digits.bytes().all(|c| c.is_ascii_digit() || c == b'.')
        && !digits.starts_with('.')
        && !digits.ends_with('.')
    {
        if let Ok(f) = s.parse::<f64>() {
            return json!(f);
        }
    }
    Value::String(s.to_string())
}

/// フロー（`[a, b]` `{k: v}`）の再帰下降
struct Flow<'a> {
    /// 入れ子の深さ（MAX_DEPTH を超えたら None = そのキーは生のまま）
    s: &'a [u8],
    src: &'a str,
    i: usize,
    depth: usize,
}

impl Flow<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] == b' ' || self.s[self.i] == b'\t') {
            self.i += 1;
        }
    }
    /// 深さを数える包み（再帰の上限 = スタックを食い潰さない）
    fn value(&mut self) -> Option<Value> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let v = self.value_inner();
        self.depth -= 1;
        v
    }

    fn value_inner(&mut self) -> Option<Value> {
        self.ws();
        match *self.s.get(self.i)? {
            b'[' => {
                self.i += 1;
                let mut out = Vec::new();
                loop {
                    self.ws();
                    if *self.s.get(self.i)? == b']' {
                        self.i += 1;
                        return Some(Value::Array(out));
                    }
                    out.push(self.value()?);
                    self.ws();
                    match *self.s.get(self.i)? {
                        b',' => self.i += 1,
                        b']' => {}
                        _ => return None,
                    }
                }
            }
            b'{' => {
                self.i += 1;
                let mut m = Map::new();
                loop {
                    self.ws();
                    if *self.s.get(self.i)? == b'}' {
                        self.i += 1;
                        return Some(Value::Object(m));
                    }
                    let k = match self.value()? {
                        Value::String(s) => s,
                        Value::Null => String::new(),
                        v => v.to_string(),
                    };
                    self.ws();
                    if *self.s.get(self.i)? != b':' {
                        return None;
                    }
                    self.i += 1;
                    self.ws();
                    let v = match *self.s.get(self.i)? {
                        b',' | b'}' => Value::Null,
                        _ => self.value()?,
                    };
                    m.insert(k, v);
                    self.ws();
                    match *self.s.get(self.i)? {
                        b',' => self.i += 1,
                        b'}' => {}
                        _ => return None,
                    }
                }
            }
            b'"' | b'\'' => {
                let (v, n) = quoted(&self.src[self.i..])?;
                self.i += n;
                Some(Value::String(v))
            }
            b'&' | b'*' | b'!' => None,
            _ => {
                // 素のスカラー: `,` `]` `}` まで。`: ` か末尾の `:` はキーの終わり（フローのマッピング）
                let start = self.i;
                while self.i < self.s.len() {
                    let c = self.s[self.i];
                    if c == b',' || c == b']' || c == b'}' {
                        break;
                    }
                    if c == b':' && (self.i + 1 == self.s.len() || matches!(self.s[self.i + 1], b' ' | b',' | b']' | b'}')) {
                        break;
                    }
                    self.i += 1;
                }
                Some(plain_scalar(&self.src[start..self.i]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Frontmatter {
        parse(s)
    }

    #[test]
    fn split_requires_first_line_and_close() {
        assert_eq!(split("---\na: 1\n---\nbody"), (Some("a: 1\n"), 13));
        assert_eq!(split("---\r\na: 1\r\n---\r\nbody").0, Some("a: 1\r\n"));
        assert_eq!(split("---\na: 1\n...\nbody").0, Some("a: 1\n"));
        assert_eq!(split("---\n---\nbody"), (Some(""), 8));
        assert_eq!(split("a\n---\nb: 1\n---\n"), (None, 0));
        assert_eq!(split("---\na: 1\n"), (None, 0), "閉じない = frontmatter ではない");
        let (fm, at) = split("---\na: 1\n---");
        assert_eq!((fm, at), (Some("a: 1\n"), 12));
    }

    #[test]
    fn scalars_lists_and_quotes() {
        let f = p("title: FOPE（傍骨端線部）\ncreated: 2026-07-04\nn: 3\nx: 0081\nok: true\nnone:\ntags: [topic, 膝関節, \"a, b\", 'it''s']\nq: \"x: y # z\"\nc: plain # comment\n");
        assert_eq!(f.get("title"), Some(&json!("FOPE（傍骨端線部）")));
        assert_eq!(f.get("created"), Some(&json!("2026-07-04")));
        assert_eq!(f.get("n"), Some(&json!(3)));
        assert_eq!(f.get("x"), Some(&json!("0081")));
        assert_eq!(f.get("ok"), Some(&json!(true)));
        assert_eq!(f.get("none"), Some(&Value::Null));
        assert_eq!(f.get("tags"), Some(&json!(["topic", "膝関節", "a, b", "it's"])));
        assert_eq!(f.get("q"), Some(&json!("x: y # z")));
        assert_eq!(f.get("c"), Some(&json!("plain")));
        assert_eq!(f.raw_count(), 0);
    }

    #[test]
    fn block_lists_nested_maps_and_flow_maps() {
        let f = p("aliases:\n  - a\n  - \"b\"\ntags:\n- x\n- y\nsources:\n  - {id: RG-0081, resource: journals/a b/04.md, layer: T}\n  - id: GZ-1\n    layer: T\ngenerated:\n  by: claude/legacy\n  at: 2026-07-04\n  deep:\n    k: [1, 2]\n");
        assert_eq!(f.get("aliases"), Some(&json!(["a", "b"])));
        assert_eq!(f.get("tags"), Some(&json!(["x", "y"])));
        assert_eq!(
            f.get("sources"),
            Some(&json!([{"id": "RG-0081", "resource": "journals/a b/04.md", "layer": "T"}, {"id": "GZ-1", "layer": "T"}]))
        );
        assert_eq!(f.get("generated"), Some(&json!({"by": "claude/legacy", "at": "2026-07-04", "deep": {"k": [1, 2]}})));
        assert_eq!(f.raw_count(), 0);
        let keys: Vec<&str> = f.fields.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["aliases", "tags", "sources", "generated"], "原文の順");
    }

    #[test]
    fn block_scalars_and_multiline() {
        let f = p("desc: |\n  line1\n\n  line2\nfold: >-\n  a\n  b\nlong: one\n  two\nflow: [a,\n  b]\n");
        assert_eq!(f.get("desc"), Some(&json!("line1\n\nline2\n")));
        assert_eq!(f.get("fold"), Some(&json!("a b")));
        assert_eq!(f.get("long"), Some(&json!("one two")));
        assert_eq!(f.get("flow"), Some(&json!(["a", "b"])));
    }

    /// 全角空白・NBSP だけの行（trim では空行・字下げは 0）でも panic しない
    #[test]
    fn block_scalar_with_unicode_space_lines() {
        let f = p("d: |\n  a\n\u{3000}\n  b\ne: >\n  x\n\u{a0}\n  y\n");
        assert_eq!(f.get("d"), Some(&json!("a\n\u{3000}\nb\n")));
        assert!(f.get("e").is_some());
        // 字下げの浅い行は字下げぶんだけ外す
        let f = p("d: |\n    a\n  b\n");
        assert!(f.get("d").is_some());
    }

    /// 深い入れ子（フロー `[` / `{a: ` ×10,000・ブロックの字下げ ×10,000）でも 2 MiB のスタック（tokio の blocking スレッド
    /// = GUI の wiki_call と同じ）で落ちず、そのキーだけ生のままになる。md 全体の解析（parse_md）も同じスレッドで通す
    #[test]
    fn deep_nesting_does_not_overflow_small_stack() {
        let flow_seq = format!("k: {}\nok: 1\n", "[".repeat(10_000));
        let flow_seq_closed = format!("k: {}{}\nok: 1\n", "[".repeat(10_000), "]".repeat(10_000));
        let flow_map = format!("k: {}\nok: 1\n", "{a: ".repeat(10_000));
        let mut block = String::from("k:\n");
        // 字下げの空白で入力は二乗に太る（3,000 段 = 約 4.5MB）。上限なしなら 2 MiB のスタックで落ちる深さ
        for d in 1..3_000 {
            block.push_str(&format!("{}a{d}:\n", " ".repeat(d)));
        }
        block.push_str("ok: 1\n");
        let mut seq = String::from("k:\n");
        for d in 0..2_000 {
            seq.push_str(&format!("{}- \n", " ".repeat(d * 2)));
        }
        seq.push_str("ok: 1\n");
        let shallow = format!("k: {}1{}\nok: 1\n", "[".repeat(MAX_DEPTH - 1), "]".repeat(MAX_DEPTH - 1));
        let h = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                for src in [&flow_seq, &flow_seq_closed, &flow_map, &block, &seq] {
                    let f = parse(src);
                    assert_eq!(f.fields[0].key, "k");
                    assert!(f.fields[0].raw.is_some(), "深すぎるキーは生のまま");
                    assert_eq!(f.get("ok"), Some(&json!(1)), "他のキーは解く");
                    let md = format!("---\n{src}---\n[[x]]\n");
                    let p = crate::wiki::parse_md::parse(&md);
                    assert_eq!(p.links.len(), 1);
                }
                // 上限より浅い入れ子は解く
                let f = parse(&shallow);
                assert!(f.fields[0].raw.is_none());
            })
            .unwrap();
        h.join().expect("2 MiB のスタックで落ちない");
    }

    #[test]
    fn unsupported_falls_back_to_raw_per_key() {
        let f = p("a: &anc 1\nb: *anc\nc: \"unterminated\nd: ok\ne: [1, 2\n? complex\n");
        assert_eq!(f.get("d"), Some(&json!("ok")));
        let raws: Vec<&str> = f.fields.iter().filter(|x| x.raw.is_some()).map(|x| x.key.as_str()).collect();
        assert_eq!(raws, ["a", "b", "c", "e", "? complex"]);
        assert_eq!(f.fields[0].value, json!("a: &anc 1"));
        let j = f.to_json();
        assert_eq!(j[0]["raw"], json!(true));
        assert!(j[3].get("raw").is_none());
    }
}
