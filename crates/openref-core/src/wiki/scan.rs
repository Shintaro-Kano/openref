//! vault の stat 走査。`.` 始まりの区画（`.obsidian/` 等）は飛ばす・symlink は辿らない
//! （vault の外を指しうる = 索引に入れない）。readdir が拒否された（Drive の TCC）ら `find(1)` に fallback する（sync.rs と同じ）。
//!
//! 除外フォルダ（`Rules`）: 走査は入口で引き返し、中へ降りない。記録するのは
//! いちばん上の除外フォルダだけ（`Walk::excluded`）。既定 = 名前が `node_modules` と、署名つきの `CACHEDIR.TAG` を持つフォルダ。
//! 加えてワークスペースごとの `exclude` / `include`（設定 `wiki_workspaces`）。`.` 始まりの区画とは別の物

use std::collections::HashMap;
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

/// CACHEDIR.TAG の規格の署名（先頭 43 バイト。https://bford.info/cachedir/）
pub const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// フォルダ D の `D/CACHEDIR.TAG` が symlink でない通常ファイルで、先頭が規格の署名か
pub fn has_cachedir_tag(dir: &Path) -> bool {
    let tag = dir.join("CACHEDIR.TAG");
    if !std::fs::symlink_metadata(&tag).is_ok_and(|m| m.file_type().is_file()) {
        return false;
    }
    let mut buf = [0u8; 43];
    std::fs::File::open(&tag).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf)).is_ok() && buf == CACHEDIR_SIGNATURE
}

/// 除外の規則（ワークスペースごと）。`include` / `exclude` = vault 相対・NFC・`/` 区切りのフォルダ（glob なし・完全一致）。
/// 判定の順（`excludes_dir`）: include にある → 入れる / exclude にある → 除く / 名前が `node_modules` → 除く /
/// 署名つきの `CACHEDIR.TAG` → 除く / それ以外 → 入れる。root 自身は対象外（常に入れる）。
/// `Default` = 既定の規則だけ（リストは空）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    include: Vec<String>,
    exclude: Vec<String>,
}

/// include / exclude の 1 要素の正規化（NFC・前後の空白と `/` を落とす）。空・空の区画・`.` 始まりの区画（`..` を含む）は None。
/// 走査の規則（`Rules::new`）と設定の書換え（`workspaces::set_excluded` の比較）が共用する = 2 か所で綴りの扱いがずれない
pub fn norm_dir(s: &str) -> Option<String> {
    let s: String = s.nfc().collect();
    let s = s.trim().trim_matches('/');
    (!s.is_empty() && !s.split('/').any(|seg| seg.is_empty() || seg.starts_with('.'))).then(|| s.to_string())
}

/// リストの要素を正規化する（`norm_dir`）。正規化できない要素は捨てる・重複は 1 つに
fn norm_list(v: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        let Some(s) = norm_dir(&s) else { continue };
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

impl Rules {
    pub fn new(include: impl IntoIterator<Item = String>, exclude: impl IntoIterator<Item = String>) -> Rules {
        Rules { include: norm_list(include), exclude: norm_list(exclude) }
    }

    pub fn include(&self) -> &[String] {
        &self.include
    }

    pub fn exclude(&self) -> &[String] {
        &self.exclude
    }

    /// 既定の規則（名前が `node_modules`・署名つきの `CACHEDIR.TAG`）に当たるか。abs = フォルダの絶対パス
    pub fn default_excludes(abs: &Path) -> bool {
        abs.file_name().is_some_and(|n| n == "node_modules") || has_cachedir_tag(abs)
    }

    /// フォルダ rel（vault 相対・root 自身 = "" は常に false）を除くか（順 = include → exclude → 名前が node_modules → 署名つき CACHEDIR.TAG・最初に当たった物で決まる）。祖先は見ない（走査は上から順に判定する）
    pub fn excludes_dir(&self, root: &Path, rel: &str) -> bool {
        if rel.is_empty() {
            return false;
        }
        if self.include.iter().any(|d| d == rel) {
            return false;
        }
        if self.exclude.iter().any(|d| d == rel) {
            return true;
        }
        Rules::default_excludes(&root.join(rel))
    }
}

/// 除外の判定を 1 回の処理（1 バッチ・1 回の書込）の中でだけ覚える（フォルダ → 結果）。バッチをまたいで使わない = 規則・ファイルの変更がすぐ効く
pub struct Judge<'a> {
    root: &'a Path,
    rules: &'a Rules,
    memo: HashMap<String, bool>,
}

impl<'a> Judge<'a> {
    pub fn new(root: &'a Path, rules: &'a Rules) -> Judge<'a> {
        Judge { root, rules, memo: HashMap::new() }
    }

    /// フォルダ rel そのものを除くか（祖先は見ない）
    pub fn dir(&mut self, rel: &str) -> bool {
        if let Some(&b) = self.memo.get(rel) {
            return b;
        }
        let b = self.rules.excludes_dir(self.root, rel);
        self.memo.insert(rel.to_string(), b);
        b
    }

    /// パス rel（vault 相対）を上から辿り、最初に除かれるフォルダ（= 走査が引き返す所）。self_is_dir = rel 自身もフォルダとして判定する。
    /// 無ければ None（走査に現れる側）
    pub fn excluded_at(&mut self, rel: &str, self_is_dir: bool) -> Option<String> {
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        let n = if self_is_dir { segs.len() } else { segs.len().saturating_sub(1) };
        let mut cur = String::new();
        for seg in &segs[..n] {
            if !cur.is_empty() {
                cur.push('/');
            }
            cur.push_str(seg);
            if self.dir(&cur) {
                return Some(cur);
            }
        }
        None
    }
}

#[derive(Debug, Default)]
pub struct Walk {
    pub entries: Vec<Entry>,
    /// 引き返した除外フォルダ（vault 相対・パス順・いちばん上の物だけ）
    pub excluded: Vec<String>,
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
pub fn walk(root: &Path, rules: &Rules) -> crate::Result<Walk> {
    let mut w = Walk::default();
    let mut excluded: Vec<String> = Vec::new();
    let it = walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
        if e.depth() == 0 {
            return true;
        }
        if e.file_name().to_string_lossy().starts_with('.') {
            return false;
        }
        // 除外フォルダ = 入口で引き返す（中へ降りない）。symlink は辿らない（is_dir は false）ので判定しない
        if e.file_type().is_dir() {
            if let Some(rel) = rel_of(root, e.path()) {
                if rules.excludes_dir(root, &rel) {
                    excluded.push(rel);
                    return false;
                }
            }
        }
        true
    });
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
    w.excluded = excluded;
    // File Provider（Drive for Desktop）では readdir が拒否 / 空になることがある → find(1)。find は除外を知らない =
    // 列挙の後に同じ判定で落とす（速さは求めない）。記録 = 落としたファイルの、いちばん上の除外フォルダ（空の除外フォルダは記録に出ない）
    if w.entries.is_empty() && w.errors > 0 {
        let mut found = find(root);
        let ex = drop_excluded(root, rules, &mut found);
        if !found.entries.is_empty() {
            w.excluded = ex;
        }
        w.merge_find(found);
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
    w.excluded.sort();
    Ok(w)
}

/// find(1) の列挙から除外フォルダの配下を落とす（walk と同じ判定）。戻り値 = 落としたファイルの、いちばん上の除外フォルダ（パス順）
pub fn drop_excluded(root: &Path, rules: &Rules, found: &mut Found) -> Vec<String> {
    let mut judge = Judge::new(root, rules);
    let mut ex: Vec<String> = Vec::new();
    found.entries.retain(|e| match judge.excluded_at(&e.rel, false) {
        Some(d) => {
            if !ex.contains(&d) {
                ex.push(d);
            }
            false
        }
        None => true,
    });
    ex.sort();
    ex
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

    fn put(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn rels(w: &Walk) -> Vec<&str> {
        w.entries.iter().map(|e| e.rel.as_str()).collect()
    }

    /// 除外の規則: node_modules・署名つき CACHEDIR.TAG・exclude で引き返す / include が既定に勝つ / include の中の
    /// node_modules はまた除く / 記録はいちばん上だけ / root は除かない / node_modules という名のファイルと署名なし・symlink の
    /// CACHEDIR.TAG は除かない / find(1) の fallback も同じ集合
    #[test]
    fn walk_excludes_and_find_matches() {
        let d = std::env::temp_dir().join(format!("openref-wiki-scan-ex-{}-{}", std::process::id(), fastrand::u64(..)));
        let root = d.join("v");
        let sig = CACHEDIR_SIGNATURE.to_vec();
        let mut sig_nl = sig.clone();
        sig_nl.extend_from_slice(b"\n# This file is a cache directory tag.\n");
        put(&root, "notes/a.md", b"a");
        put(&root, "app/node_modules/pkg/index.js", b"x");
        put(&root, "app/node_modules/pkg/node_modules/deep/x.js", b"x");
        put(&root, "app/src/main.ts", b"x");
        put(&root, "target/CACHEDIR.TAG", &sig_nl);
        put(&root, "target/debug/build.o", b"x");
        put(&root, "target/debug/node_modules/x.js", b"x");
        put(&root, "proj/testdata/1.dcm", b"x");
        put(&root, "proj/docs/r.md", b"x");
        // 署名なし・短い CACHEDIR.TAG / symlink の CACHEDIR.TAG / node_modules という名のファイル = 除かない
        put(&root, "fake/CACHEDIR.TAG", b"Signature: 00000000000000000000000000000000 not the standard one");
        put(&root, "fake/k.md", b"x");
        put(&root, "short/CACHEDIR.TAG", b"Signature: 8a47");
        put(&root, "short/k.md", b"x");
        put(&root, "real-tag/CACHEDIR.TAG", &sig);
        std::fs::create_dir_all(root.join("linked")).unwrap();
        std::os::unix::fs::symlink(root.join("real-tag/CACHEDIR.TAG"), root.join("linked/CACHEDIR.TAG")).unwrap();
        put(&root, "linked/k.md", b"x");
        put(&root, "lib/node_modules", b"file named node_modules");
        // 名前で除くのは node_modules だけ（target という名のノートのフォルダは巻き込まない）
        put(&root, "site/target/post.md", b"x");
        // include で入れたフォルダの中の node_modules はまた除く
        put(&root, "keep/CACHEDIR.TAG", &sig);
        put(&root, "keep/out.txt", b"x");
        put(&root, "keep/node_modules/y.js", b"x");
        let root = std::fs::canonicalize(&root).unwrap();
        let rules = Rules::new(["keep".to_string()], ["proj/testdata".to_string(), "/fake/".to_string()]);
        let w = walk(&root, &rules).unwrap();
        assert_eq!(
            rels(&w),
            [
                "app/src/main.ts",
                "keep/CACHEDIR.TAG",
                "keep/out.txt",
                "lib/node_modules",
                "linked/k.md",
                "notes/a.md",
                "proj/docs/r.md",
                "short/CACHEDIR.TAG",
                "short/k.md",
                "site/target/post.md",
            ]
        );
        assert_eq!(w.excluded, ["app/node_modules", "fake", "keep/node_modules", "proj/testdata", "real-tag", "target"], "いちばん上だけ・パス順");
        // 既定の規則だけ（リスト無し）: fake は署名なし = 入る・keep は除く・proj/testdata は入る
        let w0 = walk(&root, &Rules::default()).unwrap();
        assert!(rels(&w0).contains(&"fake/k.md") && rels(&w0).contains(&"proj/testdata/1.dcm"));
        assert_eq!(w0.excluded, ["app/node_modules", "keep", "real-tag", "target"]);
        // root 自身は除かない（root に CACHEDIR.TAG・root の名前が node_modules でも）
        let nm = d.join("node_modules");
        put(&nm, "CACHEDIR.TAG", &sig);
        put(&nm, "a.md", b"a");
        let nm = std::fs::canonicalize(&nm).unwrap();
        let wr = walk(&nm, &Rules::default()).unwrap();
        assert_eq!(rels(&wr), ["CACHEDIR.TAG", "a.md"]);
        assert!(wr.excluded.is_empty());
        // find(1) の fallback: 列挙の後に同じ判定で落とす = 同じ集合・同じ記録（空でない除外フォルダ）
        let mut f = find(&root);
        let ex = drop_excluded(&root, &rules, &mut f);
        f.entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        assert_eq!(f.entries, w.entries, "find(1) の fallback も同じ集合");
        assert_eq!(ex, w.excluded);
        // Judge: 上から辿り最初に除かれるフォルダ。ファイル名は判定しない・self_is_dir でフォルダ自身も
        let mut j = Judge::new(&root, &rules);
        assert_eq!(j.excluded_at("app/node_modules/pkg/node_modules/deep/x.js", false).as_deref(), Some("app/node_modules"));
        assert_eq!(j.excluded_at("keep/node_modules/y.js", false).as_deref(), Some("keep/node_modules"));
        assert_eq!(j.excluded_at("keep/out.txt", false), None);
        assert_eq!(j.excluded_at("lib/node_modules", false), None, "ファイルは判定しない");
        assert_eq!(j.excluded_at("app/node_modules", true).as_deref(), Some("app/node_modules"));
        assert_eq!(j.excluded_at("app/node_modules", false), None);
        assert_eq!(j.excluded_at("new/node_modules/x.md", false).as_deref(), Some("new/node_modules"), "無いフォルダも名前で");
        assert_eq!(j.excluded_at("", true), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Rules::new の正規化: NFC・前後の `/`・空 / `.` 始まり / `..` / 重複は捨てる
    #[test]
    fn rules_normalize_lists() {
        // NFC と NFD で字が変わる名前（デ = テ + 濁点）でないと NFC への正規化を確かめられない
        let nfd: String = "データ".nfd().collect();
        assert_ne!(nfd, "データ");
        let r = Rules::new(
            ["/a/b/".to_string(), "".into(), " ".into(), ".git".into(), "x/../y".into(), "a/b".into(), format!("{nfd}/z")],
            ["c//d".to_string(), "e/.hidden".into(), "e".into()],
        );
        assert_eq!(r.include(), ["a/b".to_string(), "データ/z".into()]);
        assert_eq!(r.exclude(), ["e".to_string()]);
        assert!(!r.excludes_dir(Path::new("/nonexistent"), ""), "root は除かない");
        assert!(r.excludes_dir(Path::new("/nonexistent"), "e"));
        assert!(r.excludes_dir(Path::new("/nonexistent"), "x/node_modules"));
        assert!(!r.excludes_dir(Path::new("/nonexistent"), "a/b"));
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
        let w = walk(&root, &Rules::default()).unwrap();
        let rels: Vec<&str> = w.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, ["a/x.md", "img.png"]);
        assert_eq!(w.entries[1].size, 3);
        assert!(!w.used_find);
        let mut f = find(&root);
        f.entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        assert_eq!(f.entries, w.entries, "find(1) の fallback も同じ集合");
        assert!(f.complete && f.error_dirs.is_empty());
        // root が無い・読めない = Err（空の結果を返さない）
        assert!(walk(&d.join("nope"), &Rules::default()).is_err());
        let locked = d.join("locked");
        std::fs::create_dir_all(locked.join("a")).unwrap();
        std::fs::write(locked.join("a/x.md"), "x").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = walk(&locked, &Rules::default());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err(), "{r:?}");
        // 空のフォルダは Ok（0 件）
        std::fs::create_dir_all(d.join("empty")).unwrap();
        assert!(walk(&d.join("empty"), &Rules::default()).unwrap().entries.is_empty());
        // 一部のサブフォルダだけ読めない = Ok で、読めなかった区画を error_dirs に（配下の判定は区画単位・名前の前方一致ではない）
        let part = d.join("part");
        std::fs::create_dir_all(part.join("sub/deep")).unwrap();
        std::fs::create_dir_all(part.join("sub2")).unwrap();
        std::fs::write(part.join("sub/deep/a.md"), "a").unwrap();
        std::fs::write(part.join("sub2/b.md"), "b").unwrap();
        std::fs::write(part.join("c.md"), "c").unwrap();
        std::fs::set_permissions(part.join("sub"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = walk(&part, &Rules::default());
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
        let w = walk(&part, &Rules::default()).unwrap();
        assert_eq!(w.entries.len(), 3);
        assert!(w.error_dirs.is_empty() && w.errors == 0);
        // x の無いフォルダ（r--）: 名前は列挙できるが stat が拒否される → 直下のファイルも下のフォルダも黙って落とさず error_dirs に
        std::fs::write(part.join("sub/direct.md"), "d").unwrap();
        std::fs::set_permissions(part.join("sub"), std::fs::Permissions::from_mode(0o400)).unwrap();
        let r = walk(&part, &Rules::default());
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
