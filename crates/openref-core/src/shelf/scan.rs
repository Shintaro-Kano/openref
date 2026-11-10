//! 本棚の走査。root の下の PDF をフォルダの木にする。読むだけ（root に何も書かない）。
//! `.` / `_` 始まりと PDF を含まないフォルダは隠す・symlink は辿らない・読めないサブフォルダは飛ばして件数を返す。
//! readdir が拒否された（Drive の TCC）ら `find(1)`（wiki::scan::find）に fallback する。
//! 段取り = 列挙（read_tree / tree_of_found が RawDir を作る）→ 畳み込み（fold）= 列挙の 2 経路で木の規則を 1 つに保つ

use super::id_for;
use crate::wiki::scan::{find, Found};
use crate::{CoreError, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use unicode_normalization::UnicodeNormalization;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfBook {
    pub id: String,
    /// 表示の題名。通常は拡張子を除いたファイル名。ファイル名が "original" の場合（記事フォルダ/original.pdf の形）は親フォルダ名
    pub title: String,
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    /// 親フォルダ名（検索・検索結果の所属の表示に使う）
    pub folder: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfFolder {
    pub id: String,
    pub name: String,
    pub path: String,
    pub children: Vec<ShelfFolder>,
    pub books: Vec<ShelfBook>,
    /// 配下（再帰）の総冊数
    pub total_books: usize,
}

/// 走査の選択（本棚の設定 = `shelf_workspaces` の項目から）
#[derive(Clone, Copy)]
pub struct ScanOpts<'a> {
    /// PDF 1 個だけを含み子フォルダを持たないフォルダを、本 1 冊に畳み込む
    pub collapse_single: bool,
    /// 号フォルダ（YYYY-MM）の表示名に子の「特集_◯◯」の題名を合成する
    pub issue_titles: bool,
    /// フォルダ名の表示の置換（例: journal-a → 雑誌 A）
    pub aliases: &'a HashMap<String, String>,
}

/// 走査の結果
#[derive(Clone, Debug, PartialEq)]
pub struct Scan {
    pub tree: ShelfFolder,
    /// 読めなかったフォルダの数（飛ばした）
    pub unreadable: usize,
    /// readdir が拒否され find(1) で列挙した
    pub used_find: bool,
}

/// 列挙の 1 冊
struct RawPdf {
    path: PathBuf,
    size: u64,
    mtime: i64,
}

/// 列挙の 1 フォルダ（畳み込みの前）
struct RawDir {
    path: PathBuf,
    dirs: BTreeMap<String, RawDir>,
    pdfs: Vec<RawPdf>,
}

impl RawDir {
    fn new(path: &Path) -> RawDir {
        RawDir { path: path.to_path_buf(), dirs: BTreeMap::new(), pdfs: Vec::new() }
    }
}

/// 隠す名前（`.` 隠し・`_` 内部資産）
fn hidden_name(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('_')
}

/// 拡張子が pdf（大小無視）なら拡張子を除いた名前（"x..pdf" = "x"）
fn pdf_stem(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    ext.eq_ignore_ascii_case("pdf").then(|| stem.trim_end_matches('.'))
}

/// 表示の題名。`記事フォルダ/original.pdf` の形では親フォルダ名を使う
fn title_for(stem: &str, folder: &str) -> String {
    if stem.eq_ignore_ascii_case("original") {
        folder.to_string()
    } else {
        stem.to_string()
    }
}

fn dir_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// フォルダの表示名（aliases の鍵は NFC で引く = 実ディスクの名前は NFD のことがある）
fn folder_name(p: &Path, opts: &ScanOpts) -> String {
    let raw = dir_name(p);
    let key: String = raw.nfc().collect();
    opts.aliases.get(&key).cloned().unwrap_or(raw)
}

/// "2025-01" の形（4 桁-2 桁ちょうど）か
fn is_issue_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 7 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[5..].iter().all(u8::is_ascii_digit)
}

/// 子フォルダの「特集_◯◯」から号の表示名を合成（最大 2 特集、超過は「ほか」）
fn issue_display_name(issue: &str, children: &[ShelfFolder]) -> String {
    let feats: Vec<String> = children.iter().filter_map(|c| c.name.strip_prefix("特集_")).map(|t| t.replace('_', " ")).collect();
    if feats.is_empty() {
        return issue.to_string();
    }
    let mut t = feats[..feats.len().min(2)].join(" / ");
    if feats.len() > 2 {
        t.push_str(" ほか");
    }
    format!("{issue}  {t}")
}

/// 大文字小文字を無視したコードポイント順（和文はコードポイント順で実用十分）
fn cmp_ja(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

/// readdir で列挙する。読めないサブフォルダは飛ばして unreadable に数える（全体を止めない）。dir 自身が読めなければ Err
fn read_tree(dir: &Path, unreadable: &mut usize) -> std::io::Result<RawDir> {
    let mut out = RawDir::new(dir);
    // 名前は読めたが種類・stat が拒否された（x の無いフォルダ r-- の中など）= このフォルダを読めなかった側に 1 回数える
    let mut failed = false;
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            failed = true;
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if hidden_name(&name) {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            failed = true;
            continue;
        };
        if ft.is_dir() {
            match read_tree(&entry.path(), unreadable) {
                Ok(d) => {
                    out.dirs.insert(name, d);
                }
                Err(_) => *unreadable += 1,
            }
        } else if ft.is_file() && pdf_stem(&name).is_some() {
            match entry.metadata() {
                Ok(m) => {
                    let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                    out.pdfs.push(RawPdf { path: entry.path(), size: m.len(), mtime });
                }
                Err(_) => failed = true,
            }
        }
    }
    if failed {
        *unreadable += 1;
    }
    Ok(out)
}

/// find(1) の列挙（root 相対・NFC）から同じ RawDir を作る。find は `.` 始まりだけ prune するので `_` 始まりと pdf 以外はここで落とす
fn tree_of_found(root: &Path, found: &Found) -> RawDir {
    let mut top = RawDir::new(root);
    for e in &found.entries {
        let segs: Vec<&str> = e.rel.split('/').collect();
        let Some((file, dirs)) = segs.split_last() else { continue };
        if segs.iter().any(|s| hidden_name(s)) || pdf_stem(file).is_none() {
            continue;
        }
        let mut cur = &mut top;
        for d in dirs {
            let path = cur.path.join(d);
            cur = cur.dirs.entry((*d).to_string()).or_insert_with(|| RawDir::new(&path));
        }
        let path = cur.path.join(file);
        cur.pdfs.push(RawPdf { path, size: e.size.max(0) as u64, mtime: e.mtime_ns / 1_000_000_000 });
    }
    top
}

/// 列挙 → 木（aliases・題名・畳み込み・並び・号の題名）。PDF を 1 冊も含まない枝は None（pages/ の jpg、page_files/ 等）
fn fold(raw: RawDir, opts: &ScanOpts) -> Option<ShelfFolder> {
    let name = folder_name(&raw.path, opts);
    let mut children = Vec::new();
    let mut books: Vec<ShelfBook> = raw
        .pdfs
        .iter()
        .map(|p| {
            let file = dir_name(&p.path);
            ShelfBook {
                id: id_for(&p.path),
                title: title_for(pdf_stem(&file).unwrap_or(&file), &name),
                path: p.path.to_string_lossy().into_owned(),
                size: p.size,
                mtime: p.mtime,
                folder: name.clone(),
            }
        })
        .collect();
    for d in raw.dirs.into_values() {
        let Some(mut child) = fold(d, opts) else { continue };
        // 畳み込みは「PDF 1 個を直接含む末端フォルダ」1 段のみ。連鎖させると 1 記事だけの特集 / 号フォルダまで消えてしまう
        // （記事が 1 つだけの特集も普通にある）
        let direct = child.books.len() == 1 && Path::new(&child.books[0].path).parent() == Some(Path::new(&child.path));
        if opts.collapse_single && child.children.is_empty() && direct {
            // 記事フォルダ（original.pdf 1 個）→ 本として親の棚に置く
            if let Some(mut b) = child.books.pop() {
                b.folder = name.clone();
                books.push(b);
            }
        } else {
            children.push(child);
        }
    }
    children.sort_by(|a, b| cmp_ja(&a.name, &b.name));
    books.sort_by(|a, b| cmp_ja(&a.title, &b.title));
    let total = books.len() + children.iter().map(|c| c.total_books).sum::<usize>();
    if total == 0 {
        return None;
    }
    // 号フォルダの名前への特集の題名の合成は表示だけ（id はパス由来のまま。YYYY-MM の頭が保たれるので名前順も崩れない）
    let display = if opts.issue_titles && is_issue_name(&name) { issue_display_name(&name, &children) } else { name };
    Some(ShelfFolder {
        id: id_for(&raw.path),
        name: display,
        path: raw.path.to_string_lossy().into_owned(),
        children,
        books,
        total_books: total,
    })
}

/// 空の木（PDF が 1 冊も無い root）
fn empty(root: &Path, opts: &ScanOpts) -> ShelfFolder {
    ShelfFolder {
        id: id_for(root),
        name: folder_name(root, opts),
        path: root.to_string_lossy().into_owned(),
        children: vec![],
        books: vec![],
        total_books: 0,
    }
}

/// root 以下を走査する。root が読めず find(1) でも 1 冊も列挙できなければ Err。
/// 何も読めなかったのに読めないフォルダがある（Drive の TCC）ときだけ find(1) へ（wiki::scan::walk と同じ条件）
pub fn scan(root: &Path, opts: &ScanOpts) -> Result<Scan> {
    let mut unreadable = 0;
    let read = read_tree(root, &mut unreadable);
    let (tree, top_err) = match read {
        Ok(raw) => (fold(raw, opts), None),
        Err(e) => (None, Some(e)),
    };
    if let Some(tree) = tree {
        return Ok(Scan { tree, unreadable, used_find: false });
    }
    if top_err.is_some() || unreadable > 0 {
        let found = find(root);
        if let Some(tree) = fold(tree_of_found(root, &found), opts) {
            let left = if found.complete { 0 } else { found.error_dirs.len().max(1) };
            return Ok(Scan { tree, unreadable: left, used_find: true });
        }
    }
    if let Some(e) = top_err {
        return Err(CoreError::Io(std::io::Error::new(e.kind(), format!("本棚 {} を読めません: {e}", dir_name(root)))));
    }
    Ok(Scan { tree: empty(root, opts), unreadable, used_find: false })
}

/// id → (絶対パス, mtime) の表へ足す
pub fn collect_books(f: &ShelfFolder, out: &mut HashMap<String, (PathBuf, i64)>) {
    for b in &f.books {
        out.insert(b.id.clone(), (PathBuf::from(&b.path), b.mtime));
    }
    for c in &f.children {
        collect_books(c, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wiki::scan::Entry;
    use std::os::unix::fs::PermissionsExt;

    /// 試験の場（drop で消す。読めないフォルダの権限は呼び手が戻す）
    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl std::ops::Deref for Dir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    fn mktree(base: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("openref-shelf-scan-{base}-{}-{:x}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(&d).unwrap();
        Dir(std::fs::canonicalize(&d).unwrap())
    }

    fn no_aliases() -> &'static HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn plain() -> ScanOpts<'static> {
        ScanOpts { collapse_single: false, issue_titles: false, aliases: no_aliases() }
    }

    fn collapse() -> ScanOpts<'static> {
        ScanOpts { collapse_single: true, ..plain() }
    }

    fn put(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"%PDF-1.4").unwrap();
    }

    fn titles(f: &ShelfFolder, out: &mut Vec<String>) {
        out.extend(f.books.iter().map(|b| format!("{}/{}", f.name, b.title)));
        f.children.iter().for_each(|c| titles(c, out));
    }

    /// PDF のみ・`_` と `.` は入らない・PDF の無い枝は落ちる
    #[test]
    fn scans_pdfs_and_skips_hidden_and_empty_dirs() {
        let tmp = mktree("basic");
        std::fs::create_dir_all(tmp.join("a")).unwrap();
        std::fs::create_dir_all(tmp.join("b")).unwrap();
        put(&tmp.join("a/x.PDF"));
        std::fs::write(tmp.join("a/note.txt"), b"x").unwrap();
        std::fs::write(tmp.join("b/video.mp4"), b"x").unwrap();
        put(&tmp.join("_draft/secret.pdf"));
        put(&tmp.join(".hidden.pdf"));
        put(&tmp.join(".dir/in-hidden.pdf"));

        let s = scan(&tmp, &plain()).unwrap();
        assert_eq!(s.tree.total_books, 1);
        assert_eq!(s.tree.children.len(), 1);
        assert_eq!(s.tree.children[0].name, "a");
        assert_eq!(s.tree.children[0].books[0].title, "x");
        assert_eq!((s.unreadable, s.used_find), (0, false));
    }

    /// 雑誌の構造: 特集/記事フォルダ/original.pdf + pages/(jpg)・畳むと題名は記事フォルダ名・所属は特集
    #[test]
    fn collapse_single_folds_article_dirs_and_renames_original() {
        let tmp = mktree("collapse");
        let art1 = tmp.join("特集_X/A-1_記事");
        let art2 = tmp.join("特集_X/A-2_症例");
        std::fs::create_dir_all(art1.join("pages")).unwrap();
        put(&art1.join("original.pdf"));
        std::fs::write(art1.join("pages/page-01.jpg"), b"x").unwrap();
        std::fs::write(art1.join("data.json"), b"{}").unwrap();
        put(&art2.join("original.pdf"));
        put(&tmp.join("特集_X/目次.pdf"));

        let f = scan(&tmp, &collapse()).unwrap().tree;
        assert_eq!(f.total_books, 3);
        assert_eq!(f.children.len(), 1, "特集フォルダのみ（記事フォルダは畳まれる）");
        let feat = &f.children[0];
        assert!(feat.children.is_empty());
        let t: Vec<_> = feat.books.iter().map(|b| b.title.as_str()).collect();
        assert!(t.contains(&"A-1_記事"), "original → フォルダ名: {t:?}");
        assert!(t.contains(&"A-2_症例") && t.contains(&"目次"), "{t:?}");
        let b1 = feat.books.iter().find(|b| b.title.starts_with("A-1")).unwrap();
        assert_eq!(b1.folder, "特集_X", "畳まれた本の所属は特集フォルダ");
    }

    /// 畳まない既定は階層のまま
    #[test]
    fn collapse_disabled_keeps_hierarchy() {
        let tmp = mktree("nocollapse");
        put(&tmp.join("解剖学/入門/本.pdf"));
        let f = scan(&tmp, &plain()).unwrap().tree;
        assert_eq!(f.children[0].children[0].books.len(), 1);
    }

    /// 畳み込みは 1 段だけ（連鎖しない）: 1 記事だけの特集フォルダは残る
    #[test]
    fn collapse_does_not_chain() {
        let tmp = mktree("chain");
        put(&tmp.join("2024-03/特集_Y/A-1_記事/original.pdf"));
        let f = scan(&tmp, &collapse()).unwrap().tree;
        let issue = &f.children[0];
        assert_eq!(issue.name, "2024-03");
        assert_eq!(issue.children[0].name, "特集_Y");
        assert_eq!(issue.children[0].books[0].title, "A-1_記事");
    }

    /// aliases はフォルダ名と本の所属を置き換える（鍵は NFC で引く）
    #[test]
    fn aliases_rename_folders_and_book_folder_field() {
        let tmp = mktree("alias");
        put(&tmp.join("journal-a/x.pdf"));
        let nfd: String = "ジャーナル".nfd().collect();
        put(&tmp.join(format!("{nfd}/y.pdf")));
        let aliases = HashMap::from([("journal-a".to_string(), "A 雑誌".to_string()), ("ジャーナル".to_string(), "雑誌".to_string())]);
        let f = scan(&tmp, &ScanOpts { aliases: &aliases, ..plain() }).unwrap().tree;
        let names: Vec<_> = f.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A 雑誌", "雑誌"]);
        assert_eq!(f.children[0].books[0].folder, "A 雑誌");
    }

    /// 号の題名の合成（特集が無い号は年月のまま・合成しても年月の順・無効なら合成しない）
    #[test]
    fn issue_titles_compose_from_tokushu_children() {
        let tmp = mktree("issue");
        put(&tmp.join("2024-03/特集_Y2024/A-1_記事/original.pdf"));
        put(&tmp.join("2024-03/serial/B-1_連載/original.pdf"));
        put(&tmp.join("2016-05/case/C-1_症例/original.pdf"));
        let f = scan(&tmp, &ScanOpts { issue_titles: true, ..collapse() }).unwrap().tree;
        let names: Vec<_> = f.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"2016-05"), "{names:?}");
        assert!(names.contains(&"2024-03  Y2024"), "{names:?}");
        assert_eq!(names[0], "2016-05");
        let issue = f.children.iter().find(|c| c.name.starts_with("2024-03")).unwrap();
        assert!(issue.children.iter().any(|c| c.name == "特集_Y2024"), "1 記事だけの特集も消えない");
        let f2 = scan(&tmp, &collapse()).unwrap().tree;
        assert!(f2.children.iter().any(|c| c.name == "2024-03"));
    }

    /// symlink は辿らない（ファイルも・フォルダも）
    #[test]
    fn symlinks_are_not_followed() {
        let tmp = mktree("link");
        let out = mktree("link-out");
        put(&out.join("o.pdf"));
        put(&tmp.join("in/a.pdf"));
        std::os::unix::fs::symlink(out.join("o.pdf"), tmp.join("in/link.pdf")).unwrap();
        std::os::unix::fs::symlink(&*out, tmp.join("linkdir")).unwrap();
        let s = scan(&tmp, &plain()).unwrap();
        assert_eq!(s.tree.total_books, 1);
        let mut t = Vec::new();
        titles(&s.tree, &mut t);
        assert_eq!(t, ["in/a"]);
    }

    /// 読めないサブフォルダは飛ばして件数を返す・root が読めなければ Err（find でも 0 件）・無い root も Err・空の root は 0 冊で Ok
    #[test]
    fn unreadable_subfolder_is_skipped_and_counted() {
        let tmp = mktree("perm");
        put(&tmp.join("ok/a.pdf"));
        put(&tmp.join("locked/deep/b.pdf"));
        put(&tmp.join("noexec/c.pdf"));
        std::fs::set_permissions(tmp.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::set_permissions(tmp.join("noexec"), std::fs::Permissions::from_mode(0o400)).unwrap();
        let s = scan(&tmp, &plain());
        std::fs::set_permissions(tmp.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(tmp.join("noexec"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = s.unwrap();
        assert_eq!(s.tree.total_books, 1);
        assert_eq!(s.unreadable, 2, "{s:?}");
        assert!(!s.used_find);

        let gone = tmp.join("nope");
        let e = scan(&gone, &plain()).unwrap_err().to_string();
        assert!(e.contains("本棚 nope を読めません"), "{e}");
        let root = mktree("perm-root");
        put(&root.join("a.pdf"));
        std::fs::set_permissions(&*root, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = scan(&root, &plain());
        std::fs::set_permissions(&*root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err(), "{r:?}");
        let blank = mktree("blank");
        let s = scan(&blank, &plain()).unwrap();
        assert_eq!((s.tree.total_books, s.unreadable), (0, 0));
        assert_eq!(s.tree.path, blank.to_string_lossy());
    }

    /// find(1) の列挙からも同じ木になる（題名・畳み込み・隠す規則・mtime の秒）。`_` は find が prune しないのでここで落ちる
    #[test]
    fn find_listing_builds_the_same_tree() {
        let tmp = mktree("find");
        put(&tmp.join("特集_X/A-1_記事/original.pdf"));
        std::fs::write(tmp.join("特集_X/A-1_記事/page.jpg"), b"x").unwrap();
        put(&tmp.join("特集_X/目次.pdf"));
        put(&tmp.join("本.PDF"));
        put(&tmp.join("_draft/secret.pdf"));
        put(&tmp.join(".hid/x.pdf"));
        std::fs::create_dir_all(tmp.join("img")).unwrap();
        std::fs::write(tmp.join("img/a.png"), b"x").unwrap();
        for opts in [plain(), collapse()] {
            let by_read = scan(&tmp, &opts).unwrap().tree;
            let found = find(&tmp);
            assert!(found.complete);
            let by_find = fold(tree_of_found(&tmp, &found), &opts).unwrap();
            assert_eq!(by_find, by_read);
        }
        // find の項目の形（root 相対・区画に隠す名前）だけで見る: 空の結果は空の木（fold = None）
        let none = Found { entries: vec![], complete: true, error_dirs: vec![] };
        assert!(fold(tree_of_found(&tmp, &none), &plain()).is_none());
        let only_hidden = Found { entries: vec![Entry { rel: "_a/b.pdf".into(), mtime_ns: 1, size: 1 }], complete: true, error_dirs: vec![] };
        assert!(fold(tree_of_found(&tmp, &only_hidden), &plain()).is_none());
    }

    /// root の中が全部読めない（readdir は通るが中のフォルダは全部拒否）= 空の Ok（読めなかった件数つき）。find も読めないので used_find は立たない
    #[test]
    fn nothing_readable_reports_the_count() {
        let tmp = mktree("allgone");
        put(&tmp.join("locked/a.pdf"));
        std::fs::set_permissions(tmp.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let s = scan(&tmp, &plain());
        std::fs::set_permissions(tmp.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = s.unwrap();
        assert_eq!((s.tree.total_books, s.unreadable, s.used_find), (0, 1, false));
    }

    #[test]
    fn pdf_stem_rules() {
        assert_eq!(pdf_stem("a.pdf"), Some("a"));
        assert_eq!(pdf_stem("a.PdF"), Some("a"));
        assert_eq!(pdf_stem("a..pdf"), Some("a"));
        assert_eq!(pdf_stem("a.b.pdf"), Some("a.b"));
        assert_eq!(pdf_stem("a.pdf.txt"), None);
        assert_eq!(pdf_stem("pdf"), None);
        assert_eq!(pdf_stem("教科書"), None);
    }

    /// 表: 全部の本が id で引ける
    #[test]
    fn collect_books_covers_the_tree() {
        let tmp = mktree("collect");
        put(&tmp.join("a.pdf"));
        put(&tmp.join("x/y/b.pdf"));
        let f = scan(&tmp, &plain()).unwrap().tree;
        let mut m = HashMap::new();
        collect_books(&f, &mut m);
        assert_eq!(m.len(), 2);
        let b = &f.books[0];
        assert_eq!(m[&b.id], (PathBuf::from(&b.path), b.mtime));
    }
}
