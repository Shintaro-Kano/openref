//! Paperpile の Google Drive mount 検出（Drive for Desktop 前提）。

use std::path::PathBuf;

/// `~/Library/CloudStorage/GoogleDrive-*/{マイドライブ,My Drive}/Paperpile` を探す。
/// `OPENREF_MOUNT` が設定されていればそれを優先（検証・将来の配置替え用）。
pub fn detect() -> Option<PathBuf> {
    if let Some(p) = crate::env_os("MOUNT").map(PathBuf::from) {
        return p.is_dir().then_some(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let cs = home.join("Library/CloudStorage");
    for entry in std::fs::read_dir(&cs).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("GoogleDrive-") {
            continue;
        }
        for dd in ["マイドライブ", "My Drive"] {
            let cand = entry.path().join(dd).join("Paperpile");
            if cand.is_dir() {
                return Some(cand);
            }
        }
    }
    None
}
