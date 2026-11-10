//! vault の stat 走査。`.` 始まりの区画（`.obsidian/` 等）は飛ばす・symlink は辿らない
//! （vault の外を指しうる = 索引に入れない）。readdir が拒否された（Drive の TCC）ら `find(1)` に fallback する（sync.rs と同じ）。

use std::path::Path;
use std::time::UNIX_EPOCH;
use unicode_normalization::UnicodeNormalization;

/// ページの種類（`pages.kind`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Md,
    Html,
    /// 画像・動画・音声・pdf（本文は読まない）
    Media,
    /// それ以外（css・json 等。本文は読まない）
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Md => "md",
            Kind::Html => "html",
            Kind::Media => "media",
            Kind::Other => "other",
        }
    }
    pub fn of(path: &str) -> Kind {
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        if path.rsplit('/').next().is_some_and(|n| !n.contains('.')) {
            return Kind::Other;
        }
        match ext.as_str() {
            "md" | "markdown" => Kind::Md,
            "html" | "htm" => Kind::Html,
            _ if media_kind(&ext).is_some() => Kind::Media,
            _ => Kind::Other,
        }
    }
    pub fn has_body(self) -> bool {
        matches!(self, Kind::Md | Kind::Html)
    }
}

/// media の下位の種類（拡張子 → image / video / audio / pdf）
pub fn media_kind(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "avif" | "heic" | "bmp" | "tif" | "tiff" => "image",
        "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" => "video",
        "mp3" | "m4a" | "wav" | "aac" | "flac" | "ogg" | "opus" => "audio",
        "pdf" => "pdf",
        _ => return None,
    })
}

/// 走査の 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// vault 相対・NFC・`/` 区切り
    pub rel: String,
    pub mtime_ns: i64,
    pub size: i64,
}

#[derive(Debug, Default)]
pub struct Walk {
    pub entries: Vec<Entry>,
    /// readdir が拒否され find(1) で列挙した
    pub used_find: bool,
    /// 読めなかったディレクトリ等の数（walkdir のエラー）
    pub errors: usize,
    /// 読めなかった区画（vault 相対・NFC・`/` 区切り。root 自身なら `""`）。この配下の既知のページは
    /// 走査に現れなくても「消えた」とは扱わない（Drive の TCC で一部のフォルダだけ readdir が拒否される）。
    /// find(1) が全体を列挙できた（終了状態 0）ときは空。find が途中で拒否されたら stderr の「find: <path>: …」のパスを足す
    pub error_dirs: Vec<String>,
}

impl Walk {
    fn add_error(&mut self, rel: String) {
        self.errors += 1;
        if !self.error_dirs.contains(&rel) {
            self.error_dirs.push(rel);
        }
    }

    /// find(1) の結果を採る（1 件以上あるとき）。find が全体を見た（終了状態 0）なら walkdir で読めなかった区画も
    /// 列挙に入っているので error_dirs を空に、途中で拒否されていれば find の stderr のパスを足す
    fn merge_find(&mut self, found: Found) {
        if found.entries.is_empty() {
            return;
        }
        self.used_find = true;
        self.entries = found.entries;
        if found.complete {
            self.error_dirs.clear();
        } else {
            for d in found.error_dirs {
                if !self.error_dirs.contains(&d) {
                    self.error_dirs.push(d);
                }
            }
        }
    }

    /// rel が読めなかった区画の中（またはその区画自身）か
    pub fn unreadable(&self, rel: &str) -> bool {
        self.error_dirs.iter().any(|d| d.is_empty() || rel == d || rel.strip_prefix(d.as_str()).is_some_and(|r| r.starts_with('/')))
    }
}

/// 区画のどれかが `.` 始まりか
pub fn hidden(rel: &str) -> bool {
    rel.split('/').any(|s| s.starts_with('.'))
}

/// 通常ファイルなら (mtime_ns, size)。無い・読めない・通常ファイルでない、は区別しない
pub fn stat(abs: &Path) -> Option<(i64, i64)> {
    stat_io(abs).ok().flatten()
}

/// stat の失敗を区別する版: Err = stat 自体の失敗（NotFound・PermissionDenied…）・Ok(None) = 通常ファイルではない。
/// mtime は walk と同じ（epoch 前・取れない = 0）
pub fn stat_io(abs: &Path) -> std::io::Result<Option<(i64, i64)>> {
    let md = std::fs::symlink_metadata(abs)?;
    if !md.file_type().is_file() {
        return Ok(None);
    }
    Ok(Some((mtime_ns(&md), md.len() as i64)))
}

fn mtime_ns(md: &std::fs::Metadata) -> i64 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

fn rel_of(root: &Path, abs: &Path) -> Option<String> {
    let r = abs.strip_prefix(root).ok()?;
    let s: String = r.to_str()?.nfc().collect();
    (!s.is_empty()).then_some(s)
}

/// 走査する。root の直下が読めず find(1) でも列挙できなかったら Err（空の結果で既知のページを消さない）
pub fn walk(root: &Path) -> crate::Result<Walk> {
    let mut w = Walk::default();
    let it = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for e in it {
        match e {
            Ok(e) if e.file_type().is_file() => {
                let Some(rel) = rel_of(root, e.path()) else { continue };
                match e.metadata() {
                    Ok(md) => w.entries.push(Entry { rel, mtime_ns: mtime_ns(&md), size: md.len() as i64 }),
                    // 名前は列挙できたが stat が拒否された（x の無いフォルダ r-- の中など）= 読めなかった側に数える
                    Err(_) => w.add_error(rel),
                }
            }
            Ok(_) => {}
            // パスの無いエラー（walkdir 内部）は root 全体を読めなかったものとして扱う（消さない側に倒す）
            Err(err) => w.add_error(err.path().and_then(|p| rel_of(root, p)).unwrap_or_default()),
        }
    }
    // File Provider（Drive for Desktop）では readdir が拒否 / 空になることがある → find(1)
    if w.entries.is_empty() && w.errors > 0 {
        w.merge_find(find(root));
    }
    // 0 件で、列挙のエラーがあった（find でも取れなかった）か root が読めない = 失敗（空の結果で既知のページを消さない）
    if w.entries.is_empty() {
        if let Err(e) = std::fs::read_dir(root) {
            return Err(crate::CoreError::Io(std::io::Error::new(e.kind(), format!("ワークスペース {} を読めません: {e}", root.display()))));
        }
        if w.errors > 0 {
            return Err(crate::CoreError::Io(std::io::Error::other(format!(
                "ワークスペース {} を列挙できません（読めないフォルダ {} 件・find(1) でも 0 件）",
                root.display(),
                w.errors
            ))));
        }
    }
    w.entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(w)
}

/// find(1) の結果
#[derive(Debug, Default)]
pub struct Found {
    pub entries: Vec<Entry>,
    /// 終了状態 0 = 全体を列挙できた（読めないフォルダがあると find は残りを出して非 0 で終わる）
    pub complete: bool,
    /// stderr の「find: <path>: Permission denied」等のパス（vault 相対）
    pub error_dirs: Vec<String>,
}

/// find(1) で列挙（`.` 始まりの区画は prune・通常ファイルだけ）。起動に失敗すれば空（complete = false）。
/// 本棚の走査（crate::shelf::scan）の fallback も共用する
pub fn find(root: &Path) -> Found {
    let out = std::process::Command::new("/usr/bin/find")
        .arg(root)
        .args(["-mindepth", "1", "(", "-name", ".*", "-prune", ")", "-o", "-type", "f", "-print0"])
        .output();
    let Ok(out) = out else { return Found::default() };
    let entries = out
        .stdout
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let abs = Path::new(std::str::from_utf8(s).ok()?);
            let rel = rel_of(root, abs)?;
            let (mtime_ns, size) = stat(abs)?;
            Some(Entry { rel, mtime_ns, size })
        })
        .collect();
    let mut error_dirs: Vec<String> = Vec::new();
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        // 「find: /abs/path: Permission denied」。root 自身や vault の外（rel_of = None）は "" = 全体
        let Some(rest) = line.strip_prefix("find: ") else { continue };
        let path = rest.rsplit_once(": ").map(|(p, _)| p).unwrap_or(rest);
        let d = rel_of(root, Path::new(path)).unwrap_or_default();
        if !error_dirs.contains(&d) {
            error_dirs.push(d);
        }
    }
    Found { entries, complete: out.status.success(), error_dirs }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(Kind::of("a/b.MD"), Kind::Md);
        assert_eq!(Kind::of("x.htm"), Kind::Html);
        assert_eq!(Kind::of("x.JPG"), Kind::Media);
        assert_eq!(Kind::of("x.pdf"), Kind::Media);
        assert_eq!(Kind::of("x.css"), Kind::Other);
        assert_eq!(Kind::of("dir.v2/README"), Kind::Other);
        assert_eq!(media_kind("MOV"), Some("video"));
    }

    /// unreadable: 区画単位（名前の前方一致ではない）・"" = root 全体
    #[test]
    fn unreadable_is_segment_wise_and_empty_means_all() {
        let w = Walk { error_dirs: vec!["sub".into(), "a/b.md".into()], ..Walk::default() };
        assert!(w.unreadable("sub"));
        assert!(w.unreadable("sub/x.md"));
        assert!(w.unreadable("sub/deep/x.md"));
        assert!(!w.unreadable("sub2/x.md"));
        assert!(!w.unreadable("subx.md"));
        assert!(!w.unreadable("x/sub/y.md"));
        assert!(w.unreadable("a/b.md"));
        assert!(!w.unreadable("a/c.md"));
        let all = Walk { error_dirs: vec![String::new()], ..Walk::default() };
        assert!(all.unreadable("anything.md") && all.unreadable("deep/er/x.md"));
        assert!(!Walk::default().unreadable("x.md"));
    }

    /// find(1) の fallback の合流: 全体を見た find は error_dirs を空に・途中で拒否された find は stderr のパスを足す・0 件は採らない
    #[test]
    fn merge_find_keeps_error_dirs_unless_find_was_complete() {
        let e = |rel: &str| Entry { rel: rel.into(), mtime_ns: 1, size: 1 };
        // root が読めず walkdir は "" を error_dirs に → find が全体を見た = 消して entries を採る
        let mut w = Walk { errors: 1, error_dirs: vec![String::new()], ..Walk::default() };
        w.merge_find(Found { entries: vec![e("a.md")], complete: true, error_dirs: vec![] });
        assert!(w.used_find && w.error_dirs.is_empty() && w.entries.len() == 1);
        // find も一部を拒否された = "" は残り（全体を消さない側）、find の見た区画も足す
        let mut w = Walk { errors: 1, error_dirs: vec![String::new()], ..Walk::default() };
        w.merge_find(Found { entries: vec![e("a.md")], complete: false, error_dirs: vec!["sub".into()] });
        assert!(w.used_find);
        assert_eq!(w.error_dirs, ["", "sub"]);
        assert!(w.unreadable("sub/x.md") && w.unreadable("other.md"));
        // walkdir がサブフォルダで失敗・find も同じ所で失敗 = 重複させない
        let mut w = Walk { errors: 1, error_dirs: vec!["sub".into()], ..Walk::default() };
        w.merge_find(Found { entries: vec![e("a.md")], complete: false, error_dirs: vec!["sub".into(), "sub2".into()] });
        assert_eq!(w.error_dirs, ["sub", "sub2"]);
        // 0 件の find は採らない（used_find も立たない・error_dirs も触らない）
        let mut w = Walk { errors: 1, error_dirs: vec![String::new()], ..Walk::default() };
        w.merge_find(Found { entries: vec![], complete: true, error_dirs: vec![] });
        assert!(!w.used_find && w.entries.is_empty() && w.error_dirs == [""]);
    }

    #[test]
    fn walk_skips_dot_and_symlinks_and_find_matches() {
        let d = std::env::temp_dir().join(format!("openref-wiki-scan-{}-{}", std::process::id(), fastrand::u64(..)));
        let root = d.join("v");
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::create_dir_all(root.join("a/.hidden")).unwrap();
        std::fs::write(root.join("a/x.md"), "x").unwrap();
        std::fs::write(root.join("a/.hidden/y.md"), "y").unwrap();
        std::fs::write(root.join(".obsidian/app.json"), "{}").unwrap();
        std::fs::write(root.join(".dot.md"), "").unwrap();
        std::fs::write(root.join("img.png"), [0u8; 3]).unwrap();
        std::fs::write(d.join("outside.md"), "o").unwrap();
        std::os::unix::fs::symlink(d.join("outside.md"), root.join("link.md")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let w = walk(&root).unwrap();
        let rels: Vec<&str> = w.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, ["a/x.md", "img.png"]);
        assert_eq!(w.entries[1].size, 3);
        assert!(!w.used_find);
        let mut f = find(&root);
        f.entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        assert_eq!(f.entries, w.entries, "find(1) の fallback も同じ集合");
        assert!(f.complete && f.error_dirs.is_empty());
        // root が無い・読めない = Err（空の結果を返さない）
        assert!(walk(&d.join("nope")).is_err());
        let locked = d.join("locked");
        std::fs::create_dir_all(locked.join("a")).unwrap();
        std::fs::write(locked.join("a/x.md"), "x").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = walk(&locked);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err(), "{r:?}");
        // 空のフォルダは Ok（0 件）
        std::fs::create_dir_all(d.join("empty")).unwrap();
        assert!(walk(&d.join("empty")).unwrap().entries.is_empty());
        // 一部のサブフォルダだけ読めない = Ok で、読めなかった区画を error_dirs に（配下の判定は区画単位・名前の前方一致ではない）
        let part = d.join("part");
        std::fs::create_dir_all(part.join("sub/deep")).unwrap();
        std::fs::create_dir_all(part.join("sub2")).unwrap();
        std::fs::write(part.join("sub/deep/a.md"), "a").unwrap();
        std::fs::write(part.join("sub2/b.md"), "b").unwrap();
        std::fs::write(part.join("c.md"), "c").unwrap();
        std::fs::set_permissions(part.join("sub"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = walk(&part);
        std::fs::set_permissions(part.join("sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let w = r.unwrap();
        let rels: Vec<&str> = w.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, ["c.md", "sub2/b.md"]);
        assert_eq!(w.errors, 1);
        assert_eq!(w.error_dirs, ["sub"]);
        assert!(w.unreadable("sub/deep/a.md"));
        assert!(w.unreadable("sub"));
        assert!(!w.unreadable("sub2/b.md"));
        assert!(!w.unreadable("c.md"));
        assert!(!w.used_find);
        // find(1) も読めないフォルダがあると残りを出して非 0 で終わる → complete = false・stderr のパスを error_dirs に
        std::fs::set_permissions(part.join("sub"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let f = find(&part);
        std::fs::set_permissions(part.join("sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut rels: Vec<&str> = f.entries.iter().map(|e| e.rel.as_str()).collect();
        rels.sort();
        assert_eq!(rels, ["c.md", "sub2/b.md"]);
        assert!(!f.complete, "{f:?}");
        assert_eq!(f.error_dirs, ["sub"], "{f:?}");
        // 読めれば error_dirs は空
        let w = walk(&part).unwrap();
        assert_eq!(w.entries.len(), 3);
        assert!(w.error_dirs.is_empty() && w.errors == 0);
        // x の無いフォルダ（r--）: 名前は列挙できるが stat が拒否される → 直下のファイルも下のフォルダも黙って落とさず error_dirs に
        std::fs::write(part.join("sub/direct.md"), "d").unwrap();
        std::fs::set_permissions(part.join("sub"), std::fs::Permissions::from_mode(0o400)).unwrap();
        let r = walk(&part);
        std::fs::set_permissions(part.join("sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let w = r.unwrap();
        let rels: Vec<&str> = w.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, ["c.md", "sub2/b.md"], "{w:?}");
        assert!(w.errors >= 2, "{w:?}");
        assert!(w.unreadable("sub/direct.md") && w.unreadable("sub/deep/a.md"), "{w:?}");
        assert!(!w.unreadable("sub2/b.md"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
