//! OpenNote の GUI の本体（Tauri の AppHandle に依存する段取り）: 有効化と root の選択・監視（WikiLive・wiki_fs_plan / wiki_fs_ingest）・
//! 参照フォルダ・URL の窓・終了の保留・チャットの設定と status・Settings の OpenNote のメニュー。
//! Tauri command の包みと generate_handler!・setup は main.rs（parity test が main.rs だけを走査する）。ここは包みが呼ぶ本体

use crate::wiki_asset::{wiki_has_dot_segment, wiki_not_regular_file};
use crate::{blocking, err, lock, save_setting, wiki_chat, AppState, SettingsItems};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

// ---------- OpenNote（読み） ----------
// 登録表にもある名前は登録表と同じ core 関数（core::wiki::commands・wiki_call）。wiki_read_gui は read（画像・PDF は asset protocol で
// 直接描くので base64 も抽出もしない）= 登録表の wiki_read（read_agent）とは別の関数なので別名・GUI_ONLY。Db のロックは設定（wiki_enabled / wiki_root / app_dir）を
// 読む間だけ握り、索引の走査・解析はロックの外・main thread の外で行う（=
// 索引の作り直し中も一覧・詳細の DB 操作を止めない）。引数は JS から `invoke(name, { args: {...} })`（登録表と同じ形・同じ検査）

type WikiFn =
    fn(&openref_core::wiki::Ctx, &serde_json::Value) -> openref_core::Result<serde_json::Value>;

pub(crate) async fn wiki_call(
    state: &AppState,
    name: &'static str,
    args: Option<serde_json::Value>,
    f: WikiFn,
) -> Result<serde_json::Value, String> {
    wiki_run(state, Some(name), args, f).await
}

/// wiki_run の Err: `CoreError::Wiki` は `{"code", "message"}` の JSON 文字列（画面は code で分岐し文言を照合しない =
/// app/src/lib/wiki/ipc.ts が解く）。他は err と同じ素の文
fn wiki_err(e: openref_core::CoreError) -> String {
    match &e {
        openref_core::CoreError::Wiki { code, msg } => {
            serde_json::json!({"code": code, "message": msg}).to_string()
        }
        _ => err(e),
    }
}

/// validate = 登録表の名前（引数をその schema で検査する）。GUI 専用（登録表に無い）は None = 引数の検査は core の関数が行う
pub(crate) async fn wiki_run(
    state: &AppState,
    validate: Option<&'static str>,
    args: Option<serde_json::Value>,
    f: WikiFn,
) -> Result<serde_json::Value, String> {
    let db = state.db.clone();
    let live = state.wiki_live.clone();
    blocking(move || {
        let args = args
            .filter(|a| !a.is_null())
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(name) = validate {
            openref_core::commands::validate(name, &args).map_err(err)?;
        }
        let mut ctx = {
            let db = lock(&db)?;
            openref_core::wiki::Ctx::from_db(&db).map_err(wiki_err)?
        }; // ここでロックを外す
        ctx.watched = wiki_watched(&live, &ctx);
        f(&ctx, &args).map_err(wiki_err)
    })
    .await
}

/// 監視が生きていれば鮮度規則の stat 走査を省く（Ctx::watched）。監視を張った後の最初の 1 回は stat 走査をしてから（wiki_prime）。
/// ただし索引が握られている（背景の走査・作り直しの最中 = index::busy）間は prime しない = 待たずに今どおりの入口へ
/// （core の status の「busy なら進捗を返す」が効く・背景の走査が済めば wiki_apply が prime する = 二重に走査しない）
fn wiki_watched(live: &WikiLive, ctx: &openref_core::wiki::Ctx) -> bool {
    match live.plan(ctx.root.as_deref()) {
        WatchPlan::Skip => true,
        WatchPlan::Prime(gen) => {
            !openref_core::wiki::index::busy() && wiki_prime(ctx) && live.prime(gen)
        }
        WatchPlan::Off => false,
    }
}

/// 監視を張った後の最初の stat 走査（今すぐ・5 秒の鮮度規則に依らない = 張る前の変更を取り込む）。true = 済んだ（以後は監視に任せてよい）。
/// 索引に作り直しの旗があれば走査しない（false = 今どおり鮮度規則が旗を判定する）。失敗も false（呼び手の鮮度規則が同じエラーを出す）
fn wiki_prime(ctx: &openref_core::wiki::Ctx) -> bool {
    let Ok((ip, root)) = ctx.ready() else {
        return false;
    };
    openref_core::wiki::with_index(&ip, &root, |ix| {
        if ix.meta("rebuilding")?.is_some() {
            return Ok(false);
        }
        Ok(ix.refresh()? == openref_core::wiki::Fresh::Ok)
    })
    .unwrap_or(false)
}

pub(crate) fn wiki_trash_core(
    ctx: &openref_core::wiki::Ctx,
    a: &serde_json::Value,
) -> openref_core::Result<serde_json::Value> {
    openref_core::wiki::edit::trash_with(ctx, a, wiki_move_to_trash)
}

/// macOS のゴミ箱へ = NSFileManager の trashItemAtURL（trash crate の NsFileManager 方式）。戻り値 = 方式の名前（結果の method）。
/// trash crate の既定（Finder 方式 = `osascript` で Finder に delete させる）は採らない: 「OpenRef が Finder を制御」の許可が要り
/// （署名なしの .app は作り直すたびに尋ね直されうる）、許可の返事を待つ間は AppleEvent の既定 120 秒止まる（実測 = -1712 で 120 秒）。
/// trashItemAtURL でもゴミ箱の .DS_Store（Finder の「戻す」の記録の置き場）が移した同じ秒に更新される（実測）。
/// 「戻す」が効かないと分かったら DeleteMethod::Finder に替える（1 行）
pub(crate) fn wiki_move_to_trash(abs: &Path) -> Result<String, String> {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    let mut ctx = trash::TrashContext::default();
    ctx.set_delete_method(DeleteMethod::NsFileManager);
    ctx.delete(abs)
        .map(|()| "nsfilemanager".to_string())
        .map_err(|e| format!("ゴミ箱へ移せません: {e}"))
}

// ---------- OpenNote（終了の保留 = 未保存の編集を失わない） ----------
// 保留できるのはアプリのメニューの終了（自作の QUIT_ID = ⌘Q）と窓の ×。Dock の「終了」・ログアウト・再起動は macOS の terminate: が
// 直接 Exit へ進む（tao に applicationShouldTerminate が無い）ので保留できない
// Wiki が有効なときだけ（無効なら今の挙動のまま）、⌘Q 等の終了（RunEvent::ExitRequested）と窓の ×（CloseRequested）を保留して
// "wiki-exit-request"（{kind: "exit" | "close"}）をフロントへ出す。フロントは保存してから wiki_exit_ready {proceed} で答える。
// 15 秒答えが無ければ終了する（JS が固まってもアプリを閉じられる）。保留中の二重の要求は保留するだけ（二重に尋ねない・終了しない）。
// 自分の wiki_exit_ready / タイムアウトによる終了は 10 秒の間は保留しない（再び保留しない）。restart（RESTART_EXIT_CODE）は保留しない
// （Tauri が prevent_exit を無視する）

/// Wiki が有効か（設定 wiki_enabled の写し。終了の判定は main thread で走るので Db のロックを取らない）
pub(crate) static WIKI_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 応答を待つ秒数
const WIKI_EXIT_TIMEOUT: Duration = Duration::from_secs(15);
/// 自分で終了を進めた後、保留しない間
const WIKI_EXIT_ALLOW: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExitKind {
    /// アプリの終了（終了コード）
    Exit(i32),
    /// 窓を閉じる（窓の label）
    Close(String),
}

#[derive(Debug, Default)]
struct ExitState {
    /// 保留中の要求と、その番号（タイムアウトがその要求のものか見分ける）
    pending: Option<(ExitKind, u64)>,
    seq: u64,
    /// この時刻までは保留しない（自分で終了を進めた）
    allow_until: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct WikiExit(Mutex<ExitState>);

#[derive(Debug, PartialEq)]
enum Hold {
    /// 保留しない（今の挙動）
    Allow,
    /// 保留するだけ（既に尋ねている）
    Prevent,
    /// 保留してフロントに尋ねる（番号 = タイムアウトの照合用）
    Ask(u64),
}

/// 終了・窓を閉じる要求を保留するか（純関数）
fn exit_hold(st: &mut ExitState, wiki_on: bool, kind: ExitKind, now: Instant) -> Hold {
    if !wiki_on
        || kind == ExitKind::Exit(tauri::RESTART_EXIT_CODE)
        || st.allow_until.is_some_and(|t| now < t)
    {
        return Hold::Allow;
    }
    if let Some(p) = st.pending.as_mut() {
        // 窓を閉じる保留の間に ⌘Q = 答えが来たらアプリごと終了する
        if matches!(kind, ExitKind::Exit(_)) {
            p.0 = kind;
        }
        return Hold::Prevent;
    }
    st.seq += 1;
    st.pending = Some((kind, st.seq));
    Hold::Ask(st.seq)
}

/// 保留を解く（純関数）。seq = タイムアウト（その要求がまだ保留中のときだけ）・None = フロントの応答。
/// proceed なら進める要求を返し、しばらく保留しない。取りやめ・該当なしは None
fn exit_take(
    st: &mut ExitState,
    seq: Option<u64>,
    proceed: bool,
    now: Instant,
) -> Option<ExitKind> {
    let (kind, s) = st.pending.clone()?;
    if seq.is_some_and(|q| q != s) {
        return None;
    }
    st.pending = None;
    if !proceed {
        return None;
    }
    st.allow_until = Some(now + WIKI_EXIT_ALLOW);
    Some(kind)
}

pub(crate) fn wiki_exit_event(app: &tauri::AppHandle, kind: ExitKind, prevent: impl FnOnce()) {
    let Some(rt) = app.try_state::<WikiExit>() else {
        return;
    };
    let is_close = matches!(kind, ExitKind::Close(_));
    let hold = {
        let mut st = rt.0.lock().unwrap_or_else(|e| e.into_inner());
        exit_hold(
            &mut st,
            WIKI_ON.load(std::sync::atomic::Ordering::Relaxed),
            kind,
            Instant::now(),
        )
    };
    match hold {
        Hold::Allow => {}
        Hold::Prevent => prevent(),
        Hold::Ask(seq) => {
            prevent();
            wiki_emit(
                app,
                "wiki-exit-request",
                serde_json::json!({"kind": if is_close { "close" } else { "exit" }}),
            );
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(WIKI_EXIT_TIMEOUT);
                wiki_exit_finish(&app, Some(seq), true);
            });
        }
    }
}

pub(crate) fn wiki_exit_finish(app: &tauri::AppHandle, seq: Option<u64>, proceed: bool) {
    let Some(rt) = app.try_state::<WikiExit>() else {
        return;
    };
    let next = {
        let mut st = rt.0.lock().unwrap_or_else(|e| e.into_inner());
        exit_take(&mut st, seq, proceed, Instant::now())
    };
    match next {
        Some(ExitKind::Exit(code)) => app.exit(code),
        Some(ExitKind::Close(label)) => {
            if let Some(w) = app.get_webview_window(&label) {
                let _ = w.close();
            }
        }
        None => {}
    }
}

// ---------- OpenNote（GUI 専用: 有効化・vault の root・索引・ファイル監視・外部で開く） ----------
// GUI 専用 command と Settings › OpenNote、ファイル監視（command の包みは main.rs・本体はここ）。
// 設定 wiki_enabled / wiki_root はここでだけ書く（ui_ で始まらないので set_ui_pref では書けない = agent が任意のフォルダを vault にしない）。
// Db のロックは設定の読み書きの間だけ。走査・監視・ダイアログは main thread の外で行う。
// フロントへの event: "wiki-state"（有効 / 無効・root・作り直しの開始が変わった = wiki_status を読み直す）・
// "wiki-changed"（索引を更新したパス = vault 相対を種別ごとに = `WikiChanged` {all, added, removed, modified}。全走査の後は all = true で配列は空 = 全部。
//   core が同期で索引に入れた書込（edit.rs の reindex = GUI の保存・チャット / MCP の wiki_write・取込）は監視が拾う時点で索引にある = 新しいページでも modified。
//   画面は増減（added / removed / all）と「知らない modified」（木に無いパス）のときだけ木を読み、Tags 箱は毎回、Ingest / Chat log 箱はそのフォルダに変更があるときだけ読む）・
// "wiki-error"（監視・走査の失敗の文・言語設定に従う）

/// Settings › OpenNote の項目 id
pub(crate) const WIKI_ENABLED_ID: &str = "wiki_enabled";
pub(crate) const WIKI_REINDEX_ID: &str = "wiki_reindex";
pub(crate) const WIKI_DROP_INDEX_ID: &str = "wiki_drop_index";

pub(crate) fn wiki_enabled_text(en: bool) -> &'static str {
    if en {
        "Enabled"
    } else {
        "有効"
    }
}

pub(crate) fn wiki_reindex_text(en: bool) -> &'static str {
    if en {
        "Rebuild index"
    } else {
        "索引を作り直す"
    }
}

pub(crate) fn wiki_drop_index_text(en: bool) -> &'static str {
    if en {
        "Delete index"
    } else {
        "索引を削除"
    }
}

/// 今の表示言語（Settings › Language のチェック = DB の ui_lang と同じ。メニューが無ければ日本語）
pub(crate) fn ui_en(app: &tauri::AppHandle) -> bool {
    app.try_state::<SettingsItems>()
        .and_then(|i| i.lang_en.is_checked().ok())
        .unwrap_or(false)
}

/// UI の言語（en = 英語）で ja / en_s のどちらかを返す（app の唯一の写し = wiki_chat・wiki_chat_log も `crate::tr`）
pub(crate) fn tr(en: bool, ja: &str, en_s: &str) -> String {
    (if en { en_s } else { ja }).to_string()
}

/// 監視と asset scope の状態（プロセスに 1 つ）
#[derive(Default)]
pub(crate) struct WikiRuntime {
    /// 動いているファイル監視（drop で止まる）
    pub(crate) watch: Mutex<Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>,
    /// 監視が生きているかの印（AppState と同じもの）
    pub(crate) live: Arc<WikiLive>,
    /// このプロセスで asset scope に足したフォルダ（allow_directory は取り消せない = OFF にしても root を替えても再起動まで配信される）
    pub(crate) allowed: Mutex<WikiAllowed>,
}

/// 監視が生きているかの印（A-16）。生きていて、張った後の stat 走査も済んでいる（primed）間は wiki_* の入口の鮮度規則
/// （5 秒ごとの stat 走査）を省く（Ctx::watched）。張り直し・止めた・監視の失敗・取り込みの失敗で外れ、外れている間は今どおり
#[derive(Default)]
pub(crate) struct WikiLive(Mutex<WikiLiveState>);

#[derive(Default, Clone, Debug, PartialEq)]
struct WikiLiveState {
    /// 監視を張るたびに増える（前の監視の知らせを今の監視に効かせない）
    gen: u64,
    /// 監視を張ったときの設定 wiki_root（入口の Ctx の root と違えば省かない）
    root: Option<String>,
    /// 監視が動いている（張り終えて、失敗していない）
    alive: bool,
    /// 張った後の stat 走査が済んでいる
    primed: bool,
}

/// 入口の鮮度規則の扱い
#[derive(Debug, PartialEq)]
enum WatchPlan {
    /// 監視が無い・落ちた・root が違う = 今どおり
    Off,
    /// 監視は生きているが張った後の stat 走査がまだ = 走査してから省く（gen = その監視）
    Prime(u64),
    /// 省く
    Skip,
}

impl WikiLive {
    fn with<T>(&self, f: impl FnOnce(&mut WikiLiveState) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
    /// 監視を張り始める（まだ生きていない = 張り終えて armed するまで省かない）。戻り値 = この監視の gen
    fn begin(&self, root: Option<String>) -> u64 {
        self.with(|s| {
            *s = WikiLiveState { gen: s.gen + 1, root, alive: false, primed: false };
            s.gen
        })
    }
    /// 張り終えた
    fn armed(&self, gen: u64) {
        self.with(|s| s.alive |= s.gen == gen);
    }
    /// 止めた
    fn stop(&self) {
        self.with(|s| *s = WikiLiveState { gen: s.gen + 1, ..WikiLiveState::default() });
    }
    /// 監視が失敗した（以後の変更を取り込めない）
    fn fail(&self, gen: u64) {
        self.with(|s| {
            if s.gen == gen {
                s.alive = false;
                s.primed = false;
            }
        });
    }
    /// 取り込みに失敗した = 索引が古いかもしれない → 次の入口で stat 走査し直す
    fn unprime(&self, gen: u64) {
        self.with(|s| s.primed &= s.gen != gen);
    }
    /// 張った後の stat 走査が済んだ。戻り値 = 今も同じ監視が生きている（= 省いてよい）
    fn prime(&self, gen: u64) -> bool {
        self.with(|s| {
            if s.gen == gen && s.alive {
                s.primed = true;
            }
            s.gen == gen && s.alive
        })
    }
    fn plan(&self, root: Option<&str>) -> WatchPlan {
        self.with(|s| match (s.alive && s.root.as_deref() == root, s.primed) {
            (false, _) => WatchPlan::Off,
            (true, false) => WatchPlan::Prime(s.gen),
            (true, true) => WatchPlan::Skip,
        })
    }
}

/// asset scope に足したフォルダ。root と参照フォルダを別に持つ = 「前の root の配信が残っている」（wiki_root_stale）は root だけで見る。
/// 配信そのもの（遷移の判定・OFF にした後の一言）は両方を数える
#[derive(Default, Clone, Debug, PartialEq)]
pub(crate) struct WikiAllowed {
    roots: Vec<PathBuf>,
    refs: Vec<PathBuf>,
}

/// asset scope に足すフォルダの種類
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WikiDir {
    Root,
    Ref,
}

impl WikiAllowed {
    /// 配信中か（root・参照フォルダのどちらでも）
    pub(crate) fn contains(&self, d: &Path) -> bool {
        self.roots.iter().chain(&self.refs).any(|a| a == d)
    }

    fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.refs.is_empty()
    }

    /// 配信中のフォルダ全部（遷移の判定 = root と参照フォルダの両方の中を許す）
    pub(crate) fn all(&self) -> Vec<PathBuf> {
        self.roots.iter().chain(&self.refs).cloned().collect()
    }

    /// 記録する。戻り値 = scope にまだ無かった（allow_directory を呼ぶ）。同じフォルダを root と参照フォルダの両方で記録してもよい
    /// （scope には 1 回だけ足す）
    fn insert(&mut self, d: &Path, kind: WikiDir) -> bool {
        let fresh = !self.contains(d);
        let list = match kind {
            WikiDir::Root => &mut self.roots,
            WikiDir::Ref => &mut self.refs,
        };
        if !list.iter().any(|a| a == d) {
            list.push(d.to_path_buf());
        }
        fresh
    }
}

/// 前の root の配信がこのプロセスに残るか（純関数）= このプロセスで scope に足した root のうち、今の root と違うものがあるか。
/// 参照フォルダは数えない（参照フォルダが配信中でも、同じ root を選び直せば再起動は要らない）
fn wiki_root_stale(allowed: &WikiAllowed, root: &Path) -> bool {
    allowed.roots.iter().any(|r| r != root)
}

/// 有効化の後の索引の扱い
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum WikiScan {
    /// 走査しない（起動時 = 最初の wiki_* の呼び出しが鮮度規則で走査する）
    None,
    /// stat 走査（変わったものだけ）
    Refresh,
    /// 中身を捨てて全走査
    Rebuild,
}

pub(crate) fn wiki_ctx(app: &tauri::AppHandle) -> Result<openref_core::wiki::Ctx, String> {
    let st = app.state::<AppState>();
    let db = lock(&st.db)?;
    openref_core::wiki::Ctx::from_db(&db).map_err(err)
} // ここでロックを外す

pub(crate) fn wiki_emit<S: serde::Serialize + Clone>(app: &tauri::AppHandle, ev: &str, payload: S) {
    if let Err(e) = app.emit(ev, payload) {
        eprintln!("openref-app: {ev} を webview へ送れません: {e}");
    }
}

fn wiki_error(app: &tauri::AppHandle, msg: String) {
    eprintln!("openref-app: OpenNote: {msg}");
    wiki_emit(app, "wiki-error", msg);
}

/// "wiki-changed" の payload（画面の types.ts の WikiChanged と同じ形）。パスは vault 相対・種別は core の取り込み結果（`Refreshed`）から =
/// 推測で付けない。all = 全走査の後（refresh / rebuild・監視がフォルダの増減で全走査に切り替えた）= 何が増減したか分からないので全部。そのとき配列は空
#[derive(serde::Serialize, Clone, Debug, PartialEq, Eq, Default)]
struct WikiChanged {
    all: bool,
    added: Vec<String>,
    removed: Vec<String>,
    modified: Vec<String>,
}

impl WikiChanged {
    /// 全走査の後 = 全部
    fn everything() -> Self {
        Self {
            all: true,
            ..Self::default()
        }
    }
    fn is_empty(&self) -> bool {
        !self.all && self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }
}

impl From<openref_core::wiki::index::Refreshed> for WikiChanged {
    fn from(r: openref_core::wiki::index::Refreshed) -> Self {
        Self {
            all: false,
            added: r.added,
            removed: r.removed,
            modified: r.modified,
        }
    }
}

/// wiki_apply の段取り（純関数の戻り値）
#[derive(Debug, PartialEq)]
enum WikiApplyPlan {
    /// 無効 = 監視を止めるだけ
    Off,
    /// 有効だが root が使えない（未設定・検査に落ちた）= 監視を止めて Err
    Unusable(String),
    /// 配信する（索引のパス・canonical な root）。参照フォルダは root を配信した後に wiki_apply_refs で決める
    Serve { index: PathBuf, root: PathBuf },
}

/// 設定（enabled・ready = 索引と canonical な root）から wiki_apply の段取りを決める。I/O は引数で受ける
fn wiki_apply_plan(enabled: bool, ready: Result<(PathBuf, PathBuf), String>) -> WikiApplyPlan {
    if !enabled {
        return WikiApplyPlan::Off;
    }
    match ready {
        Ok((index, root)) => WikiApplyPlan::Serve { index, root },
        Err(e) => WikiApplyPlan::Unusable(e),
    }
}

/// 配信する参照フォルダ（純関数）= 設定の参照フォルダ（読めなければ空 + 理由）のうち今あるもの（無くなったものは飛ばす =
/// 設定には残す・戻れば次の起動で配信される）。scope への記録は参照フォルダとして
fn wiki_apply_refs(
    refs: Result<Vec<PathBuf>, String>,
    is_dir: &dyn Fn(&Path) -> bool,
) -> (Vec<(PathBuf, WikiDir)>, Option<String>) {
    match refs {
        Ok(v) => (
            v.into_iter()
                .filter(|d| is_dir(d))
                .map(|d| (d, WikiDir::Ref))
                .collect(),
            None,
        ),
        Err(e) => (Vec::new(), Some(e)),
    }
}

/// 設定に合わせて asset scope・監視・索引を揃える（有効化・root の変更・再索引・起動時）。
/// 無効なら監視を止める。有効で root が検査を通れば scope に足して監視を（張り直し）、scan に応じて背景で走査する。
/// 走査の前後に "wiki-state" / "wiki-changed" を出す
pub(crate) fn wiki_apply(app: &tauri::AppHandle, scan: WikiScan) -> Result<(), String> {
    let ctx = wiki_ctx(app)?;
    let rt = app.state::<WikiRuntime>();
    let plan = wiki_apply_plan(ctx.enabled, ctx.ready().map_err(|e| e.to_string()));
    let (ip, root) = match plan {
        WikiApplyPlan::Off => {
            wiki_watch_stop(&rt);
            wiki_emit(app, "wiki-state", ());
            return Ok(());
        }
        WikiApplyPlan::Unusable(e) => {
            wiki_watch_stop(&rt);
            wiki_emit(app, "wiki-state", ());
            return Err(e);
        }
        WikiApplyPlan::Serve { index, root } => (index, root),
    };
    // root を先に配信する（参照フォルダの検査 = canonicalize が遅いボリュームでも root の配信を待たせない）
    wiki_allow_dir(app, &root, WikiDir::Root)?;
    // 参照フォルダも配信する
    let (refs, refs_error) = wiki_apply_refs(wiki_ref_dirs(app), &|d| d.is_dir());
    if let Some(e) = refs_error {
        eprintln!("openref-app: wiki: 参照フォルダを読めません（配信しません）: {e}");
    }
    for (d, kind) in &refs {
        if let Err(e) = wiki_allow_dir(app, d, *kind) {
            eprintln!(
                "openref-app: 参照フォルダ {} を asset scope に足せません: {e}",
                d.display()
            );
        }
    }
    let en = ui_en(app);
    // 監視の失敗は知らせるが、有効化そのものは止めない（[再索引] で張り直せる）
    let gen = wiki_watch_start(app, &rt, ip.clone(), root.clone(), ctx.root.clone());
    if let Err(e) = &gen {
        wiki_error(
            app,
            format!(
                "{}: {e}",
                tr(
                    en,
                    "ファイルの監視を始められません（変更はヘッダーの [再索引] で取り込めます）",
                    "Could not start watching files (use Reindex in the header to pick up changes)"
                )
            ),
        );
    }
    wiki_emit(app, "wiki-state", ());
    if scan != WikiScan::None {
        let app = app.clone();
        let live = rt.live.clone();
        std::thread::spawn(move || {
            let res = openref_core::wiki::with_index(&ip, &root, |ix| {
                // 索引を握ってから知らせる = この間の wiki_status は待たずに進捗を返す（index::busy）
                wiki_emit(&app, "wiki-state", ());
                if scan == WikiScan::Rebuild {
                    ix.rebuild()
                } else {
                    ix.refresh()
                }
            });
            match res {
                // 監視を張った後の走査が済んだ = 入口の stat 走査を省いてよい
                Ok(openref_core::wiki::Fresh::Ok) => {
                    if let Ok(g) = gen {
                        live.prime(g);
                    }
                }
                Ok(openref_core::wiki::Fresh::Rebuilding(p)) => wiki_error(
                    &app,
                    if en {
                        format!("Another process is rebuilding the index ({p}%). It will be readable when done")
                    } else {
                        format!("他のプロセスが索引を作り直し中です（{p}%）。終わると読めます")
                    },
                ),
                Err(e) => wiki_error(
                    &app,
                    format!(
                        "{}: {e}",
                        tr(en, "索引を作れません", "Could not build the index")
                    ),
                ),
            }
            wiki_emit(&app, "wiki-changed", WikiChanged::everything());
            wiki_emit(&app, "wiki-state", ());
        });
    }
    Ok(())
}

fn wiki_watch_stop(rt: &WikiRuntime) {
    rt.live.stop();
    if let Ok(mut w) = rt.watch.lock() {
        *w = None;
    }
}

/// root の再帰監視（FSEvents・500ms の debounce）を張る（前のは止める）。変わったパスを索引に取り込んでから "wiki-changed"。
/// root_setting = 設定 wiki_root（監視の印 WikiLive に残す）。戻り値 = この監視の gen
fn wiki_watch_start(
    app: &tauri::AppHandle,
    rt: &WikiRuntime,
    ip: PathBuf,
    root: PathBuf,
    root_setting: Option<String>,
) -> Result<u64, String> {
    wiki_watch_stop(rt);
    let gen = rt.live.begin(root_setting);
    let handler = {
        let app = app.clone();
        let root = root.clone();
        let live = rt.live.clone();
        move |res: notify_debouncer_mini::DebounceEventResult| match res {
            Ok(evs) => {
                if !wiki_on_fs(&app, &ip, &root, evs.into_iter().map(|e| e.path).collect()) {
                    live.unprime(gen);
                }
            }
            Err(e) => {
                live.fail(gen);
                wiki_error(
                    &app,
                    format!(
                        "{}: {e}",
                        tr(
                            ui_en(&app),
                            "ファイルの監視が失敗しました（変更はヘッダーの [再索引] で取り込めます）",
                            "File watching failed (use Reindex in the header to pick up changes)"
                        )
                    ),
                )
            }
        }
    };
    let mut deb =
        notify_debouncer_mini::new_debouncer(std::time::Duration::from_millis(500), handler)
            .map_err(err)?;
    deb.watcher()
        .watch(&root, notify::RecursiveMode::Recursive)
        .map_err(err)?;
    *rt.watch.lock().map_err(err)? = Some(deb);
    rt.live.armed(gen);
    Ok(gen)
}

/// 監視が拾ったパスの状態（wiki_fs_plan に渡す）
#[derive(Clone, Copy, PartialEq, Debug)]
enum FsKind {
    File,
    Dir,
    Missing,
}

/// 監視の 1 バッチの扱い
#[derive(Debug, PartialEq)]
struct FsPlan {
    /// 取り込む vault 相対パス（重複なし・届いた順）と絶対パス
    picked: Vec<(String, PathBuf)>,
    /// stat の全走査に切り替える（フォルダの作成・改名・削除は中のファイルが個別に来ないことがある）
    full: bool,
    /// vault の root そのものが無い（消えた・改名された）= 取り込まずに知らせる
    root_gone: bool,
}

/// 監視のバッチの判定（純関数）: vault の外・`.` 始まりの区画（.obsidian・.git）は除く。
/// フォルダ、または消えていて索引にも無いパス（消えたフォルダ）が混じれば全走査。root そのもののパスも全走査（フォルダの丸ごとの
/// 差し替え・FSEvents の MustScanSubDirs = イベントが落ちた知らせは root のパス 1 件で届く = 中のファイルが個別に来ない）
fn wiki_fs_plan(
    root: &Path,
    root_is_dir: bool,
    paths: Vec<PathBuf>,
    kind: impl Fn(&Path) -> FsKind,
    indexed: impl Fn(&str) -> bool,
) -> FsPlan {
    use openref_core::wiki::{paths::rel_inside, strip_root};
    let mut seen = std::collections::HashSet::new();
    let mut picked = Vec::new();
    let mut root_hit = false;
    for p in paths {
        root_hit |= strip_root(root, &p).is_some_and(|rel| rel.is_empty());
        let Some(rel) = rel_inside(root, &p) else {
            continue;
        };
        if !seen.insert(rel.clone()) {
            continue;
        }
        picked.push((rel, p));
    }
    if !root_is_dir {
        return FsPlan {
            picked: Vec::new(),
            full: false,
            root_gone: true,
        };
    }
    let full = root_hit
        || picked.iter().any(|(rel, abs)| match kind(abs) {
            FsKind::Dir => true,
            FsKind::Missing => !indexed(rel),
            FsKind::File => false,
        });
    FsPlan {
        picked,
        full,
        root_gone: false,
    }
}

/// 監視が拾ったパスの状態（stat。無い・読めない = Missing）
fn wiki_fs_kind(p: &Path) -> FsKind {
    match std::fs::metadata(p) {
        Ok(m) if m.is_dir() => FsKind::Dir,
        Ok(_) => FsKind::File,
        Err(_) => FsKind::Missing,
    }
}

/// 監視の 1 バッチを索引に取り込んだ結果（wiki_on_fs の AppHandle に依らない部分）
#[derive(Debug, PartialEq, Eq)]
enum FsOutcome {
    /// vault の root そのものが無い（消えた・改名された）= 取り込まずに知らせる
    RootGone,
    /// 取り込むものが無かった（隠し・vault の外だけ・索引が何も変えなかった）= 画面に知らせない
    Nothing,
    /// 索引を更新した = "wiki-changed" に載せる種別
    Changed(WikiChanged),
}

/// 監視が拾ったパス（絶対パス）を索引に取り込む。種別は core の取り込み結果（`refresh_paths` の `Refreshed`）から。
/// フォルダ・索引に無いパスが消えた = stat の全走査（`refresh`）= 何が増減したか分からないので all
fn wiki_fs_ingest(
    ix: &mut openref_core::wiki::Index,
    root: &Path,
    paths: Vec<PathBuf>,
    kind: impl Fn(&Path) -> FsKind,
) -> Result<FsOutcome, openref_core::CoreError> {
    let plan = wiki_fs_plan(root, root.is_dir(), paths, kind, |rel| {
        ix.page(rel).ok().flatten().is_some()
    });
    if plan.root_gone {
        return Ok(FsOutcome::RootGone);
    }
    // root そのもののパスだけ（picked は空）でも全走査する
    if plan.full {
        ix.refresh()?;
        return Ok(FsOutcome::Changed(WikiChanged::everything()));
    }
    if plan.picked.is_empty() {
        return Ok(FsOutcome::Nothing);
    }
    let abs: Vec<&PathBuf> = plan.picked.iter().map(|(_, a)| a).collect();
    let changed = WikiChanged::from(ix.refresh_paths(&abs)?);
    Ok(if changed.is_empty() {
        FsOutcome::Nothing
    } else {
        FsOutcome::Changed(changed)
    })
}

/// 監視が拾ったパス（絶対パス）を索引に取り込んで "wiki-changed"。root が消えた・改名されたら "wiki-error"。
/// 戻り値 = 取り込めた（false = root が無い・取り込みの失敗 = 索引が古いかもしれない）
fn wiki_on_fs(app: &tauri::AppHandle, ip: &Path, root: &Path, paths: Vec<PathBuf>) -> bool {
    let res = openref_core::wiki::with_index(ip, root, |ix| {
        wiki_fs_ingest(ix, root, paths.clone(), wiki_fs_kind)
    });
    let en = ui_en(app);
    match res {
        Ok(FsOutcome::RootGone) => {
            wiki_error(
                app,
                format!(
                    "{}: {}",
                    tr(en, "ワークスペースのフォルダが見つかりません（消えた・改名された）。左の木のワークスペース名を押して選び直してください", "The workspace folder is gone (deleted or renamed). Click the workspace name in the tree to choose it again"),
                    root.display()
                ),
            );
            false
        }
        Ok(FsOutcome::Nothing) => true,
        Ok(FsOutcome::Changed(c)) => {
            wiki_emit(app, "wiki-changed", c);
            true
        }
        Err(e) => {
            wiki_error(app, format!("{}: {e}", tr(en, "変更を索引に取り込めません", "Could not update the index")));
            false
        }
    }
}

/// 有効 / 無効を書いて揃える。有効にするのは root が検査を通るときだけ（未設定・検査に落ちたら Err = 書かない）。
/// 戻り値 = このプロセスで vault の配信を始めていて、再起動まで続く（OFF にしたときだけ true になりうる）
pub(crate) fn wiki_enable(app: &tauri::AppHandle, on: bool) -> Result<bool, String> {
    if on {
        let mut ctx = wiki_ctx(app)?;
        ctx.enabled = true;
        ctx.ready().map_err(err)?;
    }
    if !save_setting(app, "wiki_enabled", if on { "1" } else { "0" }) {
        return Err(tr(
            ui_en(app),
            "設定 wiki_enabled を保存できません",
            "Could not save the wiki_enabled setting",
        ));
    }
    WIKI_ON.store(on, std::sync::atomic::Ordering::Relaxed);
    let _ = app.state::<SettingsItems>().wiki_enabled.set_checked(on);
    // チャットの項目は Wiki が有効なときだけ押せる。OFF にしたら子プロセスを止める
    wiki_chat_sync_menu(app);
    wiki_chat_emit_enabled(app);
    if !on {
        wiki_chat_stop_bg(app);
    }
    wiki_apply(
        app,
        if on {
            WikiScan::Refresh
        } else {
            WikiScan::None
        },
    )?;
    Ok(!on
        && !app
            .state::<WikiRuntime>()
            .allowed
            .lock()
            .map_err(err)?
            .is_empty())
}

/// root を検査して canonical な値で書く。有効なら揃えて走査する（root が変われば索引は作り直し = core が meta.root で捨てる）。
/// 戻り値 = (書いた root, 前の root がこのプロセスで配信されたまま = 再起動を勧める)
pub(crate) fn wiki_choose_root(app: &tauri::AppHandle, path: &Path) -> Result<(PathBuf, bool), String> {
    let ctx = wiki_ctx(app)?;
    let home = ctx.home.as_deref().ok_or_else(|| {
        tr(
            ui_en(app),
            "HOME が未設定のためワークスペースのフォルダを検査できません",
            "HOME is not set, so the workspace folder cannot be checked",
        )
    })?;
    let root = openref_core::wiki::check_root(path, home, ctx.app_dir.as_deref()).map_err(err)?;
    if !save_setting(app, "wiki_root", &root.to_string_lossy()) {
        return Err(tr(
            ui_en(app),
            "設定 wiki_root を保存できません",
            "Could not save the wiki_root setting",
        ));
    }
    let stale = {
        let rt = app.state::<WikiRuntime>();
        let allowed = rt.allowed.lock().map_err(err)?;
        wiki_root_stale(&allowed, &root)
    };
    // vault を替えたらチャットの子プロセス（cwd = 前の vault）を止める
    if app
        .try_state::<wiki_chat::Chat>()
        .and_then(|c| c.root())
        .is_some_and(|r| r != root)
    {
        wiki_chat_stop_bg(app);
    }
    wiki_chat_emit_enabled(app);
    if ctx.enabled {
        wiki_apply(app, WikiScan::Refresh)?;
    } else {
        wiki_emit(app, "wiki-state", ());
    }
    Ok((root, stale))
}

/// メニューの [有効] を入れたときの段取り（純関数）
#[derive(Debug, PartialEq)]
enum EnableStep {
    /// 有効化へ進む（Some = 先にこのフォルダを root にする）
    Go(Option<PathBuf>),
    /// フォルダの選択を取り消した = チェックを戻して何もしない
    Cancel,
}

/// root が使えれば（ready）選ばせずに進む。使えなければ pick（フォルダの選択ダイアログ）を 1 回だけ呼び、取消なら Cancel
fn wiki_enable_step(ready: bool, pick: impl FnOnce() -> Option<PathBuf>) -> EnableStep {
    if ready {
        return EnableStep::Go(None);
    }
    match pick() {
        Some(p) => EnableStep::Go(Some(p)),
        None => EnableStep::Cancel,
    }
}

/// フォルダの選択ダイアログ（main thread の外から呼ぶ = blocking）。取り消し = None
pub(crate) fn wiki_pick_folder(app: &tauri::AppHandle, en: bool) -> Option<PathBuf> {
    wiki_pick_folder_titled(
        app,
        if en {
            "Choose the OpenNote folder"
        } else {
            "OpenNote のフォルダを選ぶ"
        },
    )
}

pub(crate) fn wiki_pick_folder_titled(app: &tauri::AppHandle, title: &str) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    let mut d = app.dialog().file().set_title(title);
    if let Some(docs) = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Documents"))
        .filter(|p| p.is_dir())
    {
        d = d.set_directory(docs);
    }
    d.blocking_pick_folder().and_then(|f| f.into_path().ok())
}

pub(crate) fn wiki_restart_notice(app: &tauri::AppHandle, en: bool) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let (title, msg) = if en {
        ("OpenNote", "The workspace folder was changed. Restart OpenRef to stop serving files from the previous folder.")
    } else {
        ("OpenNote", "ワークスペースのフォルダを変更しました。前のフォルダのファイルは OpenRef を再起動するまで画面から読める状態のままです。再起動してください。")
    };
    app.dialog()
        .message(msg)
        .title(title)
        .kind(MessageDialogKind::Info)
        .blocking_show();
}

/// メニューの toast の文（言語別）
pub(crate) fn wiki_toast(
    app: &tauri::AppHandle,
    en: bool,
    ok: bool,
    ja: &str,
    en_s: &str,
    detail: Option<String>,
) {
    let mut s = format!(
        "{}{}",
        if ok { "✓ " } else { "✗ " },
        if en { en_s } else { ja }
    );
    if let Some(d) = detail {
        s += &format!(": {d}");
    }
    let _ = app.emit("menu-toast", s);
}

/// OFF にした後の一言（このプロセスで配信を始めていれば再起動まで続く）
const WIKI_OFF_SERVED_JA: &str = "OpenNote を無効にしました（ワークスペースのファイルは OpenRef を再起動するまで画面から読める状態のままです）";
const WIKI_OFF_SERVED_EN: &str = "OpenNote disabled (files in the workspace stay readable by the app until OpenRef is restarted)";

/// Settings › OpenNote の操作（main thread の外で呼ぶ）
pub(crate) fn wiki_menu(app: &tauri::AppHandle, id: &str, en: bool) {
    let items = app.state::<SettingsItems>();
    match id {
        WIKI_ENABLED_ID => {
            // クリックで反転した後の値。読めなければ何も変えない（取り違えて無効にしない）
            let on = match items.wiki_enabled.is_checked() {
                Ok(c) => c,
                Err(e) => return eprintln!("openref-app: メニューのチェック状態を読めません: {e}"),
            };
            if !on {
                match wiki_enable(app, false) {
                    Ok(true) => {
                        wiki_toast(app, en, true, WIKI_OFF_SERVED_JA, WIKI_OFF_SERVED_EN, None)
                    }
                    Ok(false) => wiki_toast(
                        app,
                        en,
                        true,
                        "OpenNote を無効にしました",
                        "OpenNote disabled",
                        None,
                    ),
                    Err(e) => {
                        let _ = items.wiki_enabled.set_checked(true);
                        wiki_toast(
                            app,
                            en,
                            false,
                            "OpenNote を無効にできません",
                            "Could not disable OpenNote",
                            Some(e),
                        );
                    }
                }
                return;
            }
            // フォルダが未設定・検査に落ちる（消えた・動いた）なら選ばせる。取り消したらチェックを戻す
            let ready = wiki_ctx(app)
                .map(|mut c| {
                    c.enabled = true;
                    c.ready().is_ok()
                })
                .unwrap_or(false);
            match wiki_enable_step(ready, || wiki_pick_folder(app, en)) {
                EnableStep::Cancel => {
                    let _ = items.wiki_enabled.set_checked(false);
                    return;
                }
                EnableStep::Go(Some(p)) => {
                    if let Err(e) = wiki_choose_root(app, &p) {
                        let _ = items.wiki_enabled.set_checked(false);
                        return wiki_toast(
                            app,
                            en,
                            false,
                            "このフォルダは使えません",
                            "This folder cannot be used",
                            Some(e),
                        );
                    }
                }
                EnableStep::Go(None) => {}
            }
            match wiki_enable(app, true) {
                Ok(_) => wiki_toast(
                    app,
                    en,
                    true,
                    "OpenNote を有効にしました（索引を作成中）",
                    "OpenNote enabled (indexing)",
                    None,
                ),
                Err(e) => {
                    let _ = items.wiki_enabled.set_checked(false);
                    wiki_toast(
                        app,
                        en,
                        false,
                        "OpenNote を有効にできません",
                        "Could not enable OpenNote",
                        Some(e),
                    );
                }
            }
        }
        WIKI_REINDEX_ID => match wiki_reindex_now(app) {
            Ok(()) => wiki_toast(
                app,
                en,
                true,
                "OpenNote の索引を作り直しています",
                "Rebuilding the OpenNote index",
                None,
            ),
            Err(e) => wiki_toast(
                app,
                en,
                false,
                "OpenNote の索引を作り直せません",
                "Could not rebuild the OpenNote index",
                Some(e),
            ),
        },
        WIKI_DROP_INDEX_ID => {
            // 削除の成否と、その後の張り直し（有効なときだけ）の成否は分けて出す
            let removed = wiki_ctx(app).and_then(|ctx| {
                let dir = ctx.app_dir.clone().ok_or_else(|| {
                    tr(
                        en,
                        "in-memory の DB では索引の置き場がありません",
                        "An in-memory database has no place for the index",
                    )
                })?;
                wiki_watch_stop(&app.state::<WikiRuntime>());
                openref_core::wiki::index::remove_index(&dir.join(openref_core::wiki::index::INDEX_FILE))
                    .map_err(err)?;
                Ok(ctx.enabled)
            });
            match removed {
                Ok(enabled) => {
                    wiki_toast(
                        app,
                        en,
                        true,
                        "OpenNote の索引を削除しました",
                        "OpenNote index deleted",
                        None,
                    );
                    // 有効なら監視を張り直す（索引は次の読みで作り直る）
                    if enabled {
                        if let Err(e) = wiki_apply(app, WikiScan::None) {
                            wiki_toast(
                                app,
                                en,
                                false,
                                "OpenNote の監視を張り直せません",
                                "Could not restart OpenNote file watching",
                                Some(e),
                            );
                        }
                    }
                }
                Err(e) => wiki_toast(
                    app,
                    en,
                    false,
                    "OpenNote の索引を削除できません",
                    "Could not delete the OpenNote index",
                    Some(e),
                ),
            }
        }
        _ => {}
    }
}

/// 再索引 = 監視を張り直してから中身を捨てて全走査（背景）。無効・root が使えなければ Err
pub(crate) fn wiki_reindex_now(app: &tauri::AppHandle) -> Result<(), String> {
    wiki_ctx(app)?.ready().map_err(err)?;
    wiki_apply(app, WikiScan::Rebuild)
}

// ---------- OpenNote: 参照フォルダと URL の別窓 ----------
// 参照フォルダ = vault の外の読み取り専用のルート（設定 wiki_ref_dirs = JSON の文字列の配列・GUI 専用の command で足す / 外す）。
// その下の pdf / html / 画像 / 動画 / 音声への file:// リンクは、vault の中のファイルと同じ内蔵の表示で開く（wiki_read_ref）。
// root と同じく asset scope に足すだけ（allow_directory は取り消せない = 外しても再起動まで配信される）で、索引・MCP の範囲は広げない。
// URL（http(s)）は設定 wiki_url_window なら OpenRef の別窓（WKWebView・capabilities に無い = IPC を呼べない）で、でなければ既定のブラウザで開く

/// 設定 wiki_ref_dirs（JSON の文字列の配列）
pub(crate) const WIKI_REF_DIRS_KEY: &str = "wiki_ref_dirs";
/// URL の別窓の label（1 枚を使い回す）
pub(crate) const WIKI_URL_LABEL: &str = "wiki-url";

/// 設定の値 → 参照フォルダ。未設定・空は空。JSON の文字列の配列として読めなければ Err（黙って空にしない = 読めないことを知らせ、
/// 次の追加で旧い一覧を上書きしない）
pub(crate) fn wiki_ref_dirs_parse(v: Option<&str>, en: bool) -> Result<Vec<PathBuf>, String> {
    let Some(s) = v.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };
    let list: Vec<String> = serde_json::from_str(s).map_err(|e| {
        format!(
            "{}: {e}",
            tr(
                en,
                "設定 wiki_ref_dirs を解釈できません（JSON の文字列の配列ではありません）",
                "The wiki_ref_dirs setting could not be read (not a JSON array of strings)"
            )
        )
    })?;
    let mut out: Vec<PathBuf> = Vec::new();
    for s in list {
        if s.trim().is_empty() {
            continue;
        }
        let p = PathBuf::from(s);
        // 重複は 1 つに（一覧の描画の key が重ならない）
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

/// 壊れた設定の退避先の鍵（UTC の日時）
pub(crate) fn wiki_ref_dirs_broken_key(secs: i64) -> String {
    let t: libc::time_t = secs as libc::time_t;
    // SAFETY: gmtime_r は渡した tm を埋めるだけ
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let stamp = if unsafe { libc::gmtime_r(&t, &mut tm) }.is_null() {
        format!("{secs}")
    } else {
        format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    };
    format!("{WIKI_REF_DIRS_KEY}.broken-{stamp}")
}

/// 設定 wiki_ref_dirs の生の値と Ctx（DB のロックは読んだら外す）
pub(crate) fn wiki_ref_dirs_raw(
    app: &tauri::AppHandle,
) -> Result<(Option<String>, openref_core::wiki::Ctx), String> {
    let st = app.state::<AppState>();
    let db = lock(&st.db)?;
    Ok((
        db.setting(WIKI_REF_DIRS_KEY).map_err(err)?,
        openref_core::wiki::Ctx::from_db(&db).map_err(err)?,
    ))
}

/// 設定の参照フォルダ（解釈できなければ Err）のうち、今も参照フォルダに使えるもの
pub(crate) fn wiki_ref_dirs(app: &tauri::AppHandle) -> Result<Vec<PathBuf>, String> {
    let (raw, ctx) = wiki_ref_dirs_raw(app)?;
    Ok(wiki_ref_dirs_usable(
        wiki_ref_dirs_parse(raw.as_deref(), ui_en(app))?,
        &ctx,
    ))
}

/// [追加…] の判断（純関数の戻り値）
#[derive(Debug, PartialEq)]
pub(crate) struct WikiRefAdd {
    /// 追加の後の一覧（設定に書く値 = 生の一覧 + 追加分。検査に通らないものも残す）
    pub(crate) dirs: Vec<PathBuf>,
    /// 一覧が変わった（false = すでにあった = 書かない）
    pub(crate) changed: bool,
    /// 設定が解釈できなかった理由（Some = 生の値を退避してから dirs を書く）
    pub(crate) broken: Option<String>,
    /// scope への記録の種類
    pub(crate) kind: WikiDir,
}

/// [追加…]: 設定の生の値（解釈できなければ退避して作り直す）に canonical なフォルダを足す。追加・削除は検査に通らないものを含む
/// 生の一覧に対して行う（未接続のディスクや見えない Drive の参照フォルダを消さない）。検査（wiki_ref_dirs_usable）は配信と表示だけ
pub(crate) fn wiki_ref_dirs_for_add(raw: Option<&str>, canon: &Path, en: bool) -> WikiRefAdd {
    let (mut dirs, broken) = match wiki_ref_dirs_parse(raw, en) {
        Ok(d) => (d, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let changed = !dirs.iter().any(|d| d == canon);
    if changed {
        dirs.push(canon.to_path_buf());
    }
    WikiRefAdd {
        dirs,
        changed,
        broken,
        kind: WikiDir::Ref,
    }
}

/// 参照フォルダのうち、今も参照フォルダに使えるもの（追加のときと同じ検査を使うたびにやり直す = 設定の値が書き換えられていても・
/// 後から symlink に替わっていても、ホームや Library を配信しない）。検査に通らないものは黙って外す（stderr に 1 行）。
/// canonicalize するので main thread では呼ばない
pub(crate) fn wiki_ref_dirs_usable(dirs: Vec<PathBuf>, ctx: &openref_core::wiki::Ctx) -> Vec<PathBuf> {
    let Some(home) = ctx.home.as_deref() else {
        return Vec::new();
    };
    dirs.into_iter()
        .filter(|d| {
            match wiki_ref_dir_check(
                d,
                home,
                ctx.root.as_deref().map(Path::new),
                ctx.app_dir.as_deref(),
                false, // 理由は stderr に出すだけ（日本語）
            ) {
                Ok(c) if &c == d => true,
                Ok(_) => {
                    eprintln!(
                        "openref-app: wiki: 参照フォルダ {} は canonical でないので外します",
                        d.display()
                    );
                    false
                }
                Err(e) => {
                    eprintln!(
                        "openref-app: wiki: 参照フォルダ {} を外します: {e}",
                        d.display()
                    );
                    false
                }
            }
        })
        .collect()
}

pub(crate) fn wiki_save_ref_dirs(app: &tauri::AppHandle, dirs: &[PathBuf]) -> Result<(), String> {
    let v = serde_json::to_string(
        &dirs
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    )
    .map_err(err)?;
    if save_setting(app, WIKI_REF_DIRS_KEY, &v) {
        Ok(())
    } else {
        Err(tr(
            ui_en(app),
            "設定 wiki_ref_dirs を保存できません",
            "Could not save the wiki_ref_dirs setting",
        ))
    }
}

/// 参照フォルダに使えるか: vault の root と同じ検査（絶対・実在・ディスクの最上位 / 隠しフォルダ / ホームそのもの / OpenRef のデータの
/// 置き場 / iCloud・CloudStorage の最上位は不可）に加えて、vault そのものとその中は不可（参照フォルダにしなくても内蔵で開ける）。戻り値 = canonical
pub(crate) fn wiki_ref_dir_check(
    p: &Path,
    home: &Path,
    root: Option<&Path>,
    app_dir: Option<&Path>,
    en: bool,
) -> Result<PathBuf, String> {
    let canon = openref_core::wiki::check_root(p, home, app_dir).map_err(|e| {
        // check_root の理由（BadInput の中身）をそのまま、主語だけ替えて出す
        let m = match e {
            openref_core::CoreError::BadInput(m) => m,
            e => e.to_string(),
        };
        m.replacen("ワークスペースのフォルダに", "参照フォルダに", 1)
    })?;
    if root.is_some_and(|r| openref_core::wiki::strip_root(r, &canon).is_some()) {
        let c = canon.display();
        return Err(tr(
            en,
            &format!("参照フォルダに {c} は使えません（ワークスペースの中です = そのまま開けます）"),
            &format!("{c} cannot be a reference folder (it is inside the workspace, so it already opens as is)"),
        ));
    }
    Ok(canon)
}

/// フォルダを asset scope に足す（まだなら）。root と参照フォルダの両方がここを通る（記録は種類ごと）
pub(crate) fn wiki_allow_dir(app: &tauri::AppHandle, d: &Path, kind: WikiDir) -> Result<(), String> {
    let rt = app.state::<WikiRuntime>();
    let mut allowed = rt.allowed.lock().map_err(err)?;
    if !allowed.contains(d) {
        app.asset_protocol_scope()
            .allow_directory(d, true)
            .map_err(err)?;
    }
    allowed.insert(d, kind);
    Ok(())
}

/// 参照フォルダの中のファイルの実体。given = 絶対パス（`.` / `..` の区画・`.` 始まりの区画は拒否）。canonical も同じ参照フォルダの中
/// （symlink で外へ出ない）で通常ファイルのときだけ。戻り値 = (参照フォルダ, canonical)
pub(crate) fn wiki_ref_target(dirs: &[PathBuf], given: &str, en: bool) -> Result<(PathBuf, PathBuf), String> {
    use openref_core::wiki::paths::rel_inside;
    let g = Path::new(given.trim());
    if !g.is_absolute() || wiki_has_dot_segment(g) {
        return Err(format!(
            "{}: {given}",
            tr(en, "絶対パスではありません", "not an absolute path")
        ));
    }
    let inside = |d: &Path, p: &Path| rel_inside(d, p).is_some();
    let dir = dirs.iter().find(|d| inside(d, g)).ok_or_else(|| {
        format!(
            "{}: {given}",
            tr(
                en,
                "参照フォルダの中ではありません",
                "not under a reference folder"
            )
        )
    })?;
    let canon = std::fs::canonicalize(g)
        .map_err(|_| format!("{}: {given}", tr(en, "見つかりません", "not found")))?;
    if !inside(dir, &canon) {
        return Err(format!(
            "{}: {given}",
            tr(
                en,
                "参照フォルダの外です（symlink）",
                "outside the reference folder (symlink)"
            )
        ));
    }
    if !std::fs::metadata(&canon).map_err(err)?.is_file() {
        return Err(wiki_not_regular_file(en, given));
    }
    Ok((dir.clone(), canon))
}

/// 参照フォルダの中のファイルの見せ方（純関数の戻り値）
#[derive(Debug, PartialEq)]
pub(crate) enum WikiRefShow {
    /// probe = 開けるかだけ答える（種類が media / html なら ok）
    Probe,
    /// 画像・動画・音声・pdf = 種類だけ返す（中身は asset URL）
    Media,
    /// html = 本文を読んで返す
    Html,
}

/// 参照フォルダの中のファイルを内蔵で見せるか: media / html だけ（md・その他は Err = 画面は外部で開く）。
/// html は索引の本文の上限（BODY_MAX）まで。probe は種類の判定だけで大きさは見ない
pub(crate) fn wiki_ref_show(
    kind: openref_core::wiki::scan::Kind,
    probe: bool,
    len: u64,
    name: &str,
    en: bool,
) -> Result<WikiRefShow, String> {
    use openref_core::wiki::{index, scan};
    if !matches!(kind, scan::Kind::Media | scan::Kind::Html) {
        return Err(format!(
            "{}: {name}",
            tr(en, "OpenRef では表示しません", "not shown in OpenRef")
        ));
    }
    if probe {
        return Ok(WikiRefShow::Probe);
    }
    if kind == scan::Kind::Media {
        return Ok(WikiRefShow::Media);
    }
    if len as i64 > index::BODY_MAX {
        let max = index::BODY_MAX;
        return Err(tr(
            en,
            &format!("{name} は大きすぎて読めません（{len} バイト・上限 {max} バイト）"),
            &format!("{name} is too large to read ({len} bytes; limit {max} bytes)"),
        ));
    }
    Ok(WikiRefShow::Html)
}

/// 参照フォルダの html の本文の欄: 題（`<title>` が空でなければ名前に代えて）・core の body_fields（chars・truncated・content）・
/// links は空（リンクの解決は画面側）
pub(crate) fn wiki_ref_html_fields(
    out: &mut serde_json::Value,
    kind: openref_core::wiki::scan::Kind,
    bytes: &[u8],
    max_chars: usize,
) {
    use openref_core::wiki::commands::{body_fields, put_fields};
    let (parsed, body) = body_fields(kind, bytes, max_chars);
    if let Some(t) = parsed.title.filter(|t| !t.trim().is_empty()) {
        out["title"] = serde_json::json!(t);
    }
    put_fields(out, body);
    out["links"] = serde_json::json!([]);
}

fn wiki_url_title(u: &tauri::Url) -> String {
    u.host_str()
        .map(str::to_string)
        .unwrap_or_else(|| "OpenRef".into())
}

/// 別窓の中の遷移: http(s) と about だけ（asset / tauri / file は通さない = 外部のページからアプリの中身に触れない）
pub(crate) fn wiki_url_window_nav_allowed(url: &tauri::Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "about")
}

/// 既にあれば遷移して前面に・無ければ作る。この窓は capabilities に無い = IPC（Tauri の command）を呼べない。
/// 別窓を開く要求（target=_blank・window.open）は同じ窓で開く。ダウンロードは受けない（保存先を決める UI が無い = 外部ブラウザで）
pub(crate) fn wiki_url_window_show(app: &tauri::AppHandle, url: tauri::Url) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(WIKI_URL_LABEL) {
        w.navigate(url).map_err(err)?;
        // 最小化中は set_focus が効かない（tao）= 先に戻す
        let _ = w.unminimize().and(w.show()).and(w.set_focus());
        return Ok(());
    }
    let h = app.clone();
    let title = wiki_url_title(&url);
    tauri::WebviewWindowBuilder::new(app, WIKI_URL_LABEL, tauri::WebviewUrl::External(url))
        .title(&title)
        .inner_size(1100.0, 820.0)
        .on_new_window(move |u, _| {
            if let Some(w) = h.get_webview_window(WIKI_URL_LABEL) {
                let _ = w.navigate(u);
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .on_page_load(|w, p| {
            let _ = w.set_title(&wiki_url_title(p.url()));
        })
        .on_download(|_, _| false)
        .build()
        .map_err(err)?;
    Ok(())
}

// ---------- OpenNote（チャット = Claude Code CLI の子プロセス） ----------
// 子プロセス・stream-json・変更の記録は wiki_chat.rs。ここは設定（wiki_chat_enabled）・メニュー・GUI 専用 command の本体（包みは main.rs）。
// 有効にするのは画面の同意（説明のダイアログ）から wiki_chat_set_enabled {enabled: true} だけ。メニューの [チャット] で ON を選ぶと
// 設定は変えずに "wiki-chat-consent" を出す（チェックは実際の設定に戻す）・OFF はそのまま無効にして子を止める。
// 有効 / 無効が変わったら（メニュー・command・Wiki の ON/OFF・vault の切替）"wiki-chat" {type: "enabled", enabled} を出す（画面は
// wiki_chat_status を読み直す）。{type: "state"} は子の状態だけ（wiki_chat.rs が出す）

pub(crate) const WIKI_CHAT_ID: &str = "wiki_chat";
/// 設定のキー（"1" = 有効・Wiki が有効なときだけ効く）
pub(crate) const WIKI_CHAT_KEY: &str = "wiki_chat_enabled";
/// 設定のキー（`--model` に渡す別名か model id。空 / 無し = Claude Code の既定）
pub(crate) const WIKI_CHAT_MODEL_KEY: &str = "wiki_chat_model";

/// 設定 wiki_chat_model（空なら None）
fn wiki_chat_model(app: &tauri::AppHandle) -> Option<String> {
    let st = app.state::<AppState>();
    let db = lock(&st.db).ok()?;
    db.setting(WIKI_CHAT_MODEL_KEY)
        .ok()
        .flatten()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

/// 設定に書ける model の表記（別名 opus / sonnet 等・model id・Bedrock の ARN 風の `…/…`・`name@version`。shell を通さないが、引数の形は絞る）
pub(crate) fn wiki_chat_model_ok(m: &str) -> bool {
    m.is_empty()
        || (m.len() <= 128
            && !m.starts_with('-')
            && m.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '-' | '.' | '_' | ':' | '[' | ']' | '@' | '/')
            }))
}

pub(crate) fn wiki_chat_text(en: bool) -> &'static str {
    if en {
        "Chat"
    } else {
        "チャット"
    }
}

/// (Wiki の設定, チャットの設定, アプリの DB のパス)。Db のロックは読む間だけ
pub(crate) fn wiki_chat_settings(
    app: &tauri::AppHandle,
) -> Result<(openref_core::wiki::Ctx, bool, Option<PathBuf>), String> {
    let st = app.state::<AppState>();
    let db = lock(&st.db)?;
    let ctx = openref_core::wiki::Ctx::from_db(&db).map_err(err)?;
    let on = db.setting(WIKI_CHAT_KEY).map_err(err)?.as_deref() == Some("1");
    Ok((ctx, on, db.path.clone()))
}

/// メニューの [チャット] を設定に揃える（押せるのは Wiki が有効なときだけ）。[リンクを OpenRef の窓で開く] [チャットのログ] も同じ条件で押せる
fn wiki_chat_sync_menu(app: &tauri::AppHandle) {
    let wiki_on = WIKI_ON.load(std::sync::atomic::Ordering::Relaxed);
    let on = wiki_chat_settings(app)
        .map(|(c, on, _)| c.enabled && on)
        .unwrap_or(false);
    if let Some(items) = app.try_state::<SettingsItems>() {
        let _ = items
            .wiki_chat
            .set_enabled(wiki_on)
            .and(items.wiki_chat.set_checked(on));
        let _ = items
            .wiki_url_window
            .set_enabled(wiki_on)
            .and(items.wiki_chat_log.set_enabled(wiki_on));
    }
}

pub(crate) const WIKI_URL_WINDOW_ID: &str = "wiki_url_window";
/// 設定: "1" = ページの http(s) リンクを OpenRef の別窓（WKWebView）で開く。既定 = 外部ブラウザ
pub(crate) const WIKI_URL_WINDOW_KEY: &str = "wiki_url_window";
pub(crate) const WIKI_CHAT_LOG_ID: &str = "wiki_chat_log";
/// 設定: "0" のときだけチャットのログ（ワークスペースの chat-log/）を残さない。既定 ON
pub(crate) const WIKI_CHAT_LOG_KEY: &str = "wiki_chat_log";

pub(crate) fn wiki_url_window_text(en: bool) -> &'static str {
    if en {
        "Open web links in an OpenRef window"
    } else {
        "リンク（URL）を OpenRef の窓で開く"
    }
}

pub(crate) fn wiki_chat_log_text(en: bool) -> &'static str {
    if en {
        "Keep chat logs in chat-log/"
    } else {
        "チャットのログを chat-log/ に残す"
    }
}

/// 設定 wiki_chat_log（ログを chat-log/ に残す・既定 ON）を読む（読めなければ既定 = ON）
fn wiki_chat_log_enabled(app: &tauri::AppHandle) -> bool {
    let st = app.state::<AppState>();
    let Ok(db) = lock(&st.db) else { return true };
    wiki_chat_log_on(db.setting(WIKI_CHAT_LOG_KEY).ok().flatten().as_deref())
}

/// 設定 wiki_chat_log の値 → ON か（未設定 = ON）
pub(crate) fn wiki_chat_log_on(v: Option<&str>) -> bool {
    v != Some("0")
}

/// 有効 / 無効の知らせ（子の有無によらず）
fn wiki_chat_emit_enabled(app: &tauri::AppHandle) {
    let on = wiki_chat_settings(app)
        .map(|(c, on, _)| c.enabled && on)
        .unwrap_or(false);
    wiki_emit(
        app,
        "wiki-chat",
        serde_json::json!({"type": "enabled", "enabled": on}),
    );
}

pub(crate) fn wiki_chat_emit_state(app: &tauri::AppHandle) {
    if let Some(c) = app.try_state::<wiki_chat::Chat>() {
        wiki_emit(
            app,
            "wiki-chat",
            serde_json::json!({"type": "state", "state": c.state().as_str(), "session_id": c.session_id()}),
        );
    }
}

/// 子プロセスを背景で止める（stdin を閉じる → 猶予 → SIGTERM → SIGKILL。呼び手を待たせない）
fn wiki_chat_stop_bg(app: &tauri::AppHandle) {
    if let Some(c) = app.try_state::<wiki_chat::Chat>() {
        let c = c.inner().clone();
        std::thread::spawn(move || c.stop(Duration::from_secs(3), Duration::from_secs(2)));
    }
}

/// 有効 / 無効を書く。ON は Wiki が有効で vault が使えるときだけ・OFF は子を止める（待つ）
pub(crate) fn wiki_chat_enable(app: &tauri::AppHandle, on: bool) -> Result<(), String> {
    let en = ui_en(app);
    if on {
        let (ctx, _, _) = wiki_chat_settings(app)?;
        if !ctx.enabled {
            return Err(tr(
                en,
                "OpenNote が無効です（先に Settings › OpenNote › 有効）",
                "OpenNote is disabled (enable it first in Settings › OpenNote)",
            ));
        }
        ctx.ready().map_err(err)?;
    }
    if !save_setting(app, WIKI_CHAT_KEY, if on { "1" } else { "0" }) {
        wiki_chat_sync_menu(app);
        return Err(tr(
            en,
            "設定 wiki_chat_enabled を保存できません",
            "Could not save the wiki_chat_enabled setting",
        ));
    }
    if !on {
        if let Some(c) = app.try_state::<wiki_chat::Chat>() {
            c.stop(Duration::from_secs(3), Duration::from_secs(2));
        }
    }
    wiki_chat_sync_menu(app);
    wiki_chat_emit_enabled(app);
    Ok(())
}

/// メニューの [チャット]（main thread の外で呼ぶ）
pub(crate) fn wiki_chat_menu(app: &tauri::AppHandle, en: bool) {
    let items = app.state::<SettingsItems>();
    let checked = match items.wiki_chat.is_checked() {
        Ok(c) => c,
        Err(e) => return eprintln!("openref-app: メニューのチェック状態を読めません: {e}"),
    };
    if checked {
        // ON は画面の同意を経てから（チェックは同意されるまで今の設定のまま）
        wiki_chat_sync_menu(app);
        wiki_emit(app, "wiki-chat-consent", ());
        return;
    }
    if let Err(e) = wiki_chat_enable(app, false) {
        wiki_toast(
            app,
            en,
            false,
            "チャットを無効にできません",
            "Could not disable the chat",
            Some(e),
        );
    }
}

/// チャットが使えない理由を決める材料（調べた結果だけ・I/O なし）
#[derive(Debug, Default, Clone, PartialEq)]
struct WikiChatProbe {
    /// OpenNote が有効か
    wiki_on: bool,
    /// チャットの設定が ON か
    chat_on: bool,
    /// root が使えない理由（ready の Err）
    ready_error: Option<String>,
    /// claude コマンドが見つかったか
    cli: bool,
    /// ログインしているか（None = 分からない = 止めない）
    logged_in: Option<bool>,
    /// openref-mcp（wiki_write のある版）が見つかったか
    mcp: bool,
}

/// 使えない理由（純関数）。優先順 = Wiki 無効 › チャット無効 › root が使えない › claude が無い › 未ログイン › openref-mcp が無い。
/// None = 使える。ログインの有無が分からない（None）ときは止めない
fn wiki_chat_reason(en: bool, p: &WikiChatProbe) -> Option<String> {
    if !p.wiki_on {
        return Some(tr(
            en,
            "OpenNote が無効です",
            "OpenNote is disabled",
        ));
    }
    if !p.chat_on {
        return Some(tr(
            en,
            "チャットは無効です（Settings › OpenNote › チャット）",
            "The chat is disabled (Settings › OpenNote › Chat)",
        ));
    }
    if let Some(e) = &p.ready_error {
        return Some(e.clone());
    }
    if !p.cli {
        return Some(tr(
            en,
            "Claude Code（claude コマンド）が見つかりません。Claude Code をインストールしてください",
            "Claude Code (the claude command) was not found. Install Claude Code",
        ));
    }
    if p.logged_in == Some(false) {
        return Some(tr(
            en,
            "Claude Code にログインしていません。[ログイン] でターミナルに claude auth login を開きます",
            "Claude Code is not logged in. [Log in] opens claude auth login in Terminal",
        ));
    }
    if !p.mcp {
        return Some(tr(
            en,
            "openref-mcp が見つからないか古い版です（wiki_write がありません）。OpenRef を入れ直してください",
            "openref-mcp was not found or is too old (no wiki_write). Reinstall OpenRef",
        ));
    }
    None
}

/// 今の状態（types.ts の WikiChatStatus）。claude の検出・`--version`・`auth status`・openref-mcp の版の確認を走らせる（blocking）。
/// `--version` は実体の mtime ごと・`auth status` は AUTH_TTL の間だけ覚えた分を使う（force = 覚えた分を使わずに読む = [再確認]）
pub(crate) fn wiki_chat_status_now(app: &tauri::AppHandle, force: bool) -> Result<serde_json::Value, String> {
    let en = ui_en(app);
    let (ctx, on, db) = wiki_chat_settings(app)?;
    let chat = app.state::<wiki_chat::Chat>();
    // 無効なら何も調べない（claude の検出・auth status を起こさない）
    if !(on && ctx.enabled) {
        let reason = wiki_chat_reason(
            en,
            &WikiChatProbe {
                wiki_on: ctx.enabled,
                chat_on: on,
                ..WikiChatProbe::default()
            },
        )
        .unwrap_or_default();
        return Ok(serde_json::json!({
            "enabled": false, "cli": null, "version": null, "logged_in": null, "mcp": null,
            "reason": reason, "state": chat.state().as_str(), "session_id": chat.session_id(),
        }));
    }
    let home = ctx.home.as_deref();
    let cli = wiki_chat::find_cli(home, std::env::var_os("PATH").as_deref());
    let version = cli.as_ref().and_then(wiki_chat::cli_version);
    let auth = cli
        .as_ref()
        .and_then(|c| wiki_chat::cli_auth_cached(c, force));
    let logged_in = auth.as_ref().map(|a| a.logged_in);
    let mcp = db
        .as_deref()
        .and_then(|db| wiki_chat::find_mcp(&wiki_chat::mcp_candidates(home), db));
    let ready = ctx.ready();
    let reason = wiki_chat_reason(
        en,
        &WikiChatProbe {
            wiki_on: ctx.enabled,
            chat_on: on,
            ready_error: ready.as_ref().err().map(|e| e.to_string()),
            cli: cli.is_some(),
            logged_in,
            mcp: mcp.is_some(),
        },
    );
    let session_id = chat.session_id().or_else(|| {
        let (_, root) = ready.as_ref().ok()?;
        wiki_chat::load_session(ctx.app_dir.as_deref()?, root)
    });
    let mut v = serde_json::json!({
        "enabled": true,
        "cli": cli.as_ref().map(|c| c.path.to_string_lossy().into_owned()),
        "version": version,
        "logged_in": logged_in,
        "auth": auth,
        "model": wiki_chat_model(app),
        "mcp": mcp.map(|p| p.to_string_lossy().into_owned()),
        "state": chat.state().as_str(),
        "session_id": session_id,
    });
    if let Some(r) = reason {
        v["reason"] = serde_json::json!(r);
    }
    Ok(v)
}

/// command の引数（`args: Option<Value>`）を object に揃える（無い・object でない = `{}`）。参照フォルダ・チャットの command が使う
pub(crate) fn obj_args(args: Option<serde_json::Value>) -> serde_json::Value {
    args.filter(|a| a.is_object())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// wiki_chat_status の {force: true}（画面の [再確認]）。true 以外（無い・false・文字列）は自動の読み直し
pub(crate) fn wiki_chat_force(args: Option<serde_json::Value>) -> bool {
    obj_args(args).get("force").and_then(|v| v.as_bool()) == Some(true)
}

pub(crate) async fn wiki_chat_start_now(
    app: tauri::AppHandle,
    new: bool,
) -> Result<serde_json::Value, String> {
    blocking(move || {
        let en = ui_en(&app);
        let (ctx, on, db) = wiki_chat_settings(&app)?;
        if !(on && ctx.enabled) {
            return Err(tr(en, "チャットは無効です（Settings › OpenNote › チャット）", "The chat is disabled (Settings › OpenNote › Chat)"));
        }
        let (_, root) = ctx.ready().map_err(err)?;
        let no_db = || tr(en, "in-memory の DB ではチャットを使えません", "The chat is not available with an in-memory database");
        let app_dir = ctx.app_dir.clone().ok_or_else(no_db)?;
        // 相対の OPENREF_DB でも子（cwd = vault）が同じ DB を開くよう絶対パスに
        let db = std::path::absolute(db.ok_or_else(no_db)?).map_err(err)?;
        let home = ctx.home.as_deref();
        let cli = wiki_chat::find_cli(home, std::env::var_os("PATH").as_deref())
            .ok_or_else(|| tr(en, "Claude Code（claude コマンド）が見つかりません", "Claude Code (the claude command) was not found"))?;
        // 起こす前は覚えた分を使わずに読む（読んだ結果は後の status の読み直しが使う）
        if wiki_chat::cli_auth_cached(&cli, true).map(|a| a.logged_in) == Some(false) {
            return Err(tr(en, "Claude Code にログインしていません（[ログイン] でターミナルに claude auth login を開きます）", "Claude Code is not logged in ([Log in] opens claude auth login in Terminal)"));
        }
        let mcp = wiki_chat::find_mcp(&wiki_chat::mcp_candidates(home), &db)
            .ok_or_else(|| tr(en, "openref-mcp が見つからないか古い版です（wiki_write がありません）", "openref-mcp was not found or is too old (no wiki_write)"))?;
        let saved = wiki_chat::load_session(&app_dir, &root);
        // vault の CLAUDE.md（ふるまいの規則・user が編集する）が無ければ既定を置く。置いた / 置けなかった（規則の無いまま Claude が動く）は notice に
        let md_notice = match wiki_chat::ensure_claude_md(&root) {
            Ok(true) => Some(tr(en, "ワークスペースに CLAUDE.md（Claude のふるまいの規則）を置きました。自由に編集できます", "Placed CLAUDE.md (rules for Claude) in the workspace. Edit it freely")),
            Ok(false) => None,
            Err(e) => {
                eprintln!("openref-app: wiki chat: ワークスペースに CLAUDE.md を置けません: {e}");
                Some(tr(en, &format!("ワークスペースに CLAUDE.md（Claude のふるまいの規則）を置けませんでした: {e}"), &format!("Could not place CLAUDE.md (rules for Claude) in the workspace: {e}")))
            }
        };
        let launch = wiki_chat::Launch { cli, mcp, db, root: root.clone(), model: wiki_chat_model(&app), log: wiki_chat_log_enabled(&app) };
        let mut started = app.state::<wiki_chat::Chat>().start(&launch, saved, new, en)?;
        if let Some(n) = md_notice {
            started.notice = Some(match started.notice.take() {
                Some(prev) => format!("{prev} · {n}"),
                None => n,
            });
        }
        if let Err(e) = wiki_chat::save_session(&app_dir, &root, &started.session_id) {
            eprintln!("openref-app: wiki chat: 会話の id を保存できません（次の起動で続きになりません）: {e}");
        }
        serde_json::to_value(&started).map_err(err)
    })
    .await
}

#[cfg(test)]
mod wiki_exit {
    use super::*;

    #[test]
    fn wiki_off_never_holds() {
        let mut st = ExitState::default();
        let now = Instant::now();
        assert_eq!(
            exit_hold(&mut st, false, ExitKind::Exit(0), now),
            Hold::Allow
        );
        assert_eq!(
            exit_hold(&mut st, false, ExitKind::Close("main".into()), now),
            Hold::Allow
        );
        assert!(st.pending.is_none());
    }

    #[test]
    fn holds_once_and_double_request_only_prevents() {
        let mut st = ExitState::default();
        let now = Instant::now();
        assert_eq!(
            exit_hold(&mut st, true, ExitKind::Exit(0), now),
            Hold::Ask(1)
        );
        // ⌘Q 連打 = 保留するだけ（尋ねない・終了しない）
        assert_eq!(
            exit_hold(&mut st, true, ExitKind::Exit(0), now),
            Hold::Prevent
        );
        assert_eq!(
            exit_hold(&mut st, true, ExitKind::Close("main".into()), now),
            Hold::Prevent
        );
        assert_eq!(
            st.pending,
            Some((ExitKind::Exit(0), 1)),
            "窓を閉じる要求で終了を弱めない"
        );
        // restart は保留しない
        assert_eq!(
            exit_hold(
                &mut ExitState::default(),
                true,
                ExitKind::Exit(tauri::RESTART_EXIT_CODE),
                now
            ),
            Hold::Allow
        );
    }

    #[test]
    fn close_then_quit_upgrades_to_exit() {
        let mut st = ExitState::default();
        let now = Instant::now();
        assert_eq!(
            exit_hold(&mut st, true, ExitKind::Close("main".into()), now),
            Hold::Ask(1)
        );
        assert_eq!(
            exit_hold(&mut st, true, ExitKind::Exit(0), now),
            Hold::Prevent
        );
        assert_eq!(exit_take(&mut st, None, true, now), Some(ExitKind::Exit(0)));
    }

    #[test]
    fn proceed_allows_own_exit_and_cancel_resets() {
        let now = Instant::now();
        let mut st = ExitState::default();
        exit_hold(&mut st, true, ExitKind::Close("main".into()), now);
        assert_eq!(
            exit_take(&mut st, None, true, now),
            Some(ExitKind::Close("main".into()))
        );
        // 自分の close が出す CloseRequested と、最後の窓が閉じた後の ExitRequested は保留しない
        assert_eq!(
            exit_hold(
                &mut st,
                true,
                ExitKind::Close("main".into()),
                now + Duration::from_millis(5)
            ),
            Hold::Allow
        );
        assert_eq!(
            exit_hold(
                &mut st,
                true,
                ExitKind::Exit(0),
                now + Duration::from_millis(10)
            ),
            Hold::Allow
        );
        // 期限の後はまた保留する
        assert_eq!(
            exit_hold(
                &mut st,
                true,
                ExitKind::Exit(0),
                now + WIKI_EXIT_ALLOW + Duration::from_secs(1)
            ),
            Hold::Ask(2)
        );
        // 取りやめ = 何もせず保留を解く・次の要求はまた尋ねる
        let mut st = ExitState::default();
        exit_hold(&mut st, true, ExitKind::Exit(0), now);
        assert_eq!(exit_take(&mut st, None, false, now), None);
        assert!(st.pending.is_none());
        assert_eq!(
            exit_hold(&mut st, true, ExitKind::Exit(0), now),
            Hold::Ask(2)
        );
        assert_eq!(exit_take(&mut st, None, false, now), None);
        assert_eq!(
            exit_hold(&mut st, true, ExitKind::Exit(0), now),
            Hold::Ask(3),
            "取りやめの後は保留しない間を作らない"
        );
    }

    #[test]
    fn timeout_only_for_its_own_request() {
        let now = Instant::now();
        let mut st = ExitState::default();
        exit_hold(&mut st, true, ExitKind::Exit(0), now); // 1
        exit_take(&mut st, None, false, now); // 取りやめ
        exit_hold(&mut st, true, ExitKind::Exit(0), now); // 2
                                                          // 1 のタイムアウトが遅れて来ても 2 を進めない
        assert_eq!(exit_take(&mut st, Some(1), true, now), None);
        assert!(st.pending.is_some());
        // 2 のタイムアウトは進める（JS が答えない）
        assert_eq!(
            exit_take(&mut st, Some(2), true, now),
            Some(ExitKind::Exit(0))
        );
        // 応答が先に来ていたらタイムアウトは何もしない（二重に終了しない）
        assert_eq!(exit_take(&mut st, Some(2), true, now), None);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod wiki_trash_real {
    /// 実物の macOS のゴミ箱（NSFileManager）: フォルダも中身ごと移る（#[ignore] = 実行した人のゴミ箱にフォルダが 1 つ入る）
    #[test]
    #[ignore]
    fn folder_goes_to_trash() {
        let d = std::env::temp_dir().join(format!("openref-trash-real-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/x.md"), "x").unwrap();
        let m = super::wiki_move_to_trash(&d).unwrap();
        assert_eq!(m, "nsfilemanager");
        assert!(!d.exists(), "まだある: {}", d.display());
        // ~/.Trash は TCC で読めない（read_dir = Operation not permitted）ので中身は見ない = 消えたことだけ確かめる
    }
}

#[cfg(test)]
mod wiki_err_contract {
    use openref_core::{wiki::codes, CoreError};

    /// エラーの契約: CoreError::Wiki は {"code", "message"}（message = Display）・他は素の文のまま
    #[test]
    fn coded_errors_are_json() {
        let e = CoreError::wiki(codes::REBUILDING, "msg");
        let v: serde_json::Value = serde_json::from_str(&super::wiki_err(e)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"code": codes::REBUILDING, "message": "msg"})
        );
        assert_eq!(
            super::wiki_err(CoreError::NotFound("x".into())),
            CoreError::NotFound("x".into()).to_string()
        );
    }

    /// wiki_run が wiki_err を通す（無効の Db = unavailable が JSON で返る・message は core の文そのもの）
    #[test]
    fn wiki_run_returns_code() {
        let state = super::AppState {
            db: std::sync::Arc::new(std::sync::Mutex::new(
                openref_core::Db::open_in_memory().unwrap(),
            )),
            wiki_live: Default::default(),
        };
        let e = tauri::async_runtime::block_on(super::wiki_run(
            &state,
            Some("wiki_list"),
            None,
            openref_core::wiki::commands::list,
        ))
        .unwrap_err();
        let v: serde_json::Value =
            serde_json::from_str(&e).unwrap_or_else(|_| panic!("JSON でない: {e}"));
        assert_eq!(v["code"], codes::UNAVAILABLE, "{e}");
        assert_eq!(v["message"], openref_core::wiki::commands::DISABLED, "{e}");
    }
}

#[cfg(test)]
mod wiki_chat_model {
    use super::wiki_chat_model_ok;

    /// `--model` に渡す表記: 別名・model id・Bedrock / Vertex 風の長い名前は通し、空白・引用・先頭の `-`（別のフラグに見える）は止める
    #[test]
    fn model_syntax() {
        for ok in [
            "",
            "opus",
            "fable",
            "claude-sonnet-5",
            "claude-opus-5-5[1m]",
            "us.anthropic.claude-sonnet-5-5-v1:0",
            "arn:aws:bedrock:us-east-1:123456789012:inference-profile/us.anthropic.claude-x",
            "model@2026",
        ] {
            assert!(wiki_chat_model_ok(ok), "{ok}");
        }
        for bad in [
            " opus",
            "opus sonnet",
            "a'b",
            "a\"b",
            "a;b",
            "a$b",
            "-x",
            "--model",
            "日本",
            &"x".repeat(129),
        ] {
            assert!(!wiki_chat_model_ok(bad), "{bad}");
        }
    }
}
