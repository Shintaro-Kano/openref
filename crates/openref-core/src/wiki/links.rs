//! リンクの種類・URL の分類・解決。
//! 解決は Obsidian の規則: basename 一致（拡張子省略可 = `.md` を補う・大小無視・NFC）。同名が複数なら `ambiguous`
//! （同じフォルダの物を選ぶ・無ければ最短のパス）。`/` を含む指定は vault 相対 → 書いたファイルからの相対 → 末尾一致の順。

use percent_encoding::percent_decode_str;
use std::collections::HashMap;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// `[[…]]`
    Wiki,
    /// `![[…]]`
    Embed,
    /// md の `[](相対)` `![](相対)`
    Md,
    /// html の `<a href>` `<img src>` 等の相対
    Html,
    /// `openref://open?key=…`
    Openref,
    /// `file://…`
    File,
    /// http(s)・mailto 等
    External,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Wiki => "wiki",
            LinkKind::Embed => "embed",
            LinkKind::Md => "md",
            LinkKind::Html => "html",
            LinkKind::Openref => "openref",
            LinkKind::File => "file",
            LinkKind::External => "external",
        }
    }
    pub fn parse(s: &str) -> Option<LinkKind> {
        Some(match s {
            "wiki" => LinkKind::Wiki,
            "embed" => LinkKind::Embed,
            "md" => LinkKind::Md,
            "html" => LinkKind::Html,
            "openref" => LinkKind::Openref,
            "file" => LinkKind::File,
            "external" => LinkKind::External,
            _ => return None,
        })
    }
    /// vault の中のファイルを指しうる種類（解決の対象・未解決の数え上げの対象）
    pub fn internal(self) -> bool {
        matches!(self, LinkKind::Wiki | LinkKind::Embed | LinkKind::Md | LinkKind::Html)
    }
}

/// 抽出したリンク（解決前）。target は書かれたまま（wiki は alias を除いた `name#heading`・md / html は URL）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLink {
    pub kind: LinkKind,
    pub target: String,
}

/// URL / パスの分類。`relative` は相対パスの種類（md なら Md・html なら Html）。同じ頁の `#…` だけと空は None
pub fn classify(dest: &str, relative: LinkKind) -> Option<RawLink> {
    let d = dest.trim();
    if d.is_empty() || d.starts_with('#') {
        return None;
    }
    let kind = match scheme(d) {
        Some(s) if s.eq_ignore_ascii_case("openref") => LinkKind::Openref,
        Some(s) if s.eq_ignore_ascii_case("file") => LinkKind::File,
        Some(_) => LinkKind::External,
        None => relative,
    };
    Some(RawLink { kind, target: d.to_string() })
}

/// URL の scheme（`[A-Za-z][A-Za-z0-9+.-]*:`・2 文字以上 = `C:` 等は scheme にしない）
fn scheme(d: &str) -> Option<&str> {
    let i = d.find(':')?;
    let s = &d[..i];
    let mut ch = s.chars();
    (s.len() >= 2 && ch.next()?.is_ascii_alphabetic() && ch.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))).then_some(s)
}

/// `openref://open?key=KEY&…` の KEY（無ければ None）
pub fn openref_key(url: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    let q = q.split('#').next().unwrap_or(q);
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == "key")
        .map(|(_, v)| percent_decode_str(v).decode_utf8_lossy().trim().to_string())
        .filter(|k| !k.is_empty())
}

/// 解決の結果
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolved {
    pub path: Option<String>,
    pub ambiguous: bool,
}

/// vault のファイル一覧から引く解決器。パスは vault 相対・NFC・`/` 区切り
pub struct Resolver {
    /// 小文字のパス → パス
    by_path: HashMap<String, String>,
    /// 小文字のファイル名 → パス（複数）
    by_name: HashMap<String, Vec<String>>,
    /// vault の root（file:// の解決用・NFC の文字列・末尾 `/` なし）
    root: String,
}

fn key(s: &str) -> String {
    // ASCII は NFC で変わらず、小文字化も ASCII の規則と同じ（近道）
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.nfc().collect::<String>().to_lowercase()
}

fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn dir_of(p: &str) -> &str {
    p.rfind('/').map(|i| &p[..i]).unwrap_or("")
}

/// `a/./b/../c` を畳む。root より上に出たら None
pub fn normalize_rel(p: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    Some(out.join("/"))
}

/// wiki / embed の target → 引く名前（`#` の前・NFC）。空 = 同じページ
fn wiki_name(target: &str) -> String {
    target.split('#').next().unwrap_or("").trim().nfc().collect()
}

/// md / html の target → パス（`#` `?` の前・% を戻す・NFC）。空 = 同じページ
fn rel_path(target: &str) -> String {
    let t = target.split(['#', '?']).next().unwrap_or("");
    percent_decode_str(t).decode_utf8_lossy().nfc().collect()
}

/// md / html の相対: `/` 始まり = vault の root から・それ以外 = 書いたファイルのフォルダから
fn join(src: &str, t: &str) -> Option<String> {
    if let Some(abs) = t.strip_prefix('/') {
        Some(abs.to_string())
    } else {
        normalize_rel(&format!("{}/{}", dir_of(src), t))
    }
}

/// パスの basename の鍵（大小無視・NFC = `key` と同じ畳み方）
fn name_key(p: &str) -> String {
    file_name(&key(p)).to_string()
}

/// obsidian が引く名前（probe_names の一部）: `/` を含めば exact の 2 つ・それから by_name の名前
fn obsidian_names(src: &str, t: &str, out: &mut Vec<String>) {
    let t = t.trim_start_matches("./");
    if t.contains('/') {
        out.push(name_key(t.trim_start_matches('/')));
        out.extend(normalize_rel(&format!("{}/{}", dir_of(src), t)).map(|j| name_key(&j)));
    }
    out.push(key(file_name(t)));
}

/// 足した / 外したページ p が解決器の中で変える鍵を、probe_names と比べられる形で: p の basename の鍵と、
/// そこから exact / obsidian が補う拡張子（`.md` `.html` `.htm`）を外した形
pub fn touched_names(p: &str) -> Vec<String> {
    let mut out = Vec::new();
    for b in [key(file_name(p)), name_key(p)] {
        for ext in [".md", ".html", ".htm"] {
            if let Some(stem) = b.strip_suffix(ext) {
                out.push(stem.to_string());
            }
        }
        out.push(b);
    }
    out
}

impl Resolver {
    pub fn new<'a>(root: &Path, paths: impl IntoIterator<Item = &'a str>) -> Resolver {
        let mut by_path = HashMap::new();
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for p in paths {
            let k = key(p);
            by_name.entry(key(file_name(p))).or_default().push(p.to_string());
            by_path.insert(k, p.to_string());
        }
        for v in by_name.values_mut() {
            v.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        }
        let root = root.to_string_lossy().nfc().collect::<String>().trim_end_matches('/').to_string();
        Resolver { by_path, by_name, root }
    }

    /// src（リンクを書いたファイル）・kind・target（RawLink の target）を解決する
    pub fn resolve(&self, src: &str, kind: LinkKind, target: &str) -> Resolved {
        match kind {
            LinkKind::Wiki | LinkKind::Embed => {
                let t = wiki_name(target);
                if t.is_empty() {
                    // `[[#見出し]]` = 同じページ
                    return Resolved { path: Some(src.to_string()), ambiguous: false };
                }
                self.obsidian(src, &t)
            }
            LinkKind::Md | LinkKind::Html => {
                let t = rel_path(target);
                if t.is_empty() {
                    return Resolved { path: Some(src.to_string()), ambiguous: false };
                }
                if let Some(p) = join(src, &t).as_deref().and_then(|j| self.exact(j)) {
                    return Resolved { path: Some(p), ambiguous: false };
                }
                if kind == LinkKind::Md {
                    // Obsidian は md の相対リンクも wikilink と同じ規則で引く
                    return self.obsidian(src, &t);
                }
                Resolved::default()
            }
            LinkKind::File => match self.file_rel(target) {
                Some(rel) => Resolved { path: self.exact(&rel), ambiguous: false },
                None => Resolved::default(),
            },
            LinkKind::Openref | LinkKind::External => Resolved::default(),
        }
    }

    /// resolve が引きうるファイル名の鍵（`name_key`）。ページを足す / 外すとき、そのページの `touched_names` のどれにも
    /// 当たらないリンクは解決が変わらない（索引の差分解決が使う）。resolve・exact・obsidian の引き方を変えたらここも変える
    pub fn probe_names(&self, src: &str, kind: LinkKind, target: &str) -> Vec<String> {
        let t = match kind {
            LinkKind::Wiki | LinkKind::Embed => wiki_name(target),
            LinkKind::Md | LinkKind::Html => rel_path(target),
            LinkKind::File => return self.file_rel(target).map(|r| name_key(&r)).into_iter().collect(),
            LinkKind::Openref | LinkKind::External => return Vec::new(),
        };
        if t.is_empty() {
            return Vec::new();
        }
        // exact・obsidian が引く鍵の basename は、どれも t の basename の鍵（`/` は NFC でも小文字化でも前後と混ざらない）。
        // 末尾が `.` `..` 空のときだけ、畳んだ後のパスの basename が別の名前になる
        let mut out = vec![key(file_name(&t))];
        if matches!(file_name(&t), "" | "." | "..") {
            if matches!(kind, LinkKind::Md | LinkKind::Html) {
                out.extend(join(src, &t).map(|j| name_key(&j)));
            }
            if kind != LinkKind::Html {
                obsidian_names(src, &t, &mut out);
            }
        }
        out
    }

    /// file:// の target → vault 相対（file:// でない・vault の外は None）
    fn file_rel(&self, target: &str) -> Option<String> {
        let rest = match target.get(..7) {
            Some(s) if s.eq_ignore_ascii_case("file://") => &target[7..],
            _ => return None,
        };
        // file://localhost/… と file:///…
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        let rest = rest.split(['#', '?']).next().unwrap_or("");
        let abs = percent_decode_str(rest).decode_utf8_lossy().nfc().collect::<String>();
        abs.strip_prefix(&self.root).and_then(|r| r.strip_prefix('/')).map(str::to_string)
    }

    /// 完全一致（大小無視）。拡張子が無ければ `.md` を補ったものも試す
    fn exact(&self, rel: &str) -> Option<String> {
        let k = key(rel);
        self.by_path.get(&k).or_else(|| self.by_path.get(&format!("{k}.md"))).cloned()
    }

    fn obsidian(&self, src: &str, t: &str) -> Resolved {
        let t = t.trim_start_matches("./");
        if t.contains('/') {
            if let Some(p) = self.exact(t.trim_start_matches('/')) {
                return Resolved { path: Some(p), ambiguous: false };
            }
            if let Some(p) = normalize_rel(&format!("{}/{}", dir_of(src), t)).and_then(|j| self.exact(&j)) {
                return Resolved { path: Some(p), ambiguous: false };
            }
        }
        let k = key(t);
        let name = key(file_name(t));
        // 末尾一致（`/` を含む指定 = 途中のフォルダも合っている物だけ）
        let tail = |cands: &[String], full: &str| -> Vec<String> {
            cands
                .iter()
                .filter(|p| {
                    let pk = key(p);
                    !full.contains('/') || pk == full || pk.ends_with(&format!("/{full}"))
                })
                .cloned()
                .collect()
        };
        let mut cands = self.by_name.get(&format!("{name}.md")).map(|v| tail(v, &format!("{k}.md"))).unwrap_or_default();
        if cands.is_empty() {
            cands = self.by_name.get(&name).map(|v| tail(v, &k)).unwrap_or_default();
        }
        if cands.is_empty() {
            for ext in ["html", "htm"] {
                cands = self.by_name.get(&format!("{name}.{ext}")).map(|v| tail(v, &format!("{k}.{ext}"))).unwrap_or_default();
                if !cands.is_empty() {
                    break;
                }
            }
        }
        match cands.len() {
            0 => Resolved::default(),
            1 => Resolved { path: cands.pop(), ambiguous: false },
            _ => {
                let here = dir_of(src);
                let pick = cands.iter().find(|p| dir_of(p) == here).unwrap_or(&cands[0]).clone();
                Resolved { path: Some(pick), ambiguous: true }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_schemes() {
        assert_eq!(classify("openref://open?key=ABCD2345", LinkKind::Md).unwrap().kind, LinkKind::Openref);
        assert_eq!(classify("file:///Users/x/a.pdf", LinkKind::Md).unwrap().kind, LinkKind::File);
        assert_eq!(classify("https://a.b/c", LinkKind::Md).unwrap().kind, LinkKind::External);
        assert_eq!(classify("mailto:a@b", LinkKind::Html).unwrap().kind, LinkKind::External);
        assert_eq!(classify("a/b.md", LinkKind::Html).unwrap().kind, LinkKind::Html);
        assert_eq!(classify("ab:c", LinkKind::Md).unwrap().kind, LinkKind::External, "scheme の形なら外部");
        assert_eq!(classify("C:x.md", LinkKind::Md).unwrap().kind, LinkKind::Md, "1 文字は scheme にしない");
        assert_eq!(classify("#h", LinkKind::Md), None);
        assert_eq!(classify("  ", LinkKind::Md), None);
    }

    #[test]
    fn openref_keys() {
        assert_eq!(openref_key("openref://open?key=ABCD2345").as_deref(), Some("ABCD2345"));
        assert_eq!(openref_key("openref://open?page=3&key=ABCD2345#x").as_deref(), Some("ABCD2345"));
        assert_eq!(openref_key("openref://open?page=3"), None);
        assert_eq!(openref_key("openref://open"), None);
    }

    /// 差分解決の前提: ページ p を 1 つ足す / 外して resolve の結果が変わるリンクは、必ず probe_names が touched_names(p) に当たる。
    /// リンクは fixture の解決の例 + 置き場の各パスから作った書き方（名前・拡張子なし・相対・大小・NFD・`..` の畳み）
    #[test]
    fn probe_names_cover_every_change() {
        let f: serde_json::Value =
            serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wiki-links.json"))).unwrap();
        let r = &f["resolve"];
        let root = Path::new(r["root"].as_str().unwrap());
        let base: Vec<String> = r["files"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let mut pool = base.clone();
        pool.extend(
            [
                "beta.md", "Notes/ALPHA.md", "dup.html", "notes.md", "notes/sub.md", "deep/a.md", "missing.md", "x", "page.htm",
                "cafe\u{301}.md", "ΟΔΟΣ.md", "other/sub/beta.md", "img/PIC.jpg", "misc/dup.md.md", "a b.md", "胆道.md", "画像/胆道.md",
            ]
            .map(String::from),
        );
        let mut links: Vec<(String, LinkKind, String)> = r["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["src"].as_str().unwrap().into(), LinkKind::parse(c["kind"].as_str().unwrap()).unwrap(), c["target"].as_str().unwrap().into()))
            .collect();
        for p in &pool {
            let name = file_name(p);
            let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
            let no_ext = p.rsplit_once('.').map(|(s, _)| s).unwrap_or(p);
            let enc = percent_encoding::utf8_percent_encode(p, percent_encoding::NON_ALPHANUMERIC).to_string();
            for src in ["index.md", "notes/sub/x.md", "other/gamma.md", "deep/a/b/c.md"] {
                for (k, t) in [
                    (LinkKind::Wiki, name.to_string()),
                    (LinkKind::Wiki, stem.to_uppercase()),
                    (LinkKind::Wiki, no_ext.to_string()),
                    (LinkKind::Wiki, format!("../{no_ext}#h")),
                    (LinkKind::Wiki, format!("./{}", stem.nfd().collect::<String>())),
                    (LinkKind::Embed, format!("{name}#^b")),
                    (LinkKind::Md, format!("/{p}")),
                    (LinkKind::Md, format!("../{enc}?q")),
                    (LinkKind::Md, format!("{stem}/..")),
                    (LinkKind::Md, format!("{no_ext}/./")),
                    (LinkKind::Html, format!("../../{p}")),
                    (LinkKind::Html, format!("{stem}/../")),
                    (LinkKind::File, format!("file://{}/{enc}", root.display())),
                    (LinkKind::File, format!("FILE://localhost{}/{}#x", root.display(), p.to_uppercase())),
                ] {
                    links.push((src.into(), k, t));
                }
            }
        }
        let (mut changed, mut checked) = (0, 0);
        for p in &pool {
            let toggled: Vec<&str> = if base.contains(p) {
                base.iter().filter(|b| *b != p).map(String::as_str).collect()
            } else {
                base.iter().map(String::as_str).chain([p.as_str()]).collect()
            };
            let before = Resolver::new(root, base.iter().map(String::as_str));
            let after = Resolver::new(root, toggled);
            let names = touched_names(p);
            for (src, k, t) in &links {
                checked += 1;
                if before.resolve(src, *k, t) != after.resolve(src, *k, t) {
                    changed += 1;
                    for res in [&before, &after] {
                        let probe = res.probe_names(src, *k, t);
                        assert!(probe.iter().any(|n| names.contains(n)), "{p} を出し入れして {k:?} {t} ({src}) が変わるのに名前 {probe:?} が {names:?} に当たらない");
                    }
                }
            }
        }
        assert!(changed > 500, "変わる例が少なすぎる（{changed} / {checked}）");
    }

    /// probe_names は resolve の引き方の写し。引き方の端（大小・NFD・ß / İ / ﬁ / K（ケルビン記号）/ Σ の小文字化・二重拡張子・
    /// `.` 始まり・`%` `#` `?` を含む名前・末尾の `.` `..` `/`・空白）だけを集めた置き場で、置き場の全部と一部の 2 つを基に
    /// 1 パスずつ出し入れし、解決が変わるリンクがすべて touched_names に当たること（(a)(b)(c) に頼らない (d) だけの完全性）
    #[test]
    fn probe_names_cover_adversarial() {
        use std::collections::HashSet;
        let root = Path::new("/vault");
        let pool: Vec<String> = [
            "Note.md", "note", "note.html", "note.htm", "cafe\u{301}.md", "Café.md", "ΟΔΟΣ.md", "οδος.md", "İ.md", "i.md", "ß.md",
            "SS.md", "ﬁle.md", "\u{212A}.md", "x.md.md", "x.md", "x", "x.md/y.md", "a b.md", "a%20b.md", "a#b.md", "...md",
            "x .md", "x.", "胆道.md", "画像/胆道.md", "dir.md", "dir/sub/x.md", "page.HTML", "img/pic.jpg",
        ]
        .map(String::from)
        .to_vec();
        let srcs = ["index.md", "dir/sub/s.md", "x.md/s.md"];
        let enc = |s: &str| percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string();
        let mut texts: Vec<String> =
            ["", ".", "..", "./", "x/..", "dir/..", "dir/sub/..", "/dir/sub/..", "dir/sub/../../note", "x.md/.."].map(String::from).to_vec();
        for p in &pool {
            let name = file_name(p).to_string();
            let stem = name.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(name.clone());
            let no_ext = p.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(p.clone());
            let dir = dir_of(p).to_string();
            texts.extend([
                name.clone(),
                stem.to_uppercase(),
                no_ext.clone(),
                name.nfd().collect(),
                format!("../{p}"),
                format!("/{p}"),
                format!("{p}/.."),
                format!("{dir}/x/../{name}"),
                format!("{stem}#h"),
                format!("{name}/"),
            ]);
        }
        texts.sort();
        texts.dedup();
        let mut targets: Vec<(LinkKind, String)> = Vec::new();
        for t in &texts {
            targets.extend([
                (LinkKind::Wiki, t.clone()),
                (LinkKind::Md, enc(t)),
                (LinkKind::Md, format!("{t}?q")),
                (LinkKind::Html, t.clone()),
                (LinkKind::File, format!("file:///vault/{}", enc(t))),
                (LinkKind::File, format!("FILE://localhost/vault/{t}#x")),
            ]);
        }
        let mut rng = fastrand::Rng::with_seed(11);
        let bases: Vec<Vec<String>> = vec![pool.clone(), pool.iter().filter(|_| rng.bool()).cloned().collect()];
        let (mut changed, mut miss) = (0usize, Vec::new());
        for base in &bases {
            let before = Resolver::new(root, base.iter().map(String::as_str));
            for p in &pool {
                let toggled: Vec<&str> = if base.contains(p) {
                    base.iter().filter(|b| *b != p).map(String::as_str).collect()
                } else {
                    base.iter().map(String::as_str).chain([p.as_str()]).collect()
                };
                let after = Resolver::new(root, toggled);
                let names: HashSet<String> = touched_names(p).into_iter().collect();
                for src in srcs {
                    for (k, t) in &targets {
                        let b = before.resolve(src, *k, t);
                        if after.resolve(src, *k, t) != b {
                            changed += 1;
                            let probe = before.probe_names(src, *k, t);
                            if !probe.iter().any(|n| names.contains(n)) {
                                miss.push(format!("{p:?} {k:?} {t:?} ({src}) {probe:?} {names:?}"));
                            }
                        }
                    }
                }
            }
        }
        assert!(miss.is_empty(), "取りこぼし {}: {:?}", miss.len(), &miss[..miss.len().min(10)]);
        assert!(changed > 1000, "変わる例が少なすぎる（{changed}）");
    }

    /// key の ASCII の近道は、近道の無い形（NFC → to_lowercase）と同じ鍵を返す（ASCII と、小文字化の特殊な字の混ざり）
    #[test]
    fn key_ascii_shortcut_same_as_full() {
        let full = |s: &str| s.nfc().collect::<String>().to_lowercase();
        let specials = ["ß", "İ", "ﬁ", "Σ", "ΟΔΟΣ", "aΣ", "AΣ.B", "e\u{301}", "\u{301}", "胆道", "ＡＢＣ", "\u{212A}", "Å", "ǅ", "Σ/", "a/Σ"];
        for b in 0u8..0x80 {
            let c = (b as char).to_string();
            for pre in ["", "A", "Σ", "/"] {
                for post in ["", "Z", "Σ", "\u{301}"] {
                    let s = format!("{pre}{c}{post}");
                    assert_eq!(key(&s), full(&s), "{s:?}");
                }
            }
        }
        let mut rng = fastrand::Rng::with_seed(7);
        for _ in 0..20_000 {
            let mut s = String::new();
            for _ in 0..rng.usize(0..12) {
                if rng.u8(..4) == 0 {
                    s.push_str(specials[rng.usize(..specials.len())]);
                } else {
                    s.push(rng.u8(0..0x80) as char);
                }
            }
            assert_eq!(key(&s), full(&s), "{s:?}");
        }
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_rel("a/./b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize_rel("../x"), None);
        assert_eq!(normalize_rel("a/../../x"), None);
    }
}
