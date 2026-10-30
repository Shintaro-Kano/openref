//! 引用スタイル（CSL）の索引・導入・解決（Paperpile の "Change citation style…" に相当）。
//!
//! - 同梱 4 種（`word/styles/` が SSOT）は常にオフラインで使える。
//! - それ以外の約 1 万誌は CSL 公式リポジトリ（Zotero Style Repository = zotero.org/styles）から
//!   **必要時に取得**し `<app dir>/styles/` にキャッシュする（以後オフライン可）。索引も同所に 30 日キャッシュ。
//! - dependent style（誌名だけの別名。本体は `rel="independent-parent"` の親）は親を取得し、親の CSL で整形する。
//!   dependent が `default-locale` を持てば親の値を上書きする（例: Der Radiologe = de-DE）。
//! - locale（"et al." 等の語彙）も同様に en-US 同梱・他は citation-style-language/locales から取得してキャッシュ。
//!
//! id / lang は UI・HTTP（connectd）から来るので、ファイル名へ使う前に必ず検証する（パス横断の防止）。

use crate::{CoreError, Db, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 同梱スタイル（id, 表示名, CSL）。表示名は従来 UI の呼び名を踏襲
pub const BUILTIN: [(&str, &str, &str); 4] = [
    ("vancouver", "Vancouver", include_str!("../../../word/styles/vancouver.csl")),
    (
        "american-medical-association",
        "AMA (JAMA)",
        include_str!("../../../word/styles/american-medical-association.csl"),
    ),
    ("radiology", "Radiology (RSNA)", include_str!("../../../word/styles/radiology.csl")),
    (
        "american-journal-of-roentgenology",
        "AJR",
        include_str!("../../../word/styles/american-journal-of-roentgenology.csl"),
    ),
];

pub const LOCALE_EN_US: &str = include_str!("../../../word/vendor/locales-en-US.xml");

pub const DEFAULT_STYLE: &str = "vancouver";
const CATALOG_URL: &str = "https://www.zotero.org/styles-files/styles.json";
const CATALOG_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

/// スタイルキャッシュの置き場（DB と同階層の `styles/`）
pub fn styles_dir(db: &Db) -> PathBuf {
    db.app_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("styles")
}

fn get_text(url: &str) -> std::result::Result<String, String> {
    crate::http::text(crate::http::agent().get(url).call(), |e| match e {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        e => e.to_string(),
    })
}

/// style id = リポジトリのファイル名（小文字英数とハイフン）。それ以外は拒否
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
}

/// locale = "ja-JP" / "en-GB" / "de" / "sr-Latn-RS" 程度。英数とハイフンのみ
pub fn valid_lang(lang: &str) -> bool {
    let parts: Vec<&str> = lang.split('-').collect();
    !lang.is_empty()
        && lang.len() <= 20
        && parts.len() <= 3
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
        && parts[0].len() >= 2
        && parts[0].len() <= 3
}

pub fn builtin_xml(id: &str) -> Option<&'static str> {
    BUILTIN.iter().find(|(i, _, _)| *i == id).map(|(_, _, x)| *x)
}

fn write_atomic(path: &Path, body: &str) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{}", fastrand::u32(..)));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------- CSL の解析 ----------

/// CSL の <info> から読む最小メタ
#[derive(Debug, Clone, PartialEq)]
pub struct StyleMeta {
    pub title: String,
    pub title_short: Option<String>,
    /// numeric / author-date / note / author / label
    pub format: Option<String>,
    /// dependent なら親の id
    pub parent: Option<String>,
    pub default_locale: Option<String>,
}

pub fn parse_meta(xml: &str) -> Result<StyleMeta> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| CoreError::Parse(format!("CSL XML: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "style" {
        return Err(CoreError::Parse("CSL ではありません（root が <style> でない）".into()));
    }
    let info = root.children().find(|n| n.has_tag_name("info"));
    let child_text = |name: &str| {
        info.and_then(|i| i.children().find(|n| n.has_tag_name(name)))
            .and_then(|n| n.text())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let title = child_text("title").unwrap_or_default();
    let title_short = child_text("title-short");
    let mut format = None;
    let mut parent = None;
    if let Some(i) = info {
        for n in i.children().filter(|n| n.is_element()) {
            if n.has_tag_name("category") {
                if let Some(f) = n.attribute("citation-format") {
                    format = Some(f.to_string());
                }
            } else if n.has_tag_name("link") && n.attribute("rel") == Some("independent-parent") {
                parent = n.attribute("href").and_then(|h| h.trim_end_matches('/').rsplit('/').next()).map(str::to_string);
            }
        }
    }
    let default_locale = root.attribute("default-locale").map(str::to_string);
    if parent.is_none() && !root.children().any(|n| n.has_tag_name("citation")) {
        return Err(CoreError::Parse("CSL に <citation> がありません（独立スタイルとして不完全）".into()));
    }
    Ok(StyleMeta { title, title_short, format, parent, default_locale })
}

/// 親 CSL の root `<style …>` 開始タグの default-locale を差し替える（無ければ足す）
pub fn with_default_locale(parent_xml: &str, locale: &str) -> String {
    let re_tag = regex::Regex::new(r"<style\b[^>]*>").unwrap();
    let Some(m) = re_tag.find(parent_xml) else {
        return parent_xml.to_string();
    };
    let tag = m.as_str();
    let re_attr = regex::Regex::new(r#"\sdefault-locale\s*=\s*("[^"]*"|'[^']*')"#).unwrap();
    let new_tag = if re_attr.is_match(tag) {
        re_attr.replace(tag, format!(r#" default-locale="{locale}""#).as_str()).into_owned()
    } else {
        let at = if tag.ends_with("/>") { tag.len() - 2 } else { tag.len() - 1 };
        format!(r#"{} default-locale="{locale}"{}"#, &tag[..at], &tag[at..])
    };
    format!("{}{}{}", &parent_xml[..m.start()], new_tag, &parent_xml[m.end()..])
}

// ---------- 取得・キャッシュ ----------

fn cached_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.csl"))
}

/// 1 スタイルの生 CSL（dependent はそのまま）。同梱 → キャッシュ → 取得 の順
fn raw_style(dir: &Path, id: &str) -> Result<String> {
    if !valid_id(id) {
        return Err(CoreError::BadInput(format!("style id が不正: {id}")));
    }
    if let Some(x) = builtin_xml(id) {
        return Ok(x.to_string());
    }
    let p = cached_path(dir, id);
    if let Ok(x) = std::fs::read_to_string(&p) {
        if parse_meta(&x).is_ok() {
            return Ok(x);
        }
    }
    let urls = [
        format!("https://www.zotero.org/styles/{id}"),
        format!("https://raw.githubusercontent.com/citation-style-language/styles/master/{id}.csl"),
        format!("https://raw.githubusercontent.com/citation-style-language/styles/master/dependent/{id}.csl"),
    ];
    let mut errs = Vec::new();
    for u in &urls {
        match get_text(u) {
            Ok(x) => match parse_meta(&x) {
                Ok(_) => {
                    write_atomic(&p, &x)?;
                    return Ok(x);
                }
                Err(e) => errs.push(format!("{u}: {e}")),
            },
            Err(e) => errs.push(format!("{u}: {e}")),
        }
    }
    Err(CoreError::NotFound(format!("スタイル {id} を取得できません — {}", errs.join(" / "))))
}

/// citeproc にそのまま渡せる CSL（dependent は親を取得して解決済み）
pub fn style_xml(dir: &Path, id: &str) -> Result<String> {
    let raw = raw_style(dir, id)?;
    let meta = parse_meta(&raw)?;
    let Some(parent) = meta.parent else {
        return Ok(raw);
    };
    let parent_xml = raw_style(dir, &parent)?;
    let pmeta = parse_meta(&parent_xml)?;
    if pmeta.parent.is_some() {
        return Err(CoreError::Parse(format!("{id} の親 {parent} も dependent です（CSL 仕様違反）")));
    }
    Ok(match meta.default_locale {
        Some(loc) if Some(&loc) != pmeta.default_locale.as_ref() && valid_lang(&loc) => {
            with_default_locale(&parent_xml, &loc)
        }
        _ => parent_xml,
    })
}

/// locale XML（en-US は同梱・他は取得してキャッシュ）
pub fn locale_xml(dir: &Path, lang: &str) -> Result<String> {
    if !valid_lang(lang) {
        return Err(CoreError::BadInput(format!("locale が不正: {lang}")));
    }
    if lang == "en-US" {
        return Ok(LOCALE_EN_US.to_string());
    }
    let p = dir.join("locales").join(format!("locales-{lang}.xml"));
    if let Ok(x) = std::fs::read_to_string(&p) {
        return Ok(x);
    }
    let url = format!("https://raw.githubusercontent.com/citation-style-language/locales/master/locales-{lang}.xml");
    let x = get_text(&url).map_err(|e| CoreError::NotFound(format!("locale {lang} を取得できません — {e}")))?;
    roxmltree::Document::parse(&x).map_err(|e| CoreError::Parse(format!("locale XML: {e}")))?;
    write_atomic(&p, &x)?;
    Ok(x)
}

// ---------- 索引（約 1 万誌） ----------

/// Zotero Style Repository の索引を UI 向けに圧縮した配列
/// `[{id, title, short?, format?, dep, fields}]`。30 日キャッシュ・取得失敗時は古いキャッシュで続行
pub fn catalog(dir: &Path, refresh: bool) -> Result<Vec<Value>> {
    let p = dir.join("catalog.json");
    let fresh = std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(|age| age < CATALOG_MAX_AGE)
        .unwrap_or(false);
    let raw = if fresh && !refresh {
        std::fs::read_to_string(&p)?
    } else {
        match get_text(CATALOG_URL) {
            Ok(x) if serde_json::from_str::<Vec<Value>>(&x).is_ok() => {
                write_atomic(&p, &x)?;
                x
            }
            r => match std::fs::read_to_string(&p) {
                Ok(old) => old,
                Err(_) => {
                    let why = r.err().unwrap_or_else(|| "索引の形式が不正".into());
                    return Err(CoreError::Http(format!("スタイル索引を取得できません（{why}）")));
                }
            },
        }
    };
    let arr: Vec<Value> = serde_json::from_str(&raw)?;
    Ok(arr.iter().filter_map(compact_entry).collect())
}

fn compact_entry(v: &Value) -> Option<Value> {
    let id = v.get("name")?.as_str()?;
    if !valid_id(id) {
        return None;
    }
    let title = v.get("title").and_then(|t| t.as_str()).unwrap_or(id);
    let mut o = json!({
        "id": id,
        "title": title,
        "dep": v.get("dependent").and_then(|d| d.as_i64()).unwrap_or(0) == 1,
    });
    if let Some(s) = v.get("titleShort").and_then(|t| t.as_str()).filter(|s| !s.is_empty()) {
        o["short"] = json!(s);
    }
    if let Some(f) = v.pointer("/categories/format").and_then(|t| t.as_str()) {
        o["format"] = json!(f);
    }
    if let Some(f) = v.pointer("/categories/fields") {
        o["fields"] = f.clone();
    }
    Some(o)
}

// ---------- 導入済み（同梱 + user が選んだもの） ----------

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("installed.json")
}

fn read_manifest(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(manifest_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .unwrap_or_default()
}

/// スタイルを取得・検証し（dependent は親も）導入済み一覧へ載せる。戻り値はその一覧の 1 行
pub fn install(dir: &Path, id: &str) -> Result<Value> {
    if let Some((_, title, _)) = BUILTIN.iter().find(|(i, _, _)| *i == id) {
        return Ok(json!({"id": id, "title": title, "builtin": true}));
    }
    // 解決まで通ること（= citeproc に渡せること）を導入の条件にする
    style_xml(dir, id)?;
    let meta = parse_meta(&raw_style(dir, id)?)?;
    let format = match (&meta.format, &meta.parent) {
        (Some(f), _) => Some(f.clone()),
        (None, Some(p)) => parse_meta(&raw_style(dir, p)?).ok().and_then(|m| m.format),
        _ => None,
    };
    let mut entry = json!({
        "id": id,
        "title": if meta.title.is_empty() { id.to_string() } else { meta.title.clone() },
        "short": meta.title_short,
        "format": format,
        "parent": meta.parent,
    });
    let mut list = read_manifest(dir);
    if let Some(pos) = list.iter().position(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)) {
        // 再導入（既定に設定した時など）でも「自作」の印は保つ。導入順も保つ
        if list[pos].get("custom") == Some(&json!(true)) {
            entry["custom"] = json!(true);
        }
        list[pos] = entry.clone();
    } else {
        list.push(entry.clone());
    }
    write_atomic(&manifest_path(dir), &serde_json::to_string_pretty(&list)?)?;
    Ok(entry)
}

/// 手元の .csl を読み込んで導入（リポジトリに無い誌・投稿規定から作った自作スタイル用）。
/// id は CSL の `<info><id>` 末尾（無ければファイル名）を正規化し、**常に `custom-` を前置**する。
/// リポジトリの id のまま置くと取得キャッシュ（`<id>.csl`）を上書きし、それを親に持つ誌名スタイル全部と
/// その id を固定した Word 文書の書式が黙って変わるため（リポジトリに `custom-` で始まる id は無い）。
/// dependent なら親はリポジトリから取得する
pub fn install_file(dir: &Path, path: &Path) -> Result<Value> {
    let xml = std::fs::read_to_string(path)?;
    parse_meta(&xml)?;
    let doc = roxmltree::Document::parse(&xml).map_err(|e| CoreError::Parse(e.to_string()))?;
    let info_id = doc
        .descendants()
        .find(|n| n.has_tag_name("id") && n.parent().is_some_and(|p| p.has_tag_name("info")))
        .and_then(|n| n.text())
        .map(|t| t.trim().trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string())
        .unwrap_or_default();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let norm = |s: &str| -> String {
        let mut o = String::new();
        for c in s.to_lowercase().chars() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                o.push(c);
            } else if !o.ends_with('-') {
                o.push('-');
            }
        }
        o.trim_matches('-').to_string()
    };
    let mut id = norm(&info_id);
    if !valid_id(&id) {
        id = norm(&stem);
    }
    if !valid_id(&id) {
        return Err(CoreError::BadInput(format!("スタイル id を決められません: {}", path.display())));
    }
    if !id.starts_with("custom-") {
        id = format!("custom-{id}");
    }
    write_atomic(&cached_path(dir, &id), &xml)?;
    let mut entry = install(dir, &id)?;
    entry["custom"] = json!(true);
    let mut list = read_manifest(dir);
    for e in list.iter_mut() {
        if e.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
            *e = entry.clone();
        }
    }
    write_atomic(&manifest_path(dir), &serde_json::to_string_pretty(&list)?)?;
    Ok(entry)
}

/// 同梱 4 種 + 導入済み（導入順）。Word アドインのスタイル一覧もこれ
pub fn installed(dir: &Path) -> Vec<Value> {
    let mut out: Vec<Value> = BUILTIN
        .iter()
        .map(|(id, title, _)| json!({"id": id, "title": title, "builtin": true}))
        .collect();
    for e in read_manifest(dir) {
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if valid_id(id) && builtin_xml(id).is_none() && cached_path(dir, id).exists() {
            out.push(e);
        }
    }
    out
}

// ---------- 既定スタイル・言語（settings） ----------

/// `{style, locale, doi_always}`。locale = "" は「スタイルの既定」。
/// doi_always = 文献リストに DOI（無ければ URL）を常に含める（Paperpile の Always include DOI and URL。既定オフ）
/// settings を読めなければ Err（既定値へ倒すと、部分更新が読めなかった項目を既定で書き戻す・Word が新規文書に既定スタイルを付ける）
pub fn prefs(db: &Db) -> Result<Value> {
    let style = db
        .setting("cite_style")?
        .filter(|s| valid_id(s))
        .unwrap_or_else(|| DEFAULT_STYLE.to_string());
    let locale = db.setting("cite_locale")?.filter(|s| valid_lang(s)).unwrap_or_default();
    let doi_always = db.setting("cite_doi_always")?.as_deref() == Some("1");
    Ok(json!({"style": style, "locale": locale, "doi_always": doi_always}))
}

/// 既定スタイルを設定（導入を兼ねる）
pub fn set_prefs(db: &Db, style: &str, locale: &str, doi_always: bool) -> Result<Value> {
    if !locale.is_empty() && !valid_lang(locale) {
        return Err(CoreError::BadInput(format!("locale が不正: {locale}")));
    }
    install(&styles_dir(db), style)?;
    db.set_setting("cite_style", style)?;
    db.set_setting("cite_locale", locale)?;
    db.set_setting("cite_doi_always", if doi_always { "1" } else { "0" })?;
    prefs(db)
}

/// 部分更新: 渡さなかった項目（None）は今の設定のまま（style だけ変える等で doi_always を黙って戻さない）。全部 None は BadInput。
/// 登録表（MCP の cite_set_prefs）と GUI（Tauri の cite_set_prefs）の両方がこれを呼ぶ = 同じ振る舞い
pub fn set_prefs_partial(db: &Db, style: Option<&str>, locale: Option<&str>, doi_always: Option<bool>) -> Result<Value> {
    if style.is_none() && locale.is_none() && doi_always.is_none() {
        return Err(CoreError::BadInput("style / locale / doi_always のどれかを指定してください".into()));
    }
    let cur = prefs(db)?;
    set_prefs(
        db,
        style.unwrap_or_else(|| cur["style"].as_str().unwrap_or_default()),
        locale.unwrap_or_else(|| cur["locale"].as_str().unwrap_or_default()),
        doi_always.unwrap_or_else(|| cur["doi_always"].as_bool().unwrap_or(false)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEP: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" version="1.0" default-locale="de-DE">
  <info>
    <title>Der Radiologe (Deutsch)</title>
    <title-short>Radiologe</title-short>
    <id>http://www.zotero.org/styles/der-radiologe</id>
    <link href="http://www.zotero.org/styles/der-radiologe" rel="self"/>
    <link href="http://www.zotero.org/styles/springer-basic-brackets" rel="independent-parent"/>
    <category citation-format="numeric"/>
  </info>
</style>"#;

    #[test]
    fn ids_and_langs_are_validated() {
        assert!(valid_id("european-radiology"));
        assert!(valid_id("2d-materials"));
        assert!(!valid_id("../etc/passwd"));
        assert!(!valid_id("Radiology"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(""));
        assert!(valid_lang("ja-JP") && valid_lang("en-US") && valid_lang("de") && valid_lang("sr-Latn-RS"));
        assert!(!valid_lang("../x") && !valid_lang("ja_JP") && !valid_lang("") && !valid_lang("x"));
    }

    #[test]
    fn builtins_parse_as_independent() {
        for (id, _, xml) in BUILTIN {
            let m = parse_meta(xml).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(m.parent.is_none(), "{id}");
            assert!(!m.title.is_empty(), "{id}");
        }
        assert!(LOCALE_EN_US.contains("xml:lang=\"en-US\""));
    }

    #[test]
    fn dependent_meta_and_locale_patch() {
        let m = parse_meta(DEP).unwrap();
        assert_eq!(m.parent.as_deref(), Some("springer-basic-brackets"));
        assert_eq!(m.title_short.as_deref(), Some("Radiologe"));
        assert_eq!(m.format.as_deref(), Some("numeric"));
        assert_eq!(m.default_locale.as_deref(), Some("de-DE"));

        // 既存属性の置換
        let p = r#"<?xml version="1.0"?><style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0" default-locale="en-US"><citation/></style>"#;
        let out = with_default_locale(p, "de-DE");
        assert!(out.contains(r#"default-locale="de-DE""#) && !out.contains("en-US"));
        assert_eq!(parse_meta(&out).unwrap().default_locale.as_deref(), Some("de-DE"));
        // 属性が無い場合の追加
        let q = r#"<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0"><citation/></style>"#;
        assert_eq!(parse_meta(&with_default_locale(q, "ja-JP")).unwrap().default_locale.as_deref(), Some("ja-JP"));
    }

    #[test]
    fn dependent_resolves_to_parent_from_cache() {
        let dir = std::env::temp_dir().join(format!("openref-style-test-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        // 親はキャッシュに置く（ネットワーク不使用）。中身は同梱 Vancouver を流用
        std::fs::write(dir.join("springer-basic-brackets.csl"), builtin_xml("vancouver").unwrap()).unwrap();
        std::fs::write(dir.join("der-radiologe.csl"), DEP).unwrap();
        let xml = style_xml(&dir, "der-radiologe").unwrap();
        let m = parse_meta(&xml).unwrap();
        assert!(m.parent.is_none());
        assert_eq!(m.default_locale.as_deref(), Some("de-DE"));
        let e = install(&dir, "der-radiologe").unwrap();
        assert_eq!(e["title"], "Der Radiologe (Deutsch)");
        assert_eq!(e["parent"], "springer-basic-brackets");
        let ids: Vec<String> = installed(&dir).iter().map(|v| v["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids.len(), 5);
        assert_eq!(ids[4], "der-radiologe");
        // 手元ファイルの読み込み: id は常に custom- 前置（同梱・取得キャッシュを上書きしない）
        let f = dir.join("My Radiology.csl");
        std::fs::write(&f, builtin_xml("radiology").unwrap()).unwrap();
        let e = install_file(&dir, &f).unwrap();
        assert_eq!(e["id"], "custom-radiology");
        assert_eq!(e["custom"], true);
        assert_eq!(style_xml(&dir, "custom-radiology").unwrap(), builtin_xml("radiology").unwrap());
        assert!(installed(&dir).iter().any(|v| v["id"] == "custom-radiology" && v["custom"] == true));
        // リポジトリ id のまま編集した親を読み込んでも、取得キャッシュ（= 誌名スタイルの解決先）は変わらない
        let tweaked = dir.join("springer-basic-brackets-edited.csl");
        let parent_before = std::fs::read_to_string(dir.join("springer-basic-brackets.csl")).unwrap();
        std::fs::write(&tweaked, builtin_xml("american-medical-association").unwrap().replace(
            "http://www.zotero.org/styles/american-medical-association</id>",
            "http://www.zotero.org/styles/springer-basic-brackets</id>",
        ))
        .unwrap();
        let e2 = install_file(&dir, &tweaked).unwrap();
        assert_eq!(e2["id"], "custom-springer-basic-brackets");
        assert_eq!(std::fs::read_to_string(dir.join("springer-basic-brackets.csl")).unwrap(), parent_before);
        assert_eq!(style_xml(&dir, "der-radiologe").unwrap(), xml);
        // 既定に設定（install の再実行）しても「自作」の印が残る
        let e3 = install(&dir, "custom-radiology").unwrap();
        assert_eq!(e3["custom"], true);
        assert!(installed(&dir).iter().any(|v| v["id"] == "custom-radiology" && v["custom"] == true));
        // CSL でないファイルは拒否
        let bad = dir.join("bad.csl");
        std::fs::write(&bad, "<html/>").unwrap();
        assert!(install_file(&dir, &bad).is_err());
        // 不正 id は取得に行かずに拒否
        assert!(matches!(style_xml(&dir, "../x"), Err(CoreError::BadInput(_))));
        assert!(matches!(locale_xml(&dir, "../x"), Err(CoreError::BadInput(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefs_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(prefs(&db).unwrap()["style"], DEFAULT_STYLE);
        assert_eq!(prefs(&db).unwrap()["doi_always"], false);
        // 同梱スタイルはネットワーク不要
        let p = set_prefs(&db, "radiology", "", true).unwrap();
        assert_eq!(p["style"], "radiology");
        assert_eq!(p["doi_always"], true);
        assert_eq!(prefs(&db).unwrap()["doi_always"], true);
        assert_eq!(set_prefs(&db, "radiology", "", false).unwrap()["doi_always"], false);
        assert!(set_prefs(&db, "radiology", "../x", false).is_err());
        // 不正な style id は settings に書く前に拒否（取得にも行かない）
        assert!(matches!(set_prefs(&db, "../x", "", false), Err(CoreError::BadInput(_))));
        assert_eq!(prefs(&db).unwrap()["style"], "radiology");
    }

    /// 部分更新（登録表と GUI の cite_set_prefs が共有）: 渡さない項目は今のまま・全部 None は拒否
    #[test]
    fn partial_prefs_keep_unspecified() {
        let db = Db::open_in_memory().unwrap();
        set_prefs_partial(&db, Some("vancouver"), None, Some(true)).unwrap();
        let p = set_prefs_partial(&db, None, Some("en-US"), None).unwrap();
        assert_eq!(
            (p["style"].as_str(), p["locale"].as_str(), p["doi_always"].as_bool()),
            (Some("vancouver"), Some("en-US"), Some(true)),
            "locale だけ変えても style / doi_always は戻らない"
        );
        assert_eq!(set_prefs_partial(&db, None, None, Some(false)).unwrap()["locale"], "en-US");
        assert_eq!(set_prefs_partial(&db, Some("radiology"), Some(""), None).unwrap()["doi_always"], false);
        assert!(matches!(set_prefs_partial(&db, None, None, None), Err(CoreError::BadInput(_))));
        assert_eq!(prefs(&db).unwrap()["style"], "radiology", "失敗した呼び出しは書かない");
    }

    /// 実弾: 索引取得 + dependent（European Radiology → springer-basic-brackets）+ ja-JP locale
    #[test]
    #[ignore]
    fn live_catalog_and_dependent() {
        let dir = std::env::temp_dir().join(format!("openref-style-live-{}", fastrand::u64(..)));
        let cat = catalog(&dir, false).unwrap();
        assert!(cat.len() > 9000, "{}", cat.len());
        assert!(cat.iter().any(|v| v["id"] == "european-radiology"));
        let xml = style_xml(&dir, "european-radiology").unwrap();
        assert!(parse_meta(&xml).unwrap().parent.is_none());
        assert!(locale_xml(&dir, "ja-JP").unwrap().contains("ja-JP"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
