//! Drive 互換レイアウトへの PDF 書込。
//! `All Papers/<頭文字>/<著者部> <年> - <題名>.pdf` — Paperpile が書いていた実レイアウトを
//! 蔵書の実測から導出した規約で踏襲する（下流の知識ベースの `--sync-scan` が前提とするレイアウトの維持）:
//! - 著者部: 単著 = `家名` / 二著者 = `A and B` / 3 名以上 = `A et al.`
//! - 題名の `: ` は ` - ` に置換（例: "Meningioma: Molecular…" → "Meningioma - Molecular…"）
//! - ファイル名は 150 字上限・超過分は中間を ` ... ` で省略
//! - 頭文字フォルダ: 著者部先頭 1 字を大文字化（CJK はそのまま。例: `中`）

use crate::{model, CoreError, Result};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const MAX_FILENAME_CHARS: usize = 150;

/// CSL から `<頭文字>/<著者部> <年> - <題名>.pdf` の相対パスを組む（`All Papers` 起点）。
pub fn relative_target(csl: &serde_json::Value) -> (String, String) {
    let name_part = author_part(csl);
    let year = model::year(csl)
        .map(|y| y.to_string())
        .unwrap_or_else(|| "n.d.".into());
    let title = sanitize_component(model::s(csl, "title").unwrap_or("Untitled"));
    let stem = format!("{} {} - {}", name_part, year, title);
    let filename = truncate_middle(&stem, MAX_FILENAME_CHARS - 4) + ".pdf";
    (initial_dir(&name_part), filename)
}

/// bytes を Drive mount の互換レイアウトへ原子的に書き、最終絶対パスを返す。
/// 既存ファイルと衝突したら ` (2)` ` (3)` … を付番する。
/// 同じ名前へ同時に書く呼び手が何本あっても互いを壊さない: 一時ファイルは書き手ごとの別名（create_new）で、
/// 最終名へは「無ければ置く」の rename（[`rename_noreplace`]）で置く = 先を越されたら次の付番へ。
pub fn place_bytes(mount: &Path, csl: &serde_json::Value, bytes: &[u8]) -> Result<PathBuf> {
    use std::io::Write;
    let (initial, filename) = relative_target(csl);
    let dir = mount.join("All Papers").join(&initial);
    std::fs::create_dir_all(&dir)?;
    let (tmp, mut file) = loop {
        let tmp = dir.join(format!(".openref-{:016x}.openreftmp", fastrand::u64(..)));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    let written = file.write_all(bytes);
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    let stem = filename.strip_suffix(".pdf").unwrap_or(&filename);
    for n in 1u32.. {
        let dest = if n == 1 { dir.join(&filename) } else { dir.join(format!("{stem} ({n}).pdf")) };
        if dest.exists() {
            continue;
        }
        // File Provider 配下でも同一ディレクトリ内 rename は原子的に通る
        match rename_noreplace(&tmp, &dest) {
            Ok(()) => return Ok(dest),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(CoreError::Io(e));
            }
        }
    }
    unreachable!()
}

/// `from` を `to` へ rename する。`to` が既にあれば上書きせず `AlreadyExists`。
/// macOS は `renamex_np(RENAME_EXCL)`（原子的）。EEXIST 以外の失敗（Drive の File Provider が RENAME_EXCL を扱えない等。
/// 実機の Drive で何が返るかは未確認）は「無ければ rename」（従来の動き = 確認と rename の間は原子的でない）に落とす
fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    rename_noreplace_with(from, to, rename_excl)
}

/// [`rename_noreplace`] の本体。`excl` = 原子的な「無ければ rename」（試験で失敗を差し込む）
fn rename_noreplace_with(
    from: &Path,
    to: &Path,
    excl: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match excl(from, to) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(e),
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {}
        Err(e) => {
            // 実機の Drive が返す errno を見るため、プロセスで 1 回だけ記録する
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "openref: renamex_np(RENAME_EXCL) が失敗したので確認 + rename に切り替えます ({}): {e} (errno {:?})",
                    to.display(),
                    e.raw_os_error()
                );
            }
        }
    }
    if to.exists() {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(from, to)
}

/// 原子的な「無ければ rename」。macOS 以外は Unsupported（黙って従来の動きへ）
fn rename_excl(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let f = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let t = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: どちらも NUL 終端の C 文字列で、呼び出しの間生きている
        if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
            return Ok(());
        }
        Err(std::io::Error::last_os_error())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to);
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

fn author_part(csl: &serde_json::Value) -> String {
    let families: Vec<String> = csl
        .get("author")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    a.get("family")
                        .or_else(|| a.get("literal"))
                        .and_then(|v| v.as_str())
                        .map(|s| sanitize_component(s))
                })
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    match families.len() {
        0 => "Unknown".into(),
        1 => families[0].clone(),
        2 => format!("{} and {}", families[0], families[1]),
        _ => format!("{} et al.", families[0]),
    }
}

fn initial_dir(name_part: &str) -> String {
    name_part
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "U".into())
}

/// ファイル名成分の無害化: `: ` → ` - `、パス区切り・禁止文字を除去、空白を畳む。NFC 正規化。
fn sanitize_component(s: &str) -> String {
    let s: String = s.nfc().collect();
    let s = s.replace(": ", " - ").replace(':', " - ");
    let s: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' => '-',
            '<' | '>' | '|' | '"' | '*' | '?' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' {
            if !prev_space {
                out.push(c);
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().trim_end_matches('.').trim().to_string()
}

/// 中間省略の truncation（Paperpile 実挙動と同型: 頭と尾を残し ` ... ` を挿む）。
fn truncate_middle(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let keep = max_chars - 5; // " ... "
    let head = keep / 2 + keep % 2;
    let tail = keep / 2;
    let mut out: String = chars[..head].iter().collect();
    out.push_str(" ... ");
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// renamex_np が EEXIST 以外（EPERM / EACCES / EXDEV / EIO 等）で失敗 → 「無ければ rename」に落ちて置ける。
    /// EEXIST はそのまま AlreadyExists（次の付番へ）・落ちた先が既存の名前なら AlreadyExists（上書きしない）
    #[test]
    fn rename_noreplace_falls_back_unless_eexist() {
        let dir = std::env::temp_dir().join(format!("openref-excl-{}-{}", std::process::id(), crate::keys::new_key()));
        std::fs::create_dir_all(&dir).unwrap();
        let (from, to) = (dir.join("a.tmp"), dir.join("a.pdf"));
        for errno in [libc::EPERM, libc::EACCES, libc::EXDEV, libc::EIO, libc::ENOTSUP, libc::EINVAL] {
            std::fs::write(&from, b"x").unwrap();
            std::fs::remove_file(&to).ok();
            rename_noreplace_with(&from, &to, |_, _| Err(std::io::Error::from_raw_os_error(errno))).unwrap();
            assert_eq!(std::fs::read(&to).unwrap(), b"x", "errno {errno}");
            assert!(!from.exists());
            // 落ちた先でも既存の名前は上書きしない
            std::fs::write(&from, b"y").unwrap();
            let e = rename_noreplace_with(&from, &to, |_, _| Err(std::io::Error::from_raw_os_error(errno))).unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
            assert_eq!(std::fs::read(&to).unwrap(), b"x");
        }
        // EEXIST は落とさない（rename しない）
        std::fs::remove_file(&to).ok();
        let e = rename_noreplace_with(&from, &to, |_, _| Err(std::io::Error::from_raw_os_error(libc::EEXIST))).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(from.exists() && !to.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 同じ名前へ同時に書く 8 本: 8 つの別名（原名 + (2)〜(8)）に各自の中身が入り、一時ファイルが残らない
    /// （従来は確認と rename の間が原子的でなく、一時ファイル名も共有 = 上書き・欠落が起きた）
    #[test]
    fn place_bytes_concurrent_same_name_never_clobbers() {
        let mount = std::env::temp_dir().join(format!("openref-place-{}-{}", std::process::id(), crate::keys::new_key()));
        let csl = json!({"title": "Race", "author": [{"family": "Kano"}], "issued": {"date-parts": [[2026]]}});
        for _round in 0..5 {
            std::fs::remove_dir_all(&mount).ok();
            let bar = std::sync::Arc::new(std::sync::Barrier::new(8));
            let hs: Vec<_> = (0..8u8)
                .map(|i| {
                    let (mount, csl, bar) = (mount.clone(), csl.clone(), bar.clone());
                    std::thread::spawn(move || {
                        let bytes = vec![i; 200_000];
                        bar.wait();
                        (place_bytes(&mount, &csl, &bytes).unwrap(), bytes)
                    })
                })
                .collect();
            let placed: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
            let mut names: Vec<_> = placed.iter().map(|(p, _)| p.clone()).collect();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), 8);
            for (p, bytes) in &placed {
                assert_eq!(&std::fs::read(p).unwrap(), bytes);
            }
            let dir = mount.join("All Papers/K");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 8, "一時ファイルが残っていない");
            assert!(dir.join("Kano 2026 - Race.pdf").exists() && dir.join("Kano 2026 - Race (8).pdf").exists());
        }
        std::fs::remove_dir_all(&mount).ok();
    }

    #[test]
    fn filename_rules_from_observed_layout() {
        // 3 名以上 = et al.・`: ` → ` - `
        let (dir, f) = relative_target(&json!({
            "title": "Meningioma: Molecular updates and imaging correlates",
            "author": [{"family":"Soni"},{"family":"A"},{"family":"B"}],
            "issued": {"date-parts": [[2025]]}
        }));
        assert_eq!(dir, "S");
        assert_eq!(f, "Soni et al. 2025 - Meningioma - Molecular updates and imaging correlates.pdf");

        // 二著者 = A and B
        let (_, f) = relative_target(&json!({
            "title": "Chest CT imaging",
            "author": [{"family":"Tanabe"},{"family":"Hirai"}],
            "issued": {"date-parts": [[2026]]}
        }));
        assert_eq!(f, "Tanabe and Hirai 2026 - Chest CT imaging.pdf");

        // 単著・CJK（頭文字フォルダは 1 字そのまま）
        let (dir, f) = relative_target(&json!({
            "title": "小児脳腫瘍 update―診断から治療まで―",
            "author": [{"family":"原田"}],
            "issued": {"date-parts": [[2026]]}
        }));
        assert_eq!(dir, "原");
        assert_eq!(f, "原田 2026 - 小児脳腫瘍 update―診断から治療まで―.pdf");

        // 小文字前置詞は大文字化して頭文字に（de Toledo-Mendes → D）
        let (dir, _) = relative_target(&json!({
            "title": "Mapping nodal metastasis",
            "author": [{"family":"de Toledo-Mendes"},{"family":"X"},{"family":"Y"}],
            "issued": {"date-parts": [[2025]]}
        }));
        assert_eq!(dir, "D");

        // 著者なし・年なし
        let (dir, f) = relative_target(&json!({"title": "Some report"}));
        assert_eq!(dir, "U");
        assert_eq!(f, "Unknown n.d. - Some report.pdf");
    }

    #[test]
    fn long_title_truncates_middle_at_150() {
        let long = "X".repeat(300);
        let (_, f) = relative_target(&json!({
            "title": long,
            "author": [{"family":"Mou"},{"family":"A"},{"family":"B"}],
            "issued": {"date-parts": [[2025]]}
        }));
        assert_eq!(f.chars().count(), 150);
        assert!(f.contains(" ... "));
        assert!(f.ends_with(".pdf"));
        assert!(f.starts_with("Mou et al. 2025 - "));
    }

    #[test]
    fn sanitize_slashes_and_quotes() {
        let (_, f) = relative_target(&json!({
            "title": "PI-RADS/LI-RADS \"v2\" <update>?",
            "author": [{"family":"Kano"}],
            "issued": {"date-parts": [[2024]]}
        }));
        assert_eq!(f, "Kano 2024 - PI-RADS-LI-RADS v2 update.pdf");
    }
}
