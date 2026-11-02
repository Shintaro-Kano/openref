#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use openref_core::{
    annotate, citestyle, collections, datadir, edit, ingest, pdfingest, resolver, search, tags,
    types, update, Db,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::{Emitter, Manager};
use wiki_asset::*;
use wiki_gui::*;
mod migration;
mod wiki_chat;
mod wiki_chat_log;
mod wiki_asset;
mod wiki_gui;

struct AppState {
    db: Arc<Mutex<Db>>,
    /// OpenNote の監視が生きているかの印（WikiRuntime と同じもの）。wiki_run が見て鮮度規則の stat 走査を省く
    wiki_live: Arc<WikiLive>,
}

// ---------- 定型 ----------
// 戻り値は core の行型をそのまま返す（core 側の `#[derive(Serialize)]`・camelCase。TS の型は app/src/lib/api.ts が 1:1 で持つ）。
// GUI だけの整形は search::ItemSummary（一覧の行）だけ

/// core / rusqlite / lock / JoinError のエラーを Tauri command の Err(String) に落とす
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// DB のロック（poisoned なら Err）。`lock(&state.db)` / spawn_blocking に持ち込んだ Arc には `lock(&db)`
fn lock(m: &Mutex<Db>) -> Result<MutexGuard<'_, Db>, String> {
    m.lock().map_err(err)
}

/// core の関数に渡す DB のロック（core の Result。ロックを短く取り直す mount::enable_drive_mode / migrate_managed_to_drive 用）
fn core_lock(m: &Mutex<Db>) -> openref_core::Result<MutexGuard<'_, Db>> {
    m.lock()
        .map_err(|e| openref_core::CoreError::BadInput(format!("DB lock failed: {e}")))
}

fn migration_db(state: &AppState) -> Result<PathBuf, String> {
    lock(&state.db)?
        .conn
        .path()
        .map(PathBuf::from)
        .ok_or_else(|| "移行先 DB のパスがありません".into())
}
#[tauri::command]
fn migration_preview(
    state: tauri::State<AppState>,
    jobs: tauri::State<migration::Jobs>,
    config: migration::Config,
) -> Result<String, String> {
    jobs.preview(migration_db(&state)?, config)
}
#[tauri::command]
fn migration_status(
    jobs: tauri::State<migration::Jobs>,
) -> Result<Option<migration::Status>, String> {
    jobs.status()
}
#[tauri::command]
fn migration_start(jobs: tauri::State<migration::Jobs>, id: String) -> Result<(), String> {
    jobs.start(&id)
}
#[tauri::command]
fn migration_cancel(jobs: tauri::State<migration::Jobs>, id: String) -> Result<(), String> {
    jobs.cancel(&id)
}
#[tauri::command]
fn migration_index(jobs: tauri::State<migration::Jobs>, id: String) -> Result<(), String> {
    jobs.index(&id)
}
#[tauri::command]
async fn migration_select_report(
    state: tauri::State<'_, AppState>,
    jobs: tauri::State<'_, migration::Jobs>,
    run: String,
) -> Result<(), String> {
    let db = migration_db(&state)?;
    let shared = state.db.clone();
    let jobs = jobs.inner().clone();
    blocking(move || {
        // 開いている接続で 1 run だけ読む（DB の写しを取らない）。ロックは読む間だけ
        let report = migration::past_report(&*lock(&shared)?, &run)?;
        jobs.select_report(db, report)
    })
    .await
}
#[tauri::command]
async fn migration_save_report(
    jobs: tauri::State<'_, migration::Jobs>,
    id: String,
    path: PathBuf,
) -> Result<(), String> {
    let jobs = jobs.inner().clone();
    blocking(move || jobs.save_report(&id, path)).await
}
#[tauri::command]
async fn migration_history(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let shared = state.db.clone();
    blocking(move || {
        // 一覧の列だけを開いている接続で読む（DB の写し・全 run の report を作らない）
        let mut history =
            openref_core::import::apply::history_list(&lock(&shared)?.conn).map_err(err)?;
        history.truncate(50);
        Ok(history)
    })
    .await
}

/// main thread から外して待つ（同期 command は main thread で走る）。JoinError も Err(String) に落とす
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

// ---------- commands ----------

#[tauri::command]
async fn list_items(
    state: tauri::State<'_, AppState>,
    filter: search::ListFilter,
) -> Result<Vec<search::ItemSummary>, String> {
    // 本文検索（fulltext_fts + 抜粋）は一般語で数百 ms かかり得るので main thread から外す（UI を止めない）。
    // 応答順は保証されないため、フロント（state.svelte.ts refreshList）が古い応答を捨てる
    let db = state.db.clone();
    blocking(move || {
        let db = lock(&db)?;
        Ok(search::list_items(&db, &filter)
            .map_err(err)?
            .iter()
            .map(search::ListedItem::summary)
            .collect())
    })
    .await
}

/// 詳細（ゴミ箱の item も引ける。deleted / notes 込み = core の ItemDetail そのまま）
#[tauri::command]
fn get_item(
    state: tauri::State<AppState>,
    key: String,
) -> Result<Option<search::ItemDetail>, String> {
    search::item_detail(&*lock(&state.db)?, &key).map_err(err)
}

// ---------- 参照タイプカタログ・手動作成/編集 ----------

#[tauri::command]
fn item_types() -> Vec<types::TypeDef> {
    types::catalog()
}

#[tauri::command]
fn subtypes() -> Vec<String> {
    types::SUBTYPES.iter().map(|s| s.to_string()).collect()
}

#[tauri::command]
fn create_manual(state: tauri::State<AppState>, csl: serde_json::Value) -> Result<String, String> {
    edit::create_manual(&mut *lock(&state.db)?, &csl).map_err(err)
}

#[tauri::command]
fn update_item(
    state: tauri::State<AppState>,
    key: String,
    csl: serde_json::Value,
) -> Result<(), String> {
    edit::update_csl(&mut *lock(&state.db)?, &key, &csl).map_err(err)
}

// ---------- 引用スタイル（CSL・Paperpile の Change citation style…） ----------
// 取得はネットワークを伴い得るので main thread から外す。DB lock は保持しない（styles/ ディレクトリだけ使う）

fn styles_dir_of(state: &tauri::State<'_, AppState>) -> Result<PathBuf, String> {
    Ok(citestyle::styles_dir(&*lock(&state.db)?))
}

/// 約 1 万誌の索引（30 日キャッシュ）
#[tauri::command]
async fn cite_catalog(
    state: tauri::State<'_, AppState>,
    refresh: bool,
) -> Result<Vec<serde_json::Value>, String> {
    let dir = styles_dir_of(&state)?;
    blocking(move || citestyle::catalog(&dir, refresh).map_err(err)).await
}

/// 同梱 4 種 + 導入済み
#[tauri::command]
fn cite_installed(state: tauri::State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    Ok(citestyle::installed(&styles_dir_of(&state)?))
}

/// citeproc に渡せる CSL（dependent は親で解決済み・未取得なら取得してキャッシュ）
#[tauri::command]
async fn cite_style_xml(state: tauri::State<'_, AppState>, id: String) -> Result<String, String> {
    let dir = styles_dir_of(&state)?;
    blocking(move || citestyle::style_xml(&dir, &id).map_err(err)).await
}

#[tauri::command]
async fn cite_locale_xml(
    state: tauri::State<'_, AppState>,
    lang: String,
) -> Result<String, String> {
    let dir = styles_dir_of(&state)?;
    blocking(move || citestyle::locale_xml(&dir, &lang).map_err(err)).await
}

/// `{style, locale, doi_always}`（MCP の cite_prefs と同じ JSON。snake_case のまま = api.ts の CitePrefs 参照）
#[tauri::command]
fn cite_prefs(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    citestyle::prefs(&*lock(&state.db)?).map_err(err)
}

/// 既定スタイル・言語・「DOI / URL を常に含める」を保存（スタイルの取得・導入を兼ねる）。
/// 渡さなかった項目は今のまま（登録表の cite_set_prefs と同じ core `set_prefs_partial`。フロントは全項目を渡す）
#[tauri::command]
async fn cite_set_prefs(
    state: tauri::State<'_, AppState>,
    style: Option<String>,
    locale: Option<String>,
    doi_always: Option<bool>,
) -> Result<serde_json::Value, String> {
    // style を渡したときは取得（ネットワーク）を lock の外で済ませてから settings を書く。渡さないときは set_prefs が今の
    // style を install し直す = 同梱かキャッシュ済みなら即返るが、キャッシュが消えていれば lock を持ったまま取得が走る
    // （今のフロントは 3 項目すべて渡すので入らない経路）
    if let Some(st) = style.clone() {
        let dir = styles_dir_of(&state)?;
        blocking(move || citestyle::install(&dir, &st).map_err(err)).await?;
    }
    citestyle::set_prefs_partial(
        &*lock(&state.db)?,
        style.as_deref(),
        locale.as_deref(),
        doi_always,
    )
    .map_err(err)
}

/// 手元の .csl を導入（リポジトリに無い誌用）。戻り値は導入済み一覧の 1 行
#[tauri::command]
async fn cite_install_file(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let dir = styles_dir_of(&state)?;
    blocking(move || citestyle::install_file(&dir, Path::new(&path)).map_err(err)).await
}

/// 引用メニューの BibTeX / 引用キー / LaTeX / DOI 用（keys の順で返す・見つからない key は飛ばす）
#[tauri::command]
fn bibtex_entries(
    state: tauri::State<AppState>,
    keys: Vec<String>,
) -> Result<Vec<openref_core::bibtex::Entry>, String> {
    openref_core::bibtex::entries(&*lock(&state.db)?, &keys).map_err(err)
}

/// DOI/PMID/URL から書誌を引く（編集画面の Auto update）。DB には触れない。
#[tauri::command]
async fn lookup(input: String) -> Result<serde_json::Value, String> {
    blocking(move || {
        let parsed = resolver::parse_input(&input).map_err(err)?;
        Ok(resolver::resolve(&parsed).map_err(err)?.csl)
    })
    .await
}

// ---------- PDF viewer・注釈 ----------

/// 添付の実体のパス（attachments.path）
fn attachment_path(db: &Db, id: i64) -> Result<String, String> {
    db.conn
        .query_row("SELECT path FROM attachments WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .map_err(err)
}

/// PDF 実体を生バイトで返す（pdf.js に渡す）。`tauri::ipc::Response` = IPC の応答が ArrayBuffer になり、
/// base64 → JSON → atob の 3 段のコピーが消える（最大 30.9MB の PDF で WebView 側のピーク約 140MB → 約 60MB）。
/// パスを引いたら DB のロックを外す: ファイルの読込（Drive の未取得ファイルは取得待ちになり得る）が他の command を止めない。
/// 読込は spawn_blocking（同期 command は main thread で走る）
#[tauri::command]
async fn attachment_data(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<tauri::ipc::Response, String> {
    let path = {
        let db = lock(&state.db)?;
        let path = attachment_path(&db, id)?;
        openref_core::db::reject_connection_file(&db.conn, std::path::Path::new(&path))
            .map_err(err)?;
        path
    };
    let bytes = blocking(move || std::fs::read(&path).map_err(|e| format!("{path}: {e}"))).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
fn list_annotations(
    state: tauri::State<AppState>,
    key: String,
) -> Result<(Vec<annotate::AnnotationRow>, Vec<annotate::NoteRow>), String> {
    let db = lock(&state.db)?;
    Ok((
        annotate::list_annotations(&db, &key).map_err(err)?,
        annotate::list_notes(&db, &key).map_err(err)?,
    ))
}

#[tauri::command]
fn add_annotation(
    state: tauri::State<AppState>,
    key: String,
    quote: String,
    kind: Option<String>,
    page: Option<i64>,
    comment: Option<String>,
    color: Option<String>,
) -> Result<String, String> {
    annotate::add_annotation(
        &*lock(&state.db)?,
        &key,
        &annotate::NewAnnotation {
            kind: kind.unwrap_or_else(|| "highlight".into()),
            color: Some(color.unwrap_or_else(|| "yellow".into())),
            comment,
            page,
            quote: Some(quote),
            author: "user".into(),
            ..Default::default()
        },
    )
    .map_err(err)
}

/// comment/color は tri-state（不変更 / 消去 / 設定）。JSON の nullable Option<Option<T>> は
/// 「キー省略」と「値 null」を区別できないため、専用の clear_* 旗で明示的に受け取る
/// （clear_comment=true なら comment の中身は無視して NULL へ消去）。kind は allowlist 検証のみ。
#[tauri::command]
fn update_annotation(
    state: tauri::State<AppState>,
    key: String,
    kind: Option<String>,
    comment: Option<String>,
    clear_comment: Option<bool>,
    color: Option<String>,
    clear_color: Option<bool>,
) -> Result<(), String> {
    let comment_patch = if clear_comment.unwrap_or(false) {
        Some(None)
    } else {
        comment.map(Some)
    };
    let color_patch = if clear_color.unwrap_or(false) {
        Some(None)
    } else {
        color.map(Some)
    };
    annotate::update_annotation(
        &*lock(&state.db)?,
        &key,
        &annotate::AnnotationPatch {
            comment: comment_patch,
            color: color_patch,
            kind,
        },
    )
    .map_err(err)
}

#[tauri::command]
fn delete_annotation(state: tauri::State<AppState>, key: String) -> Result<(), String> {
    annotate::delete_annotation(&*lock(&state.db)?, &key).map_err(err)
}

#[tauri::command]
fn add_note(
    state: tauri::State<AppState>,
    key: String,
    content_md: String,
) -> Result<String, String> {
    annotate::add_note(&*lock(&state.db)?, &key, "user", &content_md).map_err(err)
}

#[tauri::command]
fn delete_note(state: tauri::State<AppState>, key: String) -> Result<(), String> {
    annotate::delete_note(&*lock(&state.db)?, &key).map_err(err)
}

#[tauri::command]
fn list_notes(
    state: tauri::State<AppState>,
    key: String,
) -> Result<Vec<annotate::NoteRow>, String> {
    annotate::list_notes(&*lock(&state.db)?, &key).map_err(err)
}

#[tauri::command]
fn update_note(
    state: tauri::State<AppState>,
    key: String,
    content_md: String,
) -> Result<(), String> {
    annotate::update_note(&*lock(&state.db)?, &key, &content_md).map_err(err)
}

// ---------- tags（LABELS UX） ----------

#[tauri::command]
fn list_tags(state: tauri::State<AppState>, by_count: bool) -> Result<Vec<tags::TagRow>, String> {
    let sort = if by_count {
        tags::TagSort::Count
    } else {
        tags::TagSort::Alphabetical
    };
    tags::list_tags(&*lock(&state.db)?, sort).map_err(err)
}

#[tauri::command]
fn create_tag(
    state: tauri::State<AppState>,
    name: String,
    color: Option<String>,
) -> Result<i64, String> {
    tags::create_tag(&*lock(&state.db)?, &name, color.as_deref()).map_err(err)
}

#[tauri::command]
fn update_tag(
    state: tauri::State<AppState>,
    id: i64,
    name: Option<String>,
    color: Option<String>,
) -> Result<(), String> {
    tags::update_tag(&*lock(&state.db)?, id, name.as_deref(), color.as_deref()).map_err(err)
}

#[tauri::command]
fn delete_tag(state: tauri::State<AppState>, id: i64) -> Result<(), String> {
    tags::delete_tag(&*lock(&state.db)?, id).map_err(err)
}

/// Manage labels → Merge: sources を target へ統合（戻り値 = 統合後の付与件数）
#[tauri::command]
fn merge_tags(
    state: tauri::State<AppState>,
    sources: Vec<i64>,
    target: i64,
) -> Result<i64, String> {
    tags::merge_tags(&*lock(&state.db)?, &sources, target).map_err(err)
}

/// Manage labels → Find and replace（dry_run = プレビュー）
#[tauri::command]
fn find_replace_tags(
    state: tauri::State<AppState>,
    find: String,
    replace: String,
    dry_run: bool,
) -> Result<Vec<tags::RenameOutcome>, String> {
    tags::find_replace_tags(&*lock(&state.db)?, &find, &replace, dry_run).map_err(err)
}

#[tauri::command]
fn attach_tag(state: tauri::State<AppState>, key: String, tag_id: i64) -> Result<(), String> {
    tags::attach_tag(&*lock(&state.db)?, &key, tag_id).map_err(err)
}

#[tauri::command]
fn detach_tag(state: tauri::State<AppState>, key: String, tag_id: i64) -> Result<(), String> {
    tags::detach_tag(&*lock(&state.db)?, &key, tag_id).map_err(err)
}

#[tauri::command]
fn collections_tree(state: tauri::State<AppState>) -> Result<Vec<search::CollectionRow>, String> {
    search::collections(&*lock(&state.db)?).map_err(err)
}

/// 選択中 item 群の所属集計（一括ツールバーのフォルダ/ラベル三状態用。JSON は [[id, n], …]）
#[tauri::command]
fn membership(
    state: tauri::State<AppState>,
    keys: Vec<String>,
) -> Result<search::Membership, String> {
    search::membership(&*lock(&state.db)?, &keys).map_err(err)
}

// ---------- フォルダ管理（MY LIBRARY） ----------

/// New folder / Create subfolder（parentId = null なら root）。同じ親の下に大小無視の同名・`/` を含む名前は拒否
#[tauri::command]
fn create_collection_in(
    state: tauri::State<AppState>,
    name: String,
    parent_id: Option<i64>,
) -> Result<i64, String> {
    collections::create_collection(&*lock(&state.db)?, &name, parent_id).map_err(err)
}

#[tauri::command]
fn rename_collection(state: tauri::State<AppState>, id: i64, name: String) -> Result<(), String> {
    collections::rename_collection(&*lock(&state.db)?, id, &name).map_err(err)
}

/// サブフォルダごと即時に削除（論文は残る）。戻り値 = 消したフォルダ数
#[tauri::command]
fn delete_collection(state: tauri::State<AppState>, id: i64) -> Result<usize, String> {
    collections::delete_collection(&*lock(&state.db)?, id).map_err(err)
}

/// Manage folders → Find and replace（dry_run = プレビュー。同名の判定は兄弟の中だけ）
#[tauri::command]
fn find_replace_collections(
    state: tauri::State<AppState>,
    find: String,
    replace: String,
    dry_run: bool,
) -> Result<Vec<tags::RenameOutcome>, String> {
    collections::find_replace_collections(&*lock(&state.db)?, &find, &replace, dry_run).map_err(err)
}

/// UI の表示設定（フォルダの並び順・折畳など）の key は `ui_` 接頭辞だけ受け付ける
/// （settings には pdf_write_mode などの運用設定も同居するので、UI から書ける範囲を絞る）
fn ui_pref_key(key: &str) -> Result<&str, String> {
    let ok = key.len() <= 64
        && key.starts_with("ui_")
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(key)
    } else {
        Err(format!(
            "UI の設定 key は ui_ で始まる英数字だけです: {key}"
        ))
    }
}

#[tauri::command]
fn get_ui_pref(state: tauri::State<AppState>, key: String) -> Result<Option<String>, String> {
    let key = ui_pref_key(&key)?;
    lock(&state.db)?.setting(key).map_err(err)
}

#[tauri::command]
fn set_ui_pref(state: tauri::State<AppState>, key: String, value: String) -> Result<(), String> {
    let key = ui_pref_key(&key)?;
    lock(&state.db)?.set_setting(key, &value).map_err(err)
}

/// 他の接続（MCP・CLI・connectd）のコミットで増える値（`PRAGMA data_version`。自分の書込では変わらない）。
/// App が 2 秒ごとに読み、変われば一覧・サイドバー・詳細を読み直す。一覧の検索が DB を握っている間に
/// main thread を止めないよう blocking へ逃がす
#[tauri::command]
async fn data_version(state: tauri::State<'_, AppState>) -> Result<i64, String> {
    let db = state.db.clone();
    blocking(move || {
        lock(&db)?
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .map_err(err)
    })
    .await
}

/// 新版の確認（起動時に App が 1 回呼ぶ）= core update::check: settings の update_check / update_checked_at / update_latest /
/// update_error と配布サイトの dl/VERSION。24 時間は読み直さない・取りに行く間は DB のロックを離す。動いている版（Info.plist の
/// CFBundleShortVersionString = tauri.conf.json の version）より新しい版番号を返す（無ければ None）。取りに行けなかったときは
/// settings の update_error（sql / MCP から読める）と stderr に出して前回の結果で続ける（起動直後に toast で邪魔しない）。
/// 開発時は OPENREF_UPDATE_URL で確認先を差し替える
#[tauri::command]
async fn check_update(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let db = state.db.clone();
    let current = app.package_info().version.to_string();
    blocking(move || {
        let url = openref_core::env_os("UPDATE_URL")
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| update::CHECK_URL.to_string());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let out = update::check(&db, &current, &url, now).map_err(err)?;
        if let Some(e) = out.error {
            eprintln!("openref-app: 新版の確認に失敗（{url}）: {e}");
        }
        Ok(out.newer)
    })
    .await
}

#[tauri::command]
fn view_counts(state: tauri::State<AppState>) -> Result<search::ViewCounts, String> {
    search::view_counts(&*lock(&state.db)?).map_err(err)
}

/// quick_add の戻り値 = 一覧の行（ItemSummary のフィールドはそのまま）+ resolver の警告（普段は空）
#[derive(serde::Serialize)]
struct QuickAdded {
    #[serde(flatten)]
    item: search::ItemSummary,
    warnings: Vec<String>,
}

#[tauri::command]
async fn quick_add(state: tauri::State<'_, AppState>, input: String) -> Result<QuickAdded, String> {
    let db = state.db.clone();
    blocking(move || {
        let parsed = resolver::parse_input(&input).map_err(err)?;
        let resolved = resolver::resolve(&parsed).map_err(err)?;
        let mut db = lock(&db)?;
        let added = ingest::add_item(&mut db, &resolved).map_err(err)?;
        let d = search::item_detail(&db, &added.key)
            .map_err(err)?
            .ok_or("登録直後の item が見つかりません")?;
        Ok(QuickAdded {
            item: search::ListedItem::from(d).summary(),
            warnings: resolved.warnings,
        })
    })
    .await
}

/// PDF 取込 1 件（取込モーダルの行に対応）。結果の区分と文言は core（MCP の import_pdf と同じ要約）。理由・降格の警告も message に入る。
/// ① md5 照合 → ② ロック外で抽出・resolver・題名検索 → ③ 再ロックして登録・添付（一括取込中も一覧・検索を止めない）
#[tauri::command]
async fn import_pdf(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<pdfingest::ImportSummary, String> {
    let db = state.db.clone();
    blocking(move || pdfingest::import_summary_unlocked(|| lock(&db), Path::new(&path))).await
}

#[tauri::command]
async fn fetch_pdf(state: tauri::State<'_, AppState>, key: String) -> Result<String, String> {
    let db = state.db.clone();
    blocking(move || {
        // ① DB 読取 → ② ロック外でネットワーク → ③ 再ロックして添付（一覧・検索を止めない）
        let plan = openref_core::pdffetch::fetch_pdf_prepare(&*lock(&db)?, &key).map_err(err)?;
        let dl = openref_core::pdffetch::fetch_pdf_download(&plan).map_err(err)?;
        let f = openref_core::pdffetch::fetch_pdf_attach(&*lock(&db)?, &plan, dl).map_err(err)?;
        Ok(format!(
            "✓ PDF 取得（{}・{} KB）",
            f.source,
            f.bytes_len / 1024
        ))
    })
    .await
}

#[tauri::command]
async fn attach_pdf(
    state: tauri::State<'_, AppState>,
    key: String,
    path: String,
) -> Result<(), String> {
    // 添付直後に本文抽出（大きい PDF で 1 秒前後）が走るので main thread から外す
    let db = state.db.clone();
    blocking(move || {
        edit::attach_auto(&*lock(&db)?, &key, Path::new(&path))
            .map(|_| ())
            .map_err(err)
    })
    .await
}

#[tauri::command]
fn set_star(state: tauri::State<AppState>, key: String, starred: bool) -> Result<(), String> {
    edit::set_starred(&*lock(&state.db)?, &key, starred).map_err(err)
}

#[tauri::command]
fn trash_item(state: tauri::State<AppState>, key: String) -> Result<(), String> {
    edit::trash(&*lock(&state.db)?, &key).map_err(err)
}

#[tauri::command]
fn restore_item(state: tauri::State<AppState>, key: String) -> Result<(), String> {
    edit::restore(&*lock(&state.db)?, &key).map_err(err)
}

/// ゴミ箱内の item を完全削除（復元不可）。ゴミ箱外は core が拒否する
#[tauri::command]
fn purge_item(state: tauri::State<AppState>, key: String) -> Result<edit::Purged, String> {
    edit::purge(&*lock(&state.db)?, &key).map_err(err)
}

#[tauri::command]
fn add_to_collection(
    state: tauri::State<AppState>,
    key: String,
    collection_id: i64,
) -> Result<(), String> {
    edit::add_to_collection(&*lock(&state.db)?, &key, collection_id).map_err(err)
}

#[tauri::command]
fn remove_from_collection(
    state: tauri::State<AppState>,
    key: String,
    collection_id: i64,
) -> Result<(), String> {
    edit::remove_from_collection(&*lock(&state.db)?, &key, collection_id).map_err(err)
}

/// `/usr/bin/open` で既定のアプリに渡す。終了を待つ（子を回収する）ので、ファイルが無い・Drive の本体が取れない等で
/// open が非 0 で終わったら stderr の 1 行目を Err にする（spawn だと起動した時点で Ok になり、失敗が UI に届かなかった）。
/// open は LaunchServices に渡すだけですぐ戻るが、同期 command は main thread で走るので呼び手は `blocking` の中で呼ぶ
fn open_with_default_app(target: &str) -> Result<(), String> {
    let out = std::process::Command::new("/usr/bin/open")
        .arg(target)
        .output()
        .map_err(err)?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    match stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(line) => Err(line.to_string()),
        None => Err(format!("open が失敗しました（{}）", out.status)),
    }
}

#[tauri::command]
async fn open_attachment(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    let path = attachment_path(&*lock(&state.db)?, id)?; // パスを引いたらロックを外す（open の終了待ちで他の command を止めない）
    blocking(move || open_file(&path)).await
}

/// 添付を開く。open の stderr は「The file <フルパス> does not exist.」= パスが 128〜256 字だと toast（先頭 200 字）で
/// 理由が切れ、Drive のパスはアカウント名（CloudStorage/GoogleDrive-<メール>）も出る → フルパスをファイル名に置き換える。
/// open はパスを NFD で書き出す（DB の値は NFC のことがある = 文字列置換では当たらない）ので、最初の `/` から最後の `/`
/// まで（= ディレクトリ部分）を落とす。ファイル名に `/` は入らない
fn open_file(path: &str) -> Result<(), String> {
    open_with_default_app(path).map_err(|e| match (e.find('/'), e.rfind('/')) {
        (Some(i), Some(j)) => format!("{}{}", &e[..i], &e[j + 1..]),
        _ => e,
    })
}

#[tauri::command]
async fn open_external(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("http(s) のみ許可".into());
    }
    blocking(move || open_with_default_app(&url)).await
}

/// Settings › 起動時に前回のタブを開く（CheckMenuItem の id）。チェック = ui_tabs を復元（既定）・外す = 次回は「すべて」1 枚
const RESTORE_TABS_ID: &str = "restore_tabs";
/// Settings › Language › 日本語 / English（CheckMenuItem 2 つ・排他）。DB の ui_lang（"ja" 既定 / "en"）に書き、
/// webview へ "ui-lang" を emit する（UI は再起動なしで表記を変える = app/src/lib/i18n.svelte.ts）
const LANG_JA_ID: &str = "lang_ja";
const LANG_EN_ID: &str = "lang_en";
/// Settings › 起動時に新版を確認（settings の update_check・"0" で止める = core update.rs。確認は App 起動時の check_update）
const UPDATE_CHECK_ID: &str = "update_check";
/// Settings › PDF の保存先 › ローカル (Mac 内) / Google Drive / ローカル保存済みの PDF を Drive へ移行
const PDF_STORAGE_MANAGED_ID: &str = "pdf_storage_managed";
const PDF_STORAGE_DRIVE_ID: &str = "pdf_storage_drive";
const PDF_MIGRATE_DRIVE_ID: &str = "pdf_migrate_drive";
const QUIT_ID: &str = "quit";

/// on_menu_event から項目のチェック状態やラベルを読み書きするために持つ（Menu::get の再帰に頼らない）
struct SettingsItems {
    restore: tauri::menu::CheckMenuItem<tauri::Wry>,
    update_check: tauri::menu::CheckMenuItem<tauri::Wry>,
    lang_ja: tauri::menu::CheckMenuItem<tauri::Wry>,
    lang_en: tauri::menu::CheckMenuItem<tauri::Wry>,
    pdf_storage: tauri::menu::Submenu<tauri::Wry>,
    pdf_managed: tauri::menu::CheckMenuItem<tauri::Wry>,
    pdf_drive: tauri::menu::CheckMenuItem<tauri::Wry>,
    pdf_migrate: tauri::menu::MenuItem<tauri::Wry>,
    wiki_enabled: tauri::menu::CheckMenuItem<tauri::Wry>,
    wiki_reindex: tauri::menu::MenuItem<tauri::Wry>,
    wiki_drop_index: tauri::menu::MenuItem<tauri::Wry>,
    wiki_chat: tauri::menu::CheckMenuItem<tauri::Wry>,
    wiki_url_window: tauri::menu::CheckMenuItem<tauri::Wry>,
    wiki_chat_log: tauri::menu::CheckMenuItem<tauri::Wry>,
    quit: tauri::menu::MenuItem<tauri::Wry>,
}

/// アプリのメニューの終了（⌘Q）。PredefinedMenuItem::quit は muda で `terminate:` になり、tao に applicationShouldTerminate が無いので
/// RunEvent::ExitRequested を通らず Exit へ直行する（保留できない）→ 自作の項目で app.exit(0) を呼んで ExitRequested を通す
/// （Wiki が無効なら保留しない = 即終了で今と同じ）。Dock の「終了」・ログアウト・再起動は今も terminate: のままで保留できない
fn quit_text(en: bool) -> &'static str {
    if en {
        "Quit OpenRef"
    } else {
        "OpenRef を終了"
    }
}

/// 言語で変わる項目名（メニューのタイトルと「日本語」「English」「Google Drive」はどちらの言語でも同じ）
fn restore_tabs_text(en: bool) -> &'static str {
    if en {
        "Restore tabs on launch"
    } else {
        "起動時に前回のタブを開く"
    }
}

fn update_check_text(en: bool) -> &'static str {
    if en {
        "Check for updates on launch"
    } else {
        "起動時に新版を確認"
    }
}

fn pdf_storage_text(en: bool) -> &'static str {
    if en {
        "PDF storage"
    } else {
        "PDF の保存先"
    }
}

fn pdf_managed_text(en: bool) -> &'static str {
    if en {
        "Local (on this Mac)"
    } else {
        "ローカル (Mac 内)"
    }
}

fn pdf_migrate_text(en: bool) -> &'static str {
    if en {
        "Move local PDFs to Google Drive"
    } else {
        "ローカル保存済みの PDF を Drive へ移行"
    }
}

/// app_menu の初期状態（起動時の設定の値。チェック項目と表示の言語）
struct MenuInit {
    restore_tabs: bool,
    update_check: bool,
    en: bool,
    pdf_drive_mode: bool,
    wiki_enabled: bool,
    wiki_chat: bool,
    wiki_url_window: bool,
    wiki_chat_log: bool,
}

/// メインメニュー = tauri 2.11.5 の `Menu::default` から **Close Window(⌘W)を外したもの**。
/// 既定メニューは File と Window の両方に `PredefinedMenuItem::close_window` を置き、WKWebView がページへ渡す前に
/// NSApp のキー等価(⌘W)として窓を閉じてしまう(PDF タブ 1 枚で ⌘W → 窓が閉じる)。
/// ⌘W はアプリ(タブを閉じる)が奪うので、メニューには置かない。File は macOS では Close Window しか無いので省く。
/// Edit の項目は WebView の ⌘C / ⌘V / ⌘A の経路なので残す。Window / Help の id は tauri が NSApp の windows / help メニューとして扱うため既定と同じ。
fn app_menu(
    app: &tauri::AppHandle,
    MenuInit {
        restore_tabs,
        update_check,
        en,
        pdf_drive_mode,
        wiki_enabled,
        wiki_chat,
        wiki_url_window,
        wiki_chat_log,
    }: MenuInit,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{
        AboutMetadata, CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu,
        WINDOW_SUBMENU_ID,
    };
    let pkg = app.package_info();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    let quit = MenuItem::with_id(app, QUIT_ID, quit_text(en), true, Some("CmdOrCtrl+Q"))?;
    let app_menu = Submenu::with_items(
        app,
        pkg.name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;
    // 既定の Window は Minimize / Maximize / 区切り / Close Window。Close Window だけ外す
    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;
    // Settings: 設定が増えたらここに並べる（ダイアログは作らない）。Help（空）と Services は置かない
    let restore = CheckMenuItem::with_id(
        app,
        RESTORE_TABS_ID,
        restore_tabs_text(en),
        true,
        restore_tabs,
        None::<&str>,
    )?;
    let update_check = CheckMenuItem::with_id(
        app,
        UPDATE_CHECK_ID,
        update_check_text(en),
        true,
        update_check,
        None::<&str>,
    )?;
    let pdf_managed = CheckMenuItem::with_id(
        app,
        PDF_STORAGE_MANAGED_ID,
        pdf_managed_text(en),
        true,
        !pdf_drive_mode,
        None::<&str>,
    )?;
    let pdf_drive = CheckMenuItem::with_id(
        app,
        PDF_STORAGE_DRIVE_ID,
        "Google Drive",
        true,
        pdf_drive_mode,
        None::<&str>,
    )?;
    let pdf_migrate = MenuItem::with_id(
        app,
        PDF_MIGRATE_DRIVE_ID,
        pdf_migrate_text(en),
        true,
        None::<&str>,
    )?;
    let pdf_storage = Submenu::with_items(
        app,
        pdf_storage_text(en),
        true,
        &[
            &pdf_managed,
            &pdf_drive,
            &PredefinedMenuItem::separator(app)?,
            &pdf_migrate,
        ],
    )?;
    let lang_ja = CheckMenuItem::with_id(app, LANG_JA_ID, "日本語", true, !en, None::<&str>)?;
    let lang_en = CheckMenuItem::with_id(app, LANG_EN_ID, "English", true, en, None::<&str>)?;
    let language = Submenu::with_items(app, "Language", true, &[&lang_ja, &lang_en])?;
    // OpenNote。サブメニューの題名はどちらの言語でも同じ
    let wiki_on = CheckMenuItem::with_id(
        app,
        WIKI_ENABLED_ID,
        wiki_enabled_text(en),
        true,
        wiki_enabled,
        None::<&str>,
    )?;
    let wiki_reindex = MenuItem::with_id(
        app,
        WIKI_REINDEX_ID,
        wiki_reindex_text(en),
        true,
        None::<&str>,
    )?;
    let wiki_drop_index = MenuItem::with_id(
        app,
        WIKI_DROP_INDEX_ID,
        wiki_drop_index_text(en),
        true,
        None::<&str>,
    )?;
    // チャット: Wiki が有効なときだけ押せる。チェックは実際の設定（ON は画面の同意を経てから = wiki_chat_menu）
    let wiki_chat = CheckMenuItem::with_id(
        app,
        WIKI_CHAT_ID,
        wiki_chat_text(en),
        wiki_enabled,
        wiki_enabled && wiki_chat,
        None::<&str>,
    )?;
    // ページの http(s) リンクを OpenRef の別窓で開く（既定 = 外部ブラウザ）・チャットのログをワークスペースの chat-log/ に残す（既定 ON）。
    // どちらも Wiki が有効なときだけ押せる（チェックは設定の値）
    let wiki_url_window = CheckMenuItem::with_id(
        app,
        WIKI_URL_WINDOW_ID,
        wiki_url_window_text(en),
        wiki_enabled,
        wiki_url_window,
        None::<&str>,
    )?;
    let wiki_chat_log = CheckMenuItem::with_id(
        app,
        WIKI_CHAT_LOG_ID,
        wiki_chat_log_text(en),
        wiki_enabled,
        wiki_chat_log,
        None::<&str>,
    )?;
    let wiki = Submenu::with_items(
        app,
        "OpenNote",
        true,
        &[
            &wiki_on,
            &wiki_url_window,
            &PredefinedMenuItem::separator(app)?,
            &wiki_reindex,
            &wiki_drop_index,
            &PredefinedMenuItem::separator(app)?,
            &wiki_chat,
            &wiki_chat_log,
        ],
    )?;
    app.manage(SettingsItems {
        restore: restore.clone(),
        update_check: update_check.clone(),
        lang_ja: lang_ja.clone(),
        lang_en: lang_en.clone(),
        pdf_storage: pdf_storage.clone(),
        pdf_managed: pdf_managed.clone(),
        pdf_drive: pdf_drive.clone(),
        pdf_migrate: pdf_migrate.clone(),
        wiki_enabled: wiki_on.clone(),
        wiki_reindex: wiki_reindex.clone(),
        wiki_drop_index: wiki_drop_index.clone(),
        wiki_chat: wiki_chat.clone(),
        wiki_url_window: wiki_url_window.clone(),
        wiki_chat_log: wiki_chat_log.clone(),
        quit: quit.clone(),
    });
    let settings = Submenu::with_items(
        app,
        "Settings",
        true,
        &[&restore, &update_check, &pdf_storage, &wiki, &language],
    )?;
    Menu::with_items(app, &[&app_menu, &edit, &view, &settings, &window])
}

/// メニューの操作。CheckMenuItem はクリックで自分のチェックを反転するので、反転後の値を DB に書く。
/// 書けなければメニューを前の状態に戻す（次回の起動・開いている UI と食い違わせない）。
/// Language は DB に書けてから項目名を差し替えて webview へ emit する。選んでいた方を押し直したときは何も変えない
fn on_menu_event(app: &tauri::AppHandle, ev: tauri::menu::MenuEvent) {
    let items = app.state::<SettingsItems>();
    match ev.id().as_ref() {
        // ExitRequested を通す（Wiki が有効なら wiki_exit_event が保留して未保存を尋ねる）
        QUIT_ID => app.exit(0),
        RESTORE_TABS_ID => {
            let checked = match items.restore.is_checked() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("openref-app: メニューのチェック状態を読めません: {e}");
                    return;
                }
            };
            if !save_setting(app, "ui_restore_tabs", if checked { "1" } else { "0" }) {
                let _ = items.restore.set_checked(!checked);
            }
        }
        UPDATE_CHECK_ID => {
            // 確認は起動時だけなので、入れ直しが効くのは次の起動から。外したら前回の結果（update_latest）も出さない = core update::check。
            // 出ている帯はその場で消す（webview へ "update-check-off"。App が updateLatest を null に）
            let checked = match items.update_check.is_checked() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("openref-app: メニューのチェック状態を読めません: {e}");
                    return;
                }
            };
            if !save_setting(app, update::KEY_ENABLED, if checked { "1" } else { "0" }) {
                let _ = items.update_check.set_checked(!checked);
            } else if !checked {
                if let Err(e) = app.emit("update-check-off", ()) {
                    eprintln!("openref-app: update-check-off を送れません: {e}");
                }
            }
        }
        id @ (LANG_JA_ID | LANG_EN_ID) => {
            let en = id == LANG_EN_ID;
            let picked = if en { &items.lang_en } else { &items.lang_ja };
            // クリックで反転済み = チェックが外れたなら、選んでいた方を押し直した（言語は変わらない）
            let switched = picked.is_checked().unwrap_or(true);
            let saved = switched && save_setting(app, "ui_lang", if en { "en" } else { "ja" });
            // 押し直し・書けた = 選んだ言語 / 書けなかった = 前の言語のまま
            let now_en = if switched && !saved { !en } else { en };
            let menu = items
                .lang_ja
                .set_checked(!now_en)
                .and(items.lang_en.set_checked(now_en))
                .and(items.restore.set_text(restore_tabs_text(now_en)))
                .and(items.update_check.set_text(update_check_text(now_en)))
                .and(items.pdf_storage.set_text(pdf_storage_text(now_en)))
                .and(items.pdf_managed.set_text(pdf_managed_text(now_en)))
                .and(items.pdf_migrate.set_text(pdf_migrate_text(now_en)))
                .and(items.wiki_enabled.set_text(wiki_enabled_text(now_en)))
                .and(items.wiki_reindex.set_text(wiki_reindex_text(now_en)))
                .and(items.wiki_drop_index.set_text(wiki_drop_index_text(now_en)))
                .and(items.wiki_chat.set_text(wiki_chat_text(now_en)))
                .and(items.wiki_url_window.set_text(wiki_url_window_text(now_en)))
                .and(items.wiki_chat_log.set_text(wiki_chat_log_text(now_en)))
                .and(items.quit.set_text(quit_text(now_en)));
            if let Err(e) = menu {
                eprintln!("openref-app: メニューを更新できません: {e}");
            }
            if saved {
                if let Err(e) = app.emit("ui-lang", if en { "en" } else { "ja" }) {
                    eprintln!("openref-app: ui-lang を webview へ送れません: {e}");
                }
            }
        }
        id @ (PDF_STORAGE_MANAGED_ID | PDF_STORAGE_DRIVE_ID) => {
            let want_drive = id == PDF_STORAGE_DRIVE_ID;
            let picked = if want_drive {
                &items.pdf_drive
            } else {
                &items.pdf_managed
            };
            let switched = picked.is_checked().unwrap_or(true);
            let en = items.lang_en.is_checked().unwrap_or(false);
            if !switched {
                // 既に選ばれている方を押し直した → チェックを戻すだけ
                let _ = items
                    .pdf_managed
                    .set_checked(!want_drive)
                    .and(items.pdf_drive.set_checked(want_drive));
                return;
            }
            // Drive の検出・作成・書込プローブは Drive（File Provider）の応答待ちで秒単位になりうる → main thread から外す
            // （「Drive へ移行」と同じ型）。DB のロックは設定の保存と件数の間だけ（core mount::enable_drive_mode）
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let st = app.state::<AppState>();
                let items = app.state::<SettingsItems>();
                let res = if want_drive {
                    openref_core::mount::enable_drive_mode(|| core_lock(&st.db)).map(|info| {
                        if en {
                            if info.managed_remaining > 0 {
                                format!(
                                    "✓ PDF storage set to Google Drive ({} local PDFs can be moved from Settings › PDF storage)",
                                    info.managed_remaining
                                )
                            } else {
                                "✓ PDF storage set to Google Drive".to_string()
                            }
                        } else if info.managed_remaining > 0 {
                            format!(
                                "✓ PDF の保存先を Google Drive に切り替えました（ローカル保存済みの {} 件は Settings › PDF の保存先 から移行できます）",
                                info.managed_remaining
                            )
                        } else {
                            "✓ PDF の保存先を Google Drive に切り替えました".to_string()
                        }
                    })
                } else {
                    core_lock(&st.db)
                        .and_then(|db| openref_core::mount::disable_drive_mode(&db))
                        .map(|()| {
                            if en {
                                "✓ PDF storage set to Local (on this Mac)".to_string()
                            } else {
                                "✓ PDF の保存先をローカル (Mac 内) に切り替えました".to_string()
                            }
                        })
                };
                match res {
                    Ok(msg) => {
                        let _ = items
                            .pdf_managed
                            .set_checked(!want_drive)
                            .and(items.pdf_drive.set_checked(want_drive));
                        let _ = app.emit("menu-toast", msg);
                    }
                    Err(e) => {
                        let _ = items
                            .pdf_managed
                            .set_checked(want_drive)
                            .and(items.pdf_drive.set_checked(!want_drive));
                        let prefix = if en {
                            "✗ Could not switch PDF storage: "
                        } else {
                            "✗ PDF 保存先の切替に失敗: "
                        };
                        let _ = app.emit("menu-toast", format!("{prefix}{e}"));
                    }
                }
            });
        }
        PDF_MIGRATE_DRIVE_ID => {
            let en = items.lang_en.is_checked().unwrap_or(false);
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let st = app.state::<AppState>();
                // ロックは一覧の読出しと 1 件ごとの確認・書換えの間だけ（コピーと読み戻しはロックの外 = core mount::migrate_managed_to_drive）
                let res =
                    openref_core::mount::migrate_managed_to_drive(|| core_lock(&st.db), false);
                let msg = match res {
                    Ok(mig) if mig.moved.is_empty() && mig.skipped.is_empty() => {
                        if en {
                            "No local PDFs to move (all PDFs are already on Google Drive)"
                                .to_string()
                        } else {
                            "移行対象のローカル PDF はありません（移行済み）".to_string()
                        }
                    }
                    Ok(mig) => {
                        let mut msg = if en {
                            format!("✓ Moved {} local PDF(s) to Google Drive", mig.moved.len())
                        } else {
                            format!(
                                "✓ ローカル保存済みの PDF {} 件を Google Drive へ移行しました",
                                mig.moved.len()
                            )
                        };
                        // 移行の間に変わった（削除・置換）添付は移さない = 件数と、消せなかったコピーがあればその場所を出す
                        if !mig.skipped.is_empty() {
                            msg += &if en {
                                format!(
                                    " ({} changed during the move and were left as they are)",
                                    mig.skipped.len()
                                )
                            } else {
                                format!(
                                    "（移行の間に変わった {} 件は移していません）",
                                    mig.skipped.len()
                                )
                            };
                        }
                        let left: Vec<String> = mig
                            .skipped
                            .iter()
                            .filter_map(|s| {
                                s.leftover
                                    .as_ref()
                                    .map(|(p, e)| format!("{} ({e})", p.display()))
                            })
                            .collect();
                        if !left.is_empty() {
                            msg += &if en {
                                format!(" — could not remove copies: {}", left.join(", "))
                            } else {
                                format!("・消せなかったコピー: {}", left.join("、"))
                            };
                        }
                        msg
                    }
                    Err(e) => {
                        let prefix = if en {
                            "✗ Failed to move PDFs to Google Drive: "
                        } else {
                            "✗ Google Drive への PDF 移行に失敗: "
                        };
                        format!("{prefix}{e}")
                    }
                };
                let _ = app.emit("menu-toast", msg);
            });
        }
        WIKI_CHAT_ID => {
            let en = items.lang_en.is_checked().unwrap_or(false);
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || wiki_chat_menu(&app, en));
        }
        // 反転後の値を書く（書けなければ戻す）。ログの ON/OFF は動いている子を止めずに次の発言から効く（下の Chat::set_log）
        id @ (WIKI_URL_WINDOW_ID | WIKI_CHAT_LOG_ID) => {
            let (item, key) = if id == WIKI_URL_WINDOW_ID {
                (&items.wiki_url_window, WIKI_URL_WINDOW_KEY)
            } else {
                (&items.wiki_chat_log, WIKI_CHAT_LOG_KEY)
            };
            let checked = match item.is_checked() {
                Ok(c) => c,
                Err(e) => return eprintln!("openref-app: メニューのチェック状態を読めません: {e}"),
            };
            if !save_setting(app, key, if checked { "1" } else { "0" }) {
                let _ = item.set_checked(!checked);
            } else if id == WIKI_CHAT_LOG_ID {
                // ログを書くのは親（Chat の Inner）なので子は止めない = 応答中でも切り替わる（次の発言から）
                if let Some(c) = app.try_state::<wiki_chat::Chat>() {
                    c.set_log(checked);
                }
            }
        }
        id @ (WIKI_ENABLED_ID | WIKI_REINDEX_ID | WIKI_DROP_INDEX_ID) => {
            // フォルダの選択ダイアログ（blocking）・走査は main thread から外す
            let en = items.lang_en.is_checked().unwrap_or(false);
            let (app, id) = (app.clone(), id.to_string());
            tauri::async_runtime::spawn_blocking(move || wiki_menu(&app, &id, en));
        }
        _ => {}
    }
}

/// メニューの設定を settings に書く。失敗はログに出して false
fn save_setting(app: &tauri::AppHandle, key: &str, value: &str) -> bool {
    let st = app.state::<AppState>();
    let locked = st.db.lock(); // let に受ける（末尾の match の一時値だと st より長く生きて E0597）
    match locked {
        Ok(db) => match db.set_setting(key, value) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("openref-app: {key} を保存できません: {e}");
                false
            }
        },
        Err(e) => {
            eprintln!("openref-app: DB のロックに失敗: {e}");
            false
        }
    }
}

// ---------- OpenNote（読み）: command の包み（wiki_call / wiki_run と節の説明は wiki_gui.rs） ----------

#[tauri::command]
async fn wiki_status(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_call(
        &state,
        "wiki_status",
        args,
        openref_core::wiki::commands::status,
    )
    .await
}

#[tauri::command]
async fn wiki_list(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_call(
        &state,
        "wiki_list",
        args,
        openref_core::wiki::commands::list,
    )
    .await
}

#[tauri::command]
async fn wiki_read_gui(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::commands::read).await
}

#[tauri::command]
async fn wiki_backlinks(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_call(
        &state,
        "wiki_backlinks",
        args,
        openref_core::wiki::commands::backlinks,
    )
    .await
}

#[tauri::command]
async fn wiki_search(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_call(
        &state,
        "wiki_search",
        args,
        openref_core::wiki::commands::search,
    )
    .await
}

#[tauri::command]
async fn wiki_tags(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_call(
        &state,
        "wiki_tags",
        args,
        openref_core::wiki::commands::tags,
    )
    .await
}

#[tauri::command]
async fn wiki_inbox(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_call(
        &state,
        "wiki_inbox",
        args,
        openref_core::wiki::commands::inbox,
    )
    .await
}

// ---------- OpenNote（編集・GUI 専用） ----------
// core::wiki::edit の関数を読みと同じ型（WikiFn）で包む（Db のロックは設定を読む間だけ・ファイル操作は blocking の中）。
// wiki_move は登録表にもある = 登録表と同じ関数・同じ検査（wiki_call）。wiki_save は GUI の
// エディタの保存 = edit::write（if_sha256 が一致した自動保存は退避しない = 世代の押し出しを防ぐ・競合ダイアログの force を受ける）で、
// 登録表の wiki_write（edit::write_agent = 上書きは必ず退避・force なし）とは別の関数 = 別名にして GUI_ONLY・登録表の schema で検査しない。
// 他も GUI_ONLY。引数は invoke(name, { args: {...} })。
// 書いたパスは core が同期で索引に取り込む。監視（wiki_on_fs）が同じ書込を拾ってもう一度取り込んでも結果は同じ（refresh_paths は冪等・
// 一時ファイルは `.` 始まり = 監視が無視する）

#[tauri::command]
async fn wiki_save(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::edit::write).await
}

#[tauri::command]
async fn wiki_move(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_call(&state, "wiki_move", args, openref_core::wiki::edit::move_).await
}

#[tauri::command]
async fn wiki_create(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::edit::create).await
}

#[tauri::command]
async fn wiki_mkdir(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::edit::mkdir).await
}

#[tauri::command]
async fn wiki_rename_preview(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::edit::rename_preview).await
}

#[tauri::command]
async fn wiki_rename(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::edit::rename).await
}

#[tauri::command]
async fn wiki_trash(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, wiki_trash_core).await
}

#[tauri::command]
async fn wiki_save_blob(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::edit::save_blob).await
}

#[tauri::command]
async fn wiki_import_file(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::edit::import_file).await
}

// ---------- OpenNote（終了の保留）: command の包み（保留の本体と節の説明は wiki_gui.rs） ----------

/// "wiki-exit-request" への応答: proceed = 保存が済んだので終了する（窓の × なら閉じる）・false = 取りやめ
#[tauri::command]
fn wiki_exit_ready(app: tauri::AppHandle, proceed: bool) {
    wiki_exit_finish(&app, None, proceed);
}

// ---------- OpenNote（GUI 専用: 有効化・vault の root・索引・外部で開く）: command の包み（本体と節の説明は wiki_gui.rs・開く先の判定は wiki_asset.rs） ----------

/// 起動時の Wiki の有効 / 無効と root（設定を読むだけ・索引に触らない = 起動の restoreTabs を Wiki の走査で待たせない）
#[tauri::command]
fn wiki_settings(state: tauri::State<AppState>) -> Result<serde_json::Value, String> {
    let ctx = openref_core::wiki::Ctx::from_db(&*lock(&state.db)?).map_err(err)?;
    Ok(serde_json::json!({"enabled": ctx.enabled, "root": ctx.root}))
}

/// 戻り値 = このプロセスで vault の配信を始めていて、再起動まで続く（OFF のときだけ true になりうる）
#[tauri::command]
async fn wiki_set_enabled(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    blocking(move || wiki_enable(&app, enabled)).await
}

/// 左の木の vault 名 → フォルダ選択ダイアログ → 検査・保存・適用（Settings のメニュー項目「フォルダを選ぶ…」をここへ移した）。
/// 取り消し = null。選んだ = {root, restart}（restart = 前のフォルダの配信が再起動まで残る）。toast は Rust が出す
#[tauri::command]
async fn wiki_pick_root(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    blocking(move || {
        let en = ui_en(&app);
        let Some(p) = wiki_pick_folder(&app, en) else {
            return Ok(serde_json::Value::Null);
        };
        match wiki_choose_root(&app, &p) {
            Ok((root, stale)) => {
                wiki_toast(&app, en, true, "OpenNote のフォルダ", "OpenNote folder", Some(root.to_string_lossy().into_owned()));
                if stale {
                    wiki_restart_notice(&app, en);
                }
                Ok(serde_json::json!({"root": root.to_string_lossy(), "restart": stale}))
            }
            Err(e) => {
                wiki_toast(&app, en, false, "このフォルダは使えません", "This folder cannot be used", Some(e.clone()));
                Err(e)
            }
        }
    })
    .await
}

#[tauri::command]
async fn wiki_reindex(app: tauri::AppHandle) -> Result<(), String> {
    blocking(move || wiki_reindex_now(&app)).await
}

/// Wiki のページから実在ファイルをホストの open で開く（vault の中、または vault の外の実在する通常ファイル = wiki_open_target）。
/// 文書の拡張子（WIKI_OPEN_EXT）以外・実行ビット付き・OpenWith の拡張属性付き・隠しフォルダの中（vault の外）は open せず
/// `open -R`（Finder で場所を表示）に落とす。戻り値 = Finder で表示しただけ（呼び手が知らせる）。
/// 既存の open_external は http(s) 専用・open_attachment は添付の id 専用なので別に置く
#[tauri::command]
async fn wiki_open_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<bool, String> {
    let ctx = {
        let db = lock(&state.db)?;
        openref_core::wiki::Ctx::from_db(&db).map_err(err)?
    }; // ここでロックを外す
    let en = ui_en(&app);
    blocking(move || {
        let (_, root) = ctx.ready().map_err(err)?;
        let (abs, reveal) = wiki_open_target(&root, &path, en)?;
        if reveal {
            let out = std::process::Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&abs)
                .output()
                .map_err(err)?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .next()
                    .unwrap_or("open -R failed")
                    .to_string());
            }
            return Ok(true);
        }
        open_file(&abs.to_string_lossy()).map(|()| false)
    })
    .await
}

// ---------- OpenNote: 参照フォルダと URL の別窓: command の包み（本体と節の説明は wiki_gui.rs） ----------

/// {} → {dirs}。設定が解釈できなければ Err（画面が toast で知らせる）。検査（canonicalize）は main thread の外で
#[tauri::command]
async fn wiki_ref_dirs_get(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    blocking(move || Ok(serde_json::json!({"dirs": wiki_ref_dirs(&app)?}))).await
}

/// フォルダの選択ダイアログを開いて検査し、足す → {dirs, added: path | null}（取り消し = null）。Wiki が有効ならその場で asset scope に足す
#[tauri::command]
async fn wiki_ref_dir_add(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    blocking(move || {
        let en = ui_en(&app);
        let Some(p) = wiki_pick_folder_titled(
            &app,
            if en {
                "Choose a reference folder"
            } else {
                "参照フォルダを選ぶ"
            },
        ) else {
            // 取り消しは静か（一覧が読めなければ null = 画面は今のまま）
            return Ok(serde_json::json!({"dirs": wiki_ref_dirs(&app).ok(), "added": null}));
        };
        let (raw, ctx) = wiki_ref_dirs_raw(&app)?;
        let home = ctx.home.as_deref().ok_or_else(|| {
            tr(
                en,
                "HOME が未設定のため検査できません",
                "HOME is not set, so the folder cannot be checked",
            )
        })?;
        let canon = wiki_ref_dir_check(
            &p,
            home,
            ctx.root.as_deref().map(Path::new),
            ctx.app_dir.as_deref(),
            en,
        )?;
        let plan = wiki_ref_dirs_for_add(raw.as_deref(), &canon, en);
        let mut set_aside: Option<String> = None;
        if let Some(reason) = &plan.broken {
            // 解釈できない値は捨てずに別の鍵へ退避してから書く（sql の settings で読める）。知らせるのは画面（応答の set_aside）と stderr
            let key = wiki_ref_dirs_broken_key(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            );
            if !save_setting(&app, &key, raw.as_deref().unwrap_or("")) {
                return Err(format!(
                    "{}: {reason}",
                    tr(
                        en,
                        "解釈できない設定 wiki_ref_dirs を退避できません（追加しません）",
                        "Could not set aside the unreadable wiki_ref_dirs setting (nothing added)"
                    )
                ));
            }
            eprintln!("openref-app: wiki: 設定 {WIKI_REF_DIRS_KEY} を解釈できないので {key} に退避しました: {reason}");
            set_aside = Some(key);
        }
        if plan.changed {
            wiki_save_ref_dirs(&app, &plan.dirs)?;
        }
        if ctx.enabled {
            wiki_allow_dir(&app, &canon, plan.kind)?;
        }
        wiki_emit(&app, "wiki-state", ());
        Ok(serde_json::json!({
            "dirs": wiki_ref_dirs_usable(plan.dirs, &ctx),
            "added": canon,
            "set_aside": set_aside,
        }))
    })
    .await
}

/// {path} → {dirs, restart}。restart = このプロセスで asset scope に足してあった（配信は再起動まで続く・リンクは外部で開くようになる）
#[tauri::command]
async fn wiki_ref_dir_remove(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let path = obj_args(args)
        .get("path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| {
            tr(
                en,
                "wiki_ref_dir_remove: path は必須です",
                "wiki_ref_dir_remove: path is required",
            )
        })?;
    blocking(move || {
        // 生の一覧から外す（検査に通らないものは残す = 見えないディスクの参照フォルダを消さない）
        let (raw, ctx) = wiki_ref_dirs_raw(&app)?;
        let mut dirs = wiki_ref_dirs_parse(raw.as_deref(), ui_en(&app))?;
        dirs.retain(|d| *d != path);
        wiki_save_ref_dirs(&app, &dirs)?;
        let restart = app
            .state::<WikiRuntime>()
            .allowed
            .lock()
            .map_err(err)?
            .contains(&path);
        wiki_emit(&app, "wiki-state", ());
        Ok(serde_json::json!({"dirs": wiki_ref_dirs_usable(dirs, &ctx), "restart": restart}))
    })
    .await
}

/// {abs, max_chars?} → WikiPageData 相当 + external（参照フォルダ）。html は本文（script は画面側が落とす・リンクの解決は画面側 = links は空）・
/// media は種類だけ。md・その他は Err（画面は外部で開く）。索引には触らない
#[tauri::command]
async fn wiki_read_ref(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    use openref_core::wiki::scan;
    let en = ui_en(&app);
    let a = obj_args(args);
    let abs = a
        .get("abs")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            tr(
                en,
                "wiki_read_ref: abs は必須です",
                "wiki_read_ref: abs is required",
            )
        })?;
    let max_chars = a
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .unwrap_or(200_000) as usize;
    // probe = 開けるかだけ確かめる（リンクを押したとき。拒否ならフロントはホストの open に落とす）
    let probe = a.get("probe").and_then(|v| v.as_bool()) == Some(true);
    blocking(move || {
        let (dir, canon) = wiki_ref_target(&wiki_ref_dirs(&app)?, &abs, en)?;
        let name = canon
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let kind = scan::Kind::of(&name);
        let meta = std::fs::metadata(&canon).map_err(err)?;
        let show = wiki_ref_show(kind, probe, meta.len(), &name, en)?;
        if show == WikiRefShow::Probe {
            return Ok(serde_json::json!({"ok": true, "external": dir.to_string_lossy()}));
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut out = serde_json::json!({
            "path": abs,
            "abs_path": canon.to_string_lossy(),
            "external": dir.to_string_lossy(),
            "kind": kind.as_str(),
            "size": meta.len(),
            "mtime": mtime,
            "title": name,
        });
        match show {
            WikiRefShow::Probe => unreachable!("probe は上で返している"),
            WikiRefShow::Media => {
                out["media"] = serde_json::json!(scan::media_kind(
                    name.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
                ));
            }
            WikiRefShow::Html => {
                let bytes = std::fs::read(&canon).map_err(err)?;
                wiki_ref_html_fields(&mut out, kind, &bytes, max_chars);
            }
        }
        Ok(out)
    })
    .await
}

/// {url} → http(s) を、設定 wiki_url_window なら OpenRef の別窓で、でなければ既定のブラウザで開く（Wiki のページのリンク専用。
/// MY LIBRARY の open_external は今どおり外部ブラウザ）
#[tauri::command]
async fn wiki_open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let en = ui_en(&app);
    let u = tauri::Url::parse(url.trim()).map_err(|_| tr(en, "URL ではありません", "Not a URL"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(tr(en, "http(s) のみ許可", "Only http(s) is allowed"));
    }
    let window = {
        let st = app.state::<AppState>();
        let db = lock(&st.db)?;
        db.setting(WIKI_URL_WINDOW_KEY).map_err(err)?.as_deref() == Some("1")
    };
    if !window {
        return blocking(move || open_with_default_app(u.as_str())).await;
    }
    // 窓は main thread で作る（WebviewWindowBuilder::build）。結果を待つ
    let (tx, rx) = std::sync::mpsc::channel();
    let h = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(wiki_url_window_show(&h, u));
    })
    .map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| {
            tr(
                en,
                "窓を開く処理が応答しません",
                "Opening the window did not respond",
            )
        })?
    })
    .await
    .map_err(err)?
}

// ---------- OpenNote（チャット）: command の包み（設定・status の本体と節の説明は wiki_gui.rs・子プロセスは wiki_chat.rs） ----------

#[tauri::command]
async fn wiki_chat_status(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    // {force: true} = 画面の [再確認]（覚えた claude auth status を捨てて読む）。自動の読み直し（mount・exit 等）は force なし
    let force = wiki_chat_force(args);
    if force {
        wiki_chat::forget_auth();
    }
    blocking(move || wiki_chat_status_now(&app, force)).await
}

/// {enabled} → WikiChatStatus（画面の同意のダイアログが ON を・画面が OFF を呼ぶ）
#[tauri::command]
async fn wiki_chat_set_enabled(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let on = a.get("enabled").and_then(|v| v.as_bool()).ok_or_else(|| {
        tr(
            en,
            "wiki_chat_set_enabled: enabled（true / false）は必須です",
            "wiki_chat_set_enabled: enabled (true / false) is required",
        )
    })?;
    blocking(move || {
        wiki_chat_enable(&app, on)?;
        wiki_chat::forget_auth();
        wiki_chat_status_now(&app, false)
    })
    .await
}

/// {new?} → WikiChatStartResult。使えない（無効・claude が無い・未ログイン・openref-mcp が無い / 古い）なら Err（理由の文）。
/// 返る前に必ず "wiki-chat" {type: "state"} を出す（成功 = idle・失敗 = off）
#[tauri::command]
async fn wiki_chat_start(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let new = obj_args(args)
        .get("new")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let h = app.clone();
    let res = wiki_chat_start_now(app, new).await;
    wiki_chat_emit_state(&h);
    res
}

/// {text, context?: {path?, selection?, item?, title?}} → {turn}（item / title = PDF のタブで開いている文献）
#[tauri::command]
async fn wiki_chat_send(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let text = a
        .get("text")
        .and_then(|v| v.as_str())
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            tr(
                en,
                "wiki_chat_send: text（空でない文字列）は必須です",
                "wiki_chat_send: text (a non-empty string) is required",
            )
        })?
        .to_string();
    let ctx = a
        .get("context")
        .filter(|c| c.is_object())
        .cloned()
        .unwrap_or_default();
    let path = ctx.get("path").and_then(|v| v.as_str()).map(str::to_string);
    let selection = ctx
        .get("selection")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let item = ctx.get("item").and_then(|v| v.as_str()).map(|k| {
        (
            k.to_string(),
            ctx.get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        )
    });
    blocking(move || {
        let item = item.as_ref().map(|(k, t)| (k.as_str(), t.as_str()));
        let turn = app.state::<wiki_chat::Chat>().send_with(
            &text,
            path.as_deref(),
            selection.as_deref(),
            item,
            ui_en(&app),
        )?;
        Ok(serde_json::json!({"turn": turn}))
    })
    .await
}

#[tauri::command]
async fn wiki_chat_interrupt(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<(), String> {
    let _ = args;
    blocking(move || app.state::<wiki_chat::Chat>().interrupt()).await
}

#[tauri::command]
async fn wiki_chat_stop(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<(), String> {
    let _ = args;
    blocking(move || {
        app.state::<wiki_chat::Chat>()
            .stop(Duration::from_secs(3), Duration::from_secs(2));
        Ok(())
    })
    .await
}

/// {model} → WikiChatStatus。設定 wiki_chat_model を書き、子プロセスを止める（次の送信で --resume + 新しい --model で起こし直す）
#[tauri::command]
async fn wiki_chat_set_model(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let model = a
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            tr(
                en,
                "wiki_chat_set_model: model（文字列・空 = 既定）は必須です",
                "wiki_chat_set_model: model (a string; empty = default) is required",
            )
        })?
        .trim()
        .to_string();
    if !wiki_chat_model_ok(&model) {
        return Err(tr(
            en,
            &format!("wiki_chat_set_model: model の表記が不正です: {model}"),
            &format!("wiki_chat_set_model: invalid model name: {model}"),
        ));
    }
    let h = app.clone();
    let res = blocking(move || {
        let en = ui_en(&app);
        if !save_setting(&app, WIKI_CHAT_MODEL_KEY, &model) {
            return Err(tr(
                en,
                "設定 wiki_chat_model を保存できません",
                "Could not save the wiki_chat_model setting",
            ));
        }
        if let Some(c) = app.try_state::<wiki_chat::Chat>() {
            c.stop(Duration::from_secs(3), Duration::from_secs(2));
        }
        wiki_chat_status_now(&app, false)
    })
    .await;
    wiki_chat_emit_state(&h);
    res
}

/// {action: "login" | "logout"} → WikiChatStatus。logout = 子を止めて `claude auth logout`・login = `<app_dir>/claude-login.command` を
/// Terminal.app に開かせる（対話 = ブラウザ + ターミナル。OpenRef は認証情報に触らない）
#[tauri::command]
async fn wiki_chat_auth(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let action = a
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            tr(
                en,
                "wiki_chat_auth: action（login / logout）は必須です",
                "wiki_chat_auth: action (login / logout) is required",
            )
        })?
        .to_string();
    if action != "login" && action != "logout" {
        return Err(tr(
            en,
            &format!("wiki_chat_auth: action は login か logout: {action}"),
            &format!("wiki_chat_auth: action must be login or logout: {action}"),
        ));
    }
    let h = app.clone();
    let res = blocking(move || {
        let en = ui_en(&app);
        let (ctx, on, _) = wiki_chat_settings(&app)?;
        if !(on && ctx.enabled) {
            return Err(tr(
                en,
                "チャットは無効です（Settings › OpenNote › チャット）",
                "The chat is disabled (Settings › OpenNote › Chat)",
            ));
        }
        let cli = wiki_chat::find_cli(ctx.home.as_deref(), std::env::var_os("PATH").as_deref())
            .ok_or_else(|| {
                tr(
                    en,
                    "Claude Code（claude コマンド）が見つかりません",
                    "Claude Code (the claude command) was not found",
                )
            })?;
        if action == "logout" {
            if let Some(c) = app.try_state::<wiki_chat::Chat>() {
                c.stop(Duration::from_secs(3), Duration::from_secs(2));
            }
            // 子を止めた exit で画面が読み直した分（ログアウト前の状態）を残さない = 成否によらず後で捨てる
            let r = wiki_chat::cli_logout(&cli, en);
            wiki_chat::forget_auth();
            r?;
        } else {
            let app_dir = ctx.app_dir.clone().ok_or_else(|| {
                tr(
                    en,
                    "in-memory の DB ではログインを開けません",
                    "Cannot open the login with an in-memory database",
                )
            })?;
            wiki_chat::open_login_terminal(&cli, &app_dir)?;
        }
        wiki_chat::forget_auth();
        wiki_chat_status_now(&app, false)
    })
    .await;
    wiki_chat_emit_state(&h);
    res
}

/// {turn, index} → WikiChatUndoResult
#[tauri::command]
async fn wiki_chat_undo(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let turn = a.get("turn").and_then(|v| v.as_u64()).ok_or_else(|| {
        tr(
            en,
            "wiki_chat_undo: turn は必須です",
            "wiki_chat_undo: turn is required",
        )
    })?;
    let index = a.get("index").and_then(|v| v.as_u64()).ok_or_else(|| {
        tr(
            en,
            "wiki_chat_undo: index は必須です",
            "wiki_chat_undo: index is required",
        )
    })? as usize;
    blocking(move || {
        let en = ui_en(&app);
        let chat = app.state::<wiki_chat::Chat>();
        let refused = |reason: String| serde_json::json!({"ok": false, "turn": turn, "index": index, "reason": reason});
        let Some((rec, later)) = chat.record(turn, index) else {
            return Ok(refused(tr(en, "その変更の記録がありません（アプリの再起動・新しい会話で消えます）", "No record of that change (records are cleared on restart or a new conversation)")));
        };
        if rec.undone {
            return Ok(refused(tr(en, "既に戻しています", "Already undone")));
        }
        let ctx = wiki_ctx(&app)?;
        let trash_item = |key: &str| -> Result<(), String> {
            let st = app.state::<AppState>();
            let mut db = lock(&st.db)?;
            openref_core::commands::call(&mut db, "trash_item", &serde_json::json!({"key": key})).map(|_| ()).map_err(err)
        };
        Ok(match wiki_chat::undo_record(&ctx, &rec, later, en, wiki_move_to_trash, trash_item) {
            wiki_chat::Undo::Done { path, key } => {
                chat.mark_undone(turn, index);
                let mut v = serde_json::json!({"ok": true, "turn": turn, "index": index});
                if let Some(p) = path {
                    v["path"] = serde_json::json!(p);
                }
                if let Some(k) = key {
                    v["key"] = serde_json::json!(k);
                }
                v
            }
            wiki_chat::Undo::Refused(reason) => refused(reason),
        })
    })
    .await
}

/// 窓を出す前の致命的エラー（データ移行の中止など）。stderr と警告ダイアログ（osascript）に出して終了する
fn fatal(msg: &str) -> ! {
    eprintln!("openref-app: {msg}");
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display alert (item 1 of argv) message (item 2 of argv) as critical",
            "-e",
            "end run",
        ])
        .args(["OpenRef を起動できません", msg])
        .status();
    std::process::exit(1);
}

fn main() {
    let db = match datadir::db_path().and_then(|p| Db::open(&p)) {
        Ok(db) => db,
        Err(e) => fatal(&e.to_string()),
    };
    // Settings › 起動時に前回のタブを開く（既定 = 開く。"0" のときだけ外す）・起動時に新版を確認（既定 = 確認。"0" のときだけ外す）
    // ・Language（既定 = 日本語。"en" のときだけ English）・PDF の保存先。
    // 読めなければ（BUSY 等）stderr に出して既定で描く = メニューの表示だけ（保存先は添付のたびに core が読み直し、読めなければ添付を失敗にする）
    let unread = |what: &str, e: openref_core::CoreError| {
        eprintln!("openref-app: 起動時に {what} を読めません（メニューは既定で表示）: {e}")
    };
    let pref = |key: &str| {
        db.setting(key).unwrap_or_else(|e| {
            unread(key, e);
            None
        })
    };
    let restore_tabs = pref("ui_restore_tabs").as_deref() != Some("0");
    let update_check = update::cached(&db).map(|c| c.enabled).unwrap_or_else(|e| {
        unread("update_check", e);
        true
    });
    let lang_en = pref("ui_lang").as_deref() == Some("en");
    let pdf_drive_mode = db.pdf_write_mode().unwrap_or_else(|e| {
        unread("pdf_write_mode", e);
        "managed".into()
    }) == "drive";
    let wiki_enabled = pref("wiki_enabled").as_deref() == Some("1");
    let wiki_chat = pref(WIKI_CHAT_KEY).as_deref() == Some("1");
    let wiki_url_window = pref(WIKI_URL_WINDOW_KEY).as_deref() == Some("1");
    let wiki_chat_log = wiki_chat_log_on(pref(WIKI_CHAT_LOG_KEY).as_deref());
    WIKI_ON.store(wiki_enabled, std::sync::atomic::Ordering::Relaxed);
    let wiki_live = Arc::new(WikiLive::default());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // asset:// は自前で配信する（Tauri の既定と同じ scope・同じ URL の形。違い = 要求した webview を見て main 以外には 403）。
        // Tauri は External URL の窓（wiki-url）にも asset を登録するので、既定のままだと外部ページが vault / 参照フォルダを読める
        .register_asynchronous_uri_scheme_protocol("asset", |ctx, req, responder| {
            let label = ctx.webview_label().to_string();
            let scope = ctx.app_handle().asset_protocol_scope();
            // ファイル IO は blocking のプール（上限付き）で = 画像の多いページ・動画のシークで要求ごとに OS スレッドを作らない
            tauri::async_runtime::spawn_blocking(move || {
                let r = asset_reply(&label, &|p| scope.is_allowed(p), &req);
                responder.respond(r);
            });
        })
        // OpenNote: WebView の遷移（iframe の中も = wry は WKNavigationDelegate の全フレームの判定をここへ渡す）を
        // wiki_nav_allowed で絞る: asset は中身が PDF のファイルだけ・http(s) は dev の origin（tauri dev の devUrl）だけ。
        // 止めた遷移は外部ブラウザにも渡さず、stderr と画面の知らせ（wiki_nav_blocked_notice）に出す
        .plugin(
            tauri::plugin::Builder::<tauri::Wry, ()>::new("openref-wiki-nav")
                .on_navigation(|webview, url| {
                    // URL の別窓は http(s) だけ（アプリの中身 = asset / tauri には触れない）
                    if webview.label() == WIKI_URL_LABEL {
                        let ok = wiki_url_window_nav_allowed(url);
                        if !ok {
                            eprintln!("openref-app: URL の窓の遷移を止めました: {url}");
                        }
                        return ok;
                    }
                    let dev = if tauri::is_dev() {
                        webview.config().build.dev_url.clone()
                    } else {
                        None
                    };
                    let scope = webview.asset_protocol_scope();
                    // main thread で走る: Tauri の is_allowed（canonicalize = stat / realpath）より先に、このプロセスで scope に足した
                    // vault の root の文字列の接頭辞で弾く（autofs 等の応答しないパスで止まらない）
                    let roots: Vec<PathBuf> = webview
                        .try_state::<WikiRuntime>()
                        .and_then(|rt| rt.allowed.lock().ok().map(|a| a.all()))
                        .unwrap_or_default();
                    let ok = wiki_nav_allowed(url, dev.as_ref(), &|p| {
                        wiki_under_roots(&roots, p) && scope.is_allowed(p)
                    });
                    if !ok {
                        eprintln!(
                            "openref-app: WebView の遷移を止めました（許可リストの外）: {url}"
                        );
                        wiki_nav_blocked_notice(webview);
                    }
                    ok
                })
                .build(),
        )
        .menu(move |app| {
            app_menu(
                app,
                MenuInit {
                    restore_tabs,
                    update_check,
                    en: lang_en,
                    pdf_drive_mode,
                    wiki_enabled,
                    wiki_chat,
                    wiki_url_window,
                    wiki_chat_log,
                },
            )
        })
        .on_menu_event(on_menu_event)
        .setup(|app| {
            // OpenNote: 有効なら vault の root を asset scope に足して監視を張る（OFF・未設定なら何もしない =
            // scope は tauri.conf.json で空のまま）。読めない・検査に落ちたときは stderr に出して足さない（起動は止めない）
            // check_root の canonicalize（Drive 上なら応答待ち）と FSEvents の開始は main thread から外す。
            // asset scope の登録は Wiki の描画（wiki_status → 木 → ページ）より前に終わっていれば足りる
            // チャット: event "wiki-chat" を出す子プロセスの持ち主（子は wiki_chat_start まで起こさない）
            let h = app.handle().clone();
            app.manage(wiki_chat::Chat::new(std::sync::Arc::new(move |v| {
                wiki_emit(&h, "wiki-chat", v)
            })));
            let h = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = wiki_apply(&h, WikiScan::None) {
                    eprintln!("openref-app: ワークスペースを asset scope に足せません: {e}");
                }
            });
            Ok(())
        })
        .manage(AppState {
            db: Arc::new(Mutex::new(db)),
            wiki_live: wiki_live.clone(),
        })
        .manage(migration::Jobs::default())
        .manage(WikiRuntime {
            live: wiki_live,
            ..WikiRuntime::default()
        })
        .manage(WikiExit::default())
        .invoke_handler(tauri::generate_handler![
            list_items,
            get_item,
            collections_tree,
            view_counts,
            quick_add,
            import_pdf,
            migration_preview,
            migration_status,
            migration_start,
            migration_cancel,
            migration_index,
            migration_select_report,
            migration_history,
            migration_save_report,
            attach_pdf,
            fetch_pdf,
            set_star,
            trash_item,
            restore_item,
            purge_item,
            item_types,
            subtypes,
            create_manual,
            update_item,
            lookup,
            attachment_data,
            list_annotations,
            add_annotation,
            update_annotation,
            delete_annotation,
            add_note,
            delete_note,
            list_notes,
            update_note,
            list_tags,
            create_tag,
            update_tag,
            delete_tag,
            attach_tag,
            detach_tag,
            add_to_collection,
            remove_from_collection,
            membership,
            open_attachment,
            open_external,
            cite_catalog,
            cite_installed,
            cite_style_xml,
            cite_locale_xml,
            cite_prefs,
            cite_set_prefs,
            cite_install_file,
            merge_tags,
            find_replace_tags,
            bibtex_entries,
            create_collection_in,
            rename_collection,
            delete_collection,
            find_replace_collections,
            get_ui_pref,
            set_ui_pref,
            data_version,
            check_update,
            wiki_status,
            wiki_list,
            wiki_read_gui,
            wiki_backlinks,
            wiki_search,
            wiki_tags,
            wiki_inbox,
            wiki_settings,
            wiki_set_enabled,
            wiki_save,
            wiki_move,
            wiki_create,
            wiki_mkdir,
            wiki_rename_preview,
            wiki_rename,
            wiki_trash,
            wiki_save_blob,
            wiki_import_file,
            wiki_exit_ready,
            wiki_reindex,
            wiki_open_path,
            wiki_pick_root,
            wiki_ref_dirs_get,
            wiki_ref_dir_add,
            wiki_ref_dir_remove,
            wiki_read_ref,
            wiki_open_url,
            wiki_chat_status,
            wiki_chat_set_enabled,
            wiki_chat_start,
            wiki_chat_send,
            wiki_chat_interrupt,
            wiki_chat_stop,
            wiki_chat_undo,
            wiki_chat_set_model,
            wiki_chat_auth
        ])
        .build(tauri::generate_context!())
        .expect("Tauri 起動に失敗しました")
        .run(|app, ev| match ev {
            // Wiki が無効なら何もしない（exit_hold が Allow）= 今の挙動のまま
            tauri::RunEvent::ExitRequested { code, api, .. } => {
                wiki_exit_event(app, ExitKind::Exit(code.unwrap_or(0)), || {
                    api.prevent_exit()
                })
            }
            // URL の別窓の × は保留しない（未保存の編集は main の窓にしか無い）
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label != WIKI_URL_LABEL => {
                wiki_exit_event(app, ExitKind::Close(label), || api.prevent_close())
            }
            // main の窓が閉じたら URL の別窓も閉じる（別窓だけが残ってアプリが生き続けない）
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } if label == "main" => {
                if let Some(w) = app.get_webview_window(WIKI_URL_LABEL) {
                    let _ = w.close();
                }
            }
            // チャットの子プロセスを残さない（⌘Q・窓の × は保留の後にここへ来る・Dock の終了・ログアウトは直接ここへ来る）
            tauri::RunEvent::Exit => {
                if let Some(c) = app.try_state::<wiki_chat::Chat>() {
                    c.shutdown();
                }
            }
            _ => {}
        });
}

/// GUI parity: GUI の動詞（Tauri command）は全部 core の登録表（MCP の `call`）からも呼べる。
/// 例外は下の GUI 専用だけ（理由付き）。Tauri runtime は使わず、このファイルの文字列を走査する
#[cfg(test)]
mod parity {
    /// GUI 専用の command と理由。理由が弱くなったら登録表へ移す（入れる方が安全）。
    /// 登録表と同名の command は登録表と同じ core 関数を通す（wiki_call）。同名で別の関数は禁止 = 別名にしてここに載せる。
    /// 検査（registry_named_wiki_commands_share_core_fn）するのは wiki_* だけ（非 wiki の command は引数の形が各自で照合できない）
    const GUI_ONLY: &[(&str, &str)] = &[
        ("migration_preview", "GUI のバックグラウンド移行ジョブ。CLI import も同じ core の事前確認を使う"),
        ("migration_status", "GUI の移行ジョブの進捗。永続結果は CLI import history/report で取得可能"),
        ("migration_start", "利用者が GUI で確認した入力だけを背景で適用。CLI import が同じ core を使う"),
        ("migration_cancel", "GUI が保持する移行ジョブの中止フラグ"),
        ("migration_index", "GUI の移行済み PDF の背景索引。CLI fulltext index でも可能"),
        ("migration_select_report", "GUI で過去の移行結果を選択。CLI import report と同じ永続結果"),
        ("migration_history", "GUI の移行元選択用。CLI import history で取得可能"),
        ("migration_save_report", "ネイティブ保存ダイアログで選んだ結果 JSON の保存先"),
        ("open_attachment", "デスクトップのアプリで開く = ホストの open（パスは get_item の attachments[].path）"),
        ("open_external", "ブラウザで開く = ホストの open"),
        ("attachment_data", "ビューア用の生バイト（IPC の ArrayBuffer）。エージェントは get_item の添付パスでファイルを直接読む"),
        ("get_ui_pref", "UI の永続状態（タブ・折畳）。読みは sql の settings で足りる"),
        ("set_ui_pref", "UI の永続状態。開いている app が持つ状態と食い違うだけで用途が無い"),
        ("cite_catalog", "約 1 万誌の索引 = 数 MB の JSON。id が分かれば cite_style_xml / cite_set_prefs で足り、探すのはホストの curl / grep"),
        ("data_version", "GUI の再読込の合図。エージェントは sql の PRAGMA data_version で読める"),
        ("check_update", "GUI の起動時の新版確認（配布サイトの dl/VERSION を 1 日 1 回）。エージェントには用途が無い（版は openref --version）"),
        ("wiki_settings", "起動時に OpenNote の UI を出すかだけを索引に触らず読む（GUI の起動の段取り）。agent は wiki_status か sql の settings"),
        ("wiki_set_enabled", "OpenNote の有効化は利用者が GUI で決める（agent が勝手に有効にして書き回らない）"),
        ("wiki_reindex", "GUI の監視の張り直し + 全走査。agent の入口は鮮度規則で自動更新される"),
        ("wiki_open_path", "デスクトップのアプリで開く = ホストの open（ワークスペースの中の実在ファイルと、本文の file:// が指すワークスペースの外の実在する通常ファイル。許可一覧の外の拡張子等は Finder で表示）。agent は wiki_read の abs_path を直接開ける"),
        ("wiki_pick_root", "ワークスペースのフォルダは利用者が GUI のフォルダ選択で決める（agent が任意のフォルダをワークスペースにしない）"),
        ("wiki_ref_dirs_get", "参照フォルダ（ワークスペースの外の読み取り専用のルート）の一覧。GUI の表示用。agent は sql の settings（wiki_ref_dirs）"),
        ("wiki_ref_dir_add", "参照フォルダは利用者が GUI のフォルダ選択で決める（agent が任意のフォルダを配信の範囲に足さない）"),
        ("wiki_ref_dir_remove", "参照フォルダを外す（GUI の [外す]）。agent には用途が無い"),
        ("wiki_read_ref", "参照フォルダの中のファイルを内蔵の表示に出すための読み（asset URL の組立てと html の本文）。agent はパスを直接読める"),
        ("wiki_open_url", "ページの http(s) リンクを設定どおり別窓 / 既定のブラウザで開く（GUI の操作）。agent には用途が無い"),
        ("wiki_create", "GUI の新規ページ（Untitled.md / .html の連番を作るだけ）。agent は wiki_write で名前と中身を決めて書く"),
        ("wiki_mkdir", "GUI の新規フォルダ（空のフォルダを作るだけ）。agent は wiki_write のパスで途中のフォルダを作れる"),
        ("wiki_rename_preview", "GUI の改名ダイアログの件数（書き換える [[旧名]] と相対リンク）。利用者が確かめてから書き換える前段"),
        ("wiki_rename", "リンクの一括書換え付きの改名は利用者が GUI で件数を見て決める（無確認の書換えはしない）"),
        ("wiki_trash", "macOS のゴミ箱へ（Finder の「戻す」）は利用者の操作。agent に消す口は出さない"),
        ("wiki_save_blob", "エディタへの画像の貼付（クリップボードの中身 = base64）。agent はファイルを直接ワークスペースに置ける"),
        ("wiki_import_file", "OS からのドロップ（Tauri の onDragDropEvent が渡す絶対パス）を attachments へコピー。agent はファイルを直接置ける"),
        ("wiki_read_gui", "画像・PDF は asset protocol で直接描くので base64 も抽出もしない read。登録表の wiki_read は read_agent（画像の base64・PDF の抽出テキスト）"),
        ("wiki_save", "エディタの保存: if_sha256 一致の自動保存は退避しない・競合ダイアログの force を受ける。登録表の wiki_write（write_agent）は上書きを必ず退避し force を持たない別の関数"),
        ("wiki_exit_ready", "⌘Q・窓の × の保留への GUI の応答（未保存の編集を保存してから終了）。agent には終了の段取りが無い"),
        ("wiki_chat_status", "右ペインのチャット（Claude Code CLI を子プロセスで起こす GUI の機能）が使えるか。agent はそれ自体がホスト"),
        ("wiki_chat_set_enabled", "チャットの有効化は利用者が GUI で同意して決める（既定 OFF）。agent が自分を起こす口にしない"),
        ("wiki_chat_start", "GUI が持つ Claude Code の子プロセスを起こす。agent から呼ぶと agent が agent を起こす（利用枠を使う）"),
        ("wiki_chat_send", "GUI のチャットの発言を子プロセスの stdin へ。agent には相手がいない"),
        ("wiki_chat_interrupt", "GUI のチャットの停止ボタン（子プロセスへの割込み）"),
        ("wiki_chat_stop", "GUI が持つ子プロセスを終える"),
        ("wiki_chat_undo", "チャットのターンの記録（GUI のプロセスの中だけ）を鍵に戻す。agent は wiki-history の退避を wiki_write で書き戻せる"),
        ("wiki_chat_set_model", "GUI のチャットのモデルの選択（設定 wiki_chat_model）。agent 自身のモデルはホストが決める"),
        ("wiki_chat_auth", "GUI のチャットのログイン / ログアウト（利用者の Claude Code CLI の auth を呼ぶ）。agent には用途が無い"),
    ];

    fn tauri_commands() -> Vec<String> {
        let src = include_str!("main.rs");
        let attr = regex::Regex::new(
            r"#\[tauri::command\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:async\s+)?fn\s+([a-z_0-9]+)",
        )
        .unwrap();
        let mut by_attr: Vec<String> = attr.captures_iter(src).map(|c| c[1].to_string()).collect();
        let handler = regex::Regex::new(r"generate_handler!\[([^\]]*)\]").unwrap();
        let mut registered: Vec<String> = handler.captures(src).expect("generate_handler! が無い")
            [1]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
        by_attr.sort();
        registered.sort();
        assert_eq!(
            by_attr, registered,
            "属性の付いた fn と generate_handler! の一覧が食い違う"
        );
        assert!(by_attr.len() > 40, "走査が壊れている: {by_attr:?}");
        by_attr
    }

    #[test]
    fn every_tauri_command_is_in_registry_or_gui_only() {
        let cmds = tauri_commands();
        let missing: Vec<&String> = cmds
            .iter()
            .filter(|n| {
                openref_core::commands::find(n).is_none() && !GUI_ONLY.iter().any(|(g, _)| g == n)
            })
            .collect();
        assert!(
            missing.is_empty(),
            "登録表（crates/openref-core/src/commands.rs）にも GUI 専用にも無い: {missing:?}"
        );
        for (g, why) in GUI_ONLY {
            assert!(!why.is_empty());
            assert!(
                cmds.iter().any(|c| c == g),
                "GUI 専用に載っているが Tauri command に無い: {g}"
            );
            assert!(
                openref_core::commands::find(g).is_none(),
                "{g} は登録表にもある（GUI 専用から外す）"
            );
        }
    }

    /// fn の本体（`\n}\n` まで）から `//` で始まる行を除いたもの（コメントに書いた呼び出しで検査を通さない）
    fn fn_body(src: &str, head: &str) -> String {
        let start = src
            .find(head)
            .unwrap_or_else(|| panic!("{head} が見つからない"));
        let body = &src[start..start + src[start..].find("\n}\n").expect("fn の終わり")];
        body.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 登録表と同名の wiki_* は wiki_call(&state, "<同名>", args, F) で登録表の schema で検査し、F は登録表の handler
    /// （commands.rs の `fn wiki_X(db…) { wiki::M::F(…) }`）と同じ core 関数（同名で別の関数を防ぐ）
    #[test]
    fn registry_named_wiki_commands_share_core_fn() {
        let src = include_str!("main.rs");
        let reg = include_str!("../../../crates/openref-core/src/commands.rs");
        let reg_fn = regex::Regex::new(r"\b(wiki::[a-z_]+::[a-z_0-9]+)\(").unwrap();
        let mut checked = 0;
        for n in tauri_commands() {
            if !n.starts_with("wiki_") || openref_core::commands::find(&n).is_none() {
                continue;
            }
            let body = fn_body(src, &format!("async fn {n}("));
            let via = regex::Regex::new(&format!(
                r#"wiki_call\(\s*&state,\s*"{n}",\s*args,\s*openref_core::(wiki::[a-z_]+::[a-z_0-9]+)\s*,?\s*\)"#
            ))
            .unwrap();
            let gui = via.captures(&body).unwrap_or_else(|| {
                panic!("{n} は登録表にもあるのに wiki_call(&state, \"{n}\", args, openref_core::wiki::…) を通らない（別の関数なら別名にして GUI_ONLY へ）")
            })[1]
                .to_string();
            let rbody = fn_body(reg, &format!("fn {n}(db: &mut Db"));
            let core = reg_fn
                .captures(&rbody)
                .unwrap_or_else(|| panic!("登録表の {n} の core 関数が見つからない"))[1]
                .to_string();
            assert_eq!(
                gui, core,
                "{n}: GUI と登録表で core 関数が違う（同名で別の関数は禁止 = 別名にして GUI_ONLY へ）"
            );
            checked += 1;
        }
        assert!(checked >= 7, "走査が壊れている: {checked}");
    }
}

#[cfg(test)]
mod quick_added_shape {
    /// quick_add の戻り値は ItemSummary のキー（camelCase）がそのまま上位にあり、warnings が 1 つ増えるだけ
    #[test]
    fn flattens_item_summary_and_adds_warnings() {
        let item = openref_core::search::ItemSummary {
            key: "ABCD1234".into(),
            title: "T".into(),
            first_author: "A".into(),
            first_author_given: None,
            year: Some(2026),
            container: "J".into(),
            starred: false,
            has_pdf: false,
            retracted: false,
            has_note: false,
            fulltext_snippet: None,
            date_added: "2026-10-30".into(),
            tags: vec![],
        };
        let plain = serde_json::to_value(&item).unwrap();
        let v = serde_json::to_value(super::QuickAdded {
            item,
            warnings: vec!["w".into()],
        })
        .unwrap();
        let mut expected = plain.as_object().unwrap().clone();
        expected.insert("warnings".into(), serde_json::json!(["w"]));
        assert_eq!(v, serde_json::Value::Object(expected));
        assert_eq!(v["firstAuthor"], "A");
    }
}

#[cfg(test)]
mod open_default_app {
    /// open が非 0 で終わる（ファイルが無い）と Err で stderr の 1 行目が返る（spawn だった頃は Ok で UI に届かなかった）。
    /// 存在しないパスなので GUI のアプリは起動しない
    #[test]
    #[cfg(target_os = "macos")]
    fn missing_file_is_err_with_stderr() {
        let e = super::open_file("/nonexistent/openref-open-test.pdf").unwrap_err();
        assert!(
            e.contains("openref-open-test.pdf"),
            "stderr の 1 行目を返す: {e}"
        );
        assert!(
            !e.contains("/nonexistent/"),
            "フルパスはファイル名に置き換える: {e}"
        );
        assert!(!e.contains('\n'));
    }

    /// 実 DB（本番の .backup コピー）で open_attachment と同じ経路（attachment_path → open）。
    /// OPENREF_DB=<コピー> OPENREF_OPEN_ID=<path を存在しない値に書き換えた添付の id> cargo test -p openref-app -- --ignored open_default_app
    #[test]
    #[ignore]
    fn real_db_missing_attachment_is_err() {
        let db =
            openref_core::db::Db::open(std::path::Path::new(&std::env::var("OPENREF_DB").unwrap()))
                .unwrap();
        let id: i64 = std::env::var("OPENREF_OPEN_ID").unwrap().parse().unwrap();
        let path = super::attachment_path(&db, id).unwrap();
        let e = super::open_file(&path).unwrap_err();
        assert!(e.ends_with("does not exist."), "{e}");
        assert!(!e.contains('/'), "フルパスを出さない: {e}");
    }
}
