// OpenNote のチャットの GUI 側（wiki_gui.rs から分けた。中身は分ける前と同じ）。command の包みは main.rs・子プロセスは wiki_chat.rs
use crate::wiki_gui::*;
use crate::{blocking, err, lock, save_setting, wiki_chat, AppState, SettingsItems};
use std::path::{Path, PathBuf};
use tauri::Manager;

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

/// メニューの [チャット] を設定に揃える（押せるのは Wiki が有効なときだけ）。[チャットのログ] も同じ条件で押せる
pub(crate) fn wiki_chat_sync_menu(app: &tauri::AppHandle) {
    let wiki_on = WIKI_ON.load(std::sync::atomic::Ordering::Relaxed);
    let on = wiki_chat_settings(app)
        .map(|(c, on, _)| c.enabled && on)
        .unwrap_or(false);
    if let Some(items) = app.try_state::<SettingsItems>() {
        let _ = items
            .wiki_chat
            .set_enabled(wiki_on)
            .and(items.wiki_chat.set_checked(on));
        let _ = items.wiki_chat_log.set_enabled(wiki_on);
    }
}

pub(crate) const WIKI_CHAT_LOG_ID: &str = "wiki_chat_log";
/// 設定: "0" のときだけチャットのログ（ワークスペースの chat-log/）を残さない。既定 ON
pub(crate) const WIKI_CHAT_LOG_KEY: &str = "wiki_chat_log";

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
pub(crate) fn wiki_chat_emit_enabled(app: &tauri::AppHandle) {
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
pub(crate) fn wiki_chat_stop_bg(app: &tauri::AppHandle) {
    if let Some(c) = app.try_state::<wiki_chat::Chat>() {
        let c = c.inner().clone();
        std::thread::spawn(move || c.stop_now());
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
                "OpenNote が無効です（先に 設定 › OpenNote › 有効）",
                "OpenNote is disabled (enable it first in Settings › OpenNote)",
            ));
        }
        wiki_ready(&ctx)?;
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
            c.stop_now();
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
pub(crate) struct WikiChatProbe {
    /// OpenNote が有効か
    pub(crate) wiki_on: bool,
    /// チャットの設定が ON か
    pub(crate) chat_on: bool,
    /// root が使えない理由（ready の Err）
    pub(crate) ready_error: Option<String>,
    /// claude コマンドが見つかったか
    pub(crate) cli: bool,
    /// ログインしているか（None = 分からない = 止めない）
    pub(crate) logged_in: Option<bool>,
    /// openref-mcp（wiki_write のある版）が見つかったか
    pub(crate) mcp: bool,
}

/// 使えない理由（純関数）。優先順 = Wiki 無効 › チャット無効 › root が使えない › claude が無い › 未ログイン › openref-mcp が無い。
/// None = 使える。ログインの有無が分からない（None）ときは止めない
pub(crate) fn wiki_chat_reason(en: bool, p: &WikiChatProbe) -> Option<String> {
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
            "チャットは無効です（設定 › OpenNote › チャット）",
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

impl WikiChatProbe {
    /// 設定の段（OpenNote・チャット）だけを調べた材料。後の段は通す = 起動・ログインが段ごとに調べて書き足し、
    /// そのつど wiki_chat_check にかける（使えない理由の順と文は wiki_chat_reason の 1 か所 = status の reason と同じ）
    pub(crate) fn settings(wiki_on: bool, chat_on: bool) -> Self {
        Self { wiki_on, chat_on, ready_error: None, cli: true, logged_in: None, mcp: true }
    }
}

/// 使えなければ Err（文 = wiki_chat_reason）
pub(crate) fn wiki_chat_check(en: bool, p: &WikiChatProbe) -> Result<(), String> {
    wiki_chat_reason(en, p).map_or(Ok(()), Err)
}

/// 見つかったか（found）を mark で材料に書き、wiki_chat_check にかけて、見つかった物を返す
pub(crate) fn wiki_chat_need<T>(
    en: bool,
    p: &mut WikiChatProbe,
    found: Option<T>,
    mark: impl FnOnce(&mut WikiChatProbe, bool),
) -> Result<T, String> {
    mark(p, found.is_some());
    wiki_chat_check(en, p)?;
    found.ok_or_else(|| wiki_chat_reason(en, p).unwrap_or_default())
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
            "root": wiki_chat_root(&chat),
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
    let ready = wiki_ready(&ctx);
    let reason = wiki_chat_reason(
        en,
        &WikiChatProbe {
            wiki_on: ctx.enabled,
            chat_on: on,
            ready_error: ready.as_ref().err().cloned(),
            cli: cli.is_some(),
            logged_in,
            mcp: mcp.is_some(),
        },
    );
    let session_id = chat.session_id().or_else(|| {
        let root = &ready.as_ref().ok()?.root;
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
        "root": wiki_chat_root(&chat),
    });
    if let Some(r) = reason {
        v["reason"] = serde_json::json!(r);
    }
    Ok(v)
}

/// チャットの [元に戻す] の Ctx = 記録した子の root（child）に固定する（設定 wiki_root は前面のタブの root で、子の root と
/// 違いうる = 固定しないと undo_record が「別のワークスペース」で断る）。履歴から外した root は Err（文 = 戻さない理由）。child が無ければ設定のまま
pub(crate) fn wiki_chat_undo_ctx(
    mut ctx: openref_core::wiki::Ctx,
    child: Option<&Path>,
) -> Result<openref_core::wiki::Ctx, String> {
    if let Some(r) = child {
        // 文は手がかりを外し・root はフォルダ名まで
        ctx.pin(&r.to_string_lossy()).map_err(|e| crate::hide_paths(e.without_hint().to_string(), &[r]))?;
    }
    Ok(ctx)
}

/// チャットの子が動いている root（canonical な文字列・子が無ければ None） = wiki_chat_status の root（画面は前面のタブの root と
/// 違う間は会話を隠す）
fn wiki_chat_root(chat: &wiki_chat::Chat) -> Option<String> {
    chat.root().map(|r| r.to_string_lossy().into_owned())
}

/// wiki_chat_status の {force: true}（画面の [再確認]）。true 以外（無い・false・文字列）は自動の読み直し
pub(crate) fn wiki_chat_force(args: Option<serde_json::Value>) -> bool {
    obj_args(args).get("force").and_then(|v| v.as_bool()) == Some(true)
}

/// root = 画面のタブの root（その root で子を起こす・履歴にあるものだけ）。None = 設定 wiki_root
pub(crate) async fn wiki_chat_start_now(
    app: tauri::AppHandle,
    new: bool,
    root: Option<String>,
) -> Result<serde_json::Value, String> {
    blocking(move || {
        let en = ui_en(&app);
        let (ctx, on, db) = wiki_chat_settings(&app)?;
        let mut p = WikiChatProbe::settings(ctx.enabled, on);
        wiki_chat_check(en, &p)?;
        let ctx = wiki_pin(ctx, root.as_deref())?;
        let ready = wiki_ready(&ctx);
        p.ready_error = ready.as_ref().err().cloned();
        wiki_chat_check(en, &p)?;
        let root = ready?.root;
        let no_db = || tr(en, "in-memory の DB ではチャットを使えません", "The chat is not available with an in-memory database");
        let app_dir = ctx.app_dir.clone().ok_or_else(no_db)?;
        // 相対の OPENREF_DB でも子（cwd = vault）が同じ DB を開くよう絶対パスに
        let db = std::path::absolute(db.ok_or_else(no_db)?).map_err(err)?;
        let home = ctx.home.as_deref();
        let found = wiki_chat::find_cli(home, std::env::var_os("PATH").as_deref());
        let cli = wiki_chat_need(en, &mut p, found, |p, ok| p.cli = ok)?;
        // 起こす前は覚えた分を使わずに読む（読んだ結果は後の status の読み直しが使う）
        p.logged_in = wiki_chat::cli_auth_cached(&cli, true).map(|a| a.logged_in);
        wiki_chat_check(en, &p)?;
        let found = wiki_chat::find_mcp(&wiki_chat::mcp_candidates(home), &db);
        let mcp = wiki_chat_need(en, &mut p, found, |p, ok| p.mcp = ok)?;
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

#[cfg(test)]
mod check {
    use super::*;

    /// 起動・ログインの段ごとの判定は status と同じ理由の文（wiki_chat_reason）を返す。調べていない段は通す
    #[test]
    fn staged_checks_give_the_status_reason() {
        for en in [false, true] {
            let reason = |p: &WikiChatProbe| wiki_chat_reason(en, p).unwrap();
            // 設定の段: OpenNote が無効 › チャットが無効 › 通る（root・claude・openref-mcp はまだ問わない）
            let p = WikiChatProbe::settings(false, true);
            assert_eq!(wiki_chat_check(en, &p), Err(reason(&p)));
            assert!(reason(&p).contains(if en { "OpenNote is disabled" } else { "OpenNote が無効" }));
            let p = WikiChatProbe::settings(true, false);
            assert_eq!(wiki_chat_check(en, &p), Err(reason(&p)));
            assert!(reason(&p).contains(if en { "chat is disabled" } else { "チャットは無効" }));
            let mut p = WikiChatProbe::settings(true, true);
            assert_eq!(wiki_chat_check(en, &p), Ok(()));
            // root の段: ready の理由をそのまま
            p.ready_error = Some("root が無い".into());
            assert_eq!(wiki_chat_check(en, &p), Err("root が無い".into()));
            p.ready_error = None;
            // claude の段: 無ければ status と同じ文・在れば返す
            let e = wiki_chat_need(en, &mut p.clone(), None::<u8>, |p, ok| p.cli = ok).unwrap_err();
            assert_eq!(e, reason(&WikiChatProbe { cli: false, ..p.clone() }));
            assert!(e.contains(if en { "Install Claude Code" } else { "インストールしてください" }), "{e}");
            assert_eq!(wiki_chat_need(en, &mut p, Some(7u8), |p, ok| p.cli = ok), Ok(7));
            // ログインの段: 分からない（None）は通す・未ログインは止める
            assert_eq!(wiki_chat_check(en, &p), Ok(()));
            p.logged_in = Some(false);
            assert_eq!(wiki_chat_check(en, &p), Err(reason(&p)));
            p.logged_in = Some(true);
            // openref-mcp の段
            let e = wiki_chat_need(en, &mut p.clone(), None::<u8>, |p, ok| p.mcp = ok).unwrap_err();
            assert!(e.contains(if en { "Reinstall OpenRef" } else { "入れ直してください" }), "{e}");
            assert_eq!(wiki_chat_need(en, &mut p, Some(()), |p, ok| p.mcp = ok), Ok(()));
        }
    }

    /// 起動は 5 段（設定 › root › claude › ログイン › openref-mcp）をこの順に判定する（runtime の中でしか動かない =
    /// 空白に依らない照合。1 段でも落とすと、その理由があっても起動してしまう）
    #[test]
    fn start_checks_every_stage_in_order() {
        let sq = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let src = body(include_str!("wiki_chat_gui.rs"));
        let start = &src[src.find("pub(crate) async fn wiki_chat_start_now(").expect("起動")..];
        let start = sq(&start[..start.find("\n}\n").expect("fn の終わり")]);
        let mut at = 0;
        for stage in [
            "let mut p = WikiChatProbe::settings(ctx.enabled, on); wiki_chat_check(en, &p)?;",
            "p.ready_error = ready.as_ref().err().cloned(); wiki_chat_check(en, &p)?;",
            "let cli = wiki_chat_need(en, &mut p, found, |p, ok| p.cli = ok)?;",
            "p.logged_in = wiki_chat::cli_auth_cached(&cli, true).map(|a| a.logged_in); wiki_chat_check(en, &p)?;",
            "let mcp = wiki_chat_need(en, &mut p, found, |p, ok| p.mcp = ok)?;",
        ] {
            let i = start[at..].find(&sq(stage)).unwrap_or_else(|| panic!("起動の段が無い・順が違う: {stage}"));
            at += i + 1;
        }
    }

    /// 本体（最初の #[cfg(test)] より前）
    fn body(src: &'static str) -> &'static str {
        &src[..src.find("#[cfg(test)]").expect("試験の節")]
    }

    /// 使えない理由の文は wiki_chat_reason の 1 か所だけ（起動・ログイン・status で書き分けない）。
    /// チャットの停止の猶予は Chat::stop_now の 1 か所だけ（GUI のコードに秒数を書かない）
    #[test]
    fn reason_texts_and_stop_grace_live_in_one_place() {
        let gui = [body(include_str!("main.rs")), body(include_str!("wiki_gui.rs")), body(include_str!("wiki_chat_gui.rs"))];
        for phrase in [
            "チャットは無効です（",
            "The chat is disabled (",
            "Claude Code（claude コマンド）が見つかりません",
            "Claude Code (the claude command) was not found",
            "Claude Code にログインしていません",
            "Claude Code is not logged in",
            "openref-mcp が見つからないか古い版です",
            "openref-mcp was not found or is too old",
        ] {
            let n: usize = gui.iter().map(|s| s.matches(phrase).count()).sum();
            assert_eq!(n, 1, "{phrase} が {n} か所");
        }
        // 起動（wiki_chat_start_now）とログイン（main.rs の wiki_chat_auth）は、設定の段を最初に判定する
        let first = "letmutp=WikiChatProbe::settings(ctx.enabled,on);wiki_chat_check(en,&p)?;";
        let n: usize = gui.iter().map(|s| s.chars().filter(|c| !c.is_whitespace()).collect::<String>().matches(first).count()).sum();
        assert_eq!(n, 2, "設定の段の判定が起動とログインの 2 か所に無い");
        for (i, s) in gui.iter().enumerate() {
            let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(!s.contains(".stop(Duration::") && !s.contains(".stop(std::time::Duration::"), "{i} 番目のファイルが猶予を直書き");
        }
    }
}
