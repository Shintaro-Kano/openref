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
pub fn place_bytes(mount: &Path, csl: &serde_json::Value, bytes: &[u8]) -> Result<PathBuf> {
    let (initial, filename) = relative_target(csl);
    let dir = mount.join("All Papers").join(&initial);
    std::fs::create_dir_all(&dir)?;
    let dest = unique_path(&dir, &filename);
    // File Provider 配下でも同一ディレクトリ内 rename は原子的に通る
    let tmp = dir.join(format!(
        ".{}.openreftmp",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        CoreError::Io(e)
    })?;
    Ok(dest)
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

fn unique_path(dir: &Path, filename: &str) -> PathBuf {
    let cand = dir.join(filename);
    if !cand.exists() {
        return cand;
    }
    let stem = filename.strip_suffix(".pdf").unwrap_or(filename);
    for n in 2.. {
        let cand = dir.join(format!("{stem} ({n}).pdf"));
        if !cand.exists() {
            return cand;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
