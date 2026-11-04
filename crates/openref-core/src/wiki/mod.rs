//! OpenNote の core 側: vault の root の検査・索引（index）・走査（scan）・解析
//! （frontmatter / parse_md / parse_html / links）・登録表の読みの command（commands）・最近のワークスペースの設定（workspaces）。
//! ここの関数は Db を要しない（設定を読む間だけ Db を握り、走査・検査・索引はロックの外で行う）。

pub mod commands;
pub mod edit;
pub mod frontmatter;
pub mod index;
pub mod links;
pub mod parse_html;
pub mod parse_md;
pub mod paths;
pub mod scan;
pub mod workspaces;

pub use commands::Ctx;

/// `CoreError::Wiki` の code の一覧（GUI の IPC が `{"code", "message"}` で画面へ渡す = 画面はこれで分岐する。文言は変わりうる）
pub mod codes {
    /// 使える状態にない（無効・in-memory の DB・フォルダ未設定・HOME 未設定）
    pub const UNAVAILABLE: &str = "unavailable";
    /// 索引の作り直し中（しばらくして呼び直す）
    pub const REBUILDING: &str = "rebuilding";
    /// wiki_search の語に英数字も和文も無く、tag も無い
    pub const BAD_QUERY: &str = "bad_query";
    /// ワークスペースのファイル操作の失敗（読めない区画・ゴミ箱へ移せない）
    pub const FS: &str = "fs";
    /// 引数 root が最近のワークスペースの履歴（設定 wiki_workspaces の鍵）に無い（鍵と文字列で等しい root だけ受ける）
    pub const UNKNOWN_ROOT: &str = "unknown_root";
}
pub use index::{with_index, Fresh, Index};

use crate::{CoreError, Result};
use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// Google Drive の CloudStorage の直下にある「ドライブの root」の名前（この下はさらに 1 段深く要求する）
const DRIVE_ROOTS: &[&str] = &["My Drive", "Shared drives", "Other computers", "マイドライブ", "共有ドライブ", "パソコン", "他のパソコン"];

/// vault の root として使えるかを検査し、canonicalize した絶対パスを返す（asset protocol の scope はこの値で許す。
/// Tauri の scope は要求側のパスを canonicalize して照合するので、登録側も canonical でないと symlink 越しに外れる）。
///
/// 拒否するもの（機微な場所や広すぎる場所を配信しない）:
/// - 絶対パスでない・実在しない・ディレクトリでない
/// - どこかの区画が `.` 始まり（`~/.ssh` 等）
/// - `/`、およびホームそのものとその上位（`/Users` 等 = ホームを丸ごと配信することになる）
/// - `app_dir`（DB・storage/・certs/ の置き場）とその配下、および `app_dir` を含む上位（DB を配信することになる）
/// - `~/Library` とその配下。例外は `~/Library/Mobile Documents/<x>/<y…>`（iCloud の中のフォルダ）と
///   `~/Library/CloudStorage/<provider>/<y…>`（provider 直下が Google Drive の root = `My Drive` / `マイドライブ` 等なら、もう 1 段下）
/// - ホームの外は `/Volumes/<vol>/<sub…>`（外付けの中のフォルダ）だけ。`/System` `/usr` `/private` `/Volumes` `/Volumes/<vol>` 等は拒否
///
/// `home` と `app_dir` は呼び手が渡す（テストは tempdir を渡す）。両者も canonicalize してから比べる（読めなければ与えられたまま）。
/// app_dir が None でも、既定の置き場 `~/Library/Application Support/OpenRef` は `~/Library` の規則で守られる
pub fn check_root(root: &Path, home: &Path, app_dir: Option<&Path>) -> Result<PathBuf> {
    let bad = |why: &str| Err(CoreError::BadInput(format!("ワークスペースのフォルダに {} は使えません（{why}）", root.display())));
    if !root.is_absolute() {
        return bad("絶対パスではありません");
    }
    let canon = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(_) => return bad("見つかりません"),
    };
    if !canon.is_dir() {
        return bad("フォルダではありません");
    }
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    match allowed(&canon, &norm(home), app_dir.map(norm).as_deref()) {
        Ok(()) => Ok(canon),
        Err(why) => bad(why),
    }
}

/// p の root からの相対（`/` 区切り・NFC）。両側を NFC にして比べる（root が NFD で実在し、渡すパスが NFC のことがある）。
/// root そのものは ""・root の外は None
pub fn strip_root(root: &Path, p: &Path) -> Option<String> {
    let r: String = root.to_string_lossy().nfc().collect();
    let p: String = p.to_string_lossy().nfc().collect();
    let r = r.trim_end_matches('/');
    if p == r {
        return Some(String::new());
    }
    p.strip_prefix(r)?.strip_prefix('/').map(|s| s.trim_end_matches('/').to_string())
}

/// check_root の場所の規則（ファイルシステムに触らない・canonical なパスを受ける）。Err = 理由
fn allowed(canon: &Path, home: &Path, app: Option<&Path>) -> std::result::Result<(), &'static str> {
    let names: Vec<String> = canon
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().nfc().collect::<String>()),
            _ => None,
        })
        .collect();
    if names.is_empty() {
        return Err("ディスクの最上位です");
    }
    if names.iter().any(|n| n.starts_with('.')) {
        return Err("`.` で始まるフォルダ（隠しフォルダ）の中です");
    }
    if home.starts_with(canon) {
        return Err("ホームフォルダそのもの、またはその上位です");
    }
    if let Some(app) = app {
        if canon.starts_with(app) || app.starts_with(canon) {
            return Err("OpenRef のデータの置き場と重なります");
        }
    }
    if let Ok(rel) = canon.strip_prefix(home) {
        // 区画名は NFC で比べる（実ディスクと canonicalize は NFD のことがある = `マイドライブ` は NFD で返る）
        let r: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().nfc().collect()).collect();
        if r.first().map(String::as_str) != Some("Library") {
            return Ok(());
        }
        let cloud = "iCloud Drive / CloudStorage の最上位です。その中のフォルダを選んでください";
        return match r.get(1).map(String::as_str) {
            // ~/Library/Mobile Documents/<x>/<y…>
            Some("Mobile Documents") if r.len() >= 4 => Ok(()),
            Some("Mobile Documents") => Err(cloud),
            // ~/Library/CloudStorage/<provider>/<y…>（Google Drive の root の下はもう 1 段）
            Some("CloudStorage") => {
                let need = if r.get(3).is_some_and(|n| DRIVE_ROOTS.contains(&n.as_str())) { 5 } else { 4 };
                if r.len() >= need {
                    Ok(())
                } else {
                    Err(cloud)
                }
            }
            _ => Err("~/Library（アプリの設定・データの置き場）の中です"),
        };
    }
    // ホームの外: /Volumes/<vol>/<sub…> だけ
    if names.first().map(String::as_str) == Some("Volumes") && names.len() >= 3 {
        return Ok(());
    }
    Err("ホームの外です（外付けの中のフォルダ /Volumes/<ボリューム>/<フォルダ> は可）")
}

#[cfg(test)]
mod tests {
    use super::{allowed, check_root};
    use std::path::{Path, PathBuf};
    use unicode_normalization::UnicodeNormalization;

    /// 偽のホーム（tempdir の下・canonicalize 済み = macOS の /var → /private/var を吸収）。drop で消す
    struct Home(PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl std::ops::Deref for Home {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }
    fn home(name: &str) -> Home {
        let h = std::env::temp_dir().join(format!("openref-wiki-{}-{}-{name}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(&h).unwrap();
        Home(std::fs::canonicalize(&h).unwrap())
    }

    fn mk(p: &Path) -> &Path {
        std::fs::create_dir_all(p).unwrap();
        p
    }

    fn rejected(root: &Path, h: &Path, app: Option<&Path>) -> String {
        check_root(root, h, app).unwrap_err().to_string()
    }

    #[test]
    fn accepts_ordinary_folder_and_returns_canonical() {
        let h = home("ok");
        let app = mk(&h.join("Library/Application Support/OpenRef")).to_path_buf();
        let v = mk(&h.join("Documents/OpenRef Wiki 和文: x")).to_path_buf();
        assert_eq!(check_root(&v, &h, Some(&app)).unwrap(), v);
        // 末尾の `/.` や `..` を含んでも canonical に直して返す
        let messy = h.join("Documents/../Documents/OpenRef Wiki 和文: x/.");
        assert_eq!(check_root(&messy, &h, Some(&app)).unwrap(), v);
        // CloudStorage でも provider の下のフォルダなら可・Google Drive の root（マイドライブ）はその下
        let deep = mk(&h.join("Library/CloudStorage/GoogleDrive-a/My Drive/wiki")).to_path_buf();
        assert_eq!(check_root(&deep, &h, Some(&app)).unwrap(), deep);
        let jp = mk(&h.join("Library/CloudStorage/GoogleDrive-a/マイドライブ/wiki")).to_path_buf();
        assert_eq!(check_root(&jp, &h, Some(&app)).unwrap(), jp);
        let nfd: String = "マイドライブ".nfd().collect();
        let jp_nfd = mk(&h.join(format!("Library/CloudStorage/GoogleDrive-b/{nfd}/wiki"))).to_path_buf();
        assert!(check_root(&jp_nfd, &h, Some(&app)).is_ok());
        let dropbox = mk(&h.join("Library/CloudStorage/Dropbox/wiki")).to_path_buf();
        assert_eq!(check_root(&dropbox, &h, Some(&app)).unwrap(), dropbox);
        let icloud = mk(&h.join("Library/Mobile Documents/com~apple~CloudDocs/wiki")).to_path_buf();
        assert_eq!(check_root(&icloud, &h, Some(&app)).unwrap(), icloud);
    }

    #[test]
    fn symlink_is_resolved() {
        let h = home("link");
        let real = mk(&h.join("real")).to_path_buf();
        let link = h.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(check_root(&link, &h, None).unwrap(), real);
        // ホームを指す symlink は拒否
        let to_home = h.join("Documents-home");
        std::os::unix::fs::symlink(&*h, &to_home).unwrap();
        assert!(rejected(&to_home, &h, None).contains("ホーム"));
        // 隠しフォルダを指す symlink も拒否（canonical で判定）
        let ssh = mk(&h.join(".ssh")).to_path_buf();
        let to_ssh = h.join("keys");
        std::os::unix::fs::symlink(&ssh, &to_ssh).unwrap();
        assert!(rejected(&to_ssh, &h, None).contains("隠しフォルダ"));
    }

    #[test]
    fn rejects_relative_missing_and_file() {
        let h = home("bad");
        assert!(rejected(Path::new("wiki"), &h, None).contains("絶対パス"));
        assert!(rejected(&h.join("nope"), &h, None).contains("見つかりません"));
        let f = h.join("file.md");
        std::fs::write(&f, b"x").unwrap();
        assert!(rejected(&f, &h, None).contains("フォルダではありません"));
    }

    #[test]
    fn rejects_root_home_and_ancestors() {
        let h = home("home");
        assert!(rejected(Path::new("/"), &h, None).contains("最上位"));
        assert!(rejected(&h, &h, None).contains("ホーム"));
        assert!(rejected(&h.join("."), &h, None).contains("ホーム"));
        assert!(rejected(h.parent().unwrap(), &h, None).contains("ホーム"));
        // ホームの直下のフォルダは可
        let docs = mk(&h.join("Documents")).to_path_buf();
        assert!(check_root(&docs, &h, None).is_ok());
    }

    #[test]
    fn rejects_dot_folders() {
        let h = home("dot");
        for r in [".ssh", ".config/wiki", "Documents/.secret/wiki"] {
            let p = mk(&h.join(r)).to_path_buf();
            assert!(rejected(&p, &h, None).contains("隠しフォルダ"), "{r}");
        }
    }

    #[test]
    fn rejects_app_dir_its_children_and_ancestors() {
        let h = home("app");
        let app = mk(&h.join("Library/Application Support/OpenRef")).to_path_buf();
        let storage = mk(&app.join("storage")).to_path_buf();
        for r in [&app, &storage, &app.parent().unwrap().to_path_buf(), &h.join("Library")] {
            assert!(rejected(r, &h, Some(&app)).contains("データの置き場"), "{}", r.display());
        }
        // ~/Library の中は app_dir の兄弟でも拒否（app_dir が None = in-memory でも既定の置き場を守る）
        let sib = mk(&app.parent().unwrap().join("Other")).to_path_buf();
        assert!(rejected(&sib, &h, Some(&app)).contains("~/Library"));
        assert!(rejected(&app, &h, None).contains("~/Library"));
        assert!(rejected(&storage, &h, None).contains("~/Library"));
        let prefs = mk(&h.join("Library/Preferences")).to_path_buf();
        assert!(rejected(&prefs, &h, None).contains("~/Library"));
        // ~/Library の外の兄弟（scratch DB の隣に vault を置く試験の形）は可
        let scratch_app = mk(&h.join("openref-scratch/app")).to_path_buf();
        let vault = mk(&h.join("openref-scratch/wiki-vault")).to_path_buf();
        assert!(check_root(&vault, &h, Some(&scratch_app)).is_ok());
    }

    #[test]
    fn rejects_cloud_roots_and_their_direct_children() {
        let h = home("cloud");
        for r in [
            "Library/Mobile Documents",
            "Library/Mobile Documents/com~apple~CloudDocs",
            "Library/CloudStorage",
            "Library/CloudStorage/GoogleDrive-a",
            "Library/CloudStorage/GoogleDrive-a/My Drive",
            "Library/CloudStorage/GoogleDrive-a/マイドライブ",
            "Library/CloudStorage/GoogleDrive-a/共有ドライブ",
            "Library/CloudStorage/GoogleDrive-a/Shared drives",
        ] {
            let p = mk(&h.join(r)).to_path_buf();
            assert!(rejected(&p, &h, None).contains("CloudStorage"), "{r}");
        }
        // 実機の Drive の root は NFD で返る（canonicalize も NFD）。NFD の名前でも root として拒否する
        for name in ["マイドライブ", "共有ドライブ"] {
            let nfd: String = name.nfd().collect();
            assert_ne!(nfd, name);
            let p = mk(&h.join(format!("Library/CloudStorage/GoogleDrive-n/{nfd}"))).to_path_buf();
            assert!(rejected(&p, &h, None).contains("CloudStorage"), "{name} (NFD)");
            assert!(allowed(Path::new(&format!("/Users/u/Library/CloudStorage/G/{nfd}")), Path::new("/Users/u"), None).is_err());
        }
    }

    /// ホームの外（ファイルシステムに触らない規則の部分）
    #[test]
    fn outside_home_only_volumes_subfolders() {
        let h = Path::new("/Users/someone");
        for p in ["/System", "/usr/local/wiki", "/private/tmp/wiki", "/Volumes", "/Volumes/USB", "/opt/wiki", "/Users", "/Users/other/wiki"] {
            assert!(allowed(Path::new(p), h, None).is_err(), "{p}");
        }
        for p in ["/Volumes/USB/wiki", "/Volumes/外付け/a/b"] {
            assert!(allowed(Path::new(p), h, None).is_ok(), "{p}");
        }
        assert!(allowed(Path::new("/Volumes/USB/.Trashes/x"), h, None).is_err());
        assert!(allowed(Path::new("/Users/someone/Documents/wiki"), h, None).is_ok());
        assert!(allowed(Path::new("/Users/someone/Library/Mobile Documents/com~apple~CloudDocs/w"), h, None).is_ok());
    }
}
