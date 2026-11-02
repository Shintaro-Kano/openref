//! md ページの解析: frontmatter・題名・tags・items・リンク。
//! リンクの抽出規則は `tests/fixtures/wiki-links.json` が固定する（描画側の JS も同じ fixture を読む）。
//! - frontmatter は文字列の値（配列の中も・入れ子も）の `[[…]]` だけを wiki / embed として先に数える（Obsidian 1.4 の properties の
//!   リンクと同じ = バックリンクに出る）。md リンク・tag は frontmatter から取らない
//! - 抽出しない所: コードブロック（``` / ~~~。引用・リストの中の字下げ付きも）・インラインコード（同じ行の中の
//!   同じ長さの backtick の対）・HTML コメント `<!-- -->`・字下げ 4 の code block（CommonMark に寄せた近似 = 段落の続きでも
//!   リストの中でもない字下げ 4 以上の行。リストは行頭のリスト記号で始まり、空行の後の字下げ 0 の行か見出しで終わる）
//! - `[[name|alias]]` `[[name#h]]` → wiki（target = alias を除いた `name#h`・表の中の `\|` は `|` と同じ）。`![[…]]` → embed
//! - `[text](dest)` `![alt](dest)` `<scheme:…>`（autolink）`[ref]: dest`（参照の定義）→ dest を分類（相対 = md）
//! - 本文の `#tag`（行頭か空白の直後・英数字 / 和文 / `_` `-` `/`・数字だけは除く・HTML タグの中は見ない）

use super::frontmatter::{self, Frontmatter};
use super::links::{classify, openref_key, LinkKind, RawLink};
use serde_json::Value;

/// md / html の解析結果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// frontmatter の title → 最初の h1（html は `<title>`）。無ければ None（呼び手がファイル名を使う）
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub items: Vec<String>,
    pub links: Vec<RawLink>,
    pub frontmatter: Option<Frontmatter>,
    /// 全文検索に入れる本文（索引の pages_fts.body の元）。md = frontmatter を除いた本文 + frontmatter の値（title 以外の
    /// 文字列・数値の葉。キー名は入れない）・html = タグを除いたテキスト（script / style / template / title の中は除く）
    pub text: String,
}

pub fn parse(text: &str) -> Parsed {
    let (fm_src, body_at) = frontmatter::split(text);
    let fm = fm_src.map(frontmatter::parse);
    let mut out = Parsed::default();
    let mut h1 = None;
    if let Some(f) = &fm {
        for field in f.fields.iter().filter(|x| x.raw.is_none()) {
            fm_links(&field.value, &mut out.links);
        }
    }
    let mut spans = Vec::new();
    scan_body(&text[body_at..], &mut spans, &mut out.tags, &mut h1);
    out.links.extend(spans.into_iter().map(|s| RawLink { kind: s.kind, target: s.target }));
    if let Some(f) = &fm {
        let title = match f.get("title") {
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        out.title = title;
        let mut fm_tags = Vec::new();
        for k in ["tags", "tag"] {
            fm_tags.extend(string_list(f.get(k), true));
        }
        fm_tags.append(&mut out.tags);
        out.tags = fm_tags;
        out.items = string_list(f.get("items"), false);
    }
    if out.title.is_none() {
        out.title = h1;
    }
    for l in &out.links {
        if l.kind == LinkKind::Openref {
            if let Some(k) = openref_key(&l.target) {
                out.items.push(k);
            }
        }
    }
    out.tags = dedup_nocase(std::mem::take(&mut out.tags));
    out.items = dedup_nocase(std::mem::take(&mut out.items));
    out.text = join_search_text(&text[body_at..], fm.as_ref());
    out.frontmatter = fm;
    out
}

/// 全文検索に入れる本文（`Parsed::text` と同じ）だけを作る（リンク・tag の走査をしない = 検索結果の抜粋用に軽く）
pub fn search_text(text: &str) -> String {
    let (fm_src, body_at) = frontmatter::split(text);
    let fm = fm_src.map(frontmatter::parse);
    join_search_text(&text[body_at..], fm.as_ref())
}

/// 本文のリンク 1 本と、書き換えるときに置き換える範囲（byte・`text` 全体での位置）。
/// wiki / embed = 名前の部分（`[[name#h|alias]]` の `name`）・md = dest（`<…>` の中・`#frag` も含む）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub kind: LinkKind,
    pub target: String,
    pub range: std::ops::Range<usize>,
}

/// リンクの位置（GUI の改名のリンク書換え = `wiki::edit`）。抽出の規則は parse と同じ（コードブロック・インラインコード・コメントの中は
/// 出さない）。frontmatter は生の行の `[[…]]` を拾う（parse は解けた値の中だけを数える = 解けない行の `[[…]]` はここだけに出る）
pub fn link_spans(text: &str) -> Vec<Span> {
    let (fm_src, body_at) = frontmatter::split(text);
    let mut out = Vec::new();
    if fm_src.is_some() {
        let mut off = 0;
        for line in text[..body_at].split_inclusive('\n') {
            let mut m = line.as_bytes().to_vec();
            let mut found = Vec::new();
            wikilinks(line, &mut m, &mut found);
            out.extend(found.into_iter().map(|(_, l, r)| Span { kind: l.kind, target: l.target, range: off + r.start..off + r.end }));
            off += line.len();
        }
    }
    let mut body = Vec::new();
    scan_body(&text[body_at..], &mut body, &mut Vec::new(), &mut None);
    out.extend(body.into_iter().map(|s| Span { range: body_at + s.range.start..body_at + s.range.end, ..s }));
    out
}

/// 本文 + frontmatter の値（title 以外の葉）
fn join_search_text(body: &str, fm: Option<&Frontmatter>) -> String {
    let mut search = String::with_capacity(body.len() + 64);
    search.push_str(body);
    if let Some(f) = fm {
        for field in f.fields.iter().filter(|x| x.key != "title") {
            fm_leaves(&field.value, &mut search);
        }
    }
    search
}

/// frontmatter の値の葉（文字列・数値）を改行区切りで足す（全文検索用）
fn fm_leaves(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push('\n');
            out.push_str(s);
        }
        Value::Number(n) => {
            out.push('\n');
            out.push_str(&n.to_string());
        }
        Value::Array(a) => a.iter().for_each(|x| fm_leaves(x, out)),
        Value::Object(m) => m.values().for_each(|x| fm_leaves(x, out)),
        _ => {}
    }
}

/// frontmatter の値 → 文字列の列（配列・文字列はカンマ区切り）。tags は `#` を外す
pub fn string_list(v: Option<&Value>, tag: bool) -> Vec<String> {
    let clean = |s: &str| {
        let s = s.trim();
        let s = if tag { s.trim_start_matches('#') } else { s };
        s.to_string()
    };
    let mut out = Vec::new();
    match v {
        Some(Value::Array(a)) => {
            for x in a {
                match x {
                    Value::String(s) => out.push(clean(s)),
                    Value::Number(n) => out.push(n.to_string()),
                    _ => {}
                }
            }
        }
        Some(Value::String(s)) => out.extend(s.split(',').map(clean)),
        Some(Value::Number(n)) => out.push(n.to_string()),
        _ => {}
    }
    out.retain(|s| !s.is_empty());
    out
}

pub fn dedup_nocase(v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    v.into_iter().filter(|s| seen.insert(s.to_lowercase())).collect()
}

/// frontmatter の値の中の `[[…]]`
fn fm_links(v: &Value, links: &mut Vec<RawLink>) {
    match v {
        Value::String(s) => {
            let mut m = s.as_bytes().to_vec();
            let mut found = Vec::new();
            wikilinks(s, &mut m, &mut found);
            links.extend(found.into_iter().map(|(_, l, _)| l));
        }
        Value::Array(a) => a.iter().for_each(|x| fm_links(x, links)),
        Value::Object(o) => o.values().for_each(|x| fm_links(x, links)),
        _ => {}
    }
}

/// 本文（frontmatter の後）を行ごとに見る
fn scan_body(body: &str, links: &mut Vec<Span>, tags: &mut Vec<String>, h1: &mut Option<String>) {
    let mut fence: Option<(u8, usize)> = None;
    let mut in_comment = false;
    // 字下げ 4 の code block の判定（CommonMark の近似）
    let mut para_open = false; // 直前の行が段落の本文（字下げ 4 の行は段落の続きになる）
    let mut in_list = false; // リストの中（字下げ 4 の行は項目の続き）
    let mut in_icode = false;
    let mut prev_blank = true;
    for line in body.lines() {
        // この行の body の中での位置（visible は line と同じ byte 長 = strip_comments は同じ長さの空白に置き換える）
        let off = line.as_ptr() as usize - body.as_ptr() as usize;
        if let Some((c, n)) = fence {
            if fence_marker(line).is_some_and(|(c2, n2, info)| c2 == c && n2 >= n && info.trim().is_empty()) {
                fence = None;
            }
            continue;
        }
        let visible = strip_comments(line, &mut in_comment);
        if visible.trim().is_empty() {
            para_open = false;
            prev_blank = true;
            continue;
        }
        let ind = indent_cols(line); // コメントを空白にした visible では字下げが増えるので原文で数える
        if in_icode && ind >= 4 {
            continue;
        }
        in_icode = false;
        if ind >= 4 && !para_open && !in_list {
            in_icode = true;
            prev_blank = false;
            continue;
        }
        let heading = atx_heading(&visible);
        if list_marker(&visible) {
            in_list = true;
        } else if ind == 0 && (prev_blank || heading) {
            in_list = false;
        }
        prev_blank = false;
        para_open = !heading;
        if let Some((c, n, info)) = fence_marker(&visible) {
            // backtick の fence の info に backtick は不可（CommonMark）
            if !(c == b'`' && info.contains('`')) {
                fence = Some((c, n));
                para_open = false;
                continue;
            }
        }
        if h1.is_none() {
            if let Some(t) = atx_h1(&visible) {
                *h1 = Some(t);
            }
        }
        if let Some((dest, r)) = ref_definition(&visible) {
            links.extend(classify(&dest, LinkKind::Md).map(|l| Span { kind: l.kind, target: l.target, range: off + r.start..off + r.end }));
            continue;
        }
        inline(&visible, off, links, tags);
    }
}

/// 行頭の字下げ（列。tab は 4 の倍数へ）
fn indent_cols(line: &str) -> usize {
    let mut col = 0;
    for b in line.bytes() {
        match b {
            b' ' => col += 1,
            b'\t' => col += 4 - col % 4,
            _ => break,
        }
    }
    col
}

/// ATX 見出し（`#`〜`######` + 空白か行末・字下げ 3 まで）
fn atx_heading(line: &str) -> bool {
    if indent_cols(line) > 3 {
        return false;
    }
    let t = line.trim_start();
    let n = t.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&n) && t[n..].chars().next().is_none_or(|c| c == ' ' || c == '\t')
}

/// リストの項目の行（`- ` `* ` `+ ` `1. ` `1) `・字下げは問わない = 入れ子の項目も）
fn list_marker(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") || matches!(t, "-" | "*" | "+") {
        return true;
    }
    let d = t.bytes().take_while(u8::is_ascii_digit).count();
    d > 0 && d < 10 && (t[d..].starts_with(". ") || t[d..].starts_with(") "))
}

/// fence の開き / 閉じ（行頭の空白・引用 `>`・リスト記号の後でもよい）: (文字, 長さ, 残り)
fn fence_marker(line: &str) -> Option<(u8, usize, &str)> {
    let mut t = line.trim_start();
    loop {
        if let Some(r) = t.strip_prefix('>') {
            t = r.trim_start();
            continue;
        }
        // リスト記号（`- ` `* ` `+ ` `1. `）の後の fence
        let lm = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ "));
        if let Some(r) = lm {
            t = r.trim_start();
            continue;
        }
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && digits < 10 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") ")) {
            t = t[digits + 2..].trim_start();
            continue;
        }
        break;
    }
    let c = *t.as_bytes().first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = t.bytes().take_while(|&b| b == c).count();
    (n >= 3).then(|| (c, n, &t[n..]))
}

/// `<!-- … -->` を空白にする（複数行にまたがる物は in_comment で持ち越す）
fn strip_comments(line: &str, in_comment: &mut bool) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        if *in_comment {
            match rest.find("-->") {
                Some(i) => {
                    out.extend(std::iter::repeat_n(' ', i + 3));
                    rest = &rest[i + 3..];
                    *in_comment = false;
                }
                None => {
                    out.extend(std::iter::repeat_n(' ', rest.len()));
                    return out;
                }
            }
        } else {
            match rest.find("<!--") {
                Some(i) => {
                    out.push_str(&rest[..i]);
                    out.push_str("    ");
                    rest = &rest[i + 4..];
                    *in_comment = true;
                }
                None => {
                    out.push_str(rest);
                    return out;
                }
            }
        }
    }
}

/// `# 題名`（行頭の空白 3 まで・末尾の `#` 列は除く）
fn atx_h1(line: &str) -> Option<String> {
    let ind = line.len() - line.trim_start_matches(' ').len();
    if ind > 3 {
        return None;
    }
    let t = line[ind..].strip_prefix("# ").or_else(|| line[ind..].strip_prefix("#\t"))?;
    let t = t.trim();
    let t = t.trim_end_matches('#').trim_end();
    (!t.is_empty()).then(|| t.to_string())
}

/// `[label]: dest`（脚注 `[^1]:` は除く）。(dest, 行の中での dest の範囲)
fn ref_definition(line: &str) -> Option<(String, std::ops::Range<usize>)> {
    let ind = line.len() - line.trim_start_matches(' ').len();
    if ind > 3 {
        return None;
    }
    let t = &line[ind..];
    let inner = t.strip_prefix('[')?;
    if inner.starts_with('^') || inner.starts_with('[') {
        return None;
    }
    let close = inner.find("]:")?;
    if close == 0 || inner[..close].contains(['[', ']']) {
        return None;
    }
    let rest = inner[close + 2..].trim_start();
    let dest = if let Some(r) = rest.strip_prefix('<') { r.split('>').next()? } else { rest.split_whitespace().next()? };
    let at = dest.as_ptr() as usize - line.as_ptr() as usize;
    Some((dest.to_string(), at..at + dest.len()))
}

/// 1 行の中のリンクと tag（off = この行の body の中での位置）
fn inline(line: &str, off: usize, links: &mut Vec<Span>, tags: &mut Vec<String>) {
    let mut m: Vec<u8> = line.as_bytes().to_vec();
    mask_code_spans(&mut m);
    // (位置, リンク, 置き換える範囲)。最後に位置で並べる
    let mut found: Vec<Found> = Vec::new();
    wikilinks(line, &mut m, &mut found);
    md_links(line, &mut m, &mut found);
    autolinks(line, &mut m, &mut found);
    found.sort_by_key(|(i, _, _)| *i);
    links.extend(found.into_iter().map(|(_, l, r)| Span { kind: l.kind, target: l.target, range: off + r.start..off + r.end }));
    mask_html_tags(&mut m);
    hashtags(&m, tags);
}

/// 行の中のリンク: (並べる位置, リンク, 書き換えるときに置き換える範囲)
type Found = (usize, RawLink, std::ops::Range<usize>);

fn blank(m: &mut [u8], from: usize, to: usize) {
    for b in &mut m[from..to] {
        *b = b' ';
    }
}

/// インラインコード（同じ長さの backtick の対）を空白にする。backtick の連なりを先に 1 回で数え、閉じは長さごとの位置の列を
/// 二分探索で引く（長さの違う閉じない連なりが多い行でも O(n log n)）。`\` の直後の backtick は開きにならない（残りの連なりは開きになりうる）
fn mask_code_spans(m: &mut [u8]) {
    // (開始, 長さ)
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < m.len() {
        if m[i] == b'`' {
            let n = m[i..].iter().take_while(|&&b| b == b'`').count();
            runs.push((i, n));
            i += n;
        } else {
            i += 1;
        }
    }
    if runs.len() < 2 {
        return;
    }
    let mut by_len: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (start, n) in &runs {
        by_len.entry(*n).or_default().push(*start);
    }
    let mut cursor = 0;
    for &(start, n) in &runs {
        if start < cursor {
            continue;
        }
        // 開き: `\` の直後なら 1 つ後ろから（長さ n - 1）
        let (open, len) = if start > 0 && m[start - 1] == b'\\' { (start + 1, n - 1) } else { (start, n) };
        if len == 0 {
            continue;
        }
        let Some(cands) = by_len.get(&len) else { continue };
        let k = cands.partition_point(|&p| p < open + len);
        if let Some(&close) = cands.get(k) {
            blank(m, open, close + len);
            cursor = close + len;
        }
    }
}

/// `[[…]]` と `![[…]]`
fn wikilinks(line: &str, m: &mut [u8], found: &mut Vec<Found>) {
    let mut i = 0;
    while i + 1 < m.len() {
        if !(m[i] == b'[' && m[i + 1] == b'[') {
            i += 1;
            continue;
        }
        let start = i + 2;
        let mut j = start;
        let mut end = None;
        while j + 1 < m.len() {
            if m[j] == b'[' && m[j + 1] == b'[' {
                break; // 中に `[[` = 外側は閉じない
            }
            if m[j] == b']' && m[j + 1] == b']' {
                end = Some(j);
                break;
            }
            j += 1;
        }
        let Some(e) = end else {
            i = if j > i + 2 { j } else { i + 2 };
            continue;
        };
        let embed = i > 0 && m[i - 1] == b'!';
        let inner = &line[start..e];
        let seg = inner.split('|').next().unwrap_or("").trim_end_matches('\\');
        let target = seg.trim();
        if !target.is_empty() {
            let kind = if embed { LinkKind::Embed } else { LinkKind::Wiki };
            // 名前の部分（`#` の前・前後の空白を除く）の範囲
            let name = seg.split('#').next().unwrap_or("");
            let lead = name.len() - name.trim_start().len();
            let r = start + lead..start + lead + name.trim().len();
            found.push((if embed { i - 1 } else { i }, RawLink { kind, target: target.to_string() }, r));
        }
        blank(m, if embed { i - 1 } else { i }, e + 2);
        i = e + 2;
    }
}

/// `[text](dest)` `![alt](dest)`（入れ子の画像 `[![a](x)](y)` は外 → 内の順に両方）。
/// 括弧の対応は 1 回の走査（stack）で先に求め、dest の探索は「閉じが行末まで無かった」位置を覚えて以後打ち切る（線形）
fn md_links(line: &str, m: &mut [u8], found: &mut Vec<Found>) {
    let mut close_of: Vec<usize> = vec![usize::MAX; m.len()];
    {
        let mut stack: Vec<usize> = Vec::new();
        let mut j = 0;
        while j < m.len() {
            match m[j] {
                b'\\' => j += 1,
                b'[' => stack.push(j),
                b']' => {
                    if let Some(o) = stack.pop() {
                        close_of[o] = j;
                    }
                }
                _ => {}
            }
            j += 1;
        }
    }
    let mut lim = DestLimits::default();
    for i in 0..m.len() {
        let c = close_of[i];
        if m[i] != b'[' || c == usize::MAX || m[c] != b']' || m.get(c + 1) != Some(&b'(') {
            continue;
        }
        if let Some((dest, r, end)) = link_dest(line, m, c + 2, &mut lim) {
            let at = if i > 0 && m[i - 1] == b'!' { i - 1 } else { i };
            if let Some(l) = classify(&dest, LinkKind::Md) {
                found.push((at, l, r));
            }
            blank(m, c + 1, end);
        }
    }
}

/// link_dest の打ち切り位置（この位置以降から探しても閉じは無い）
struct DestLimits {
    /// `<dest>` の `>`
    gt: usize,
    /// 括弧なしの dest が行末まで続いた（空白・`)` が無い）
    bare: usize,
    /// title の閉じ `"` `'` `)`
    quote: [usize; 3],
}

impl Default for DestLimits {
    fn default() -> Self {
        DestLimits { gt: usize::MAX, bare: usize::MAX, quote: [usize::MAX; 3] }
    }
}

/// `(` の直後から dest と（任意の）title を読み、(dest, dest の範囲, `)` の次の位置) を返す
fn link_dest(line: &str, m: &[u8], mut p: usize, lim: &mut DestLimits) -> Option<(String, std::ops::Range<usize>, usize)> {
    while p < m.len() && m[p] == b' ' {
        p += 1;
    }
    let dest;
    let range;
    if m.get(p) == Some(&b'<') {
        if p + 1 >= lim.gt {
            return None;
        }
        let Some(rel) = m[p + 1..].iter().position(|&b| b == b'>') else {
            lim.gt = p + 1;
            return None;
        };
        let e = p + 1 + rel;
        dest = line[p + 1..e].to_string();
        range = p + 1..e;
        p = e + 1;
    } else {
        if p >= lim.bare {
            return None;
        }
        let s = p;
        let mut depth = 0i32;
        while p < m.len() {
            match m[p] {
                b'\\' => p += 1,
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b' ' | b'\t' => break,
                _ => {}
            }
            p += 1;
        }
        if p >= m.len() {
            // 行末まで閉じが無い。以後の dest も（入れ子の括弧の稀な形を除き）閉じない
            lim.bare = s;
            return None;
        }
        dest = line[s..p].to_string();
        range = s..p;
    }
    while p < m.len() && m[p] == b' ' {
        p += 1;
    }
    // title: "…" '…' (…)
    if let Some(&q) = m.get(p) {
        if q == b'"' || q == b'\'' || q == b'(' {
            let (cq, k) = match q {
                b'"' => (b'"', 0),
                b'\'' => (b'\'', 1),
                _ => (b')', 2),
            };
            if p + 1 >= lim.quote[k] {
                return None;
            }
            let Some(rel) = m[p + 1..].iter().position(|&b| b == cq) else {
                lim.quote[k] = p + 1;
                return None;
            };
            p = p + 1 + rel + 1;
            while p < m.len() && m[p] == b' ' {
                p += 1;
            }
        }
    }
    (m.get(p) == Some(&b')')).then(|| (dest, range, p + 1))
}

/// `<scheme:…>`（CommonMark の autolink）
fn autolinks(line: &str, m: &mut [u8], found: &mut Vec<Found>) {
    let mut i = 0;
    while i < m.len() {
        if m[i] != b'<' {
            i += 1;
            continue;
        }
        let Some(rel) = m[i + 1..].iter().position(|&b| b == b'>' || b == b'<' || b == b' ') else { break };
        let e = i + 1 + rel;
        if m[e] != b'>' {
            i += 1;
            continue;
        }
        let inner = &line[i + 1..e];
        let colon = inner.find(':');
        let ok = colon.is_some_and(|c| {
            c >= 2 && inner.as_bytes()[0].is_ascii_alphabetic() && inner[..c].bytes().all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
        });
        if ok {
            if let Some(l) = classify(inner, LinkKind::Md) {
                found.push((i, l, i + 1..e));
            }
            blank(m, i, e + 1);
            i = e + 1;
        } else {
            i += 1;
        }
    }
}

/// HTML のタグ（`<a …>` `</a>`）を空白にする（属性の `#fff` 等を tag にしない）。閉じ `>` が以後に無ければ打ち切る（線形）
fn mask_html_tags(m: &mut [u8]) {
    let mut i = 0;
    while i < m.len() {
        if m[i] == b'<' && m.get(i + 1).is_some_and(|b| b.is_ascii_alphabetic() || *b == b'/') {
            match m[i..].iter().position(|&b| b == b'>') {
                Some(rel) => {
                    blank(m, i, i + rel + 1);
                    i += rel + 1;
                    continue;
                }
                None => return,
            }
        }
        i += 1;
    }
}

/// 本文の `#tag`（m = リンク・コード・HTML タグを空白にした行）
fn hashtags(m: &[u8], tags: &mut Vec<String>) {
    let Ok(masked) = std::str::from_utf8(m) else { return };
    let mut prev: Option<char> = None;
    for (i, c) in masked.char_indices() {
        if c == '#' && prev.is_none_or(char::is_whitespace) {
            let rest = &masked[i + 1..];
            let len: usize = rest.chars().take_while(|&ch| ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '/').map(char::len_utf8).sum();
            let t = rest[..len].trim_end_matches('/');
            if !t.is_empty() && !t.chars().all(|ch| ch.is_ascii_digit()) {
                tags.push(t.to_string());
            }
        }
        prev = Some(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_tags_items() {
        let p = parse("---\ntitle: T\ntags: [a, \"#b\"]\nitems: [KEY22222]\n---\n# H1\nbody #c and #A x#no #123 [x](openref://open?key=KEY33333)\n");
        assert_eq!(p.title.as_deref(), Some("T"));
        assert_eq!(p.tags, ["a", "b", "c"]);
        assert_eq!(p.items, ["KEY22222", "KEY33333"]);
        let p = parse("intro\n\n# 最初の見出し #\n\n# 二つ目\n");
        assert_eq!(p.title.as_deref(), Some("最初の見出し"));
        assert!(p.frontmatter.is_none());
        let p = parse("---\ntags: x, y\n---\n");
        assert_eq!(p.tags, ["x", "y"]);
        assert_eq!(p.title, None);
    }

    #[test]
    fn heading_inside_code_is_not_title() {
        let p = parse("```\n# not\n```\n# yes\n");
        assert_eq!(p.title.as_deref(), Some("yes"));
    }

    #[test]
    fn html_attr_colors_are_not_tags() {
        let p = parse("<span style=\"color: #fff\">x</span> #real\n");
        assert_eq!(p.tags, ["real"]);
    }

    #[test]
    fn code_spans_and_escapes() {
        let links = |s: &str| parse(s).links.into_iter().map(|l| l.target).collect::<Vec<_>>();
        assert_eq!(links("`[[a]]` [[b]]"), ["b"]);
        assert_eq!(links("`` ` [[a]] `` [[b]]"), ["b"]);
        assert_eq!(links("` [[a]] `` [[b]]"), ["a", "b"], "長さの違う連なりは閉じない");
        assert_eq!(links("\\`[[a]]` [[b]]"), ["a", "b"], "エスケープした ` は開きにならない");
        assert_eq!(links("\\``[[a]]` [[b]]"), ["b"], "エスケープの後の残りは開きになる");
    }

    /// 1 行 16 万字の病的な入力（閉じない `[` `(<` `<x` `(` `"`・長さの違う backtick）が線形で終わる（debug でも 1 秒未満）
    #[test]
    fn pathological_lines_are_linear() {
        let n = 160_000;
        let mut ticks = String::new();
        let mut k = 1;
        while ticks.len() < n {
            ticks.push_str(&"`".repeat(k));
            ticks.push(' ');
            k += 1;
        }
        let cases = [
            "[".repeat(n),
            "[a](<".repeat(n / 5),
            "<x".repeat(n / 2),
            "[a](x".repeat(n / 5),
            "[a](x \"".repeat(n / 7),
            "[a](x (".repeat(n / 7),
            "[[a".repeat(n / 3),
            "#a ".repeat(n / 3),
            ticks,
        ];
        for c in &cases {
            let t = std::time::Instant::now();
            let _ = parse(c);
            let el = t.elapsed();
            assert!(el < std::time::Duration::from_secs(1), "{:?} … {el:?}", &c[..10]);
        }
    }
}
