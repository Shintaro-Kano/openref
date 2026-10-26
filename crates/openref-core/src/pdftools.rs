//! PDF テキスト抽出の統一入口（配布物の外部依存ゼロ）。
//!
//! 第 1 候補 = 内蔵 pure-Rust（pdf-extract）。失敗 or ほぼ空（スキャン PDF）or **文字化け**（フォントを復号できず
//! U+FFFD が混じる。一部の出版社の PDF では本文の 30〜70% が化け、著者名検索が当たらなくなる）
//! のときだけ pdftotext（poppler）があれば fallback する。poppler が無ければ化けたまま返し backend を
//! "builtin-garbled" にする（呼び手は status='partial' で記録し、poppler が入ったら自動で取り直す）。pdftotext の所在は候補探索で解決する:
//! GUI アプリ / connectd は launchd の最小 PATH（/usr/bin:/bin:…）で起動するため Homebrew の
//! /opt/homebrew/bin が見えず、素の `Command::new("pdftotext")` は .app 内で必ず失敗していた。

use crate::{CoreError, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Homebrew（Apple Silicon / Intel）・MacPorts の既定置き場
const CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/pdftotext",
    "/usr/local/bin/pdftotext",
    "/opt/local/bin/pdftotext",
];

pub struct Extracted {
    /// 1 要素 = 1 ページ（要求範囲内）
    pub pages: Vec<String>,
    /// "builtin" | "pdftotext" | "builtin-garbled"（内蔵が文字化けしたが pdftotext が無く、化けたまま）
    pub backend: &'static str,
    /// PDF Info 辞書の /Title（同じ解析から取る = 取込の題名推定・要確認 stub の題名に使う。無ければ None）。
    /// 内蔵解析が失敗して pdftotext だけで取れたときも None
    pub info_title: Option<String>,
}

/// 1-based・両端含む。`to` は総頁数で丸める。ファイルは 1 度だけ読む（解析はバイト列から）
pub fn extract_pages(pdf: &Path, from: usize, to: usize) -> Result<Extracted> {
    let bytes = read_file(pdf)?;
    extract(&bytes, Some(pdf), Some((from, to)))
}

/// バイト列版（取込・添付・照合の経路: 手元のバイト列を解析し、ファイルは読み直さない）。
/// `file` = 同じ内容が置いてあるパス（pdftotext fallback に渡す）。None なら fallback が要るときだけ一時ファイルに落とす
pub fn extract_pages_bytes(bytes: &[u8], file: Option<&Path>, from: usize, to: usize) -> Result<Extracted> {
    extract(bytes, file, Some((from, to)))
}

pub fn extract_all_bytes(bytes: &[u8], file: Option<&Path>) -> Result<Extracted> {
    extract(bytes, file, None)
}

fn read_file(pdf: &Path) -> Result<Vec<u8>> {
    if !pdf.exists() {
        return Err(CoreError::NotFound(format!("file {}", pdf.display())));
    }
    Ok(std::fs::read(pdf)?)
}

fn extract(bytes: &[u8], file: Option<&Path>, range: Option<(usize, usize)>) -> Result<Extracted> {
    let (builtin, info_title) = builtin_pages(bytes, range);
    if let Ok(pages) = &builtin {
        if has_text(pages) && !looks_garbled(pages) {
            return Ok(Extracted { pages: builtin.unwrap(), backend: "builtin", info_title });
        }
    }
    // 内蔵で取れない（エラー / 実質空 / 文字化け）→ poppler があれば頼る。pdftotext も失敗したら内蔵の結果
    // （内蔵もエラーなら pdftotext のエラー）
    if let Some(bin) = find_pdftotext() {
        match pdftotext_pages(&bin, bytes, file, range) {
            Ok(pages) => return Ok(Extracted { pages, backend: "pdftotext", info_title }),
            Err(e) if builtin.is_err() => return Err(e),
            Err(_) => {}
        }
    }
    builtin.map(|pages| {
        let backend = if looks_garbled(&pages) { "builtin-garbled" } else { "builtin" };
        Extracted { pages, backend, info_title }
    })
}

/// 非空白文字に占める U+FFFD（復号できなかったグリフ）の割合
pub fn garbled_ratio(pages: &[String]) -> f64 {
    let (bad, total) = pages
        .iter()
        .flat_map(|p| p.chars())
        .filter(|c| !c.is_whitespace())
        .fold((0usize, 0usize), |(b, t), c| (b + usize::from(c == '\u{FFFD}'), t + 1));
    if total == 0 {
        0.0
    } else {
        bad as f64 / total as f64
    }
}

/// 内蔵抽出の文字化け判定: 文書全体で非空白の 1% 超、または 200 字以上あるページのどれかで 5% 超が U+FFFD。
/// 正常な PDF では U+FFFD は 0 なので、閾値は保守的でよい
pub fn looks_garbled(pages: &[String]) -> bool {
    if garbled_ratio(pages) > 0.01 {
        return true;
    }
    pages.iter().any(|p| {
        p.chars().filter(|c| !c.is_whitespace()).count() >= 200 && garbled_ratio(std::slice::from_ref(p)) > 0.05
    })
}

/// 非空白が 20 文字以上あれば「テキスト層あり」とみなす
pub fn has_text(pages: &[String]) -> bool {
    pages
        .iter()
        .map(|p| p.chars().filter(|c| !c.is_whitespace()).count())
        .sum::<usize>()
        >= 20
}

/// 内蔵抽出（解析 1 回）。文書を読めたら頁抽出の成否に関わらず Info /Title も返す
fn builtin_pages(bytes: &[u8], range: Option<(usize, usize)>) -> (Result<Vec<String>>, Option<String>) {
    use pdf_extract::{output_doc_page, Document, PlainTextOutput};
    // pdf-extract は想定外のフォント/構造で panic し得るので、1 件の失敗で一括取込を落とさない
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> (std::result::Result<Vec<String>, String>, Option<String>) {
            let doc = match Document::load_mem(bytes) {
                Ok(d) => d,
                Err(e) => return (Err(e.to_string()), None),
            };
            let title = info_title_of(&doc);
            let n = doc.get_pages().len();
            let (from, to) = match range {
                Some((f, t)) => (f.max(1), t.min(n)),
                None => (1, n),
            };
            let mut pages = Vec::new();
            for p in from..=to {
                let mut s = String::new();
                {
                    let mut out = PlainTextOutput::new(&mut s);
                    if let Err(e) = output_doc_page(&doc, &mut out, p as u32) {
                        return (Err(e.to_string()), title);
                    }
                }
                pages.push(s);
            }
            (Ok(pages), title)
        },
    ));
    match res {
        Ok((Ok(p), t)) => (Ok(p), t),
        Ok((Err(e), t)) => (Err(CoreError::Parse(format!("内蔵 PDF 抽出に失敗: {e}"))), t),
        Err(_) => (Err(CoreError::Parse("内蔵 PDF 抽出が panic しました（想定外のフォント/構造）".into())), None),
    }
}

/// `bin` = `find_pdftotext` で見つけた pdftotext。渡すファイル: `file` があればそれ、無ければバイト列を一時ファイルに
/// 落とす（Drop で消える）
fn pdftotext_pages(bin: &Path, bytes: &[u8], file: Option<&Path>, range: Option<(usize, usize)>) -> Result<Vec<String>> {
    let mut cmd = Command::new(bin);
    if let Some((f, t)) = range {
        cmd.args(["-f", &f.to_string(), "-l", &t.to_string()]);
    }
    let tmp;
    let pdf: &Path = match file {
        Some(p) if p.exists() => p,
        _ => {
            tmp = temp_pdf(bytes)?;
            &tmp.0
        }
    };
    let out = cmd
        .arg(pdf)
        .arg("-")
        .output()
        .map_err(|e| CoreError::Http(format!("pdftotext 実行失敗: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let hint = if stderr.trim().is_empty() {
            "（stderr 空 = ファイル読取権限の可能性。macOS『プライバシーとセキュリティ > ファイルとフォルダ』を確認）"
        } else {
            ""
        };
        return Err(CoreError::Parse(format!(
            "pdftotext エラー (exit {:?}): {stderr}{hint}",
            out.status.code()
        )));
    }
    // pdftotext は各頁の後に \f を出す（最終頁の後にも）。末尾を落としてから頁に分ける
    let text = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches('\u{c}')
        .to_string();
    Ok(text.split('\u{c}').map(|s| s.to_string()).collect())
}

/// 一時 PDF（Drop で必ず消す。`fs::write` の途中失敗・panic でも残さない）。手元のバイト列を pdftotext（poppler）に
/// 渡すときだけ使う（内蔵解析はバイト列から行うので通常は作らない）。
struct TempPdf(PathBuf);
impl Drop for TempPdf {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// バイト列を pdftotext 用の一時ファイルに落とす（Drop で消える）
fn temp_pdf(bytes: &[u8]) -> Result<TempPdf> {
    let tmp = TempPdf(std::env::temp_dir().join(format!(
        "openref-pdftotext-{}-{}.pdf",
        std::process::id(),
        fastrand::u64(..)
    )));
    std::fs::write(&tmp.0, bytes)?;
    Ok(tmp)
}

/// PDF Info 辞書の /Title（出版社 PDF は論文題名が入っていることが多い）。
/// UTF-16BE（BOM 付き）/ PDFDocEncoding（ASCII 域）を復号。無ければ None。
/// 取込経路は `Extracted::info_title`（同じ解析から取る）を使い、これは単独で題名だけ要るとき（1 回の解析）
pub fn info_title_bytes(bytes: &[u8]) -> Option<String> {
    use pdf_extract::Document;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = Document::load_mem(bytes).ok()?;
        info_title_of(&doc)
    }))
    .ok()
    .flatten()
}

/// 解析済み文書から /Title を復号（panic は呼び手の catch_unwind が受ける）
fn info_title_of(doc: &pdf_extract::Document) -> Option<String> {
    use pdf_extract::Object;
    let info = match doc.trailer.get(b"Info").ok()? {
        Object::Reference(id) => doc.get_object(*id).ok()?.as_dict().ok()?,
        Object::Dictionary(d) => d,
        _ => return None,
    };
    let bytes: Vec<u8> = match info.get(b"Title").ok()? {
        Object::String(b, _) => b.clone(),
        Object::Reference(id) => doc.get_object(*id).ok()?.as_str().ok()?.to_vec(),
        _ => return None,
    };
    let s = decode_pdf_string(&bytes).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn decode_pdf_string(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let u: Vec<u16> = b[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        // PDFDocEncoding は ASCII 域で Latin-1 と一致（題名用途ではこれで十分）
        b.iter().map(|&c| c as char).collect()
    }
}

// ---------- pdftotext（fallback）の所在解決 ----------

pub fn find_pdftotext() -> Option<PathBuf> {
    let over = crate::env_os("PDFTOTEXT");
    // `OPENREF_PDFTOTEXT=off` = fallback を完全無効化（poppler の無い配布先 Mac の挙動を再現する検証用）
    if over.as_deref().map(|s| s == "off").unwrap_or(false) {
        return None;
    }
    find_in(
        over.map(PathBuf::from),
        std::env::var_os("PATH"),
        &CANDIDATES.iter().map(PathBuf::from).collect::<Vec<_>>(),
    )
}

/// 所在解決の本体（テスト可能なよう環境を引数で受ける）。存在するファイルのみ採用。
fn find_in(env_override: Option<PathBuf>, path_var: Option<OsString>, candidates: &[PathBuf]) -> Option<PathBuf> {
    if let Some(p) = env_override {
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(path) = path_var {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("pdftotext");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    candidates.iter().find(|p| p.is_file()).cloned()
}

/// 最小の 1 頁 PDF（Helvetica・/Info /Title 付き）を正しい xref 付きで組み立てる（テスト用。integration テストからも使う）
#[doc(hidden)]
pub fn minimal_pdf(text: &str, title: Option<&str>) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
    ];
    let content = format!("BT /F1 12 Tf 72 720 Td ({}) Tj ET", text.replace('(', "\\(").replace(')', "\\)"));
    objs.push(format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), content));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    if let Some(t) = title {
        objs.push(format!("<< /Title ({t}) >>"));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{}\nendobj\n", i + 1, o));
    }
    let xref_pos = out.len();
    out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1));
    for off in &offsets {
        out.push_str(&format!("{off:010} 00000 n \n"));
    }
    let info = if title.is_some() { format!(" /Info {} 0 R", objs.len()) } else { String::new() };
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R{info} >>\nstartxref\n{xref_pos}\n%%EOF\n",
        objs.len() + 1
    ));
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("openref-pdftools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        p
    }

    #[test]
    fn resolution_order() {
        let over = touch("override-pdftotext");
        let dir = over.parent().unwrap().to_path_buf();
        let on_path = touch("pdftotext");
        let cand = touch("cand-pdftotext");
        let path_var = Some(std::env::join_paths([&dir]).unwrap());
        assert_eq!(find_in(Some(over.clone()), path_var.clone(), &[cand.clone()]), Some(over.clone()));
        assert_eq!(find_in(Some(dir.join("nope")), path_var.clone(), &[cand.clone()]), Some(on_path.clone()));
        assert_eq!(find_in(None, Some(OsString::from("/nonexistent-dir")), &[cand.clone()]), Some(cand.clone()));
        assert_eq!(find_in(None, Some(OsString::from("/nonexistent-dir")), &[dir.join("missing")]), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn garbled_detection() {
        let clean = vec!["Chest CT imaging toward personalized management ".repeat(10)];
        assert!(!looks_garbled(&clean));
        assert_eq!(garbled_ratio(&clean), 0.0);
        // 実例（att 13）: 101k 字中 61k 字が U+FFFD
        let bad = vec![format!("{} {}", "\u{FFFD}".repeat(600), "a".repeat(400))];
        assert!(looks_garbled(&bad));
        assert!(garbled_ratio(&bad) > 0.5);
        // 1 ページだけ化けている（文書全体では 1% 未満でもページ単位で 5% 超）
        let one_page = vec!["x".repeat(100_000), format!("{}{}", "\u{FFFD}".repeat(20), "y".repeat(200))];
        assert!(looks_garbled(&one_page));
        // 短いページの数文字はノイズ扱い
        let noise = vec!["ok text ".repeat(50), "\u{FFFD}\u{FFFD} short".into()];
        assert!(!looks_garbled(&noise));
    }

    /// file=None で fallback に落ちる経路: 一時ファイルを作って pdftotext に渡し、Drop で消す。
    /// 一時ファイルは**自分が作ったものを名前で**確かめる（temp_dir の同接頭辞を数えると、同じテストバイナリで
    /// 並行に走る別テストの fallback（pdffetch の check_pdf_identity 等）と競合して不安定になる）
    #[test]
    fn bytes_fallback_without_file_uses_temp_and_cleans_up() {
        let bytes = minimal_pdf("", None);
        let tmp = temp_pdf(&bytes).unwrap();
        let path = tmp.0.clone();
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("openref-pdftotext-"), "{path:?}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "バイト列をそのまま書く");
        drop(tmp);
        assert!(!path.exists(), "Drop で消える: {path:?}");
        // テキスト層の無い PDF = 内蔵は「実質空」→ poppler があれば pdftotext（file=None なので上の一時ファイル経路）。
        // 無い環境では内蔵のまま
        let ex = extract_all_bytes(&bytes, None).unwrap();
        assert_eq!(ex.pages.len(), 1);
        assert_eq!(ex.backend, if find_pdftotext().is_some() { "pdftotext" } else { "builtin" });
    }

    #[test]
    fn builtin_extracts_text_and_info_title() {
        let dir = std::env::temp_dir().join(format!("openref-pdftools-x-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.pdf");
        std::fs::write(&p, minimal_pdf("Hello DOI 10.1148/rg.240011 here", Some("A Test Title"))).unwrap();
        let ex = extract_pages(&p, 1, 2).unwrap();
        assert_eq!(ex.pages.len(), 1, "総頁数で丸める");
        assert!(ex.pages[0].contains("10.1148/rg.240011"), "{:?}", ex.pages);
        assert_eq!(ex.info_title.as_deref(), Some("A Test Title"), "同じ解析から /Title も取る");
        // バイト列版はファイルを読まない（ファイル無しでも同じ結果）
        let bytes = std::fs::read(&p).unwrap();
        let exb = extract_pages_bytes(&bytes, None, 1, 2).unwrap();
        assert_eq!((exb.pages, exb.info_title.as_deref()), (ex.pages.clone(), Some("A Test Title")));
        assert_eq!(info_title_bytes(&bytes).as_deref(), Some("A Test Title"));
        assert_eq!(extract_all_bytes(&bytes, None).unwrap().pages.len(), 1);
        // /Title 無し → None
        let plain = minimal_pdf("no title here", None);
        assert_eq!(extract_all_bytes(&plain, None).unwrap().info_title, None);
        // UTF-16BE(BOM) の /Title も復号できる
        let utf16: Vec<u8> = [0xFE, 0xFF].into_iter().chain("Ünïcode".encode_utf16().flat_map(|u| u.to_be_bytes())).collect();
        assert_eq!(decode_pdf_string(&utf16), "Ünïcode");
        std::fs::remove_dir_all(&dir).ok();
    }
}
