//! 本棚の GUI の本体（Tauri の AppHandle に依存する段取り）: 開く / 切替 / 履歴から外す・asset scope・表紙・Finder / 外部で開く。
//! Tauri command の包みと generate_handler!・setup は main.rs（parity test が main.rs だけを走査する）。ここは包みが呼ぶ本体。
//! 設定 shelf_workspaces の鍵を足すのはここだけ（agent が任意のフォルダを本棚にしない）。root の下には何も書かない。
//!
//! asset scope は wiki_allow_dir を通さない: wiki の WikiAllowed に Root として記録すると wiki_root_stale が立ち、OpenNote の
//! 「再起動まで残る」の一言を誤って出す。allow_directory は取り消せない（wiki と同じ）が、本棚は読むだけなので知らせない。
//! toast: 開く / 切替 / 外すの失敗と外した結果は Rust が出す（画面は Err を受けるだけ = wiki_switch_root と同じ）。
//! Db のロックは設定の読み書きの間だけ。走査・表紙・open はロックの外

use crate::wiki_gui::{tr, ui_en, wiki_broken_key, wiki_now, wiki_pick_folder_titled, wiki_toast};
use crate::{drop_dir_part, err, lock, open_file, AppState};
use openref_core::shelf::{self, thumbs};
use openref_core::wiki::workspaces;
use openref_core::{CoreError, Db};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Manager;

/// 走査結果の表と asset scope の記録（プロセスに 1 つ）
#[derive(Default)]
pub(crate) struct ShelfRuntime {
    pub(crate) cache: shelf::Cache,
    /// このプロセスで asset scope に足したフォルダ（同じフォルダを 2 度足さない）
    allowed: Mutex<HashSet<PathBuf>>,
}

/// core の失敗 → 画面の文（「入力を解釈できません」等の前置きを重ねない）
pub(crate) fn shelf_err(e: CoreError) -> String {
    match e {
        CoreError::BadInput(m) | CoreError::NotFound(m) => m,
        CoreError::Io(e) => err(e),
        e => err(e),
    }
}

/// asset scope の足し方（試験は差し替える）
type Allow<'a> = &'a dyn Fn(&Path) -> Result<(), String>;

/// 解釈できない設定 shelf_workspaces を `shelf_workspaces.broken-<UTC>` へ退避して消す。戻り値 = 退避先の鍵
fn shelf_heal(db: &Db, now: i64, en: bool) -> Result<Option<String>, String> {
    let raw = db.setting(shelf::KEY).map_err(err)?;
    let Err(why) = workspaces::parse_for(shelf::KEY, raw.as_deref()) else {
        return Ok(None);
    };
    let to = wiki_broken_key(shelf::KEY, now);
    db.set_setting(&to, raw.as_deref().unwrap_or("")).map_err(|e| {
        let msg = tr(
            en,
            "解釈できない設定 shelf_workspaces を退避できません（変更しません）",
            "Could not set aside the unreadable shelf_workspaces setting (nothing changed)",
        );
        format!("{msg}: {why}: {e}")
    })?;
    db.delete_setting(shelf::KEY).map_err(err)?;
    eprintln!("openref-app: shelf: 設定 {} を解釈できないので {to} に退避しました: {why}", shelf::KEY);
    Ok(Some(to))
}

/// 設定を読む（壊れていれば先に退避）。戻り値 = (Ctx, 退避先の鍵)。ロックはこの間だけ
pub(crate) fn shelf_ctx_db(
    db: &Mutex<Db>,
    now: i64,
    en: bool,
) -> Result<(shelf::Ctx, Option<String>), String> {
    let g = lock(db)?;
    let set_aside = shelf_heal(&g, now, en)?;
    Ok((shelf::Ctx::from_db(&g).map_err(shelf_err)?, set_aside))
}

/// shelf_ctx_db を AppState の DB で（退避したら Rust が知らせる）
pub(crate) fn shelf_ctx(app: &tauri::AppHandle) -> Result<shelf::Ctx, String> {
    let en = ui_en(app);
    let st = app.state::<AppState>();
    let (ctx, set_aside) = shelf_ctx_db(&st.db, wiki_now(), en)?;
    if set_aside.is_some() {
        wiki_toast(app, en, false, "解釈できない設定を退避しました", "Set aside an unreadable setting", set_aside);
    }
    Ok(ctx)
}

/// root の走査結果の表と表紙を消す（履歴は呼び手が）。戻り値 = (消した枚数, 本が分からなかった = 表に無く走査もできない = 表紙が残る)
fn shelf_drop_thumbs(ctx: &shelf::Ctx, cache: &shelf::Cache, root: &str) -> (usize, bool) {
    let scanned = match cache.forget(root) {
        Some(s) => Some(s),
        None => shelf::Scanned::of(ctx, root).ok().map(std::sync::Arc::new),
    };
    let (Some(s), Some(app_dir)) = (scanned, ctx.app_dir.as_deref()) else {
        return (0, true);
    };
    let ids: HashSet<&str> = s.books.keys().map(String::as_str).collect();
    (thumbs::remove_for(&thumbs::dir(app_dir), &ids), false)
}

/// 開く・切替の共通の段: 走査（読めないフォルダは履歴に残さない）→ 履歴の last_used → asset scope → 表へ →
/// 上限で落ちた root の表と表紙を消す（ctx は touch の前の設定 = 落ちた root をまだ知っている）
fn shelf_enter(
    db: &Mutex<Db>,
    cache: &shelf::Cache,
    ctx: &shelf::Ctx,
    root: &str,
    canon: &Path,
    now: i64,
    allow: Allow,
) -> Result<(), String> {
    let s = shelf::Scanned::at(canon, &ctx.opts(root)).map_err(shelf_err)?;
    let evicted = {
        let g = lock(db)?;
        shelf::with_workspaces(&g, |m| Ok(shelf::touch(m, root, now))).map_err(shelf_err)?
    }; // ここでロックを外す
    allow(canon)?;
    cache.put(root, s);
    for r in &evicted {
        shelf_drop_thumbs(ctx, cache, r);
    }
    Ok(())
}

/// フォルダを本棚として開く（AppHandle 無し）: canonical（場所の規則は wiki::check_root）→ shelf_enter。戻り値 = 履歴の鍵 = canonical な root
pub(crate) fn shelf_open_steps(
    db: &Mutex<Db>,
    cache: &shelf::Cache,
    ctx: &shelf::Ctx,
    given: &Path,
    now: i64,
    allow: Allow,
) -> Result<String, String> {
    let canon = shelf::check_root(given, ctx).map_err(shelf_err)?;
    let root = canon.to_string_lossy().into_owned();
    shelf_enter(db, cache, ctx, &root, &canon, now, allow)?;
    Ok(root)
}

/// 履歴の本棚へ切り替える（AppHandle 無し）: 履歴にある物だけ・走査し直す
pub(crate) fn shelf_switch_steps(
    db: &Mutex<Db>,
    cache: &shelf::Cache,
    ctx: &shelf::Ctx,
    root: &str,
    now: i64,
    allow: Allow,
) -> Result<(), String> {
    let canon = ctx.ready(root).map_err(shelf_err)?;
    shelf_enter(db, cache, ctx, root, &canon, now, allow)
}

/// 履歴から外す（AppHandle 無し）: 履歴の項目・走査結果の表・表紙を消す。戻り値 = shelf_drop_thumbs の値。
/// 無くなったフォルダも外せる（本が分からないので表紙は残る）
pub(crate) fn shelf_forget_steps(
    db: &Mutex<Db>,
    cache: &shelf::Cache,
    ctx: &shelf::Ctx,
    root: &str,
) -> Result<(usize, bool), String> {
    ctx.known(root).map_err(shelf_err)?;
    let dropped = shelf_drop_thumbs(ctx, cache, root);
    {
        let g = lock(db)?;
        shelf::with_workspaces(&g, |m| {
            workspaces::remove(m, root);
            Ok(())
        })
        .map_err(shelf_err)?;
    } // ここでロックを外す
    Ok(dropped)
}

/// asset scope に足す（まだなら）。wiki_allow_dir を通さない（節の説明）
fn shelf_allow_dir(app: &tauri::AppHandle, d: &Path) -> Result<(), String> {
    let rt = app.state::<ShelfRuntime>();
    let mut allowed = rt.allowed.lock().map_err(err)?;
    if !allowed.contains(d) {
        app.asset_protocol_scope().allow_directory(d, true).map_err(err)?;
        allowed.insert(d.to_path_buf());
    }
    Ok(())
}

/// 表紙の置き場を作って canonical を返す（asset scope の照合は canonical）
fn shelf_thumbs_ready(app_dir: &Path) -> std::io::Result<PathBuf> {
    let d = thumbs::dir(app_dir);
    std::fs::create_dir_all(&d)?;
    std::fs::canonicalize(&d)
}

/// 起動時: 表紙の置き場を asset scope へ（main thread の外で 1 回）。失敗は stderr だけ（起動は止めない・本棚の表紙が出ないだけ）
pub(crate) fn shelf_startup(app: &tauri::AppHandle) {
    let st = app.state::<AppState>();
    let app_dir = match lock(&st.db) {
        Ok(db) => db.app_dir(),
        Err(_) => None,
    };
    let Some(app_dir) = app_dir else { return };
    let r = shelf_thumbs_ready(&app_dir).map_err(err).and_then(|d| shelf_allow_dir(app, &d));
    if let Err(e) = r {
        eprintln!("openref-app: shelf: 表紙の置き場を配信できません: {e}");
    }
}

/// 失敗の toast + Err（画面は Err を受けるだけ）
fn shelf_failed<T>(app: &tauri::AppHandle, en: bool, ja: &str, en_s: &str, e: String) -> Result<T, String> {
    wiki_toast(app, en, false, ja, en_s, Some(e.clone()));
    Err(e)
}

/// フォルダを本棚として開く（GUI 専用 command shelf_open_root の本体）。native のフォルダの選択ダイアログだけ（取り消し = {root: null}）。→ {root}。
/// パスを受けない = 本棚のフォルダは利用者の選択だけが決める（wiki_pick_root と同じ）
pub(crate) fn shelf_open(app: &tauri::AppHandle) -> Result<Value, String> {
    let en = ui_en(app);
    let title = if en { "Choose a folder to open as a bookshelf" } else { "本棚として開くフォルダを選ぶ" };
    let Some(given) = wiki_pick_folder_titled(app, title) else {
        return Ok(json!({"root": null}));
    };
    let ctx = shelf_ctx(app)?;
    let st = app.state::<AppState>();
    let rt = app.state::<ShelfRuntime>();
    match shelf_open_steps(&st.db, &rt.cache, &ctx, &given, wiki_now(), &|d| shelf_allow_dir(app, d)) {
        Ok(root) => Ok(json!({"root": root})),
        Err(e) => shelf_failed(app, en, "このフォルダは本棚にできません", "This folder cannot be opened as a bookshelf", e),
    }
}

/// 履歴の本棚へ切り替える（GUI 専用 command shelf_switch_root の本体）。走査し直す = 画面の読み直しもこれ。→ {root}
pub(crate) fn shelf_switch(app: &tauri::AppHandle, root: &str) -> Result<Value, String> {
    let en = ui_en(app);
    let ctx = shelf_ctx(app)?;
    let st = app.state::<AppState>();
    let rt = app.state::<ShelfRuntime>();
    match shelf_switch_steps(&st.db, &rt.cache, &ctx, root, wiki_now(), &|d| shelf_allow_dir(app, d)) {
        Ok(()) => Ok(json!({"root": root})),
        Err(e) => shelf_failed(app, en, "本棚を開けません", "Could not open the bookshelf", e),
    }
}

/// 履歴から外す（GUI 専用 command shelf_forget_root の本体）。→ {thumbs, unknown}（thumbs = 消した表紙の枚数・unknown = フォルダが読めず表紙を消せなかった）
pub(crate) fn shelf_forget(app: &tauri::AppHandle, root: &str) -> Result<Value, String> {
    let en = ui_en(app);
    let ctx = shelf_ctx(app)?;
    let st = app.state::<AppState>();
    let rt = app.state::<ShelfRuntime>();
    match shelf_forget_steps(&st.db, &rt.cache, &ctx, root) {
        Ok((n, unknown)) => {
            let name = ctx.opts(root).display_name(Path::new(root));
            wiki_toast(app, en, true, "本棚を履歴から外しました", "Removed the bookshelf from the history", Some(name));
            Ok(json!({"thumbs": n, "unknown": unknown}))
        }
        Err(e) => shelf_failed(app, en, "履歴から外せません", "Could not remove from the history", e),
    }
}

/// 履歴の本棚を画面が読めるようにする（走査しない・last_used を変えない）= 再起動で戻したタブ（読み手は走査なしで戻す）。→ {root}
pub(crate) fn shelf_front(app: &tauri::AppHandle, root: &str) -> Result<Value, String> {
    let canon = shelf_ctx(app)?.ready(root).map_err(shelf_err)?;
    shelf_allow_dir(app, &canon)?;
    Ok(json!({"root": root}))
}

/// 走査結果の JSON + 表紙の置き場（AppHandle 無し）。thumbs_dir = canonical（asset scope は canonical で足してある = app_dir が symlink を
/// 含むと、綴りのままではまだ無い表紙が 404 でなく 403 になり、画面の「404 → shelf_thumb」が組めない）
fn shelf_tree_steps(ctx: &shelf::Ctx, cache: &shelf::Cache, root: &str) -> Result<Value, String> {
    let s = cache.get_or_scan(ctx, root).map_err(shelf_err)?;
    let mut v = shelf::commands::tree_json(root, &s, None).map_err(shelf_err)?;
    v["thumbs_dir"] = match ctx.app_dir.as_deref() {
        Some(d) => json!(shelf_thumbs_ready(d).map_err(err)?),
        None => Value::Null,
    };
    Ok(v)
}

/// 走査結果（GUI 専用 command shelf_tree の本体）。表に無い・設定が変わっていれば 1 回走査する。asset scope も足す（再起動で戻したタブ）。
/// thumbs_dir = 表紙の置き場（画面は `<thumbs_dir>/<id>-<mtime>.png` を asset:// で描き、404 なら shelf_thumb で作る）
pub(crate) fn shelf_tree_of(app: &tauri::AppHandle, root: &str) -> Result<Value, String> {
    let ctx = shelf_ctx(app)?;
    let canon = ctx.ready(root).map_err(shelf_err)?;
    shelf_allow_dir(app, &canon)?;
    let rt = app.state::<ShelfRuntime>();
    shelf_tree_steps(&ctx, &rt.cache, root)
}

/// 本の実体（canonical）= core の shelf::book_path
fn shelf_book(app: &tauri::AppHandle, root: &str, id: &str) -> Result<PathBuf, String> {
    let ctx = shelf_ctx(app)?;
    let rt = app.state::<ShelfRuntime>();
    Ok(shelf::book_path(&ctx, &rt.cache, root, id).map_err(shelf_err)?.0)
}

/// 表紙の png（AppHandle 無し）。無ければ作る。戻り値は canonical（thumbs_dir と同じ綴り）
fn shelf_thumb_steps(ctx: &shelf::Ctx, cache: &shelf::Cache, root: &str, id: &str, en: bool) -> Result<PathBuf, String> {
    let (pdf, mtime) = shelf::book_path(ctx, cache, root, id).map_err(shelf_err)?;
    let app_dir = ctx.app_dir.as_deref().ok_or_else(|| tr(en, "表紙の置き場がありません（in-memory の DB）", "No place for covers (in-memory DB)"))?;
    let dir = shelf_thumbs_ready(app_dir).map_err(err)?;
    thumbs::ensure_thumb(&dir, &pdf, id, mtime).map_err(shelf_err)
}

/// 表紙（GUI 専用 command shelf_thumb の本体）。無ければ作る。→ {path}（png の絶対パス・asset URL は画面が組む）
pub(crate) fn shelf_thumb_of(app: &tauri::AppHandle, root: &str, id: &str) -> Result<Value, String> {
    let ctx = shelf_ctx(app)?;
    let rt = app.state::<ShelfRuntime>();
    let p = shelf_thumb_steps(&ctx, &rt.cache, root, id, ui_en(app))?;
    Ok(json!({"path": p.to_string_lossy()}))
}

/// shelf_reveal の `args.id`: 無い・null = 本棚のフォルダ自身（見出しのメニュー）。文字列でなければ Err（空文字は本の id として渡る = 本棚に無い本）
pub(crate) fn shelf_reveal_id(args: &Value) -> Result<Option<String>, String> {
    match args.get("id") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err("shelf_reveal: id は文字列です".to_string()),
    }
}

/// Finder で表示する先（AppHandle 無し）: id なし = 本棚のフォルダ自身（履歴にある root だけ）・あり = 本の実体
fn shelf_reveal_steps(ctx: &shelf::Ctx, cache: &shelf::Cache, root: &str, id: Option<&str>) -> Result<PathBuf, String> {
    match id {
        Some(id) => Ok(shelf::book_path(ctx, cache, root, id).map_err(shelf_err)?.0),
        None => ctx.ready(root).map_err(shelf_err),
    }
}

/// Finder で表示（GUI 専用 command shelf_reveal の本体）。失敗の文からパスを落とす
pub(crate) fn shelf_reveal_book(app: &tauri::AppHandle, root: &str, id: Option<&str>) -> Result<(), String> {
    let ctx = shelf_ctx(app)?;
    let rt = app.state::<ShelfRuntime>();
    let p = shelf_reveal_steps(&ctx, &rt.cache, root, id)?;
    let out = std::process::Command::new("/usr/bin/open").arg("-R").arg(&p).output().map_err(err)?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("open -R failed");
    Err(drop_dir_part(line.to_string()))
}

/// 既定のアプリで開く（GUI 専用 command shelf_open_external の本体）。失敗の文からパスを落とす = open_file
pub(crate) fn shelf_open_book(app: &tauri::AppHandle, root: &str, id: &str) -> Result<(), String> {
    open_file(&shelf_book(app, root, id)?.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 試験の場（home・app_dir = DB の隣・DB はファイル）
    struct Fix {
        base: PathBuf,
        db: Mutex<Db>,
        cache: shelf::Cache,
        home: PathBuf,
    }
    impl Drop for Fix {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn fix(tag: &str) -> Fix {
        let base = std::env::temp_dir().join(format!("openref-shelf-gui-{tag}-{}-{}", std::process::id(), openref_core::keys::new_key()));
        std::fs::create_dir_all(base.join("home/app")).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let home = base.join("home");
        let db = Db::open(&home.join("app/openref.sqlite")).unwrap();
        Fix { base, db: Mutex::new(db), cache: shelf::Cache::default(), home }
    }

    impl Fix {
        fn ctx(&self) -> shelf::Ctx {
            let (mut c, _) = shelf_ctx_db(&self.db, 1, false).unwrap();
            c.home = Some(self.home.clone());
            c
        }
        fn shelf(&self, name: &str) -> PathBuf {
            let r = self.home.join(name);
            std::fs::create_dir_all(r.join("sub")).unwrap();
            std::fs::write(r.join("a.pdf"), "%PDF-1.4").unwrap();
            std::fs::write(r.join("sub/b.pdf"), "%PDF-1.4").unwrap();
            r
        }
        fn thumb_dir(&self) -> PathBuf {
            let d = thumbs::dir(&self.home.join("app"));
            std::fs::create_dir_all(&d).unwrap();
            d
        }
        fn open(&self, p: &Path, now: i64) -> Result<String, String> {
            shelf_open_steps(&self.db, &self.cache, &self.ctx(), p, now, &|_| Ok(()))
        }
        fn setting(&self) -> Value {
            let raw = self.db.lock().unwrap().setting(shelf::KEY).unwrap();
            raw.map(|s| serde_json::from_str(&s).unwrap()).unwrap_or(json!({}))
        }
    }

    /// Finder の先: id なし = 本棚のフォルダ自身・あり = 本の実体。履歴に無い root は id の有無によらず通さない（任意のフォルダを Finder に出さない）
    #[test]
    fn reveal_target_is_root_or_book() {
        let f = fix("reveal");
        let root = f.open(&f.shelf("教科書"), 1).unwrap();
        let ctx = f.ctx();
        assert_eq!(shelf_reveal_steps(&ctx, &f.cache, &root, None).unwrap(), PathBuf::from(&root));
        let id = shelf::id_for(&Path::new(&root).join("a.pdf"));
        assert_eq!(shelf_reveal_steps(&ctx, &f.cache, &root, Some(&id)).unwrap(), Path::new(&root).join("a.pdf"));
        assert!(shelf_reveal_steps(&ctx, &f.cache, &root, Some("../../x")).is_err());
        assert!(shelf_reveal_steps(&ctx, &f.cache, &root, Some("")).is_err(), "空文字が本棚のフォルダになった");
        let other = f.home.to_string_lossy().into_owned();
        assert!(shelf_reveal_steps(&ctx, &f.cache, &other, None).is_err());
    }

    /// shelf_reveal の id の取り出し: あり = 本・無い / null = 本棚のフォルダ・空文字は本の id のまま（上の試験で通らない）・文字列でなければ Err
    #[test]
    fn reveal_id_from_args() {
        assert_eq!(shelf_reveal_id(&json!({"root": "/r", "id": "abc"})).unwrap().as_deref(), Some("abc"));
        assert_eq!(shelf_reveal_id(&json!({"root": "/r"})).unwrap(), None);
        assert_eq!(shelf_reveal_id(&json!({"root": "/r", "id": null})).unwrap(), None);
        assert_eq!(shelf_reveal_id(&json!({"root": "/r", "id": ""})).unwrap().as_deref(), Some(""));
        assert!(shelf_reveal_id(&json!({"root": "/r", "id": 1})).is_err());
    }

    /// 開く: canonical の root を履歴に・asset scope は canonical に 1 回・表に載る・開き直しても設定は保つ
    #[test]
    fn open_steps_record_canonical_root() {
        let f = fix("open");
        let real = f.shelf("教科書");
        let link = f.home.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let allowed = RefCell::new(Vec::new());
        let ctx = f.ctx();
        let root = shelf_open_steps(&f.db, &f.cache, &ctx, &link, 50, &|d| {
            allowed.borrow_mut().push(d.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(root, real.to_string_lossy());
        assert_eq!(*allowed.borrow(), vec![real.clone()]);
        assert_eq!(f.setting(), json!({root.clone(): {"last_used": 50, "collapse_single": false}}));
        let s = f.cache.get_or_scan(&f.ctx(), &root).unwrap();
        assert_eq!(s.tree.total_books, 2);
        // 別の綴りで開き直す = 同じ項目（設定は保つ）
        {
            let g = f.db.lock().unwrap();
            shelf::commands::set_opts(&g, &json!({"root": root, "collapse_single": true})).unwrap();
        }
        f.open(&real, 60).unwrap();
        assert_eq!(f.setting(), json!({root.clone(): {"last_used": 60, "collapse_single": true}}));
    }

    /// 場所の規則に落ちるフォルダ = 履歴に載らず asset scope にも足さない
    #[test]
    fn open_steps_reject_bad_places() {
        let f = fix("reject");
        let dot = f.home.join(".ssh");
        std::fs::create_dir_all(&dot).unwrap();
        let hit = RefCell::new(0);
        let allow = |_: &Path| {
            *hit.borrow_mut() += 1;
            Ok(())
        };
        let ctx = f.ctx();
        for (p, want) in [
            (dot.as_path(), "隠しフォルダ"),
            (f.home.as_path(), "ホーム"),
            (f.home.join("app").as_path(), "データの置き場"),
            (f.home.join("nope").as_path(), "見つかりません"),
        ] {
            let e = shelf_open_steps(&f.db, &f.cache, &ctx, p, 1, &allow).unwrap_err();
            assert!(e.contains("本棚のフォルダに") && e.contains(want) && !e.contains("入力を解釈できません"), "{e}");
        }
        assert_eq!(*hit.borrow(), 0);
        assert_eq!(f.setting(), json!({}));
    }

    /// 読めないフォルダ = 走査の失敗を返し、履歴に残さず asset scope にも足さない（履歴に残ると [外す] の無い壊れた行になる）
    #[test]
    fn open_steps_keep_unreadable_folder_out_of_history() {
        use std::os::unix::fs::PermissionsExt;
        let f = fix("unread");
        let r = f.shelf("locked");
        std::fs::set_permissions(&r, std::fs::Permissions::from_mode(0o000)).unwrap();
        let hit = RefCell::new(0);
        let ctx = f.ctx();
        let e = shelf_open_steps(&f.db, &f.cache, &ctx, &r, 1, &|_| {
            *hit.borrow_mut() += 1;
            Ok(())
        });
        std::fs::set_permissions(&r, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(e.unwrap_err().contains("本棚 locked を読めません"));
        assert_eq!(*hit.borrow(), 0);
        assert_eq!(f.setting(), json!({}));
    }

    /// 切替: 履歴にある物だけ・走査し直す（足したファイルが見える）・last_used を更新・無くなったフォルダは Err で履歴は変えない
    #[test]
    fn switch_steps_rescan_history_roots() {
        let f = fix("switch");
        let real = f.shelf("教科書");
        let root = f.open(&real, 10).unwrap();
        std::fs::write(real.join("c.pdf"), "%PDF-1.4").unwrap();
        assert_eq!(f.cache.get_or_scan(&f.ctx(), &root).unwrap().tree.total_books, 2, "表のまま");
        shelf_switch_steps(&f.db, &f.cache, &f.ctx(), &root, 20, &|_| Ok(())).unwrap();
        assert_eq!(f.cache.get_or_scan(&f.ctx(), &root).unwrap().tree.total_books, 3);
        assert_eq!(f.setting()[&root]["last_used"], 20);
        let e = shelf_switch_steps(&f.db, &f.cache, &f.ctx(), &format!("{root}/"), 30, &|_| panic!("足さない")).unwrap_err();
        assert!(e.contains("履歴に無い本棚"), "{e}");
        std::fs::remove_dir_all(&real).unwrap();
        let e = shelf_switch_steps(&f.db, &f.cache, &f.ctx(), &root, 30, &|_| panic!("足さない")).unwrap_err();
        assert!(e.contains("見つかりません"), "{e}");
        assert_eq!(f.setting()[&root]["last_used"], 20);
    }

    /// 外す: 履歴・表・その本棚の表紙が消える（他の本棚の表紙は残る）・履歴に無い root は Err
    #[test]
    fn forget_steps_remove_history_table_and_covers() {
        let f = fix("forget");
        let a = f.open(&f.shelf("A"), 1).unwrap();
        let b = f.open(&f.shelf("B"), 2).unwrap();
        let cache_dir = f.thumb_dir();
        let cover = |root: &str, rel: &str| cache_dir.join(format!("{}-1.png", shelf::id_for(&Path::new(root).join(rel))));
        for p in [cover(&a, "a.pdf"), cover(&a, "sub/b.pdf"), cover(&b, "a.pdf")] {
            std::fs::write(p, b"png").unwrap();
        }
        assert_eq!(shelf_forget_steps(&f.db, &f.cache, &f.ctx(), &a).unwrap(), (2, false));
        assert_eq!(f.setting().as_object().unwrap().keys().collect::<Vec<_>>(), vec![&b]);
        assert!(f.cache.forget(&a).is_none(), "表も消えた");
        assert!(!cover(&a, "a.pdf").exists() && !cover(&a, "sub/b.pdf").exists());
        assert!(cover(&b, "a.pdf").exists());
        let e = shelf_forget_steps(&f.db, &f.cache, &f.ctx(), &a).unwrap_err();
        assert!(e.contains("履歴に無い本棚"), "{e}");
    }

    /// 外す: 表に無ければ走査して本を知る（再起動の後）・フォルダが無ければ本が分からない = unknown で履歴だけ外す
    #[test]
    fn forget_steps_without_table() {
        let f = fix("forget2");
        let r = f.shelf("A");
        let root = f.open(&r, 1).unwrap();
        f.cache.forget(&root);
        let cache_dir = f.thumb_dir();
        let c = cache_dir.join(format!("{}-1.png", shelf::id_for(&r.join("a.pdf"))));
        std::fs::write(&c, b"png").unwrap();
        assert_eq!(shelf_forget_steps(&f.db, &f.cache, &f.ctx(), &root).unwrap(), (1, false));
        assert!(!c.exists());
        let root = f.open(&r, 2).unwrap();
        f.cache.forget(&root);
        std::fs::remove_dir_all(&r).unwrap();
        assert_eq!(shelf_forget_steps(&f.db, &f.cache, &f.ctx(), &root).unwrap(), (0, true));
        assert_eq!(f.setting(), json!({}));
    }

    /// 上限 10: 11 冊目の本棚を開くと最も古い root が落ち、その表と表紙も消える
    #[test]
    fn eviction_drops_table_and_covers() {
        let f = fix("evict");
        let roots: Vec<String> = (0..shelf::MAX).map(|i| f.open(&f.shelf(&format!("s{i}")), 10 + i as i64).unwrap()).collect();
        let cache_dir = f.thumb_dir();
        let c = cache_dir.join(format!("{}-1.png", shelf::id_for(&Path::new(&roots[0]).join("a.pdf"))));
        std::fs::write(&c, b"png").unwrap();
        f.open(&f.shelf("new"), 100).unwrap();
        let m = f.setting();
        assert_eq!(m.as_object().unwrap().len(), shelf::MAX);
        assert!(m.get(&roots[0]).is_none());
        assert!(!c.exists());
        assert!(f.cache.forget(&roots[0]).is_none());
    }

    /// 壊れた設定: 退避して空から（値は `.broken-<UTC>` に残る）・その後の開くは通る・wiki の設定には触らない
    #[test]
    fn broken_setting_is_set_aside() {
        let f = fix("broken");
        {
            let g = f.db.lock().unwrap();
            g.set_setting(shelf::KEY, "not json").unwrap();
            g.set_setting(workspaces::KEY, r#"{"/w": {"last_used": 1}}"#).unwrap();
        }
        let (ctx, to) = shelf_ctx_db(&f.db, 0, false).unwrap();
        assert_eq!(to.as_deref(), Some("shelf_workspaces.broken-19700101T000000Z"));
        assert!(ctx.workspaces.is_empty());
        {
            let g = f.db.lock().unwrap();
            assert_eq!(g.setting(to.as_deref().unwrap()).unwrap().as_deref(), Some("not json"));
            assert_eq!(g.setting(shelf::KEY).unwrap(), None);
        }
        assert_eq!(shelf_ctx_db(&f.db, 0, false).unwrap().1, None, "2 度目は退避しない");
        let root = f.open(&f.shelf("A"), 5).unwrap();
        assert_eq!(f.setting()[&root]["last_used"], 5);
        assert_eq!(f.db.lock().unwrap().setting(workspaces::KEY).unwrap().as_deref(), Some(r#"{"/w": {"last_used": 1}}"#));
    }

    /// 本棚の asset scope は wiki の記録を通さない（通すと wiki_root_stale が立ち OpenNote の toast を誤って出す）。
    /// 走査が root の下に何も足さない（表紙の置き場は app_dir）
    #[test]
    fn scope_stays_out_of_wiki_and_root_stays_clean() {
        let src = include_str!("shelf_gui.rs");
        let code: Vec<&str> = src.lines().take_while(|l| !l.starts_with("#[cfg(test)]")).filter(|l| !l.trim_start().starts_with("//")).collect();
        assert!(code.iter().any(|l| l.contains("allow_directory")));
        assert!(!code.iter().any(|l| l.contains("wiki_allow_dir") || l.contains("WikiAllowed")));
        let f = fix("clean");
        let r = f.shelf("A");
        let list = |p: &Path| -> Vec<String> {
            let mut v: Vec<String> = walk(p).into_iter().map(|e| e.to_string_lossy().into_owned()).collect();
            v.sort();
            v
        };
        let before = list(&r);
        let root = f.open(&r, 1).unwrap();
        shelf_switch_steps(&f.db, &f.cache, &f.ctx(), &root, 2, &|_| Ok(())).unwrap();
        shelf_forget_steps(&f.db, &f.cache, &f.ctx(), &root).unwrap();
        assert_eq!(list(&r), before);
    }

    fn walk(p: &Path) -> Vec<PathBuf> {
        let mut out = vec![p.to_path_buf()];
        if p.is_dir() {
            for e in std::fs::read_dir(p).unwrap().flatten() {
                out.extend(walk(&e.path()));
            }
        }
        out
    }

    /// app_dir が symlink を含んでも thumbs_dir と shelf_thumb の戻り値は canonical（asset scope の綴り）。画面が組む URL に、まだ無い表紙は
    /// 404（= shelf_thumb で作る）を返す。綴りのままだと 403 になり「404 → shelf_thumb」が組めない
    #[test]
    fn covers_are_reported_canonical() {
        let f = fix("canon");
        let r = f.shelf("A");
        let root = f.open(&r, 1).unwrap();
        let link = f.home.join("applink");
        std::os::unix::fs::symlink(f.home.join("app"), &link).unwrap();
        let ctx = shelf::Ctx { app_dir: Some(link.clone()), ..f.ctx() };
        let real = f.home.join("app/shelf-thumbs");
        let v = shelf_tree_steps(&ctx, &f.cache, &root).unwrap();
        assert_eq!(v["thumbs_dir"], json!(real.to_string_lossy()));
        assert_eq!(v["tree"]["totalBooks"], 2);
        let id = shelf::id_for(&r.join("a.pdf"));
        let (_, mtime) = shelf::book_path(&ctx, &f.cache, &root, &id).unwrap();
        std::fs::write(real.join(format!("{id}-{mtime}.png")), b"png").unwrap();
        assert_eq!(shelf_thumb_steps(&ctx, &f.cache, &root, &id, false).unwrap(), real.join(format!("{id}-{mtime}.png")));
        let scope = |p: &Path| p.starts_with(&real);
        let ask = |dir: &Path| {
            let uri = format!("asset://localhost/{}", dir.join("none-1.png").display());
            let req = tauri::http::Request::builder().uri(uri).body(Vec::new()).unwrap();
            crate::wiki_asset::asset_reply("main", &scope, &req).status().as_u16()
        };
        assert_eq!(ask(Path::new(v["thumbs_dir"].as_str().unwrap())), 404);
        assert_eq!(ask(&thumbs::dir(&link)), 403, "綴りのままだと scope の外");
        // in-memory（app_dir 無し）= thumbs_dir は null・表紙は Err
        let none = shelf::Ctx { app_dir: None, ..f.ctx() };
        assert_eq!(shelf_tree_steps(&none, &f.cache, &root).unwrap()["thumbs_dir"], Value::Null);
        assert!(shelf_thumb_steps(&none, &f.cache, &root, &id, false).unwrap_err().contains("in-memory"));
    }

    #[test]
    fn thumbs_ready_makes_the_folder() {
        let f = fix("ready");
        let d = shelf_thumbs_ready(&f.home.join("app")).unwrap();
        assert!(d.is_dir() && d.ends_with("shelf-thumbs"));
        assert_eq!(shelf_thumbs_ready(&f.home.join("app")).unwrap(), d);
    }

    /// 実フォルダでの手動試験: 実フォルダと scratch DB で 開く → 木 → 表紙 → 2 回目 → 外す を通す（`OPENREF_SHELF_DB` = scratch の DB・`OPENREF_SHELF_ROOT` = 本棚のフォルダ・
    /// `OPENREF_SHELF_THUMBS` = 表紙を作る冊数）。利用中の DB を指さない: pdf_write_mode は見ないが DB の名前で止める
    #[test]
    #[ignore]
    fn real_folder() {
        let db_path = PathBuf::from(std::env::var("OPENREF_SHELF_DB").expect("OPENREF_SHELF_DB"));
        assert!(db_path.to_string_lossy().contains("scratch"), "scratch の DB だけ");
        let root = PathBuf::from(std::env::var("OPENREF_SHELF_ROOT").expect("OPENREF_SHELF_ROOT"));
        let n: usize = std::env::var("OPENREF_SHELF_THUMBS").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
        let db = Mutex::new(Db::open(&db_path).unwrap());
        let cache = shelf::Cache::default();
        let (ctx, _) = shelf_ctx_db(&db, wiki_now(), false).unwrap();
        let t0 = std::time::Instant::now();
        let key = shelf_open_steps(&db, &cache, &ctx, &root, wiki_now(), &|_| Ok(())).unwrap();
        println!("open: {:?} root={key}", t0.elapsed());
        let (ctx, _) = shelf_ctx_db(&db, wiki_now(), false).unwrap();
        let s = cache.get_or_scan(&ctx, &key).unwrap();
        println!("books={} unreadable={} used_find={} table={}", s.tree.total_books, s.unreadable, s.used_find, s.books.len());
        let dir = thumbs::dir(&ctx.app_dir.clone().unwrap());
        let ids: Vec<String> = s.books.keys().take(n).cloned().collect();
        let t1 = std::time::Instant::now();
        let mut made = 0;
        for id in &ids {
            let (p, m) = shelf::book_path(&ctx, &cache, &key, id).unwrap();
            if thumbs::ensure_thumb(&dir, &p, id, m).is_ok() {
                made += 1;
            }
        }
        println!("thumbs 1st: {made}/{} in {:?}", ids.len(), t1.elapsed());
        let t2 = std::time::Instant::now();
        for id in &ids {
            let (p, m) = shelf::book_path(&ctx, &cache, &key, id).unwrap();
            thumbs::ensure_thumb(&dir, &p, id, m).unwrap();
        }
        println!("thumbs 2nd: {:?}", t2.elapsed());
        println!("files in shelf-thumbs: {}", std::fs::read_dir(&dir).unwrap().count());
    }

    /// 実フォルダでの手動試験: real_folder の本棚を履歴から外す（表紙も消える）
    #[test]
    #[ignore]
    fn real_forget() {
        let db_path = PathBuf::from(std::env::var("OPENREF_SHELF_DB").expect("OPENREF_SHELF_DB"));
        assert!(db_path.to_string_lossy().contains("scratch"), "scratch の DB だけ");
        let root = std::env::var("OPENREF_SHELF_ROOT").expect("OPENREF_SHELF_ROOT");
        let db = Mutex::new(Db::open(&db_path).unwrap());
        let cache = shelf::Cache::default();
        let (ctx, _) = shelf_ctx_db(&db, wiki_now(), false).unwrap();
        let dir = thumbs::dir(&ctx.app_dir.clone().unwrap());
        let before = std::fs::read_dir(&dir).unwrap().count();
        let r = shelf_forget_steps(&db, &cache, &ctx, &root).unwrap();
        let after = std::fs::read_dir(&dir).unwrap().count();
        println!("forget: {r:?} files {before} -> {after}");
    }
}
