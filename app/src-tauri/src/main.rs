#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use openref_core::{annotate, citestyle, collections, datadir, edit, ingest, pdfingest, resolver, search, tags, types, update, Db};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{Emitter, Manager};

struct AppState {
    db: Arc<Mutex<Db>>,
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

/// main thread から外して待つ（同期 command は main thread で走る）。JoinError も Err(String) に落とす
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

// ---------- commands ----------

#[tauri::command]
async fn list_items(state: tauri::State<'_, AppState>, filter: search::ListFilter) -> Result<Vec<search::ItemSummary>, String> {
    // 本文検索（fulltext_fts + 抜粋）は一般語で数百 ms かかり得るので main thread から外す（UI を止めない）。
    // 応答順は保証されないため、フロント（state.svelte.ts refreshList）が古い応答を捨てる
    let db = state.db.clone();
    blocking(move || {
        let db = lock(&db)?;
        Ok(search::list_items(&db, &filter).map_err(err)?.iter().map(search::ListedItem::summary).collect())
    })
    .await
}

/// 詳細（ゴミ箱の item も引ける。deleted / notes 込み = core の ItemDetail そのまま）
#[tauri::command]
fn get_item(state: tauri::State<AppState>, key: String) -> Result<Option<search::ItemDetail>, String> {
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
fn update_item(state: tauri::State<AppState>, key: String, csl: serde_json::Value) -> Result<(), String> {
    edit::update_csl(&mut *lock(&state.db)?, &key, &csl).map_err(err)
}

// ---------- 引用スタイル（CSL・Paperpile の Change citation style…） ----------
// 取得はネットワークを伴い得るので main thread から外す。DB lock は保持しない（styles/ ディレクトリだけ使う）

fn styles_dir_of(state: &tauri::State<'_, AppState>) -> Result<PathBuf, String> {
    Ok(citestyle::styles_dir(&*lock(&state.db)?))
}

/// 約 1 万誌の索引（30 日キャッシュ）
#[tauri::command]
async fn cite_catalog(state: tauri::State<'_, AppState>, refresh: bool) -> Result<Vec<serde_json::Value>, String> {
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
async fn cite_locale_xml(state: tauri::State<'_, AppState>, lang: String) -> Result<String, String> {
    let dir = styles_dir_of(&state)?;
    blocking(move || citestyle::locale_xml(&dir, &lang).map_err(err)).await
}

/// `{style, locale, doi_always}`（MCP の cite_prefs と同じ JSON。snake_case のまま = api.ts の CitePrefs 参照）
#[tauri::command]
fn cite_prefs(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    Ok(citestyle::prefs(&*lock(&state.db)?))
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
    citestyle::set_prefs_partial(&*lock(&state.db)?, style.as_deref(), locale.as_deref(), doi_always).map_err(err)
}

/// 手元の .csl を導入（リポジトリに無い誌用）。戻り値は導入済み一覧の 1 行
#[tauri::command]
async fn cite_install_file(state: tauri::State<'_, AppState>, path: String) -> Result<serde_json::Value, String> {
    let dir = styles_dir_of(&state)?;
    blocking(move || citestyle::install_file(&dir, Path::new(&path)).map_err(err)).await
}

/// 引用メニューの BibTeX / 引用キー / LaTeX / DOI 用（keys の順で返す・見つからない key は飛ばす）
#[tauri::command]
fn bibtex_entries(state: tauri::State<AppState>, keys: Vec<String>) -> Result<Vec<openref_core::bibtex::Entry>, String> {
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
    db.conn.query_row("SELECT path FROM attachments WHERE id = ?1", [id], |r| r.get(0)).map_err(err)
}

/// PDF 実体を生バイトで返す（pdf.js に渡す）。`tauri::ipc::Response` = IPC の応答が ArrayBuffer になり、
/// base64 → JSON → atob の 3 段のコピーが消える（最大 30.9MB の PDF で WebView 側のピーク約 140MB → 約 60MB）。
/// パスを引いたら DB のロックを外す: ファイルの読込（Drive の未取得ファイルは取得待ちになり得る）が他の command を止めない。
/// 読込は spawn_blocking（同期 command は main thread で走る）
#[tauri::command]
async fn attachment_data(state: tauri::State<'_, AppState>, id: i64) -> Result<tauri::ipc::Response, String> {
    let path = attachment_path(&*lock(&state.db)?, id)?;
    let bytes = blocking(move || std::fs::read(&path).map_err(|e| format!("{path}: {e}"))).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
fn list_annotations(
    state: tauri::State<AppState>,
    key: String,
) -> Result<(Vec<annotate::AnnotationRow>, Vec<annotate::NoteRow>), String> {
    let db = lock(&state.db)?;
    Ok((annotate::list_annotations(&db, &key).map_err(err)?, annotate::list_notes(&db, &key).map_err(err)?))
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
    let comment_patch = if clear_comment.unwrap_or(false) { Some(None) } else { comment.map(Some) };
    let color_patch = if clear_color.unwrap_or(false) { Some(None) } else { color.map(Some) };
    annotate::update_annotation(
        &*lock(&state.db)?,
        &key,
        &annotate::AnnotationPatch { comment: comment_patch, color: color_patch, kind },
    )
    .map_err(err)
}

#[tauri::command]
fn delete_annotation(state: tauri::State<AppState>, key: String) -> Result<(), String> {
    annotate::delete_annotation(&*lock(&state.db)?, &key).map_err(err)
}

#[tauri::command]
fn add_note(state: tauri::State<AppState>, key: String, content_md: String) -> Result<String, String> {
    annotate::add_note(&*lock(&state.db)?, &key, "user", &content_md).map_err(err)
}

#[tauri::command]
fn delete_note(state: tauri::State<AppState>, key: String) -> Result<(), String> {
    annotate::delete_note(&*lock(&state.db)?, &key).map_err(err)
}

#[tauri::command]
fn list_notes(state: tauri::State<AppState>, key: String) -> Result<Vec<annotate::NoteRow>, String> {
    annotate::list_notes(&*lock(&state.db)?, &key).map_err(err)
}

#[tauri::command]
fn update_note(state: tauri::State<AppState>, key: String, content_md: String) -> Result<(), String> {
    annotate::update_note(&*lock(&state.db)?, &key, &content_md).map_err(err)
}

// ---------- tags（LABELS UX） ----------

#[tauri::command]
fn list_tags(state: tauri::State<AppState>, by_count: bool) -> Result<Vec<tags::TagRow>, String> {
    let sort = if by_count { tags::TagSort::Count } else { tags::TagSort::Alphabetical };
    tags::list_tags(&*lock(&state.db)?, sort).map_err(err)
}

#[tauri::command]
fn create_tag(state: tauri::State<AppState>, name: String, color: Option<String>) -> Result<i64, String> {
    tags::create_tag(&*lock(&state.db)?, &name, color.as_deref()).map_err(err)
}

#[tauri::command]
fn update_tag(state: tauri::State<AppState>, id: i64, name: Option<String>, color: Option<String>) -> Result<(), String> {
    let db = lock(&state.db)?;
    if let Some(n) = name {
        tags::rename_tag(&db, id, &n).map_err(err)?;
    }
    if color.is_some() {
        tags::set_tag_color(&db, id, color.as_deref()).map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
fn delete_tag(state: tauri::State<AppState>, id: i64) -> Result<(), String> {
    tags::delete_tag(&*lock(&state.db)?, id).map_err(err)
}

/// Manage labels → Merge: sources を target へ統合（戻り値 = 統合後の付与件数）
#[tauri::command]
fn merge_tags(state: tauri::State<AppState>, sources: Vec<i64>, target: i64) -> Result<i64, String> {
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
fn membership(state: tauri::State<AppState>, keys: Vec<String>) -> Result<search::Membership, String> {
    search::membership(&*lock(&state.db)?, &keys).map_err(err)
}

/// フォルダ作成（'Parent/Child' パス可・既存なら既存 id を返す = 冪等）
#[tauri::command]
fn create_collection(state: tauri::State<AppState>, name: String) -> Result<i64, String> {
    edit::ensure_collection_path(&*lock(&state.db)?, &name).map(|(id, _)| id).map_err(err)
}

// ---------- フォルダ管理（MY LIBRARY） ----------

/// New folder / Create subfolder（parentId = null なら root）。同じ親の下に大小無視の同名・`/` を含む名前は拒否
#[tauri::command]
fn create_collection_in(state: tauri::State<AppState>, name: String, parent_id: Option<i64>) -> Result<i64, String> {
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
    let ok = key.len() <= 64 && key.starts_with("ui_") && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(key)
    } else {
        Err(format!("UI の設定 key は ui_ で始まる英数字だけです: {key}"))
    }
}

#[tauri::command]
fn get_ui_pref(state: tauri::State<AppState>, key: String) -> Result<Option<String>, String> {
    let key = ui_pref_key(&key)?;
    Ok(lock(&state.db)?.setting(key))
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
        lock(&db)?.conn.query_row("PRAGMA data_version", [], |r| r.get(0)).map_err(err)
    })
    .await
}

/// 新版の確認（起動時に App が 1 回呼ぶ）= core update::check: settings の update_check / update_checked_at / update_latest /
/// update_error と配布サイトの dl/VERSION。24 時間は読み直さない・取りに行く間は DB のロックを離す。動いている版（Info.plist の
/// CFBundleShortVersionString = tauri.conf.json の version）より新しい版番号を返す（無ければ None）。取りに行けなかったときは
/// settings の update_error（sql / MCP から読める）と stderr に出して前回の結果で続ける（起動直後に toast で邪魔しない）。
/// 開発時は OPENREF_UPDATE_URL で確認先を差し替える
#[tauri::command]
async fn check_update(state: tauri::State<'_, AppState>, app: tauri::AppHandle) -> Result<Option<String>, String> {
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

#[tauri::command]
async fn quick_add(state: tauri::State<'_, AppState>, input: String) -> Result<search::ItemSummary, String> {
    let db = state.db.clone();
    blocking(move || {
        let parsed = resolver::parse_input(&input).map_err(err)?;
        let resolved = resolver::resolve(&parsed).map_err(err)?;
        let mut db = lock(&db)?;
        let added = ingest::add_item(&mut db, &resolved).map_err(err)?;
        let d = search::item_detail(&db, &added.key).map_err(err)?.ok_or("登録直後の item が見つかりません")?;
        Ok(search::ListedItem::from(d).summary())
    })
    .await
}

/// PDF 取込 1 件（取込モーダルの行に対応）。結果の区分と文言は core（MCP の import_pdf と同じ要約）。理由・降格の警告も message に入る
#[tauri::command]
async fn import_pdf(state: tauri::State<'_, AppState>, path: String) -> Result<pdfingest::ImportSummary, String> {
    let db = state.db.clone();
    blocking(move || Ok(pdfingest::import_summary(&mut *lock(&db)?, Path::new(&path)))).await
}

#[tauri::command]
async fn fetch_pdf(state: tauri::State<'_, AppState>, key: String) -> Result<String, String> {
    let db = state.db.clone();
    blocking(move || {
        // ① DB 読取 → ② ロック外でネットワーク → ③ 再ロックして添付（一覧・検索を止めない）
        let plan = openref_core::pdffetch::fetch_pdf_prepare(&*lock(&db)?, &key).map_err(err)?;
        let dl = openref_core::pdffetch::fetch_pdf_download(&plan).map_err(err)?;
        let f = openref_core::pdffetch::fetch_pdf_attach(&*lock(&db)?, &plan, dl).map_err(err)?;
        Ok(format!("✓ PDF 取得（{}・{} KB）", f.source, f.bytes_len / 1024))
    })
    .await
}

#[tauri::command]
async fn attach_pdf(state: tauri::State<'_, AppState>, key: String, path: String) -> Result<(), String> {
    // 添付直後に本文抽出（大きい PDF で 1 秒前後）が走るので main thread から外す
    let db = state.db.clone();
    blocking(move || {
        edit::attach_auto(&*lock(&db)?, &key, Path::new(&path)).map(|_| ()).map_err(err)
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
fn add_to_collection(state: tauri::State<AppState>, key: String, collection_id: i64) -> Result<(), String> {
    edit::add_to_collection(&*lock(&state.db)?, &key, collection_id).map_err(err)
}

#[tauri::command]
fn remove_from_collection(state: tauri::State<AppState>, key: String, collection_id: i64) -> Result<(), String> {
    edit::remove_from_collection(&*lock(&state.db)?, &key, collection_id).map_err(err)
}

#[tauri::command]
fn open_attachment(state: tauri::State<AppState>, id: i64) -> Result<(), String> {
    let path = attachment_path(&*lock(&state.db)?, id)?;
    std::process::Command::new("/usr/bin/open").arg(&path).spawn().map_err(err)?;
    Ok(())
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("http(s) のみ許可".into());
    }
    std::process::Command::new("/usr/bin/open").arg(&url).spawn().map_err(err)?;
    Ok(())
}

/// Settings › 起動時に前回のタブを開く（CheckMenuItem の id）。チェック = ui_tabs を復元（既定）・外す = 次回は「すべて」1 枚
const RESTORE_TABS_ID: &str = "restore_tabs";
/// Settings › Language › 日本語 / English（CheckMenuItem 2 つ・排他）。DB の ui_lang（"ja" 既定 / "en"）に書き、
/// webview へ "ui-lang" を emit する（UI は再起動なしで表記を変える = app/src/lib/i18n.svelte.ts）
const LANG_JA_ID: &str = "lang_ja";
const LANG_EN_ID: &str = "lang_en";
/// Settings › 起動時に新版を確認（settings の update_check・"0" で止める = core update.rs。確認は App 起動時の check_update）
const UPDATE_CHECK_ID: &str = "update_check";
/// on_menu_event から項目のチェック状態を読み書きするために持つ（Menu::get の再帰に頼らない）
struct SettingsItems {
    restore: tauri::menu::CheckMenuItem<tauri::Wry>,
    update_check: tauri::menu::CheckMenuItem<tauri::Wry>,
    lang_ja: tauri::menu::CheckMenuItem<tauri::Wry>,
    lang_en: tauri::menu::CheckMenuItem<tauri::Wry>,
}

/// 言語で変わる項目名（メニューのタイトルと「日本語」「English」はどちらの言語でも同じ）
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

/// メインメニュー = tauri 2.11.5 の `Menu::default` から **Close Window(⌘W)を外したもの**。
/// 既定メニューは File と Window の両方に `PredefinedMenuItem::close_window` を置き、WKWebView がページへ渡す前に
/// NSApp のキー等価(⌘W)として窓を閉じてしまう(PDF タブ 1 枚で ⌘W → 窓が閉じる)。
/// ⌘W はアプリ(タブを閉じる)が奪うので、メニューには置かない。File は macOS では Close Window しか無いので省く。
/// Edit の項目は WebView の ⌘C / ⌘V / ⌘A の経路なので残す。Window / Help の id は tauri が NSApp の windows / help メニューとして扱うため既定と同じ。
fn app_menu(app: &tauri::AppHandle, restore_tabs: bool, update_check: bool, en: bool) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, CheckMenuItem, Menu, PredefinedMenuItem, Submenu, WINDOW_SUBMENU_ID};
    let pkg = app.package_info();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
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
            &PredefinedMenuItem::quit(app, None)?,
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
    let view = Submenu::with_items(app, "View", true, &[&PredefinedMenuItem::fullscreen(app, None)?])?;
    // 既定の Window は Minimize / Maximize / 区切り / Close Window。Close Window だけ外す
    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[&PredefinedMenuItem::minimize(app, None)?, &PredefinedMenuItem::maximize(app, None)?],
    )?;
    // Settings: 設定が増えたらここに並べる（ダイアログは作らない）。Help（空）と Services は置かない
    let restore = CheckMenuItem::with_id(app, RESTORE_TABS_ID, restore_tabs_text(en), true, restore_tabs, None::<&str>)?;
    let update_check = CheckMenuItem::with_id(app, UPDATE_CHECK_ID, update_check_text(en), true, update_check, None::<&str>)?;
    let lang_ja = CheckMenuItem::with_id(app, LANG_JA_ID, "日本語", true, !en, None::<&str>)?;
    let lang_en = CheckMenuItem::with_id(app, LANG_EN_ID, "English", true, en, None::<&str>)?;
    let language = Submenu::with_items(app, "Language", true, &[&lang_ja, &lang_en])?;
    app.manage(SettingsItems { restore: restore.clone(), update_check: update_check.clone(), lang_ja: lang_ja.clone(), lang_en: lang_en.clone() });
    let settings = Submenu::with_items(app, "Settings", true, &[&restore, &update_check, &language])?;
    Menu::with_items(app, &[&app_menu, &edit, &view, &settings, &window])
}

/// メニューの操作。CheckMenuItem はクリックで自分のチェックを反転するので、反転後の値を DB に書く。
/// 書けなければメニューを前の状態に戻す（次回の起動・開いている UI と食い違わせない）。
/// Language は DB に書けてから項目名を差し替えて webview へ emit する。選んでいた方を押し直したときは何も変えない
fn on_menu_event(app: &tauri::AppHandle, ev: tauri::menu::MenuEvent) {
    let items = app.state::<SettingsItems>();
    match ev.id().as_ref() {
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
            let menu = items.lang_ja.set_checked(!now_en).and(items.lang_en.set_checked(now_en)).and(items.restore.set_text(restore_tabs_text(now_en))).and(items.update_check.set_text(update_check_text(now_en)));
            if let Err(e) = menu {
                eprintln!("openref-app: メニューを更新できません: {e}");
            }
            if saved {
                if let Err(e) = app.emit("ui-lang", if en { "en" } else { "ja" }) {
                    eprintln!("openref-app: ui-lang を webview へ送れません: {e}");
                }
            }
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

/// 窓を出す前の致命的エラー（データ移行の中止など）。stderr と警告ダイアログ（osascript）に出して終了する
fn fatal(msg: &str) -> ! {
    eprintln!("openref-app: {msg}");
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", "on run argv", "-e", "display alert (item 1 of argv) message (item 2 of argv) as critical", "-e", "end run"])
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
    // ・Language（既定 = 日本語。"en" のときだけ English）
    let restore_tabs = db.setting("ui_restore_tabs").as_deref() != Some("0");
    let update_check = update::cached(&db).enabled;
    let lang_en = db.setting("ui_lang").as_deref() == Some("en");
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .menu(move |app| app_menu(app, restore_tabs, update_check, lang_en))
        .on_menu_event(on_menu_event)
        .manage(AppState {
            db: Arc::new(Mutex::new(db)),
        })
        .invoke_handler(tauri::generate_handler![
            list_items,
            get_item,
            collections_tree,
            view_counts,
            quick_add,
            import_pdf,
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
            create_collection,
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
            check_update
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 起動に失敗しました");
}

/// GUI parity: GUI の動詞（Tauri command）は全部 core の登録表（MCP の `call`）からも呼べる。
/// 例外は下の GUI 専用だけ（理由付き）。Tauri runtime は使わず、このファイルの文字列を走査する
#[cfg(test)]
mod parity {
    /// GUI 専用の command と理由。理由が弱くなったら登録表へ移す（入れる方が安全）
    const GUI_ONLY: &[(&str, &str)] = &[
        ("open_attachment", "デスクトップのアプリで開く = ホストの open（パスは get_item の attachments[].path）"),
        ("open_external", "ブラウザで開く = ホストの open"),
        ("attachment_data", "ビューア用の生バイト（IPC の ArrayBuffer）。エージェントは get_item の添付パスでファイルを直接読む"),
        ("get_ui_pref", "UI の永続状態（タブ・折畳）。読みは sql の settings で足りる"),
        ("set_ui_pref", "UI の永続状態。開いている app が持つ状態と食い違うだけで用途が無い"),
        ("cite_catalog", "約 1 万誌の索引 = 数 MB の JSON。id が分かれば cite_style_xml / cite_set_prefs で足り、探すのはホストの curl / grep"),
        ("data_version", "GUI の再読込の合図。エージェントは sql の PRAGMA data_version で読める"),
        ("check_update", "GUI の起動時の新版確認（配布サイトの dl/VERSION を 1 日 1 回）。エージェントには用途が無い（版は openref --version）"),
    ];

    fn tauri_commands() -> Vec<String> {
        let src = include_str!("main.rs");
        let attr = regex::Regex::new(r"#\[tauri::command\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:async\s+)?fn\s+([a-z_0-9]+)").unwrap();
        let mut by_attr: Vec<String> = attr.captures_iter(src).map(|c| c[1].to_string()).collect();
        let handler = regex::Regex::new(r"generate_handler!\[([^\]]*)\]").unwrap();
        let mut registered: Vec<String> = handler.captures(src).expect("generate_handler! が無い")[1]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        by_attr.sort();
        registered.sort();
        assert_eq!(by_attr, registered, "属性の付いた fn と generate_handler! の一覧が食い違う");
        assert!(by_attr.len() > 40, "走査が壊れている: {by_attr:?}");
        by_attr
    }

    #[test]
    fn every_tauri_command_is_in_registry_or_gui_only() {
        let cmds = tauri_commands();
        let missing: Vec<&String> = cmds
            .iter()
            .filter(|n| openref_core::commands::find(n).is_none() && !GUI_ONLY.iter().any(|(g, _)| g == n))
            .collect();
        assert!(missing.is_empty(), "登録表（crates/openref-core/src/commands.rs）にも GUI 専用にも無い: {missing:?}");
        for (g, why) in GUI_ONLY {
            assert!(!why.is_empty());
            assert!(cmds.iter().any(|c| c == g), "GUI 専用に載っているが Tauri command に無い: {g}");
            assert!(openref_core::commands::find(g).is_none(), "{g} は登録表にもある（GUI 専用から外す）");
        }
    }
}
