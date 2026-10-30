//! Drive mount と DB の差分スキャン（報告のみ）。
//! Paperpile との併用期に、Paperpile 側で足された新論文を検出する。

use crate::{Db, Result};
use std::collections::HashSet;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug)]
pub struct SyncReport {
    pub mount_pdfs: usize,
    pub linked: usize,
    pub new_files: Vec<String>,
    pub orphans: Vec<String>,
    /// 列挙が 0 件なのに linked 添付の実体 stat は通る = OS が readdir を拒否している
    /// （macOS TCC / File Provider）。この状態では新規検出は機能しない。
    pub enumeration_blocked: bool,
}

pub fn scan(db: &Db, mount: &Path) -> Result<SyncReport> {
    // mount 側: 全 PDF（NFC 正規化した絶対パス → 表示用相対パス）
    let mut mount_files: Vec<(String, String)> = Vec::new();
    let file_entry = |p: &Path| -> (String, String) {
        (p.to_string_lossy().nfc().collect(), p.strip_prefix(mount).unwrap_or(p).to_string_lossy().to_string())
    };
    for entry in walkdir::WalkDir::new(mount).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .map_or(false, |e| e.eq_ignore_ascii_case("pdf"))
        {
            mount_files.push(file_entry(entry.path()));
        }
    }

    // File Provider（Drive for Desktop）配下は環境によって readdir 列挙が空になることがある。
    // その場合は find(1) に fallback（直接 stat は常に通るため existence 判定には影響しない）。
    if mount_files.is_empty() {
        for line in find_pdfs(mount) {
            mount_files.push(file_entry(Path::new(&line)));
        }
        if !mount_files.is_empty() {
            eprintln!("(note) ディレクトリ列挙が空だったため find(1) fallback を使用");
        }
    }

    // DB 側: linked 添付
    let mut stmt = db
        .conn
        .prepare("SELECT path FROM attachments WHERE mode = 'linked'")?;
    let linked: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let linked_nfc: HashSet<String> = linked.iter().map(|p| p.nfc().collect()).collect();

    let mut new_files: Vec<String> = mount_files
        .iter()
        .filter(|(abs, _)| !linked_nfc.contains(abs))
        .map(|(_, rel)| rel.clone())
        .collect();
    new_files.sort();

    let orphans: Vec<String> = linked
        .iter()
        .filter(|p| !Path::new(p.as_str()).exists())
        .cloned()
        .collect();

    // 列挙 0 件でも mount 配下の linked 実体が stat できる場合は「列挙ブロック」と判定
    let enumeration_blocked = mount_files.is_empty()
        && linked
            .iter()
            .any(|p| p.starts_with(&*mount.to_string_lossy()) && Path::new(p.as_str()).exists());
    let new_files = if enumeration_blocked { Vec::new() } else { new_files };

    Ok(SyncReport {
        mount_pdfs: mount_files.len(),
        linked: linked.len(),
        new_files,
        orphans,
        enumeration_blocked,
    })
}

/// find(1) で mount 配下の PDF を列挙する（拡張子は walkdir 側と同じく大小無視 = `.PDF` も拾う）。find が失敗すれば空
fn find_pdfs(mount: &Path) -> Vec<String> {
    match std::process::Command::new("/usr/bin/find").arg(mount).args(["-type", "f", "-iname", "*.pdf"]).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_fallback_ignores_extension_case() {
        let dir = std::env::temp_dir().join(format!("openref-sync-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("All Papers/K")).unwrap();
        for f in ["All Papers/K/a.pdf", "All Papers/K/B.PDF", "All Papers/c.Pdf", "All Papers/note.txt", "All Papers/pdf"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let mut got: Vec<String> = find_pdfs(&dir)
            .iter()
            .map(|p| Path::new(p).strip_prefix(&dir).unwrap().to_string_lossy().to_string())
            .collect();
        got.sort();
        assert_eq!(got, ["All Papers/K/B.PDF", "All Papers/K/a.pdf", "All Papers/c.Pdf"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
