//! 本棚（表示名 = 本棚・技術名 = shelf）の core 側: フォルダの走査（scan）・表紙（thumbs）・登録表の command（commands）・
//! 履歴 `shelf_workspaces`・プロセス内の走査結果（Cache）。
//!
//! 正はディスクのフォルダ。本棚の root に何も書かない（表紙は `<app_dir>/shelf-thumbs/`・設定は DB の settings）。索引（DB）は持たない。
//! 設定 `shelf_workspaces` の形 = `{ "<canonical な root>": { "last_used", "collapse_single", "aliases"?, "issue_titles"?, "name"? } }`。
//! 読み書きの純関数は `wiki::workspaces` を共用する（鍵を知らない）。鍵を足すのは GUI の開く / 切替だけ（agent が任意のフォルダを走査・登録して回らない）

pub mod commands;
pub mod scan;
pub mod thumbs;

use crate::wiki::{self, paths, workspaces};
use crate::{CoreError, Db, Result};
use scan::{ScanOpts, ShelfFolder};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use unicode_normalization::UnicodeNormalization;

pub use workspaces::Map;

/// settings の鍵
pub const KEY: &str = "shelf_workspaces";
/// 履歴の上限
pub const MAX: usize = workspaces::MAX;

fn bad(msg: impl Into<String>) -> CoreError {
    CoreError::BadInput(msg.into())
}

/// パスから安定した id（sha256 の先頭 16 hex）。settings と command の鍵・表紙のファイル名に使う。
/// NFC に揃えてから（readdir は実ディスクの綴り・find の fallback は NFC = 同じ本が別の id にならない）
pub fn id_for(path: &Path) -> String {
    let s: String = path.to_string_lossy().nfc().collect();
    Sha256::digest(s.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// `shelf_workspaces` の 1 項目の表示の設定
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Opts {
    pub collapse_single: bool,
    pub issue_titles: bool,
    pub aliases: HashMap<String, String>,
    pub name: Option<String>,
}

impl Opts {
    /// 項目から読む（無い・型が違う = 既定。書くのは `commands::set_opts` だけで型は検査済み）
    pub fn of(v: Option<&Value>) -> Opts {
        let flag = |k: &str| v.and_then(|o| o.get(k)).and_then(Value::as_bool).unwrap_or(false);
        let aliases = v.and_then(|o| o.get("aliases")).and_then(Value::as_object).map(|m| {
            m.iter().filter_map(|(k, x)| Some((k.nfc().collect::<String>(), x.as_str()?.to_string()))).collect()
        });
        Opts {
            collapse_single: flag("collapse_single"),
            issue_titles: flag("issue_titles"),
            aliases: aliases.unwrap_or_default(),
            name: v.and_then(|o| o.get("name")).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        }
    }

    fn scan_opts(&self) -> ScanOpts<'_> {
        ScanOpts { collapse_single: self.collapse_single, issue_titles: self.issue_titles, aliases: &self.aliases }
    }

    /// 本棚の表示名（`name`・無ければフォルダ名）
    pub fn display_name(&self, root: &Path) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.to_string_lossy().into_owned()))
    }
}

/// 登録表の shelf_* と GUI の包みの入口（Db を要しない = 設定を読んだら Db を手放して走査する）
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub app_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub workspaces: Map,
}

impl Ctx {
    /// 設定 `shelf_workspaces` と `db.app_dir()`・環境変数 HOME。壊れた設定は Err（黙って空にしない・退避は GUI の仕事 = wiki::workspaces::parse と同じ）
    pub fn from_db(db: &Db) -> Result<Ctx> {
        Ok(Ctx {
            app_dir: db.app_dir(),
            home: std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from),
            workspaces: workspaces::parse_for(KEY, db.setting(KEY)?.as_deref())?,
        })
    }

    /// root が履歴（`shelf_workspaces` の鍵）と文字列で等しいか。canonicalize しない = symlink・末尾 `/` 等の生の綴りと空文字は拒否
    /// （agent が任意のフォルダを走査して回らない = wiki::Ctx::pin と同型）
    pub fn known(&self, root: &str) -> Result<()> {
        if self.workspaces.contains_key(root) {
            Ok(())
        } else {
            Err(bad(format!("履歴に無い本棚です: {root}（root には shelf_status の root をそのまま渡す）")))
        }
    }

    /// 履歴にある root の canonical（check_root を通る物だけ = 履歴に載った後に場所の規則に落ちたフォルダを走査しない）
    pub fn ready(&self, root: &str) -> Result<PathBuf> {
        self.known(root)?;
        check_root(Path::new(root), self)
    }

    /// root の表示の設定（履歴に無ければ既定）
    pub fn opts(&self, root: &str) -> Opts {
        Opts::of(self.workspaces.get(root))
    }
}

/// 本棚の root として使えるかの検査（wiki::check_root と同じ規則 = asset scope に足す場所の規則）→ canonical。文言だけ本棚に
pub fn check_root(path: &Path, ctx: &Ctx) -> Result<PathBuf> {
    let home = ctx.home.as_deref().ok_or_else(|| bad("HOME が未設定です"))?;
    wiki::check_root(path, home, ctx.app_dir.as_deref()).map_err(|e| match e {
        CoreError::BadInput(m) => bad(m.replacen("ワークスペースのフォルダに", "本棚のフォルダに", 1)),
        e => e,
    })
}

/// 履歴の last_used を now に（無ければ作る・他の項目は保つ）。上限で落ちた root を古い順に返す
pub fn touch(m: &mut Map, root: &str, now: i64) -> Vec<String> {
    let evicted = workspaces::touch(m, root, now, MAX);
    if let Some(o) = m.get_mut(root).and_then(Value::as_object_mut) {
        o.entry("collapse_single").or_insert(json!(false));
    }
    evicted
}

/// 設定 `shelf_workspaces` の 1 回の読み書き（BEGIN IMMEDIATE = GUI と CLI / MCP の別のプロセスの書込を混ぜない）。
/// 読む → f → 中身が変わっていれば書く。壊れた設定は Err（f を呼ばない）・f が Err なら書かない
pub fn with_workspaces<R>(db: &Db, f: impl FnOnce(&mut Map) -> Result<R>) -> Result<R> {
    let tx = rusqlite::Transaction::new_unchecked(&db.conn, rusqlite::TransactionBehavior::Immediate)?;
    let raw = db.setting(KEY)?;
    let before = workspaces::parse_for(KEY, raw.as_deref())?;
    let mut m = before.clone();
    let r = f(&mut m)?;
    if m != before {
        db.set_setting(KEY, &workspaces::serialize(&m))?;
    }
    tx.commit()?;
    Ok(r)
}

/// 1 つの root の走査結果（木 + id → (絶対パス, mtime) の表）
#[derive(Clone, Debug, PartialEq)]
pub struct Scanned {
    /// 走査した実体（canonical）
    pub root: PathBuf,
    pub tree: ShelfFolder,
    /// 読めなかったフォルダの数
    pub unreadable: usize,
    pub used_find: bool,
    pub books: HashMap<String, (PathBuf, i64)>,
    /// この結果を作ったときの設定（変わったら読み直す）
    pub opts: Opts,
}

impl Scanned {
    /// canon（check_root の戻り）を走査する。木の名前は本棚の表示名
    pub fn at(canon: &Path, opts: &Opts) -> Result<Scanned> {
        let s = scan::scan(canon, &opts.scan_opts())?;
        let mut tree = s.tree;
        tree.name = opts.display_name(canon);
        let mut books = HashMap::new();
        scan::collect_books(&tree, &mut books);
        Ok(Scanned { root: canon.to_path_buf(), tree, unreadable: s.unreadable, used_find: s.used_find, books, opts: opts.clone() })
    }

    /// 履歴にある root を走査する
    pub fn of(ctx: &Ctx, root: &str) -> Result<Scanned> {
        Scanned::at(&ctx.ready(root)?, &ctx.opts(root))
    }
}

/// プロセス内の走査結果（root の文字列 = 履歴の鍵 → Scanned）。索引の代わり = 表紙・Finder・外部で開く・読み手の `{root, id}` はこの表で引く
#[derive(Default)]
pub struct Cache(Mutex<HashMap<String, Arc<Scanned>>>);

impl Cache {
    fn map(&self) -> MutexGuard<'_, HashMap<String, Arc<Scanned>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn put(&self, root: &str, s: Scanned) -> Arc<Scanned> {
        let s = Arc::new(s);
        self.map().insert(root.to_string(), s.clone());
        s
    }

    fn get(&self, root: &str) -> Option<Arc<Scanned>> {
        self.map().get(root).cloned()
    }

    /// 走査し直して表を置き換える
    pub fn rescan(&self, ctx: &Ctx, root: &str) -> Result<Arc<Scanned>> {
        Ok(self.put(root, Scanned::of(ctx, root)?))
    }

    /// 今の設定で走査した結果（表に無い・設定が変わっていれば 1 回走査する）
    pub fn get_or_scan(&self, ctx: &Ctx, root: &str) -> Result<Arc<Scanned>> {
        ctx.known(root)?;
        match self.get(root).filter(|s| s.opts == ctx.opts(root)) {
            Some(s) => Ok(s),
            None => self.rescan(ctx, root),
        }
    }

    /// id の (絶対パス, mtime)。表に無ければ 1 回だけ走査し直す
    pub fn book(&self, ctx: &Ctx, root: &str, id: &str) -> Result<(PathBuf, i64)> {
        ctx.known(root)?;
        if let Some(b) = self.get(root).and_then(|s| s.books.get(id).cloned()) {
            return Ok(b);
        }
        self.rescan(ctx, root)?.books.get(id).cloned().ok_or_else(|| CoreError::NotFound(format!("本棚に無い本です: {id}")))
    }

    /// 表から外す（外した結果を返す）
    pub fn forget(&self, root: &str) -> Option<Arc<Scanned>> {
        self.map().remove(root)
    }
}

/// 本の実体: `{root, id}` → 表で引いて root の中の通常の PDF か検査 → (canonical, mtime)。
/// 検査は wiki::paths と同じ（字面の外・`..`・`.` 始まり → canonicalize → 実体の外（symlink）・隠し・種類）+ 実体の拡張子が pdf。
/// 走査の後に symlink へ差し替えられても外のファイルを開かない。文にフルパスを出さない（Drive のパスはアカウント名を含む）
pub fn book_path(ctx: &Ctx, cache: &Cache, root: &str, id: &str) -> Result<(PathBuf, i64)> {
    let canon_root = ctx.ready(root)?;
    let (path, mtime) = cache.book(ctx, root, id)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (_, canon) = paths::resolve_existing(&canon_root, &path, paths::Opts::default()).map_err(|r| {
        use paths::Reject::*;
        let why = match r {
            Missing(_) => "見つかりません（移動か削除された可能性）",
            Escapes | Outside | DotDot => "本棚の外です（symlink）",
            Root | Hidden(_) | DotStart(_) => "隠しフォルダの中です",
            NotFile(_) => "通常のファイルではありません",
        };
        bad(format!("{name} を開けません: {why}"))
    })?;
    if !canon.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return Err(bad(format!("{name} を開けません: 実体が PDF ではありません")));
    }
    Ok((canon, mtime))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 試験の場（home の下に教科書の root・app_dir）
    pub(crate) struct Fix {
        pub base: PathBuf,
        pub root: String,
        pub ctx: Ctx,
    }
    impl Drop for Fix {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    pub(crate) fn fix(tag: &str) -> Fix {
        let base = std::env::temp_dir().join(format!("openref-shelf-{tag}-{}-{:x}", std::process::id(), fastrand::u64(..)));
        let root = base.join("home/教科書");
        std::fs::create_dir_all(root.join("解剖学")).unwrap();
        std::fs::create_dir_all(base.join("home/app")).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let root = base.join("home/教科書");
        std::fs::write(root.join("解剖学/a.pdf"), "%PDF-1.4").unwrap();
        std::fs::write(root.join("b.pdf"), "%PDF-1.4").unwrap();
        let key = root.to_string_lossy().into_owned();
        let mut workspaces = Map::new();
        touch(&mut workspaces, &key, 100);
        let ctx = Ctx { app_dir: Some(base.join("home/app")), home: Some(base.join("home")), workspaces };
        Fix { base, root: key, ctx }
    }

    /// id: sha256 の先頭 16 hex・決定的・NFD と NFC で同じ・別のパスは別
    #[test]
    fn id_is_stable_and_hex16() {
        let id = id_for(Path::new("/tmp/教科書/foo.pdf"));
        assert_eq!(id.len(), 16);
        assert_eq!(id, id_for(Path::new("/tmp/教科書/foo.pdf")));
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(id, format!("{:x}", Sha256::digest("/tmp/教科書/foo.pdf".as_bytes()))[..16]);
        let nfd: String = "/tmp/ジャーナル/x.pdf".nfd().collect();
        assert_eq!(id_for(Path::new(&nfd)), id_for(Path::new("/tmp/ジャーナル/x.pdf")));
        assert_ne!(id, id_for(Path::new("/tmp/教科書/bar.pdf")));
    }

    /// known: 履歴の鍵と文字列で等しい物だけ（空・空白・末尾 `/`・symlink の綴り・履歴に無い = BadInput）
    #[test]
    fn known_accepts_only_history_keys() {
        let f = fix("known");
        assert!(f.ctx.known(&f.root).is_ok());
        let link = f.base.join("home/link");
        std::os::unix::fs::symlink(&f.root, &link).unwrap();
        for bad_root in ["", " ", "/nope", &format!("{}/", f.root), &format!(" {}", f.root), &*link.to_string_lossy(), &format!("{}/解剖学", f.root)] {
            let e = f.ctx.known(bad_root).unwrap_err();
            assert!(matches!(e, CoreError::BadInput(_)), "{bad_root:?}: {e:?}");
            assert!(e.to_string().contains("履歴に無い本棚"), "{e}");
        }
    }

    /// ready: 履歴にあっても check_root に落ちれば Err・文言は本棚・HOME が無ければ Err
    #[test]
    fn ready_runs_the_place_rules() {
        let f = fix("ready");
        assert_eq!(f.ctx.ready(&f.root).unwrap(), Path::new(&f.root));
        let mut ctx = f.ctx.clone();
        let hidden = f.base.join("home/.secret");
        std::fs::create_dir_all(&hidden).unwrap();
        let hidden = hidden.to_string_lossy().into_owned();
        touch(&mut ctx.workspaces, &hidden, 1);
        let e = ctx.ready(&hidden).unwrap_err().to_string();
        assert!(e.contains("本棚のフォルダに") && e.contains("隠しフォルダ") && !e.contains("ワークスペースのフォルダに"), "{e}");
        let gone = f.base.join("home/gone").to_string_lossy().into_owned();
        touch(&mut ctx.workspaces, &gone, 1);
        assert!(ctx.ready(&gone).unwrap_err().to_string().contains("見つかりません"));
        ctx.home = None;
        assert!(ctx.ready(&f.root).unwrap_err().to_string().contains("HOME"));
        // ホームそのもの・app_dir も拒否（wiki と同じ規則）
        let mut c = f.ctx.clone();
        let home = f.base.join("home").to_string_lossy().into_owned();
        touch(&mut c.workspaces, &home, 1);
        assert!(c.ready(&home).is_err());
    }

    /// touch: 新しい root は collapse_single: false を持つ・既存の値と未知の項目は保つ・上限 10 の古い順に落ちる
    #[test]
    fn touch_shapes_the_entry() {
        let mut m = Map::new();
        assert!(touch(&mut m, "/a", 1).is_empty());
        assert_eq!(m["/a"], json!({"last_used": 1, "collapse_single": false}));
        m.get_mut("/a").unwrap()["collapse_single"] = json!(true);
        m.get_mut("/a").unwrap()["future"] = json!({"x": 1});
        touch(&mut m, "/a", 2);
        assert_eq!(m["/a"], json!({"last_used": 2, "collapse_single": true, "future": {"x": 1}}));
        for i in 0..MAX {
            touch(&mut m, &format!("/w{i}"), 10 + i as i64);
        }
        assert_eq!(m.len(), MAX);
        assert!(!m.contains_key("/a"), "最も古い /a が落ちる");
        assert_eq!(touch(&mut m, "/new", 100), vec!["/w0".to_string()]);
        assert_eq!(MAX, 10);
    }

    /// Opts: 既定・読む・型が違えば既定・空白の name は無し・鍵は NFC に
    #[test]
    fn opts_of_reads_entry() {
        assert_eq!(Opts::of(None), Opts::default());
        let nfd: String = "ジャーナル".nfd().collect();
        let v = json!({"collapse_single": true, "issue_titles": true, "aliases": {"a": "A", "b": 3, nfd: "J"}, "name": " 教科書 "});
        let o = Opts::of(Some(&v));
        assert!(o.collapse_single && o.issue_titles);
        assert_eq!(o.aliases.len(), 2);
        assert_eq!(o.aliases["ジャーナル"], "J");
        assert_eq!(o.name.as_deref(), Some("教科書"));
        assert_eq!(o.display_name(Path::new("/x/y")), "教科書");
        let o = Opts::of(Some(&json!({"collapse_single": "yes", "aliases": 1, "name": "  "})));
        assert_eq!(o, Opts::default());
        assert_eq!(o.display_name(Path::new("/x/y")), "y");
    }

    /// with_workspaces: 変わったときだけ書く・f の Err は書かない・壊れた設定は f を呼ばず Err で壊れた値を残す・別の接続が書いていても読み直す
    #[test]
    fn with_workspaces_reads_and_writes_once() {
        let db = Db::open_in_memory().unwrap();
        with_workspaces(&db, |m| {
            touch(m, "/a", 5);
            Ok(())
        })
        .unwrap();
        assert!(db.setting(KEY).unwrap().unwrap().contains("\"/a\""));
        // 整形された値は中身が同じなら書き直さない
        db.set_setting(KEY, "{ \"/a\" : {\"last_used\": 5} }").unwrap();
        with_workspaces(&db, |_| Ok(())).unwrap();
        assert_eq!(db.setting(KEY).unwrap().as_deref(), Some("{ \"/a\" : {\"last_used\": 5} }"));
        let r: Result<()> = with_workspaces(&db, |m| {
            touch(m, "/b", 6);
            Err(bad("x"))
        });
        assert!(r.is_err());
        assert!(!db.setting(KEY).unwrap().unwrap().contains("/b"));
        // 次の呼び出しが通る = 失敗で transaction を開いたままにしない
        with_workspaces(&db, |_| Ok(())).unwrap();
        db.set_setting(KEY, "not json").unwrap();
        let e = with_workspaces(&db, |_| -> Result<()> { panic!("呼ばない") }).unwrap_err().to_string();
        assert!(e.contains("設定 shelf_workspaces を解釈できません"), "{e}");
        assert_eq!(db.setting(KEY).unwrap().as_deref(), Some("not json"));
        assert!(Ctx::from_db(&db).is_err(), "壊れた設定を黙って空にしない");
    }

    /// Scanned: 木の名前 = 本棚の表示名・表に全部の本・設定を持つ
    #[test]
    fn scanned_names_the_tree() {
        let f = fix("scanned");
        let s = Scanned::of(&f.ctx, &f.root).unwrap();
        assert_eq!(s.tree.name, "教科書");
        assert_eq!((s.tree.total_books, s.books.len()), (2, 2));
        assert_eq!(s.root, Path::new(&f.root));
        let mut ctx = f.ctx.clone();
        ctx.workspaces.get_mut(&f.root).unwrap()["name"] = json!("本棚 A");
        assert_eq!(Scanned::of(&ctx, &f.root).unwrap().tree.name, "本棚 A");
        assert!(Scanned::of(&f.ctx, "/nope").unwrap_err().to_string().contains("履歴に無い本棚"));
    }

    /// Cache: 1 回だけ走査する（2 回目は同じ Arc）・設定が変われば読み直す・rescan は置き換える・forget で消える
    #[test]
    fn cache_scans_once_and_follows_opts() {
        let f = fix("cache");
        let c = Cache::default();
        let a = c.get_or_scan(&f.ctx, &f.root).unwrap();
        let b = c.get_or_scan(&f.ctx, &f.root).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let mut ctx = f.ctx.clone();
        ctx.workspaces.get_mut(&f.root).unwrap()["collapse_single"] = json!(true);
        let d = c.get_or_scan(&ctx, &f.root).unwrap();
        assert!(!Arc::ptr_eq(&a, &d), "設定が変わった");
        assert!(d.tree.children.is_empty() && d.tree.books.len() == 2, "畳まれた: {:?}", d.tree);
        let e = c.rescan(&ctx, &f.root).unwrap();
        assert!(!Arc::ptr_eq(&d, &e));
        assert!(Arc::ptr_eq(&e, &c.get_or_scan(&ctx, &f.root).unwrap()));
        assert!(c.forget(&f.root).is_some());
        assert!(c.forget(&f.root).is_none());
        assert!(c.get_or_scan(&f.ctx, "/nope").is_err());
    }

    /// book: 表にある = 走査しない（ファイルを足しても表は同じ Arc のまま）・表に無い = 1 回だけ走査し直して引く・それでも無い = NotFound
    #[test]
    fn cache_book_rescans_once_on_miss() {
        let f = fix("book");
        let c = Cache::default();
        let first = c.rescan(&f.ctx, &f.root).unwrap();
        let (id, want) = first.books.iter().next().map(|(k, v)| (k.clone(), v.clone())).unwrap();
        assert_eq!(c.book(&f.ctx, &f.root, &id).unwrap(), want);
        assert!(Arc::ptr_eq(&first, &c.get(&f.root).unwrap()), "表にあれば走査しない");
        let added = Path::new(&f.root).join("c.pdf");
        std::fs::write(&added, "%PDF-1.4").unwrap();
        let (p, _) = c.book(&f.ctx, &f.root, &id_for(&added)).unwrap();
        assert_eq!(p, added);
        assert!(!Arc::ptr_eq(&first, &c.get(&f.root).unwrap()), "無ければ走査し直す");
        let e = c.book(&f.ctx, &f.root, "0000000000000000").unwrap_err();
        assert!(matches!(e, CoreError::NotFound(_)), "{e:?}");
        assert!(c.book(&f.ctx, "/nope", &id).is_err());
    }

    /// book_path: 中の PDF = canonical・走査の後に消えた / symlink で外へ・隠しへ・PDF でない物へ差し替え = Err（文にフルパスを出さない）
    #[test]
    fn book_path_checks_the_real_file() {
        let f = fix("path");
        let c = Cache::default();
        let root = Path::new(&f.root);
        let a = root.join("解剖学/a.pdf");
        let b = root.join("b.pdf");
        let extra = root.join("d.pdf");
        let hid = root.join("e.pdf");
        let pdfin = root.join("f.pdf");
        for p in [&extra, &hid, &pdfin] {
            std::fs::write(p, "%PDF-1.4").unwrap();
        }
        c.rescan(&f.ctx, &f.root).unwrap();
        let (p, m) = book_path(&f.ctx, &c, &f.root, &id_for(&a)).unwrap();
        assert_eq!(p, a);
        assert!(m > 0);
        let out = f.base.join("outside.pdf");
        std::fs::write(&out, "%PDF-1.4").unwrap();
        let script = root.join("run.sh");
        std::fs::write(&script, "#!/bin/sh").unwrap();
        std::fs::create_dir_all(root.join(".hid")).unwrap();
        std::fs::write(root.join(".hid/x.pdf"), "%PDF-1.4").unwrap();
        let swap = |p: &Path, to: &Path| {
            std::fs::remove_file(p).unwrap();
            std::os::unix::fs::symlink(to, p).unwrap();
        };
        swap(&b, &out);
        swap(&extra, &script);
        swap(&hid, &root.join(".hid/x.pdf"));
        std::fs::remove_file(&a).unwrap();
        // フォルダへ差し替えられた
        swap(&pdfin, &root.join("解剖学"));
        let msg = |p: &Path| book_path(&f.ctx, &c, &f.root, &id_for(p)).unwrap_err().to_string();
        assert!(msg(&a).contains("見つかりません"), "{}", msg(&a));
        assert!(msg(&b).contains("本棚の外"), "{}", msg(&b));
        assert!(msg(&extra).contains("PDF ではありません"), "{}", msg(&extra));
        assert!(msg(&hid).contains("隠しフォルダ"), "{}", msg(&hid));
        assert!(msg(&pdfin).contains("通常のファイルではありません"), "{}", msg(&pdfin));
        for p in [&a, &b, &extra, &hid, &pdfin] {
            assert!(!msg(p).contains(&*f.base.to_string_lossy()), "{}", msg(p));
        }
        assert!(book_path(&f.ctx, &c, "/nope", "x").is_err());
    }
}
