#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use openref_core::{
    annotate, citestyle, collections, datadir, edit, ingest, pdfingest, resolver, search, tags,
    types, update, Db,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use shelf_gui::*;
use tauri::{Emitter, Manager};
use wiki_asset::*;
use wiki_chat_gui::*;
use wiki_gui::*;
mod migration;
mod shelf_gui;
mod wiki_chat;
mod wiki_chat_gui;
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

// ---------- OpenNote の文献の引用（エディタの [文献]・DOI / PubMed の URL の貼り付け）----------
// 照合（ネットワークなし）と登録（resolver = PubMed / Crossref の機械確定）の 2 本。どちらも GUI 専用（parity の GUI_ONLY に理由）。
// 登録は quick_add と同じ core の経路（parse_input → resolve → add_item）だが、重複を文字のエラーではなく既存の key で返す
// （DOI しか持たない蔵書の論文に、同じ論文の PubMed の URL を貼った = resolver の書誌の DOI が既存と一致 = 増やさずその key へ）。
// 画面の側（一覧のタブを開く・検索を消す = state.quickAdd の reveal）は通さない = 呼ぶのは編集中のエディタだけ

/// 打った語・貼った URL を parse_input に渡せる形へ（parse_input は前置きと host の大小・末尾の句読点を区別するので、ここで揃える）:
/// - 前後の空白と `<>`・文の終わりの句読点（`.` `,` `;` `。` `、`・釣り合わない `)`）を外す
/// - `doi:` / `pmid:` の前置きは大小を問わず解く（`Pmid: 1` → `1` = PMID）
/// - URL は scheme と host を小文字に。doi.org の URL は `?` `#` から後ろを落として `%xx` を戻す
///   （`HTTPS://DOI.ORG/10.1000%2Fabc?x=1#y` → `https://doi.org/10.1000/abc`）。PubMed の URL は parse_input が頭の数字だけを取る
/// 先頭が非 ASCII でも panic しない（byte の位置で切らない = `str::get`）
fn cite_id_input(raw: &str) -> String {
    let mut t = raw.trim().trim_start_matches('<').trim_end_matches('>').trim();
    loop {
        let before = t;
        t = t.trim_end_matches(['.', ',', ';', '。', '、']).trim_end();
        if t.ends_with(')') && t.matches('(').count() < t.matches(')').count() {
            t = &t[..t.len() - 1];
        }
        if t == before {
            break;
        }
    }
    let prefix = |p: &str| t.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p));
    if prefix("doi:") {
        return t["doi:".len()..].trim().to_string();
    }
    if prefix("pmid:") {
        return t["pmid:".len()..].trim().to_string();
    }
    if let Some((scheme, rest)) = t.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if scheme == "http" || scheme == "https" {
            let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = host.to_ascii_lowercase();
            if host == "doi.org" || host.ends_with(".doi.org") {
                let doi = path.trim_start_matches('/').split(['?', '#']).next().unwrap_or("");
                let doi = percent_encoding::percent_decode_str(doi).decode_utf8_lossy();
                return format!("{scheme}://{host}/{doi}");
            }
            return format!("{scheme}://{host}{path}");
        }
    }
    t.to_string()
}

/// parse_input の結果 → find_by_ids の (pmid, doi)
fn cite_ids(p: &resolver::AddInput) -> (Option<&str>, Option<&str>) {
    match p {
        resolver::AddInput::Pmid(s) => (Some(s.as_str()), None),
        resolver::AddInput::Doi(s) => (None, Some(s.as_str())),
    }
}

/// key → 一覧の行（候補の表示 = 題名・著者・年・誌名・撤回の印）
fn cite_summary(db: &Db, key: &str) -> Result<search::ItemSummary, String> {
    let d = search::item_detail(db, key)
        .map_err(err)?
        .ok_or_else(|| format!("item {key} が見つかりません"))?;
    Ok(search::ListedItem::from(d).summary())
}

/// 照合の結果。None = 識別子（DOI・PMID・doi.org / PubMed の URL）ではない・Some(item: None) = 識別子だが蔵書に無い
#[derive(serde::Serialize, Debug)]
struct CiteLookup {
    item: Option<search::ItemSummary>,
}

fn cite_lookup_in(db: &Db, input: &str) -> Result<Option<CiteLookup>, String> {
    let Ok(parsed) = resolver::parse_input(&cite_id_input(input)) else {
        return Ok(None);
    };
    let (pmid, doi) = cite_ids(&parsed);
    let item = match search::find_by_ids(db, pmid, doi).map_err(err)? {
        Some((_, key)) => Some(cite_summary(db, &key)?),
        None => None,
    };
    Ok(Some(CiteLookup { item }))
}

/// 照合（ネットワークなし・DB を読むだけ）。ゴミ箱の item は見ない（find_by_ids = add_item の重複判定と同じ）
#[tauri::command]
async fn wiki_cite_lookup(
    state: tauri::State<'_, AppState>,
    input: String,
) -> Result<Option<CiteLookup>, String> {
    // ロックを待つ間（PDF の本文の抽出は約 1 秒握る）に main thread を止めない = list_items と同じく外す
    let db = state.db.clone();
    blocking(move || cite_lookup_in(&*lock(&db)?, &input)).await
}

/// 登録の結果 = 一覧の行 + created（false = 既にあった）+ resolver の警告
#[derive(serde::Serialize, Debug)]
struct CiteAdded {
    #[serde(flatten)]
    item: search::ItemSummary,
    created: bool,
    warnings: Vec<String>,
}

/// 登録の本体（resolve は差し替えられる = 試験はネットワークに出ない）。① 照合（ロック）→ ② resolver（ロックを離す）→
/// ③ 登録（ロック）。① で見つかれば resolver を呼ばない。② の間に同じ論文が入った・PubMed の URL で引いた書誌の DOI が
/// 既存と一致した = ③ の Duplicate → 既存の key（created = false）
fn cite_register_with(
    db: &Mutex<Db>,
    input: &str,
    resolve: impl Fn(&resolver::AddInput) -> openref_core::Result<resolver::Resolved>,
) -> Result<CiteAdded, String> {
    let parsed = resolver::parse_input(&cite_id_input(input)).map_err(err)?;
    let (pmid, doi) = cite_ids(&parsed);
    let found = search::find_by_ids(&*lock(db)?, pmid, doi).map_err(err)?;
    if let Some((_, key)) = found {
        let item = cite_summary(&*lock(db)?, &key)?;
        return Ok(CiteAdded { item, created: false, warnings: vec![] });
    }
    let resolved = resolve(&parsed).map_err(err)?;
    let mut g = lock(db)?;
    let (key, created) = match ingest::add_item(&mut g, &resolved) {
        Ok(a) => (a.key, true),
        Err(openref_core::CoreError::Duplicate { existing_key, .. }) => (existing_key, false),
        Err(e) => return Err(err(e)),
    };
    Ok(CiteAdded {
        item: cite_summary(&g, &key)?,
        created,
        warnings: resolved.warnings,
    })
}

/// 登録（利用者が [MY LIBRARY に登録して文献リンクに] を押したときだけ = ネットワークに出るのはここの resolver だけ）
#[tauri::command]
async fn wiki_cite_register(
    state: tauri::State<'_, AppState>,
    input: String,
) -> Result<CiteAdded, String> {
    let db = state.db.clone();
    blocking(move || cite_register_with(&db, &input, resolver::resolve)).await
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
/// CFBundleShortVersionString = Cargo.toml の [workspace.package] version。tauri.conf.json に version を置かないので Tauri が
/// CARGO_PKG_VERSION を使う）より新しい版番号を返す（無ければ None）。取りに行けなかったときは
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
    blocking(move || attach_pdf_now(&db, &key, &path)).await
}

/// attach_pdf の本体: ファイルの読込・md5・本文の抽出は DB のロックの外（core edit::attach_auto_unlocked）、
/// 添付の書込・行・本文の行だけロックの中。結果（添付の行・本文の行・page_count）とエラーの文は edit::attach_auto と同じ
fn attach_pdf_now(db: &Mutex<Db>, key: &str, path: &str) -> Result<(), String> {
    edit::attach_auto_unlocked(|| lock(db), key, Path::new(path))?
        .map(|_| ())
        .map_err(err)
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

/// `/usr/bin/open <args…>` を走らせて終了を待つ（子を回収する）。ファイルが無い・Drive の本体が取れない等で
/// open が非 0 で終わったら stderr の空でない最初の行を Err にする（spawn だと起動した時点で Ok になり、失敗が UI に届かなかった）。
/// open は LaunchServices に渡すだけですぐ戻るが、同期 command は main thread で走るので呼び手は `blocking` の中で呼ぶ。
/// `open` を起こすのはここだけ（既定のアプリで開く・Finder で表示・Terminal でログイン）。パスを渡したなら失敗の文は drop_dir_part を通す
pub(crate) fn open_cmd<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Result<(), String> {
    let out = std::process::Command::new("/usr/bin/open")
        .args(args)
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

/// 既定のアプリに渡す（http(s) の URL）。失敗の文はそのまま（パスなら open_file = ディレクトリ部分を落とす）
fn open_with_default_app(target: &str) -> Result<(), String> {
    open_cmd(&[target])
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
    open_with_default_app(path).map_err(drop_dir_part)
}

/// Finder で場所を表示する（`open -R`）。失敗の文は open_file と同じくディレクトリ部分を落とす（Drive のパスはアカウント名を含む）
pub(crate) fn reveal_in_finder(path: &Path) -> Result<(), String> {
    open_cmd(&reveal_args(path)).map_err(drop_dir_part)
}

/// Finder で表示の open の引数（`-R` = 開かずに場所を見せる。落とすと既定のアプリで開いてしまう）
fn reveal_args(path: &Path) -> [&std::ffi::OsStr; 2] {
    [std::ffi::OsStr::new("-R"), path.as_os_str()]
}

/// open の失敗の文のディレクトリ部分（最初の `/` から最後の `/` まで）を落とす
fn drop_dir_part(e: String) -> String {
    match (e.find('/'), e.rfind('/')) {
        (Some(i), Some(j)) => format!("{}{}", &e[..i], &e[j + 1..]),
        _ => e,
    }
}

/// 失敗 / 完了の案内の文に出る root をフォルダ名に置き換える（本棚・OpenNote・チャットの画面の文。
/// Drive のパスはアカウント名を含む）。綴りは長い順に置き換える（渡した綴りと canonical のどちらも文に出うる）。画面の tree.ts
/// hideRootParents と同じ規則（名前 = 末尾の / を除いた最後の区画・名前が空 = `/` そのもの = 隠す親が無いので変えない）。
/// drop_dir_part（最初と最後の / の間を落とす）は使わない: 理由の文に「iCloud Drive / CloudStorage」の / がある = 理由が欠ける。
/// 畳むのは画面へ出す殻だけ（core の文 = MCP / CLI の agent と stderr のログはフルパスのまま）
pub(crate) fn hide_roots(msg: String, roots: &[String]) -> String {
    let mut roots: Vec<&String> = roots.iter().filter(|r| !r.is_empty()).collect();
    roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
    roots.into_iter().fold(msg, |m, root| {
        let trimmed = root.trim_end_matches('/');
        let name = &trimmed[trimmed.rfind('/').map_or(0, |i| i + 1)..];
        if name.is_empty() || !m.contains(root.as_str()) {
            return m;
        }
        // パスの途中から始まる一致は置き換えない（canonical を取れない綴り `/var/x` が文の `/private/var/x` の中に一致して
        // `/privatex` に崩れる類）= 一致の前の字がパスの字（英数字・日本語・`/._-~)`）なら飛ばす
        let mut out = String::with_capacity(m.len());
        let mut last = 0;
        for (i, _) in m.match_indices(root.as_str()) {
            if i < last {
                continue;
            }
            let mid_path = m[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || "/._-~)".contains(c));
            if mid_path {
                continue;
            }
            out.push_str(&m[last..i]);
            out.push_str(name);
            last = i + root.len();
        }
        out.push_str(&m[last..]);
        out
    })
}

/// パスの綴り（渡された物と、読めれば canonical）= hide_roots に渡す物
fn root_spellings(root: &Path) -> Vec<String> {
    let mut v = vec![root.to_string_lossy().into_owned()];
    if let Ok(c) = std::fs::canonicalize(root) {
        let c = c.to_string_lossy().into_owned();
        if c != v[0] {
            v.push(c);
        }
    }
    v
}

/// 文に出るパス（それぞれ渡した綴りと canonical）を名前に畳む（hide_roots）
pub(crate) fn hide_paths(msg: String, paths: &[&Path]) -> String {
    let v: Vec<String> = paths.iter().flat_map(|p| root_spellings(p)).collect();
    hide_roots(msg, &v)
}

/// Settings › PDF の保存先 › 保存先のフォルダを Finder で開く。戻り値 = toast の文（普通に開けたら無し）。
/// open_file が失敗の文からパスを落とす（Drive のパスはアカウント名を含む）。呼び手は spawn_blocking の中で呼ぶ
fn reveal_storage_folder(db: &Mutex<Db>, en: bool) -> Option<String> {
    reveal_storage_with(db, en, open_file)
}

/// 今の保存先（core mount::storage_folder）を `open` に渡す。`open` を分けるのは試験で Finder を開かないため。
/// ロックは設定を読む間だけ・Drive を見るのは drive モードのときだけ
fn reveal_storage_with(
    db: &Mutex<Db>,
    en: bool,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Option<String> {
    let opened = (|| {
        let (mode, app_dir) = {
            let db = lock(db)?;
            let dir = db.app_dir().ok_or("in-memory DB には保存先がありません")?;
            (db.pdf_write_mode().map_err(err)?, dir)
        };
        let mount = (mode == "drive")
            .then(openref_core::mount::detect)
            .flatten();
        let (folder, degraded) =
            openref_core::mount::storage_folder(&mode, &app_dir, mount.as_deref());
        open(&folder.to_string_lossy()).map(|()| degraded)
    })();
    match opened {
        Ok(false) => None,
        Ok(true) if en => {
            Some("Google Drive was not found, so new PDFs are being saved on this Mac".into())
        }
        Ok(true) => {
            Some("Google Drive が見つからないため、新しい PDF は Mac 内に保存されています".into())
        }
        Err(e) if en => Some(format!("✗ Could not open the storage folder: {e}")),
        Err(e) => Some(format!("✗ 保存先のフォルダを開けません: {e}")),
    }
}

#[tauri::command]
async fn open_external(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("http(s) のみ許可".into());
    }
    blocking(move || open_with_default_app(&url)).await
}

/// メニューの題名「Settings」「Language」（サブメニューの id = 文言を MENU_TEXT から取る = 日本語の表示では「設定」「言語」）。
/// Edit / View / Window の題名と中の定義済みの項目も MENU_TEXT から取る（英語の表示 = muda の既定の字）
const SETTINGS_MENU_ID: &str = "settings_menu";
const LANGUAGE_MENU_ID: &str = "language_menu";
/// Settings › 起動時に前回のタブを開く（CheckMenuItem の id）。チェック = ui_tabs を復元（既定）・外す = 次回は「すべて」1 枚
const RESTORE_TABS_ID: &str = "restore_tabs";
/// Settings › Language › 日本語 / English（CheckMenuItem 2 つ・排他）。DB の ui_lang（"ja" 既定 / "en"）に書き、
/// webview へ "ui-lang" を emit する（UI は再起動なしで表記を変える = app/src/lib/i18n.svelte.ts）
const LANG_JA_ID: &str = "lang_ja";
const LANG_EN_ID: &str = "lang_en";
/// Settings › 起動時に新版を確認（settings の update_check・"0" で止める = core update.rs。確認は App 起動時の check_update）
const UPDATE_CHECK_ID: &str = "update_check";
/// Settings › PDF の保存先（サブメニュー）› ローカル (Mac 内) / Google Drive / ローカル保存済みの PDF を Drive へ移行 / 保存先のフォルダを Finder で開く
const PDF_STORAGE_ID: &str = "pdf_storage";
const PDF_STORAGE_MANAGED_ID: &str = "pdf_storage_managed";
const PDF_STORAGE_DRIVE_ID: &str = "pdf_storage_drive";
const PDF_MIGRATE_DRIVE_ID: &str = "pdf_migrate_drive";
const PDF_REVEAL_STORAGE_ID: &str = "pdf_reveal_storage";
/// Settings › 他の文献管理ソフトから移行…（MenuItem）。webview へ "migration-open" を emit し、App が MigrationModal を開く
/// （参照エディタ・モーダル・PDF 取込の間は開かない = state.svelte.ts の openMigration）
const MIGRATION_OPEN_ID: &str = "migration_open";
/// MIGRATION_OPEN_ID を押すと出す event（App.svelte が listen する名前 = app.test.mjs が同じ名前を emit して確かめる）
const MIGRATION_OPEN_EVENT: &str = "migration-open";
/// アプリのメニューの終了（⌘Q）。PredefinedMenuItem::quit は muda で `terminate:` になり、tao に applicationShouldTerminate が無いので
/// RunEvent::ExitRequested を通らず Exit へ直行する（保留できない）→ 自作の項目で app.exit(0) を呼んで ExitRequested を通す
/// （Wiki が無効なら保留しない = 即終了で今と同じ）。Dock の「終了」・ログアウト・再起動は今も terminate: のままで保留できない
const QUIT_ID: &str = "quit";

/// on_menu_event から項目のチェック状態やラベルを読み書きするために持つ（Menu::get の再帰に頼らない）
struct SettingsItems {
    restore: tauri::menu::CheckMenuItem<tauri::Wry>,
    update_check: tauri::menu::CheckMenuItem<tauri::Wry>,
    lang_ja: tauri::menu::CheckMenuItem<tauri::Wry>,
    lang_en: tauri::menu::CheckMenuItem<tauri::Wry>,
    pdf_managed: tauri::menu::CheckMenuItem<tauri::Wry>,
    pdf_drive: tauri::menu::CheckMenuItem<tauri::Wry>,
    wiki_enabled: tauri::menu::CheckMenuItem<tauri::Wry>,
    wiki_chat: tauri::menu::CheckMenuItem<tauri::Wry>,
    wiki_chat_log: tauri::menu::CheckMenuItem<tauri::Wry>,
    /// 文言を MENU_TEXT から取る項目の全部（Language で差し替える）
    labeled: Vec<(&'static str, tauri::menu::MenuItemKind<tauri::Wry>)>,
}

/// 項目の文言（id, ja, en）。組立て（app_menu）も Language の切替（on_menu_event）もここから取る =
/// 項目を足すのは id の定数・この表・app_menu で並べる所。メニューの題名のうち Settings / Language / Edit / View / Window は表から取る（日本語の表示で
/// 「設定」「言語」「編集」「表示」「ウインドウ」）。OpenNote（固有の名前）は表に入れない。
/// 「Google Drive」「日本語」「English」は ja = en
const MENU_TEXT: &[(&str, &str, &str)] = &[
    (QUIT_ID, "OpenRef を終了", "Quit OpenRef"),
    (SETTINGS_MENU_ID, "設定", "Settings"),
    (LANGUAGE_MENU_ID, "言語", "Language"),
    (RESTORE_TABS_ID, "起動時に前回のタブを開く", "Restore tabs on launch"),
    (UPDATE_CHECK_ID, "起動時に新版を確認", "Check for updates on launch"),
    (PDF_STORAGE_ID, "PDF の保存先", "PDF storage"),
    (PDF_STORAGE_MANAGED_ID, "ローカル (Mac 内)", "Local (on this Mac)"),
    (PDF_STORAGE_DRIVE_ID, "Google Drive", "Google Drive"),
    (PDF_MIGRATE_DRIVE_ID, "ローカル保存済みの PDF を Drive へ移行", "Move local PDFs to Google Drive"),
    (PDF_REVEAL_STORAGE_ID, "保存先のフォルダを Finder で開く", "Show Storage Folder in Finder"),
    (WIKI_ENABLED_ID, "有効", "Enabled"),
    (WIKI_CHAT_ID, "チャット", "Chat"),
    (WIKI_CHAT_LOG_ID, "チャットのログを chat-log/ に残す", "Keep chat logs in chat-log/"),
    (LANG_JA_ID, "日本語", "日本語"),
    (LANG_EN_ID, "English", "English"),
    (MIGRATION_OPEN_ID, "他の文献管理ソフトから移行…", "Migrate from another reference manager…"),
    // macOS の定義済みの項目とメニューの題名。ja = macOS の標準の日本語の表記（Finder ほか）・en = muda 0.19.3 が text 無しで出す字
    // （predefined.rs の text() から & を除いた物・About / Hide はアプリ名 = 配布の .app の名 OpenRef）。英語で起動したときは text を渡さない
    // = muda の既定の字のまま（build_node）。押したときの働き（selector）とショートカットは種類で決まり、文言では変わらない
    (EDIT_MENU_ID, "編集", "Edit"),
    (VIEW_MENU_ID, "表示", "View"),
    (tauri::menu::WINDOW_SUBMENU_ID, "ウインドウ", "Window"),
    (Std::About.id(), "OpenRef について", "About OpenRef"),
    (Std::Hide.id(), "OpenRef を非表示", "Hide OpenRef"),
    (Std::HideOthers.id(), "ほかを非表示", "Hide Others"),
    (Std::Undo.id(), "取り消す", "Undo"),
    (Std::Redo.id(), "やり直す", "Redo"),
    (Std::Cut.id(), "カット", "Cut"),
    (Std::Copy.id(), "コピー", "Copy"),
    (Std::Paste.id(), "ペースト", "Paste"),
    (Std::SelectAll.id(), "すべてを選択", "Select All"),
    (Std::Fullscreen.id(), "フルスクリーンにする", "Toggle Full Screen"),
    (Std::Minimize.id(), "しまう", "Minimize"),
    (Std::Maximize.id(), "拡大/縮小", "Zoom"),
];

/// Edit / View の題名の id（文言を MENU_TEXT から取る = 言語の切替で差し替える。押しても event は出ない）
const EDIT_MENU_ID: &str = "edit_menu";
const VIEW_MENU_ID: &str = "view_menu";

/// 項目の id → その言語の文言（表に無い id は id のまま = 試験 menu::* が表の全部を確かめる）
fn menu_text(id: &'static str, en: bool) -> &'static str {
    MENU_TEXT
        .iter()
        .find(|t| t.0 == id)
        .map_or(id, |&(_, ja, en_s)| if en { en_s } else { ja })
}

/// 項目の文言を差し替える（tauri の MenuItemKind には set_text が無い）
fn set_label(item: &tauri::menu::MenuItemKind<tauri::Wry>, text: &str) -> tauri::Result<()> {
    use tauri::menu::MenuItemKind as K;
    match item {
        K::MenuItem(i) => i.set_text(text),
        K::Submenu(i) => i.set_text(text),
        K::Predefined(i) => i.set_text(text),
        K::Check(i) => i.set_text(text),
        K::Icon(i) => i.set_text(text),
    }
}

/// app_menu の初期状態（起動時の設定の値。チェック項目と表示の言語）
#[derive(Debug, Clone, Copy, Default)]
struct MenuInit {
    restore_tabs: bool,
    update_check: bool,
    en: bool,
    pdf_drive_mode: bool,
    wiki_enabled: bool,
    wiki_chat: bool,
    wiki_chat_log: bool,
}

/// OS 標準の項目（PredefinedMenuItem = 働きとショートカットは muda が種類で決める）。題は MENU_TEXT の表（日本語の表示 = 自分で付ける・英語の表示 = muda の既定の字）
#[derive(Debug, Clone, Copy, PartialEq)]
enum Std {
    About,
    Hide,
    HideOthers,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Fullscreen,
    Minimize,
    Maximize,
}

impl Std {
    /// 文言の表（MENU_TEXT）の鍵（muda の項目の id は muda が振る = これは文言を引く鍵だけ）
    const fn id(self) -> &'static str {
        match self {
            Std::About => "std_about",
            Std::Hide => "std_hide",
            Std::HideOthers => "std_hide_others",
            Std::Undo => "std_undo",
            Std::Redo => "std_redo",
            Std::Cut => "std_cut",
            Std::Copy => "std_copy",
            Std::Paste => "std_paste",
            Std::SelectAll => "std_select_all",
            Std::Fullscreen => "std_fullscreen",
            Std::Minimize => "std_minimize",
            Std::Maximize => "std_maximize",
        }
    }
}

/// サブメニューの題名
#[derive(Debug, Clone, PartialEq)]
enum Title {
    /// アプリ名（package_info の name）
    App,
    /// どちらの言語でも同じ
    Fixed(&'static str),
    /// MENU_TEXT の文言（id はサブメニューの id）
    Text,
}

/// メニューの形（runtime の要らない値）。app_menu は menu_spec を読んで組むだけ = 並び・有効 / 無効・ショートカット・
/// Language で差し替える項目の試験は menu_spec の値に表明する（試験 menu::*）
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Std(Std),
    Sep,
    /// 押す項目（MenuItem）。文言は MENU_TEXT
    Item { id: &'static str, enabled: bool, accel: Option<&'static str> },
    /// チェック項目（CheckMenuItem）。文言は MENU_TEXT
    Check { id: &'static str, enabled: bool, checked: bool },
    /// サブメニュー（id が None なら tauri が振る）
    Sub { id: Option<&'static str>, title: Title, items: Vec<Node> },
}

impl Node {
    /// 文言を MENU_TEXT から取る項目の id（Language の切替で差し替える）
    fn label_id(&self) -> Option<&'static str> {
        match self {
            Node::Item { id, .. } | Node::Check { id, .. } => Some(*id),
            Node::Std(s) => Some(s.id()),
            Node::Sub { id, title: Title::Text, .. } => *id,
            _ => None,
        }
    }
}

/// メインメニュー = tauri 2.11.5 の `Menu::default` から **Close Window(⌘W)を外したもの**。
/// 既定メニューは File と Window の両方に `PredefinedMenuItem::close_window` を置き、WKWebView がページへ渡す前に
/// NSApp のキー等価(⌘W)として窓を閉じてしまう(PDF タブ 1 枚で ⌘W → 窓が閉じる)。
/// ⌘W はアプリ(タブを閉じる)が奪うので、メニューには置かない。File は macOS では Close Window しか無いので省く。
/// Edit の項目は WebView の ⌘C / ⌘V / ⌘A の経路なので残す。Window / Help の id は tauri が NSApp の windows / help メニューとして扱うため既定と同じ。
fn menu_spec(init: &MenuInit) -> Vec<Node> {
    use Node::{Check, Item, Sep, Sub};
    let item = |id| Item { id, enabled: true, accel: None };
    let check = |id, checked| Check { id, enabled: true, checked };
    let wiki_on = init.wiki_enabled;
    vec![
        Sub {
            id: None,
            title: Title::App,
            items: vec![
                Node::Std(Std::About),
                Sep,
                Node::Std(Std::Hide),
                Node::Std(Std::HideOthers),
                Sep,
                Item { id: QUIT_ID, enabled: true, accel: Some("CmdOrCtrl+Q") },
            ],
        },
        Sub {
            id: Some(EDIT_MENU_ID),
            title: Title::Text,
            items: vec![
                Node::Std(Std::Undo),
                Node::Std(Std::Redo),
                Sep,
                Node::Std(Std::Cut),
                Node::Std(Std::Copy),
                Node::Std(Std::Paste),
                Node::Std(Std::SelectAll),
            ],
        },
        Sub { id: Some(VIEW_MENU_ID), title: Title::Text, items: vec![Node::Std(Std::Fullscreen)] },
        // Settings: 設定が増えたらここに並べる（ダイアログは作らない）。Help（空）と Services は置かない
        Sub {
            id: Some(SETTINGS_MENU_ID),
            title: Title::Text,
            items: vec![
                check(RESTORE_TABS_ID, init.restore_tabs),
                check(UPDATE_CHECK_ID, init.update_check),
                Sub {
                    id: Some(PDF_STORAGE_ID),
                    title: Title::Text,
                    items: vec![
                        check(PDF_STORAGE_MANAGED_ID, !init.pdf_drive_mode),
                        check(PDF_STORAGE_DRIVE_ID, init.pdf_drive_mode),
                        Sep,
                        item(PDF_MIGRATE_DRIVE_ID),
                        item(PDF_REVEAL_STORAGE_ID),
                    ],
                },
                // OpenNote。サブメニューの題名はどちらの言語でも同じ。チャットとチャットのログは Wiki が有効なときだけ押せる。
                // チャットのチェックは実際の設定（ON は画面の同意を経てから = wiki_chat_menu）・ログは既定 ON（設定の値）。
                // 索引を作り直すのは画面のワークスペース名のメニュー（メニューバーに置くと、どのワークスペースの索引か分からない）
                Sub {
                    id: None,
                    title: Title::Fixed("OpenNote"),
                    items: vec![
                        check(WIKI_ENABLED_ID, wiki_on),
                        Sep,
                        Check { id: WIKI_CHAT_ID, enabled: wiki_on, checked: wiki_on && init.wiki_chat },
                        Check { id: WIKI_CHAT_LOG_ID, enabled: wiki_on, checked: init.wiki_chat_log },
                    ],
                },
                Sub {
                    id: Some(LANGUAGE_MENU_ID),
                    title: Title::Text,
                    items: vec![check(LANG_JA_ID, !init.en), check(LANG_EN_ID, init.en)],
                },
                Sep,
                item(MIGRATION_OPEN_ID),
            ],
        },
        // 既定の Window は Minimize / Maximize / 区切り / Close Window。Close Window だけ外す
        Sub {
            id: Some(tauri::menu::WINDOW_SUBMENU_ID),
            title: Title::Text,
            items: vec![Node::Std(Std::Minimize), Node::Std(Std::Maximize)],
        },
    ]
}

/// menu_spec の 1 項目を組む。文言を持つ項目は labeled に入る（Language の切替の対象 = label_id と同じ判定）
fn build_node(
    app: &tauri::AppHandle,
    node: &Node,
    en: bool,
    about: &tauri::menu::AboutMetadata,
    labeled: &mut Vec<(&'static str, tauri::menu::MenuItemKind<tauri::Wry>)>,
) -> tauri::Result<tauri::menu::MenuItemKind<tauri::Wry>> {
    use tauri::menu::{CheckMenuItem, IsMenuItem, MenuItem, PredefinedMenuItem as P, Submenu};
    let kind = match node {
        Node::Std(s) => {
            // 英語 = text を渡さない（muda の既定の字 = 前と 1 字も違わない）・日本語 = 表の字（言語の切替は relabel の set_text）
            let t = (!en).then(|| menu_text(s.id(), false));
            match s {
                Std::About => P::about(app, t, Some(about.clone())),
                Std::Hide => P::hide(app, t),
                Std::HideOthers => P::hide_others(app, t),
                Std::Undo => P::undo(app, t),
                Std::Redo => P::redo(app, t),
                Std::Cut => P::cut(app, t),
                Std::Copy => P::copy(app, t),
                Std::Paste => P::paste(app, t),
                Std::SelectAll => P::select_all(app, t),
                Std::Fullscreen => P::fullscreen(app, t),
                Std::Minimize => P::minimize(app, t),
                Std::Maximize => P::maximize(app, t),
            }?
        }
        .kind(),
        Node::Sep => P::separator(app)?.kind(),
        Node::Item { id, enabled, accel } => {
            MenuItem::with_id(app, *id, menu_text(*id, en), *enabled, *accel)?.kind()
        }
        Node::Check { id, enabled, checked } => {
            CheckMenuItem::with_id(app, *id, menu_text(*id, en), *enabled, *checked, None::<&str>)?.kind()
        }
        Node::Sub { id, title, items } => {
            let kids = items
                .iter()
                .map(|n| build_node(app, n, en, about, labeled))
                .collect::<tauri::Result<Vec<_>>>()?;
            let kids: Vec<&dyn IsMenuItem<tauri::Wry>> = kids.iter().map(|k| k as _).collect();
            let text = match title {
                Title::App => app.package_info().name.clone(),
                Title::Fixed(t) => t.to_string(),
                Title::Text => id.map_or("", |i| menu_text(i, en)).to_string(),
            };
            match id {
                Some(id) => Submenu::with_id_and_items(app, *id, text, true, &kids)?,
                None => Submenu::with_items(app, text, true, &kids)?,
            }
            .kind()
        }
    };
    if let Some(id) = node.label_id() {
        labeled.push((id, kind.clone()));
    }
    Ok(kind)
}

fn app_menu(app: &tauri::AppHandle, init: MenuInit) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, IsMenuItem, Menu};
    let pkg = app.package_info();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    let mut labeled = Vec::new();
    let top = menu_spec(&init)
        .iter()
        .map(|n| build_node(app, n, init.en, &about, &mut labeled))
        .collect::<tauri::Result<Vec<_>>>()?;
    // handler が読み書きするチェック項目（menu_spec に在ることは試験 menu::* が確かめる）
    let check = |id: &str| {
        labeled
            .iter()
            .find(|(i, _)| *i == id)
            .and_then(|(_, k)| k.as_check_menuitem().cloned())
            .ok_or_else(|| tauri::Error::from(std::io::Error::other(format!("メニューに {id} のチェック項目が無い"))))
    };
    let items = SettingsItems {
        restore: check(RESTORE_TABS_ID)?,
        update_check: check(UPDATE_CHECK_ID)?,
        lang_ja: check(LANG_JA_ID)?,
        lang_en: check(LANG_EN_ID)?,
        pdf_managed: check(PDF_STORAGE_MANAGED_ID)?,
        pdf_drive: check(PDF_STORAGE_DRIVE_ID)?,
        wiki_enabled: check(WIKI_ENABLED_ID)?,
        wiki_chat: check(WIKI_CHAT_ID)?,
        wiki_chat_log: check(WIKI_CHAT_LOG_ID)?,
        labeled,
    };
    app.manage(items);
    let top: Vec<&dyn IsMenuItem<tauri::Wry>> = top.iter().map(|k| k as _).collect();
    Menu::with_items(app, &top)
}

/// Language の切替: 文言を持つ項目すべてを en の言語の文言にする（set = 項目の set_text。試験では偽物）。
/// 途中で失敗しても全部を差し替え、最初の失敗を返す
fn relabel<T>(
    items: &[(&'static str, T)],
    en: bool,
    mut set: impl FnMut(&T, &'static str) -> tauri::Result<()>,
) -> tauri::Result<()> {
    items.iter().fold(Ok(()), |r, (id, item)| {
        let done = set(item, menu_text(id, en));
        r.and(done)
    })
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
            toggle_setting(&items.restore, |v| save_setting(app, "ui_restore_tabs", v));
        }
        UPDATE_CHECK_ID => {
            // 確認は起動時だけなので、入れ直しが効くのは次の起動から。外したら前回の結果（update_latest）も出さない = core update::check。
            // 出ている帯はその場で消す（webview へ "update-check-off"。App が updateLatest を null に）
            let on = toggle_setting(&items.update_check, |v| save_setting(app, update::KEY_ENABLED, v));
            if on == Some(false) {
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
            // .and は引数を先に評価する = 途中で失敗しても全部を差し替え、最初の失敗を出す（前の 15 行の連鎖と同じ）
            let checks = items.lang_ja.set_checked(!now_en).and(items.lang_en.set_checked(now_en));
            let menu = checks.and(relabel(&items.labeled, now_en, set_label));
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
            let en = ui_en(app);
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
                                "✓ PDF の保存先を Google Drive に切り替えました（ローカル保存済みの {} 件は 設定 › PDF の保存先 から移行できます）",
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
            let en = ui_en(app);
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
        PDF_REVEAL_STORAGE_ID => {
            let en = ui_en(app);
            let app = app.clone();
            // Drive の検出と stat は File Provider の応答待ちで秒単位になりうる・open の終了も待つ → main thread から外す
            tauri::async_runtime::spawn_blocking(move || {
                let st = app.state::<AppState>();
                if let Some(msg) = reveal_storage_folder(&st.db, en) {
                    let _ = app.emit("menu-toast", msg);
                }
            });
        }
        // 画面を開くのは webview（App の listen → openMigration）。開けない間（エディタ・モーダル・取込）は何もしない
        MIGRATION_OPEN_ID => {
            if let Err(e) = app.emit(MIGRATION_OPEN_EVENT, ()) {
                eprintln!("openref-app: migration-open を webview へ送れません: {e}");
            }
        }
        WIKI_CHAT_ID => {
            let en = ui_en(app);
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || wiki_chat_menu(&app, en));
        }
        // 反転後の値を書く（書けなければ戻す）。ログの ON/OFF は動いている子を止めずに次の発言から効く（下の Chat::set_log）。
        // ログを書くのは親（Chat の Inner）なので子は止めない = 応答中でも切り替わる（次の発言から）
        WIKI_CHAT_LOG_ID => {
            let log = toggle_setting(&items.wiki_chat_log, |v| save_setting(app, WIKI_CHAT_LOG_KEY, v));
            if let (Some(on), Some(c)) = (log, app.try_state::<wiki_chat::Chat>()) {
                c.set_log(on);
            }
        }
        id @ WIKI_ENABLED_ID => {
            // フォルダの選択ダイアログ（blocking）・走査は main thread から外す
            let en = ui_en(app);
            let (app, id) = (app.clone(), id.to_string());
            tauri::async_runtime::spawn_blocking(move || wiki_menu(&app, &id, en));
        }
        _ => {}
    }
}

/// メニューのチェック項目の読み書き（toggle_setting を試験で偽物に替える口）
trait Checkable {
    fn checked(&self) -> tauri::Result<bool>;
    fn set(&self, on: bool) -> tauri::Result<()>;
}

impl<R: tauri::Runtime> Checkable for tauri::menu::CheckMenuItem<R> {
    fn checked(&self) -> tauri::Result<bool> {
        self.is_checked()
    }
    fn set(&self, on: bool) -> tauri::Result<()> {
        self.set_checked(on)
    }
}

/// クリックで反転した後のチェックを save に渡す（"1" / "0"）。書けなければチェックを戻す。
/// 書けた値を返す（チェックを読めない・書けない = None。読めないときは save を呼ばない）
fn toggle_setting(item: &impl Checkable, save: impl FnOnce(&'static str) -> bool) -> Option<bool> {
    let checked = match item.checked() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("openref-app: メニューのチェック状態を読めません: {e}");
            return None;
        }
    };
    if save(if checked { "1" } else { "0" }) {
        Some(checked)
    } else {
        let _ = item.set(!checked);
        None
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
// 移動 / 改名はリンクの書換え付きの wiki_rename（GUI 専用）。登録表の wiki_move（MCP の call）は Tauri には出さない。wiki_save は GUI の
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
async fn wiki_duplicate(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_run(&state, None, args, openref_core::wiki::edit::duplicate).await
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

/// 左の木の vault 名 → フォルダ選択ダイアログ → 検査・保存・適用（Settings のメニュー項目「フォルダを選ぶ…」をここへ移した）。
/// 取り消し = null。選んだ = {root, restart}（restart = 前のフォルダの配信が再起動まで残る = toast に一言・プロセスで最初の 1 回だけ）。
/// toast は Rust が出す
#[tauri::command]
async fn wiki_pick_root(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    blocking(move || {
        let en = ui_en(&app);
        let Some(p) = wiki_pick_folder(&app, en) else {
            return Ok(serde_json::Value::Null);
        };
        match wiki_choose_root(&app, &p) {
            Ok((root, stale)) => {
                let detail = wiki_root_toast_detail(en, &root, stale);
                wiki_toast(&app, en, true, "OpenNote のフォルダ", "OpenNote folder", Some(detail));
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

/// `args.root`（文字列・空白は無し）
fn root_arg(args: Option<serde_json::Value>) -> Option<String> {
    obj_args(args)
        .get("root")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// 最近のワークスペース（wiki_status の workspaces）の 1 つへ切り替える。{root} → {root, restart}（restart = 前の root の
/// 配信が再起動まで残る）。履歴に無い root・check_root に落ちる root は Err。成功・失敗の toast は Rust が出す（画面は出さない）
#[tauri::command]
async fn wiki_switch_root(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = root_arg(args);
    blocking(move || {
        let en = ui_en(&app);
        let r = root
            .ok_or_else(|| arg_missing(en, "wiki_switch_root", "root"))
            .and_then(|r| wiki_switch_workspace(&app, &r));
        match r {
            Ok((root, stale)) => {
                let detail = wiki_root_toast_detail(en, &root, stale);
                wiki_toast(&app, en, true, "ワークスペースを切り替えました", "Switched workspace", Some(detail));
                Ok(serde_json::json!({"root": root.to_string_lossy(), "restart": stale}))
            }
            Err(e) => {
                wiki_toast(&app, en, false, "ワークスペースを切り替えられません", "Could not switch workspace", Some(e.clone()));
                Err(e)
            }
        }
    })
    .await
}

/// 最近のワークスペースの履歴から外す（その索引も消す）。{root} → {}。現在の root・履歴に無い root は Err。
/// 成功・失敗の toast は Rust が出す。画面は "wiki-state" で wiki_status を読み直す
#[tauri::command]
async fn wiki_forget_root(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = root_arg(args);
    blocking(move || {
        let en = ui_en(&app);
        let r = root
            .ok_or_else(|| arg_missing(en, "wiki_forget_root", "root"))
            .and_then(|r| wiki_forget_workspace(&app, &r).map(|()| r));
        match r {
            Ok(root) => {
                // 外した root の索引と、その root の参照フォルダの設定（wiki_workspaces の項目ごと）も消えた
                let note = tr(en, "索引と参照フォルダの設定も消しました", "its index and reference folder settings were removed too");
                let detail = wiki_toast_detail(en, wiki_workspace_name(Path::new(&root)), Some(note));
                wiki_toast(&app, en, true, "履歴から外しました", "Removed from the history", Some(detail));
                Ok(serde_json::json!({}))
            }
            Err(e) => {
                wiki_toast(&app, en, false, "履歴から外せません", "Could not remove from the history", Some(e.clone()));
                Err(e)
            }
        }
    })
    .await
}

/// 前面の OpenNote タブの root に設定 wiki_root（= MCP / CLI と wiki_status の既定）と監視を揃える。{root} → {root, restart}
/// （restart = 前の root の配信が再起動まで残る）。履歴にある root だけ（無ければ unknown_root）・last_used は変えない・チャットの子は
/// 止めない・toast は出さない（失敗は Err = 画面がヘッダーの警告に出す）。asset scope への追加を済ませてから返る（監視の張り直しと走査の結果は
/// "wiki-state" / "wiki-changed"）。画面はタブの行き来のたびに直列に呼ぶ
#[tauri::command]
async fn wiki_front_root(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = wiki_root_arg(&obj_args(args))?;
    blocking(move || {
        let en = ui_en(&app);
        let root = root.ok_or_else(|| arg_missing(en, "wiki_front_root", "root"))?;
        let (root, stale) = wiki_front_workspace(&app, &root)?;
        Ok(serde_json::json!({"root": root.to_string_lossy(), "restart": stale}))
    })
    .await
}

#[tauri::command]
async fn wiki_reindex(app: tauri::AppHandle) -> Result<(), String> {
    blocking(move || wiki_reindex_now(&app)).await
}

/// {dir, excluded, root?} → {dir, excluded}。木の右クリックの「索引から除く」/「索引に入れる」（本体は wiki_gui::wiki_exclude_set_now）
#[tauri::command]
async fn wiki_exclude_set(app: tauri::AppHandle, args: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let a = obj_args(args);
    blocking(move || wiki_gui::wiki_exclude_set_now(&app, &a)).await
}

/// Wiki のページから実在ファイルをホストの open で開く（vault の中、または vault の外の実在する通常ファイル = wiki_open_target）。
/// 文書の拡張子（WIKI_OPEN_EXT）以外・実行ビット付き・OpenWith の拡張属性付き・隠しフォルダの中（vault の外）は open せず
/// `open -R`（Finder で場所を表示）に落とす。戻り値 = Finder で表示しただけ（呼び手が知らせる）。
/// root = 画面のタブの root（履歴にあるものだけ = 無ければ unknown_root）。省略 = 設定 wiki_root。
/// reveal = true: 開かずに Finder で表示するだけ（ワークスペースの中の実在するファイル・フォルダ = 木の除外フォルダの「Finder で表示」。戻り値 true）。
/// 既存の open_external は http(s) 専用・open_attachment は添付の id 専用なので別に置く
#[tauri::command]
async fn wiki_open_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    root: Option<String>,
    reveal: Option<bool>,
) -> Result<bool, String> {
    let ctx = {
        let db = lock(&state.db)?;
        openref_core::wiki::Ctx::from_db(&db).map_err(err)?
    }; // ここでロックを外す
    let ctx = wiki_pin(ctx, root.as_deref())?;
    let en = ui_en(&app);
    blocking(move || {
        let (abs, reveal) = wiki_open_resolve(&wiki_ready(&ctx)?.root, &path, reveal == Some(true), en)?;
        // open / open -R の失敗の文は前から drop_dir_part（ディレクトリ部分を落とす）
        if reveal {
            return reveal_in_finder(&abs).map(|()| true);
        }
        open_file(&abs.to_string_lossy()).map(|()| false)
    })
    .await
}

// ---------- 本棚: command の包み（本体と節の説明は shelf_gui.rs・core は openref_core::shelf） ----------
// 引数は OpenNote と同じ invoke(name, { args: {...} })。root = 履歴の鍵（shelf_status の root）

/// `args.root`（無ければ Err = root は必須）
fn shelf_root_arg(name: &str, args: &serde_json::Value) -> Result<String, String> {
    openref_core::shelf::commands::root_arg(args).map_err(|e| format!("{name}: {}", shelf_err(e)))
}

/// `args.id`
fn shelf_id_arg(name: &str, args: &serde_json::Value) -> Result<String, String> {
    args.get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{name}: id は必須です"))
}

/// 履歴（登録表と同じ関数）。壊れた設定は退避して空から（Rust が toast）
#[tauri::command]
async fn shelf_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    blocking(move || {
        openref_core::shelf::commands::status(&shelf_ctx(&app)?, &serde_json::json!({}))
            .map_err(shelf_err)
    })
    .await
}

/// 表示の設定（登録表と同じ関数・同じ検査）。{root, collapse_single?, aliases?, issue_titles?, name?} → 書いた後の設定
#[tauri::command]
async fn shelf_set_opts(
    state: tauri::State<'_, AppState>,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let args = obj_args(args);
    let db = state.db.clone();
    blocking(move || {
        let r = (|| {
            openref_core::commands::validate("shelf_set_opts", &args).map_err(shelf_err)?;
            openref_core::shelf::commands::set_opts(&*lock(&db)?, &args).map_err(shelf_err)
        })();
        r.map_err(|e| shelf_hide_arg_root(e, &args))
    })
    .await
}

/// shelf_set_opts の失敗の文の args.root はフォルダ名まで（切替 / 外すと同じ）
fn shelf_hide_arg_root(e: String, args: &serde_json::Value) -> String {
    let root = args.get("root").and_then(|v| v.as_str()).unwrap_or_default();
    hide_roots(e, &[root.to_string()])
}

/// フォルダを本棚として開く。→ {root}（native のフォルダの選択ダイアログ・取り消し = {root: null}）。失敗の toast は Rust が出す。
/// 引数を受けない = 本棚のフォルダは利用者の選択だけが決める（webview から任意のフォルダを履歴と asset scope に足す道を作らない）
#[tauri::command]
async fn shelf_open_root(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    blocking(move || shelf_open(&app)).await
}

/// 履歴の本棚へ切り替える（走査し直す = 画面の読み直しもこれ）。{root} → {root}。履歴に無い root は Err。失敗の toast は Rust が出す
#[tauri::command]
async fn shelf_switch_root(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = shelf_root_arg("shelf_switch_root", &obj_args(args))?;
    blocking(move || shelf_switch(&app, &root)).await
}

/// 履歴から外す（走査結果の表と表紙も消す）。{root} → {thumbs, unknown}。結果の toast は Rust が出す
#[tauri::command]
async fn shelf_forget_root(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = shelf_root_arg("shelf_forget_root", &obj_args(args))?;
    blocking(move || shelf_forget(&app, &root)).await
}

/// 再起動で戻したタブの本棚を画面が読めるようにする（走査しない・last_used を変えない）。{root} → {root}
#[tauri::command]
async fn shelf_front_root(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = shelf_root_arg("shelf_front_root", &obj_args(args))?;
    blocking(move || shelf_front(&app, &root)).await
}

/// 走査結果を読む。{root} → {root, dir: null, tree, unreadable, used_find, thumbs_dir}（shelf_list の形 + 表紙の置き場）。表に無い・設定が変わった = 1 回走査する
#[tauri::command]
async fn shelf_tree(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = shelf_root_arg("shelf_tree", &obj_args(args))?;
    blocking(move || shelf_tree_of(&app, &root)).await
}

/// 表紙。{root, id} → {path}（無ければ作る・png の絶対パス・asset URL は画面が組む）
#[tauri::command]
async fn shelf_thumb(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let args = obj_args(args);
    let root = shelf_root_arg("shelf_thumb", &args)?;
    let id = shelf_id_arg("shelf_thumb", &args)?;
    blocking(move || shelf_thumb_of(&app, &root, &id)).await
}

/// Finder で表示。{root, id}
#[tauri::command]
async fn shelf_reveal(app: tauri::AppHandle, args: Option<serde_json::Value>) -> Result<(), String> {
    let args = obj_args(args);
    let root = shelf_root_arg("shelf_reveal", &args)?;
    let id = shelf_reveal_id(&args)?;
    blocking(move || shelf_reveal_book(&app, &root, id.as_deref())).await
}

/// 既定のアプリで開く。{root, id}
#[tauri::command]
async fn shelf_open_external(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<(), String> {
    let args = obj_args(args);
    let root = shelf_root_arg("shelf_open_external", &args)?;
    let id = shelf_id_arg("shelf_open_external", &args)?;
    blocking(move || shelf_open_book(&app, &root, &id)).await
}

// ---------- OpenNote: 参照フォルダと URL: command の包み（本体と節の説明は wiki_gui.rs） ----------

/// {root?} → {dirs}（そのワークスペースの参照フォルダ = wiki_workspaces[root].ref_dirs・root = 画面のタブの root（履歴にあるものだけ）・
/// 省略 = 設定 wiki_root）。解釈できない設定は退避して空から（Rust が toast）。検査（canonicalize）は main thread の外で
#[tauri::command]
async fn wiki_ref_dirs_get(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = wiki_root_arg(&obj_args(args))?;
    blocking(move || Ok(serde_json::json!({"dirs": wiki_ref_dirs(&app, root.as_deref())?}))).await
}

/// {root?} → フォルダの選択ダイアログを開いて検査し、そのワークスペース（root = 画面のタブの root（履歴にあるものだけ）・省略 = 設定
/// wiki_root）の参照フォルダに足す → {dirs, added: path | null}（取り消し = null）。Wiki が有効ならその場で asset scope に足す
#[tauri::command]
async fn wiki_ref_dir_add(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let root = wiki_root_arg(&obj_args(args))?;
    blocking(move || {
        let en = ui_en(&app);
        let root = root.as_deref();
        // 履歴に無い root はフォルダを選ばせる前に断る（ctx = その root に固定 = vault の中の検査もその root で）
        let ctx = wiki_pin(wiki_ctx(&app)?, root)?;
        let Some(p) = wiki_pick_folder_titled(
            &app,
            if en {
                "Choose a reference folder"
            } else {
                "参照フォルダを選ぶ"
            },
        ) else {
            // 取り消しは静か（一覧が読めなければ null = 画面は今のまま）
            return Ok(serde_json::json!({"dirs": wiki_ref_dirs(&app, root).ok(), "added": null}));
        };
        // 失敗の文の選んだフォルダ（綴りと canonical）はフォルダ名まで（画面の toast「参照フォルダを追加できません: …」）
        wiki_ref_dir_add_picked(&app, &ctx, root, &p, en).map_err(|e| hide_paths(e, &[&p]))
    })
    .await
}

/// wiki_ref_dir_add の選んだ後の段（検査 → 設定に足す → asset scope）
fn wiki_ref_dir_add_picked(
    app: &tauri::AppHandle,
    ctx: &openref_core::wiki::Ctx,
    root: Option<&str>,
    p: &Path,
    en: bool,
) -> Result<serde_json::Value, String> {
    {
        let home = ctx.home.as_deref().ok_or_else(|| {
            tr(
                en,
                "HOME が未設定のため検査できません",
                "HOME is not set, so the folder cannot be checked",
            )
        })?;
        let canon = wiki_ref_dir_check(
            p,
            home,
            ctx.root.as_deref().map(Path::new),
            ctx.app_dir.as_deref(),
            en,
        )?;
        // その root の ref_dirs に足す。解釈できない設定は wiki_ws_tx が退避してから空で始める = 応答の set_aside で画面が知らせる
        let (plan, ctx, set_aside) = wiki_ws_with(app, false, |m, cur| {
            let root = wiki_ws_target(m, root, cur)?.ok_or_else(|| {
                tr(
                    en,
                    "ワークスペースのフォルダが未設定です（先にフォルダを選ぶ）",
                    "No workspace folder is set (choose one first)",
                )
            })?;
            let plan = wiki_ref_dirs_for_add(m, root, &canon);
            if plan.changed {
                wiki_save_ref_dirs(m, root, &plan.dirs);
            }
            Ok(plan)
        })?;
        let ctx = wiki_pin(ctx, root)?;
        if ctx.enabled {
            wiki_allow_dir(app, &canon, plan.kind)?;
        }
        wiki_emit(app, "wiki-state", ());
        Ok(serde_json::json!({
            "dirs": wiki_ref_dirs_usable(plan.dirs, &ctx),
            "added": canon,
            "set_aside": set_aside,
        }))
    }
}

/// {path, root?} → {dirs, restart}（root = 画面のタブの root（履歴にあるものだけ）・省略 = 設定 wiki_root）。restart = このプロセスで
/// asset scope に足してあった（配信は再起動まで続く・リンクは外部で開くようになる）
#[tauri::command]
async fn wiki_ref_dir_remove(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let root = wiki_root_arg(&a)?;
    let path = PathBuf::from(req_str(&a, en, "wiki_ref_dir_remove", "path")?);
    blocking(move || {
        // 失敗の文の参照フォルダはフォルダ名まで（画面の toast「参照フォルダを外せません: …」）
        wiki_ref_dir_remove_now(&app, root.as_deref(), &path).map_err(|e| hide_paths(e, &[&path]))
    })
    .await
}

/// wiki_ref_dir_remove の本体
fn wiki_ref_dir_remove_now(app: &tauri::AppHandle, root: Option<&str>, path: &Path) -> Result<serde_json::Value, String> {
    {
        // その root の生の一覧から外す（検査に通らないものは残す = 見えないディスクの参照フォルダを消さない）
        let (dirs, ctx, _) = wiki_ws(app, |m, cur| {
            let target = wiki_ws_target(m, root, cur)?;
            Ok(wiki_ref_dirs_for_remove(m, target, path))
        })?;
        let ctx = wiki_pin(ctx, root)?;
        let restart = app
            .state::<WikiRuntime>()
            .allowed
            .lock()
            .map_err(err)?
            .contains(path);
        wiki_emit(app, "wiki-state", ());
        Ok(serde_json::json!({"dirs": wiki_ref_dirs_usable(dirs, &ctx), "restart": restart}))
    }
}

/// {abs, max_chars?, probe?, root?} → WikiPageData 相当 + external（参照フォルダ = root（画面のタブの root・履歴にあるものだけ）・省略 =
/// 設定 wiki_root の参照フォルダ）。html は本文（script は画面側が落とす・リンクの解決は画面側 = links は空）・
/// media は種類だけ。md・その他は Err（画面は外部で開く）。索引には触らない
#[tauri::command]
async fn wiki_read_ref(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let abs = req_str(&a, en, "wiki_read_ref", "abs")?.to_string();
    let max_chars = a
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .unwrap_or(200_000) as usize;
    // probe = 開けるかだけ確かめる（リンクを押したとき。拒否ならフロントはホストの open に落とす）
    let probe = a.get("probe").and_then(|v| v.as_bool()) == Some(true);
    let root = wiki_root_arg(&a)?;
    blocking(move || wiki_read_ref_now(&wiki_ref_dirs(&app, root.as_deref())?, &abs, max_chars, probe, en)).await
}

/// wiki_read_ref の本体（AppHandle 無し = 試験はこれで通す）。失敗の文の参照フォルダのパスはフォルダ名まで・渡したパスはファイル名まで
fn wiki_read_ref_now(dirs: &[PathBuf], abs: &str, max_chars: usize, probe: bool, en: bool) -> Result<serde_json::Value, String> {
    use openref_core::wiki::scan;
    let r = (|| -> Result<serde_json::Value, String> {
        let (dir, canon) = wiki_ref_target(dirs, abs, en)?;
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
                let bytes = wiki_ref_html_bytes(&canon, &name, en)?;
                wiki_ref_html_fields(&mut out, kind, &bytes, max_chars);
            }
        }
        Ok(out)
    })();
    // 参照フォルダの外のパス（外した参照フォルダのタブを復元した等）も = 渡したパスはファイル名まで
    let mut paths: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    paths.push(Path::new(abs.trim()));
    r.map_err(|e| hide_paths(e, &paths))
}

/// {url} → http(s) を既定のブラウザで開く（Wiki のページのリンク専用。検査は wiki_url_checked = http(s) 以外は拒む）。
/// OpenRef の別窓で開く設定は 3.0.0 で外した = DB に古い設定の行が残っていても読まない
#[tauri::command]
async fn wiki_open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let u = wiki_url_checked(&url, ui_en(&app))?;
    blocking(move || open_with_default_app(u.as_str())).await
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
        arg_missing_as(en, "wiki_chat_set_enabled", "enabled（true / false）", "enabled (true / false)")
    })?;
    blocking(move || {
        wiki_chat_enable(&app, on)?;
        wiki_chat::forget_auth();
        wiki_chat_status_now(&app, false)
    })
    .await
}

/// {new?, root?} → WikiChatStartResult。root = その root で子を起こす（画面の前面のタブの root・履歴にあるものだけ）・省略 = 設定
/// wiki_root。使えない（無効・claude が無い・未ログイン・openref-mcp が無い / 古い）なら Err（理由の文）。
/// 返る前に必ず "wiki-chat" {type: "state"} を出す（成功 = idle・失敗 = off）
#[tauri::command]
async fn wiki_chat_start(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let a = obj_args(args);
    let new = a.get("new").and_then(|v| v.as_bool()).unwrap_or(false);
    let root = wiki_root_arg(&a)?;
    let h = app.clone();
    let res = wiki_chat_start_now(app, new, root).await;
    wiki_chat_emit_state(&h);
    res
}

/// {text, context?: {path?, selection?, item?, title?}, root?} → {turn}（item / title = PDF のタブで開いている文献・一覧のタブで選んでいる文献）。root = 画面の前面のタブの
/// root: 子の root と違えば Err（送らない = 画面が先に wiki_chat_stop → wiki_chat_start {root} する）
#[tauri::command]
async fn wiki_chat_send(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let root = wiki_root_arg(&a)?;
    let text = a
        .get("text")
        .and_then(|v| v.as_str())
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| arg_missing_as(en, "wiki_chat_send", "text（空でない文字列）", "text (a non-empty string)"))?
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
        let turn = app.state::<wiki_chat::Chat>().send_to(
            root.as_deref().map(Path::new),
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
            .stop_now();
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
            arg_missing_as(en, "wiki_chat_set_model", "model（文字列・空 = 既定）", "model (a string; empty = default)")
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
            c.stop_now();
        }
        wiki_chat_status_now(&app, false)
    })
    .await;
    wiki_chat_emit_state(&h);
    res
}

/// {action: "login"} → WikiChatStatus。login = `<app_dir>/claude-login.command` を Terminal.app に開かせる（対話 = ブラウザ + ターミナル。
/// OpenRef は認証情報に触らない）。ログアウトは画面から呼ばない（ターミナルの claude auth logout を案内する）= action は login だけ
#[tauri::command]
async fn wiki_chat_auth(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    wiki_chat_auth_action(&obj_args(args), ui_en(&app))?;
    let h = app.clone();
    let res = blocking(move || {
        let en = ui_en(&app);
        // 使えるかは設定と claude の有無だけ（ログインの操作なので未ログイン・root・openref-mcp は問わない）
        let (ctx, on, _) = wiki_chat_settings(&app)?;
        let mut p = WikiChatProbe::settings(ctx.enabled, on);
        wiki_chat_check(en, &p)?;
        let found = wiki_chat::find_cli(ctx.home.as_deref(), std::env::var_os("PATH").as_deref());
        let cli = wiki_chat_need(en, &mut p, found, |p, ok| p.cli = ok)?;
        let app_dir = ctx.app_dir.clone().ok_or_else(|| {
            tr(
                en,
                "in-memory の DB ではログインを開けません",
                "Cannot open the login with an in-memory database",
            )
        })?;
        wiki_chat::open_login_terminal(&cli, &app_dir)?;
        wiki_chat::forget_auth();
        wiki_chat_status_now(&app, false)
    })
    .await;
    wiki_chat_emit_state(&h);
    res
}

/// wiki_chat_auth の action の検査（純関数）: login だけ（無い・文字列でない = 必須の文・login 以外 = Err）
fn wiki_chat_auth_action(a: &serde_json::Value, en: bool) -> Result<(), String> {
    let action = a
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| arg_missing_as(en, "wiki_chat_auth", "action（login）", "action (login)"))?;
    if action != "login" {
        return Err(tr(
            en,
            &format!("wiki_chat_auth: action は login だけです: {action}"),
            &format!("wiki_chat_auth: action must be login: {action}"),
        ));
    }
    Ok(())
}

/// {turn, index} → WikiChatUndoResult
#[tauri::command]
async fn wiki_chat_undo(
    app: tauri::AppHandle,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let en = ui_en(&app);
    let a = obj_args(args);
    let turn = req_u64(&a, en, "wiki_chat_undo", "turn")?;
    let index = req_u64(&a, en, "wiki_chat_undo", "index")? as usize;
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
        // 戻す先 = 記録した子の root（設定 wiki_root は前面のタブの root で、子の root と違いうる）。履歴から外した root なら戻さない
        let child = rec.root.clone().or_else(|| chat.root());
        let ctx = match wiki_chat_undo_ctx(wiki_ctx(&app)?, child.as_deref()) {
            Ok(c) => c,
            Err(reason) => return Ok(refused(reason)),
        };
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
    let wiki_chat_log = wiki_chat_log_on(pref(WIKI_CHAT_LOG_KEY).as_deref());
    WIKI_ON.store(wiki_enabled, std::sync::atomic::Ordering::Relaxed);
    let wiki_live = Arc::new(WikiLive::default());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // asset:// は自前で配信する（Tauri の既定と同じ scope・同じ URL の形。違い = 要求した webview を見て main 以外には 403）。
        // Tauri は External URL の窓にも asset を登録するので、窓を足したときに外部ページが vault / 参照フォルダを読めないように
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
            // 起動時の root はここで同期に読む（背景の家事と適用はこの root にだけ効かせる = 画面の揃えが先に済んでも前面の root を
            // touch しない・監視を起動時の root に張り直さない）
            let start_root = wiki_ctx(&h).ok().and_then(|c| c.root);
            tauri::async_runtime::spawn_blocking(move || {
                // 最近のワークスペースの更新・wiki-index/ の孤児の掃除→
                // 設定が起動時のままなら asset scope と監視（root を変える段の鍵の中 = wiki_front_root と直列）
                wiki_startup(&h, start_root);
            });
            // 本棚: 表紙の置き場を asset scope へ（root は開いたとき）
            let h = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || shelf_startup(&h));
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
        .manage(ShelfRuntime::default())
        .invoke_handler(tauri::generate_handler![
            list_items,
            get_item,
            collections_tree,
            view_counts,
            quick_add,
            wiki_cite_lookup,
            wiki_cite_register,
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
            wiki_save,
            wiki_create,
            wiki_mkdir,
            wiki_rename_preview,
            wiki_rename,
            wiki_duplicate,
            wiki_trash,
            wiki_save_blob,
            wiki_import_file,
            wiki_exit_ready,
            wiki_reindex,
            wiki_open_path,
            wiki_pick_root,
            wiki_switch_root,
            wiki_forget_root,
            wiki_front_root,
            wiki_ref_dirs_get,
            wiki_ref_dir_add,
            wiki_ref_dir_remove,
            wiki_exclude_set,
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
            wiki_chat_auth,
            shelf_status,
            shelf_set_opts,
            shelf_open_root,
            shelf_switch_root,
            shelf_forget_root,
            shelf_front_root,
            shelf_tree,
            shelf_thumb,
            shelf_reveal,
            shelf_open_external
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
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => wiki_exit_event(app, ExitKind::Close(label), || api.prevent_close()),
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
    /// 検査（registry_named_wiki_commands_share_core_fn・registry_named_shelf_commands_share_core_fn）するのは wiki_* と shelf_* だけ（ほかの command は引数の形が各自で照合できない）
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
        ("wiki_reindex", "GUI の監視の張り直し + 全走査。agent の入口は鮮度規則で自動更新される"),
        ("wiki_open_path", "デスクトップのアプリで開く = ホストの open（ワークスペースの中の実在ファイルと、本文の file:// が指すワークスペースの外の実在する通常ファイル。許可一覧の外の拡張子等は Finder で表示・reveal = Finder で表示だけ）。agent は wiki_read の abs_path を直接開ける"),
        ("wiki_exclude_set", "索引から除く / 入れるフォルダは利用者が GUI の木で決める（設定 wiki_workspaces の exclude / include を書いて走査し直す）。agent が除外を変えると利用者の木と検索が黙って変わる = 読むのは wiki_list の excluded"),
        ("wiki_pick_root", "ワークスペースのフォルダは利用者が GUI のフォルダ選択で決める（agent が任意のフォルダをワークスペースにしない）"),
        ("wiki_switch_root", "ワークスペースの切替は利用者が GUI で決める（履歴にある root だけ）= agent が任意のフォルダへ切り替えて書き回らない。agent は wiki_status の workspaces で読む"),
        ("wiki_forget_root", "最近のワークスペースの履歴から外す（GUI の [外す]・その索引も消す）。ワークスペースの選択は利用者が GUI で決める。agent は wiki_status の workspaces で読む"),
        ("wiki_front_root", "前面の OpenNote タブの root に設定 wiki_root（= agent の既定の root）と監視を揃える GUI のタブの切替（履歴にある root だけ・last_used は変えない）。agent は既定を動かさず wiki_* の root で固定する"),
        ("wiki_ref_dirs_get", "参照フォルダ（ワークスペースの外の読み取り専用のルート）の一覧。GUI の表示用。agent は sql の settings（wiki_workspaces）"),
        ("wiki_ref_dir_add", "参照フォルダは利用者が GUI のフォルダ選択で決める（agent が任意のフォルダを配信の範囲に足さない）"),
        ("wiki_ref_dir_remove", "参照フォルダを外す（GUI の [外す]）。agent には用途が無い"),
        ("wiki_read_ref", "参照フォルダの中のファイルを内蔵の表示に出すための読み（asset URL の組立てと html の本文）。agent はパスを直接読める"),
        ("wiki_open_url", "ページの http(s) リンクを既定のブラウザで開く（GUI の操作）。agent には用途が無い"),
        ("wiki_create", "GUI の新規ページ（Untitled.md / .html の連番を作るだけ）。agent は wiki_write で名前と中身を決めて書く"),
        ("wiki_mkdir", "GUI の新規フォルダ（空のフォルダを作るだけ）。agent は wiki_write のパスで途中のフォルダを作れる"),
        ("wiki_rename_preview", "GUI の改名ダイアログの件数（書き換える [[旧名]] と相対リンク）。利用者が確かめてから書き換える前段"),
        ("wiki_rename", "リンクの一括書換え付きの改名は利用者が GUI で件数を見て決める（無確認の書換えはしない）"),
        ("wiki_duplicate", "GUI の右クリックの複製（連番の名前で写して開く）。agent は wiki_read + wiki_write で名前も中身も決めて書ける"),
        ("wiki_trash", "macOS のゴミ箱へ（Finder の「戻す」）は利用者の操作。agent に消す口は出さない"),
        ("wiki_save_blob", "エディタへの画像の貼付（クリップボードの中身 = base64）。agent はファイルを直接ワークスペースに置ける"),
        ("wiki_import_file", "OS からのドロップ（Tauri の onDragDropEvent が渡す絶対パス）を attachments へコピー。agent はファイルを直接置ける"),
        ("wiki_read_gui", "画像・PDF は asset protocol で直接描くので base64 も抽出もしない read。登録表の wiki_read は read_agent（画像の base64・PDF の抽出テキスト）"),
        ("wiki_save", "エディタの保存: if_sha256 一致の自動保存は退避しない・競合ダイアログの force を受ける。登録表の wiki_write（write_agent）は上書きを必ず退避し force を持たない別の関数"),
        ("wiki_exit_ready", "⌘Q・窓の × の保留への GUI の応答（未保存の編集を保存してから終了）。agent には終了の段取りが無い"),
        ("wiki_chat_status", "画面右端のチャット（Claude Code CLI を子プロセスで起こす GUI の機能）が使えるか。agent はそれ自体がホスト"),
        ("wiki_chat_set_enabled", "チャットの有効化は利用者が GUI で同意して決める（既定 OFF）。agent が自分を起こす口にしない"),
        ("wiki_chat_start", "GUI が持つ Claude Code の子プロセスを起こす。agent から呼ぶと agent が agent を起こす（利用枠を使う）"),
        ("wiki_chat_send", "GUI のチャットの発言を子プロセスの stdin へ。agent には相手がいない"),
        ("wiki_chat_interrupt", "GUI のチャットの停止ボタン（子プロセスへの割込み）"),
        ("wiki_chat_stop", "GUI が持つ子プロセスを終える"),
        ("wiki_chat_undo", "チャットのターンの記録（GUI のプロセスの中だけ）を鍵に戻す。agent は wiki-history の退避を wiki_write で書き戻せる"),
        ("wiki_chat_set_model", "GUI のチャットのモデルの選択（設定 wiki_chat_model）。agent 自身のモデルはホストが決める"),
        ("wiki_chat_auth", "GUI のチャットのログイン（利用者の Claude Code CLI の claude auth login をターミナルで開く）。agent には用途が無い"),
        ("shelf_open_root", "本棚のフォルダは利用者が GUI のフォルダ選択で決める（agent が任意のフォルダを走査・配信の範囲に足さない）"),
        ("shelf_switch_root", "本棚の切替は GUI のタブの操作（履歴にある root だけ・asset scope に足す）。agent は shelf_list の root で読む"),
        ("shelf_forget_root", "本棚を履歴から外す（GUI の [外す]・表と表紙も消す）。本棚の選択は利用者が GUI で決める。agent は shelf_status で読む"),
        ("shelf_front_root", "再起動で戻したタブの本棚を asset scope に足す GUI の段取り。agent はパスを直接読める"),
        ("shelf_tree", "GUI のプロセス内の走査結果を読む（画面の表示用）。agent は shelf_list で毎回走査する"),
        ("shelf_thumb", "表紙の PNG を作る（asset:// で描く GUI 用）。agent はパスを直接読める"),
        ("shelf_reveal", "Finder で表示 = ホストの open -R（パスは shelf_list の path）"),
        ("shelf_open_external", "既定のアプリで開く = ホストの open（パスは shelf_list の path）"),
        ("wiki_cite_lookup", "OpenNote のエディタの文献の引用で、打った・貼った DOI / PMID / URL を蔵書と照合する（GUI の候補の表示）。agent は sql で items の DOI / PMID を引ける"),
        ("wiki_cite_register", "OpenNote のエディタに貼った DOI / PubMed の URL を、利用者が [登録して文献リンクに] を押したときに登録して key を返す（重複は既存の key）。agent は登録表の quick_add で登録できる"),
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
    pub(super) fn fn_body(src: &str, head: &str) -> String {
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
        assert!(checked >= 6, "走査が壊れている: {checked}");
    }

    /// 本棚のフォルダは利用者のフォルダ選択だけが決める: shelf_open_root は引数を受けず、本体は native の picker を通る。
    /// path を受ける形に戻すと、webview の invoke 1 回で任意のフォルダが履歴と asset scope（再帰）に入る
    #[test]
    fn shelf_open_root_takes_no_path() {
        let head = |body: &str| body[..body.find('{').expect("fn の本体")].to_string();
        let cmd = fn_body(include_str!("main.rs"), "async fn shelf_open_root(");
        assert!(!head(&cmd).contains("args") && !head(&cmd).contains("path"), "{cmd}");
        assert!(!cmd.contains("\"path\""), "{cmd}");
        let open = fn_body(include_str!("shelf_gui.rs"), "pub(crate) fn shelf_open(");
        assert_eq!(head(&open).matches(':').count(), 3, "引数は app だけ: {}", head(&open));
        assert!(open.contains("wiki_pick_folder_titled("), "{open}");
        // 履歴に鍵を足す段（shelf_open_steps）を呼ぶ command は picker の 1 本だけ
        let gui = include_str!("shelf_gui.rs");
        let code = &gui[..gui.find("#[cfg(test)]").expect("試験の節")];
        assert_eq!(code.matches("shelf_open_steps(").count(), 2, "定義と shelf_open の 2 箇所だけ");
    }

    /// wiki_open_url は検査（wiki_url_checked = http(s) だけ）してから既定のブラウザで開くだけ（3.0.0）。
    /// 設定を読んで OpenRef の別窓を作る枝を戻さない（DB に古い設定の行が残っていても挙動を変えない）
    #[test]
    fn wiki_open_url_opens_the_default_browser_only() {
        let cmd = fn_body(include_str!("main.rs"), "async fn wiki_open_url(");
        assert!(cmd.contains("let u = wiki_url_checked(&url, ui_en(&app))?;"), "{cmd}");
        assert!(cmd.contains("blocking(move || open_with_default_app(u.as_str())).await"), "{cmd}");
        for bad in ["setting(", "AppState", "run_on_main_thread", "Window", "navigate"] {
            assert!(!cmd.contains(bad), "{bad}: {cmd}");
        }
    }

    /// 空白・改行と、閉じ括弧の前のカンマを除いた字面（整形・インデントで落ちない）
    pub(super) fn squash(s: &str) -> String {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        s.replace(",)", ")").replace(",}", "}")
    }

    /// 本体（最初の #[cfg(test)] より前 = 試験の節の文字列で通さない）を squash したもの
    pub(super) fn code_squashed() -> String {
        let src = include_str!("main.rs");
        squash(&src[..src.find("#[cfg(test)]").expect("試験の節")])
    }

    /// Settings › OpenNote › チャットのログの handler と窓の × の保留の配線（Tauri の runtime の外では押せない所）:
    /// チャットのログは反転後の値を toggle_setting で WIKI_CHAT_LOG_KEY に書き、書けたら Chat::set_log（子は止めない）・
    /// Wiki の ON/OFF で押せる / 押せないが変わる・窓の × は条件なしに wiki_exit_event を通す（未保存の編集を保存してから閉じる）。
    /// 書く値（"1" / "0"）と書けないときにチェックを戻すことは menu::toggle_* が挙動で確かめる
    #[test]
    fn chat_log_menu_and_close_hold_stay_wired() {
        let code = code_squashed();
        for want in [
            "WIKI_CHAT_LOG_ID => {
                let log = toggle_setting(&items.wiki_chat_log, |v| save_setting(app, WIKI_CHAT_LOG_KEY, v));
                if let (Some(on), Some(c)) = (log, app.try_state::<wiki_chat::Chat>()) { c.set_log(on); }
            }",
            "event: tauri::WindowEvent::CloseRequested { api, .. }, .. } => wiki_exit_event(app, ExitKind::Close(label), || api.prevent_close()),",
        ] {
            assert!(code.contains(&squash(want)), "配線が無い: {want}");
        }
        let sync = squash(&fn_body(include_str!("wiki_chat_gui.rs"), "fn wiki_chat_sync_menu("));
        assert!(sync.contains(&squash("items.wiki_chat_log.set_enabled(wiki_on)")), "{sync}");
    }

    /// shelf_reveal は args の id を本体へ渡す（常に None にすると本のカードの「Finder で表示」が本棚のフォルダを出す）
    #[test]
    fn shelf_reveal_passes_the_id() {
        let cmd = fn_body(include_str!("main.rs"), "async fn shelf_reveal(");
        assert!(cmd.contains("let id = shelf_reveal_id(&args)?;"), "{cmd}");
        assert!(cmd.contains("shelf_reveal_book(&app, &root, id.as_deref())"), "{cmd}");
    }

    /// 登録表と同名の shelf_*（shelf_status / shelf_set_opts）は登録表の handler と同じ core 関数を通す
    #[test]
    fn registry_named_shelf_commands_share_core_fn() {
        let src = include_str!("main.rs");
        let reg = include_str!("../../../crates/openref-core/src/commands.rs");
        let core_fn = regex::Regex::new(r"\b(shelf::commands::[a-z_0-9]+)\(").unwrap();
        let mut checked = 0;
        for n in tauri_commands() {
            if !n.starts_with("shelf_") || openref_core::commands::find(&n).is_none() {
                continue;
            }
            let body = fn_body(src, &format!("async fn {n}("));
            let rbody = fn_body(reg, &format!("fn {n}(db: &mut Db"));
            let gui = core_fn.captures(&body).unwrap_or_else(|| panic!("{n} が core 関数を呼ばない"))[1].to_string();
            let core = core_fn.captures(&rbody).unwrap_or_else(|| panic!("登録表の {n} の core 関数が見つからない"))[1].to_string();
            assert_eq!(gui, core, "{n}: GUI と登録表で core 関数が違う");
            checked += 1;
        }
        assert_eq!(checked, 2, "走査が壊れている");
    }
}

#[cfg(test)]
mod menu {
    use super::parity::{code_squashed, squash};
    use super::*;
    use std::cell::{Cell, RefCell};

    /// 文言を持つ項目の全部（id, ja, en）。メニューの題名（Settings / Language / OpenNote / Edit / View / Window）は入れない
    const WANT: &[(&str, &str, &str)] = &[
        (QUIT_ID, "OpenRef を終了", "Quit OpenRef"),
        (SETTINGS_MENU_ID, "設定", "Settings"),
        (LANGUAGE_MENU_ID, "言語", "Language"),
        (RESTORE_TABS_ID, "起動時に前回のタブを開く", "Restore tabs on launch"),
        (UPDATE_CHECK_ID, "起動時に新版を確認", "Check for updates on launch"),
        (PDF_STORAGE_ID, "PDF の保存先", "PDF storage"),
        (PDF_STORAGE_MANAGED_ID, "ローカル (Mac 内)", "Local (on this Mac)"),
        (PDF_STORAGE_DRIVE_ID, "Google Drive", "Google Drive"),
        (PDF_MIGRATE_DRIVE_ID, "ローカル保存済みの PDF を Drive へ移行", "Move local PDFs to Google Drive"),
        (PDF_REVEAL_STORAGE_ID, "保存先のフォルダを Finder で開く", "Show Storage Folder in Finder"),
        (WIKI_ENABLED_ID, "有効", "Enabled"),
        (WIKI_CHAT_ID, "チャット", "Chat"),
        (WIKI_CHAT_LOG_ID, "チャットのログを chat-log/ に残す", "Keep chat logs in chat-log/"),
        (LANG_JA_ID, "日本語", "日本語"),
        (LANG_EN_ID, "English", "English"),
        (MIGRATION_OPEN_ID, "他の文献管理ソフトから移行…", "Migrate from another reference manager…"),
        (EDIT_MENU_ID, "編集", "Edit"),
        (VIEW_MENU_ID, "表示", "View"),
        (tauri::menu::WINDOW_SUBMENU_ID, "ウインドウ", "Window"),
        ("std_about", "OpenRef について", "About OpenRef"),
        ("std_hide", "OpenRef を非表示", "Hide OpenRef"),
        ("std_hide_others", "ほかを非表示", "Hide Others"),
        ("std_undo", "取り消す", "Undo"),
        ("std_redo", "やり直す", "Redo"),
        ("std_cut", "カット", "Cut"),
        ("std_copy", "コピー", "Copy"),
        ("std_paste", "ペースト", "Paste"),
        ("std_select_all", "すべてを選択", "Select All"),
        ("std_fullscreen", "フルスクリーンにする", "Toggle Full Screen"),
        ("std_minimize", "しまう", "Minimize"),
        ("std_maximize", "拡大/縮小", "Zoom"),
    ];

    /// 定義済みの項目の英語は muda 0.19.3 の既定の字（items/predefined.rs の PredefinedMenuItemType::text() から
    /// ニーモニックの & を除いた物・About / Hide は platform_impl/macos の「About / Hide + アプリ名」= 配布の .app の名 OpenRef）。
    /// メニューの題名の英語は前の Title::Fixed の字。英語で起動したときは text を渡さない（muda の既定のまま）
    #[test]
    fn predefined_items_keep_the_english_of_muda() {
        let strip = |s: &str| s.replace('&', "");
        for (s, muda) in [
            (Std::About, "About OpenRef".to_string()),
            (Std::Hide, "Hide OpenRef".to_string()),
            (Std::HideOthers, strip("Hide Others")),
            (Std::Undo, strip("Undo")),
            (Std::Redo, strip("Redo")),
            (Std::Cut, strip("Cu&t")),
            (Std::Copy, strip("&Copy")),
            (Std::Paste, strip("&Paste")),
            (Std::SelectAll, strip("Select &All")),
            (Std::Fullscreen, strip("Toggle Full Screen")),
            (Std::Minimize, strip("&Minimize")),
            (Std::Maximize, strip("Zoom")),
        ] {
            assert_eq!(menu_text(s.id(), true), muda, "{s:?}");
        }
        for (id, before) in [(EDIT_MENU_ID, "Edit"), (VIEW_MENU_ID, "View"), (tauri::menu::WINDOW_SUBMENU_ID, "Window")] {
            assert_eq!(menu_text(id, true), before);
        }
        let build = squash(&super::parity::fn_body(include_str!("main.rs"), "fn build_node("));
        assert!(build.contains("lett=(!en).then(||menu_text(s.id(),false));"), "英語で起動 = text を渡さない: {build}");
        for f in ["about(app,t,Some(about.clone()))", "hide(app,t)", "hide_others(app,t)", "undo(app,t)", "redo(app,t)", "cut(app,t)", "copy(app,t)", "paste(app,t)", "select_all(app,t)", "fullscreen(app,t)", "minimize(app,t)", "maximize(app,t)"] {
            assert!(build.contains(&format!("P::{f}")), "{f}");
        }
    }

    /// 項目ごとの ja / en の文言（組立てと Language の切替は同じ窓口 menu_text から文言を取る）
    #[test]
    fn every_item_has_its_text_in_both_languages() {
        let mut ids: Vec<&str> = WANT.iter().map(|w| w.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), WANT.len(), "id が重複している");
        for &(id, ja, en) in WANT {
            assert_eq!(menu_text(id, false), ja, "{id} の日本語");
            assert_eq!(menu_text(id, true), en, "{id} の英語");
        }
    }

    /// 索引を作り直す入口は画面のワークスペース名のメニュー 1 つ: メニューバーに作り直す / 削除の項目が無く、監視の失敗の案内（2 か所）は
    /// 今の置き場（ワークスペース名のメニューの「索引を作り直す」）を指す（前の「ヘッダーの [再索引]」を案内しない）
    #[test]
    fn reindex_lives_in_the_workspace_menu_only() {
        let ids: Vec<&str> = MENU_TEXT.iter().map(|t| t.0).collect();
        assert!(!ids.iter().any(|id| id.contains("reindex") || id.contains("drop_index")), "メニューバーに索引の項目が残った: {ids:?}");
        let gui = include_str!("wiki_gui.rs");
        assert!(!gui.contains("ヘッダーの [再索引]") && !gui.contains("Reindex in the header"), "前の置き場（ヘッダーの [再索引]）を案内している");
        assert_eq!(gui.matches("ワークスペース名のメニューの「索引を作り直す」で取り込めます").count(), 2, "監視の失敗の案内（日本語）が 2 か所でない");
        // 英語は項目名を引用符で括る（ソースの字面 = \" の escape）
        assert_eq!(gui.matches(r#"use \"Rebuild index\" in the workspace name menu to pick up changes"#).count(), 2, "監視の失敗の案内（英語）が 2 か所でない");
    }

    /// 表（MENU_TEXT）の id は上の WANT と同じ集合で、重複しない（使わない行・同じ id の 2 行目が残らない）
    #[test]
    fn the_table_has_exactly_these_items() {
        let ids = |t: &[(&'static str, &str, &str)]| {
            let mut v: Vec<&'static str> = t.iter().map(|r| r.0).collect();
            v.sort_unstable();
            v
        };
        let table = ids(MENU_TEXT);
        let mut uniq = table.clone();
        uniq.dedup();
        assert_eq!(uniq, table, "表の id が重複している");
        assert_eq!(table, ids(WANT));
    }

    /// Settings › 他の文献管理ソフトから移行… は押すと条件なしに "migration-open" を出す。App.svelte は同じ名前を listen して
    /// openMigration を呼ぶ（受け手は app.test.mjs が実部品の App.svelte へ "migration-open" を emit して確かめる。
    /// 名前が食い違うと押しても黙って何も起きない）
    #[test]
    fn migration_item_emits_what_the_app_listens() {
        assert_eq!(MIGRATION_OPEN_EVENT, "migration-open");
        let arm = squash("MIGRATION_OPEN_ID => { if let Err(e) = app.emit(MIGRATION_OPEN_EVENT, ()) {");
        assert!(code_squashed().contains(&arm), "handler の枝が最初の文で emit しない");
    }

    /// 偽のチェック項目（on = クリックで反転した後の状態）
    struct Fake {
        on: Cell<bool>,
        unreadable: bool,
    }

    impl Checkable for Fake {
        fn checked(&self) -> tauri::Result<bool> {
            if self.unreadable {
                Err(std::io::Error::other("読めない").into())
            } else {
                Ok(self.on.get())
            }
        }
        fn set(&self, on: bool) -> tauri::Result<()> {
            self.on.set(on);
            Ok(())
        }
    }

    /// チェック項目（起動時に前回のタブ・新版の確認・チャットのログ）: 反転後の値を "1" / "0" で書く・
    /// 書けなければチェックを前に戻して None・読めなければ書かずに動かさない
    #[test]
    fn toggle_writes_the_state_after_the_click_and_reverts_when_it_cannot() {
        for (after_click, saved, value, ret, left) in [
            (true, true, "1", Some(true), true),
            (false, true, "0", Some(false), false),
            (true, false, "1", None, false),
            (false, false, "0", None, true),
        ] {
            let item = Fake { on: Cell::new(after_click), unreadable: false };
            let wrote = RefCell::new(Vec::new());
            let got = toggle_setting(&item, |v| {
                wrote.borrow_mut().push(v);
                saved
            });
            let case = format!("クリック後 {after_click}・書け {saved}");
            assert_eq!(*wrote.borrow(), vec![value], "{case}: 書いた値");
            assert_eq!(got, ret, "{case}: 戻り値");
            assert_eq!(item.on.get(), left, "{case}: 残ったチェック");
        }
        let item = Fake { on: Cell::new(true), unreadable: true };
        let mut called = false;
        let got = toggle_setting(&item, |_| {
            called = true;
            true
        });
        assert_eq!(got, None);
        assert!(!called, "読めないのに書いた");
        assert!(item.on.get(), "読めないのにチェックを動かした");
    }

    /// menu_spec を 1 項目 1 行の字にする（[x] / [ ] = チェック・(押せない) = enabled が false・末尾 = ショートカット・#id = サブメニューの id）
    fn outline(nodes: &[Node], depth: usize, out: &mut String) {
        for n in nodes {
            let off = |enabled: bool| if enabled { "" } else { " (押せない)" };
            let line = match n {
                Node::Std(s) => format!("{s:?}"),
                Node::Sep => "—".to_string(),
                Node::Item { id, enabled, accel } => {
                    format!("{id}{}{}", off(*enabled), accel.map_or(String::new(), |a| format!(" {a}")))
                }
                Node::Check { id, enabled, checked } => {
                    format!("[{}] {id}{}", if *checked { "x" } else { " " }, off(*enabled))
                }
                Node::Sub { id, title, .. } => {
                    let t = match title {
                        Title::App => "<アプリ名>".to_string(),
                        Title::Fixed(t) => t.to_string(),
                        Title::Text => "<表の文言>".to_string(),
                    };
                    format!("{t} ▸{}", id.map_or(String::new(), |i| format!(" #{i}")))
                }
            };
            out.push_str(&format!("{}{line}\n", "  ".repeat(depth)));
            if let Node::Sub { items, .. } = n {
                outline(items, depth + 1, out);
            }
        }
    }

    fn outline_of(init: MenuInit) -> String {
        let mut s = String::new();
        outline(&menu_spec(&init), 0, &mut s);
        s
    }

    /// メニューの形の全部（並び・区切り・⌘Q・有効 / 無効・初期のチェック）。
    /// 「他の文献管理ソフトから移行…」は Settings の最後に区切りの後で押せる（3.0.0 以後、移行の窓の入口はここだけ）。
    /// チャットとチャットのログは OpenNote が有効なときだけ押せる。Close Window(⌘W)は置かない
    #[test]
    fn menu_spec_is_the_menu() {
        let all_off = "\
<アプリ名> ▸
  About
  —
  Hide
  HideOthers
  —
  quit CmdOrCtrl+Q
<表の文言> ▸ #edit_menu
  Undo
  Redo
  —
  Cut
  Copy
  Paste
  SelectAll
<表の文言> ▸ #view_menu
  Fullscreen
<表の文言> ▸ #settings_menu
  [ ] restore_tabs
  [ ] update_check
  <表の文言> ▸ #pdf_storage
    [x] pdf_storage_managed
    [ ] pdf_storage_drive
    —
    pdf_migrate_drive
    pdf_reveal_storage
  OpenNote ▸
    [ ] wiki_enabled
    —
    [ ] wiki_chat (押せない)
    [ ] wiki_chat_log (押せない)
  <表の文言> ▸ #language_menu
    [x] lang_ja
    [ ] lang_en
  —
  migration_open
<表の文言> ▸ #__tauri_window_menu__
  Minimize
  Maximize
";
        assert_eq!(outline_of(MenuInit::default()), all_off);
        let all_on = MenuInit {
            restore_tabs: true,
            update_check: true,
            en: true,
            pdf_drive_mode: true,
            wiki_enabled: true,
            wiki_chat: true,
            wiki_chat_log: true,
        };
        let want = all_off
            .replace("[ ] restore_tabs", "[x] restore_tabs")
            .replace("[ ] update_check", "[x] update_check")
            .replace("[x] pdf_storage_managed", "[ ] pdf_storage_managed")
            .replace("[ ] pdf_storage_drive", "[x] pdf_storage_drive")
            .replace("[ ] wiki_enabled", "[x] wiki_enabled")
            .replace("[ ] wiki_chat (押せない)", "[x] wiki_chat")
            .replace("[ ] wiki_chat_log (押せない)", "[x] wiki_chat_log")
            .replace("[x] lang_ja", "[ ] lang_ja")
            .replace("[ ] lang_en", "[x] lang_en");
        assert_eq!(outline_of(all_on), want);
        // チャットのチェックは OpenNote が無効なら設定が ON でも外す・ログのチェックは設定の値のまま（押せないだけ）
        let wiki_off = MenuInit { wiki_chat: true, wiki_chat_log: true, ..MenuInit::default() };
        let got = outline_of(wiki_off);
        assert!(got.contains("[ ] wiki_chat (押せない)") && got.contains("[x] wiki_chat_log (押せない)"), "{got}");
    }

    /// 文言を持つ項目の id を menu_spec から集める（Node::label_id = 組立てが labeled に入れる判定と同じ）
    fn label_ids(nodes: &[Node], out: &mut Vec<&'static str>) {
        for n in nodes {
            out.extend(n.label_id());
            if let Node::Sub { items, .. } = n {
                label_ids(items, out);
            }
        }
    }

    /// Language で差し替える項目 = 表（MENU_TEXT）の項目すべて（足りない・余る・2 度入るが無い）。初期の状態に依らない
    #[test]
    fn language_switch_covers_every_item_in_the_table() {
        let mut table: Vec<&str> = MENU_TEXT.iter().map(|t| t.0).collect();
        table.sort_unstable();
        for init in [MenuInit::default(), MenuInit { en: true, wiki_enabled: true, ..MenuInit::default() }] {
            let mut ids = Vec::new();
            label_ids(&menu_spec(&init), &mut ids);
            ids.sort_unstable();
            assert_eq!(ids, table, "{init:?}");
        }
    }

    /// relabel は項目ごとに 1 回、その言語の表の文言で set を呼ぶ。途中で失敗しても残りを差し替え、最初の失敗を返す
    #[test]
    fn relabel_sets_each_item_to_the_text_of_the_language() {
        let items: Vec<(&'static str, &'static str)> = MENU_TEXT.iter().map(|t| (t.0, t.0)).collect();
        for en in [false, true] {
            let mut got = Vec::new();
            relabel(&items, en, |item, text| {
                got.push((*item, text));
                Ok(())
            })
            .unwrap();
            let want: Vec<(&str, &str)> =
                MENU_TEXT.iter().map(|&(id, ja, en_s)| (id, if en { en_s } else { ja })).collect();
            assert_eq!(got, want, "en = {en}");
        }
        let mut calls = 0;
        let res = relabel(&items, true, |item, _| {
            calls += 1;
            match *item {
                QUIT_ID => Err(std::io::Error::other("1 つ目の失敗").into()),
                WIKI_CHAT_ID => Err(std::io::Error::other("2 つ目の失敗").into()),
                _ => Ok(()),
            }
        });
        assert_eq!(calls, items.len(), "失敗の後も差し替える");
        assert!(res.unwrap_err().to_string().contains("1 つ目の失敗"));
    }

    /// Tauri の runtime の中でしか動かない配線（muda は main thread でしか Menu を作れない = 単体試験で組めない）:
    /// 組立てが menu_spec の enabled / ショートカット / チェックを渡し、label_id の項目を labeled に入れる・
    /// Language の切替が今の言語で relabel に set_label を渡す。空白・改行・閉じ括弧の前のカンマに依らない
    #[test]
    fn builder_and_language_switch_stay_wired() {
        let code = code_squashed();
        for want in [
            "let top = menu_spec(&init).iter().map(|n| build_node(app, n, init.en, &about, &mut labeled))",
            "Node::Item { id, enabled, accel } => { MenuItem::with_id(app, *id, menu_text(*id, en), *enabled, *accel)?.kind() }",
            "Node::Check { id, enabled, checked } => { CheckMenuItem::with_id(app, *id, menu_text(*id, en), *enabled, *checked, None::<&str>)?.kind() }",
            "Some(id) => Submenu::with_id_and_items(app, *id, text, true, &kids)?,",
            "if let Some(id) = node.label_id() { labeled.push((id, kind.clone())); }",
            "let menu = checks.and(relabel(&items.labeled, now_en, set_label));",
        ] {
            assert!(code.contains(&squash(want)), "配線が無い: {want}");
        }
    }
}

#[cfg(test)]
mod attach_pdf {
    use openref_core::{db::Db, edit, pdftools};
    use std::sync::Mutex;

    /// 手動の PDF 添付（attach_pdf_now = ロックの外で抽出）は、前の呼び方（edit::attach_auto をロックの中で）と同じ行を作る:
    /// 添付の行（kind・mode・md5）・本文の行（text・status・md5）・page_count。無いファイル・DB 自身のファイルは同じ文の Err
    #[test]
    fn manual_attach_matches_the_locked_path() {
        let tmp = std::env::temp_dir().join(format!("openref-attach-pdf-{}-{}", std::process::id(), openref_core::keys::new_key()));
        std::fs::create_dir_all(&tmp).unwrap();
        let dbp = tmp.join("t.sqlite");
        let mut db = Db::open(&dbp).unwrap();
        let add = |db: &mut Db, n: &str| {
            edit::create_manual(db, &serde_json::json!({"type": "article-journal", "title": format!("attach_pdf {n}")})).unwrap()
        };
        let (k_old, k_new) = (add(&mut db, "old"), add(&mut db, "new"));
        let pdf = tmp.join("a.pdf");
        std::fs::write(&pdf, pdftools::minimal_pdf("Manual zebracorn attachment text", None)).unwrap();
        let old = edit::attach_auto(&db, &k_old, &pdf).unwrap().id;
        let db = Mutex::new(db);
        super::attach_pdf_now(&db, &k_new, &pdf.to_string_lossy()).unwrap();
        let db = db.into_inner().unwrap();
        let new: i64 = db
            .conn
            .query_row("SELECT a.id FROM attachments a JOIN items i ON i.id = a.item_id WHERE i.key = ?1", [&k_new], |r| r.get(0))
            .unwrap();
        type Row = (String, String, String, Option<i64>, String, String, Option<String>);
        let row = |id: i64| -> Row {
            db.conn
                .query_row(
                    "SELECT a.kind, a.mode, a.md5, a.page_count, f.text, f.status, f.md5 FROM attachments a JOIN fulltext f ON f.attachment_id = a.id WHERE a.id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
                )
                .unwrap()
        };
        let want = row(old);
        assert!(want.4.contains("zebracorn") && want.5 == "ok" && want.3 == Some(1), "{want:?}");
        assert_eq!(row(new), want);
        // エラーの文も同じ（無いファイル・DB 自身のファイル）
        for bad in [tmp.join("none.pdf"), dbp.clone()] {
            let before = edit::attach_auto(&db, &k_old, &bad).unwrap_err().to_string();
            let db = Mutex::new(Db::open(&dbp).unwrap());
            assert_eq!(super::attach_pdf_now(&db, &k_new, &bad.to_string_lossy()).unwrap_err(), before);
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// attach_pdf はロックの外で抽出する本体（attach_pdf_now）を通る（ロックの中の edit::attach_auto に戻さない）
    #[test]
    fn command_uses_the_unlocked_path() {
        use super::parity::{fn_body, squash};
        let cmd = squash(&fn_body(include_str!("main.rs"), "async fn attach_pdf("));
        assert!(cmd.contains("blocking(move||attach_pdf_now(&db,&key,&path)).await"), "{cmd}");
        let body = squash(&fn_body(include_str!("main.rs"), "fn attach_pdf_now("));
        assert!(body.contains("edit::attach_auto_unlocked(||lock(db),key,Path::new(path))?") && !body.contains("attach_auto("), "{body}");
    }
}

#[cfg(test)]
mod chat_auth {
    use super::wiki_chat_auth_action;
    use serde_json::json;

    /// wiki_chat_auth の action は login だけ（ログアウトは画面から呼ばない = ターミナルを案内する）。logout と未知の値は Err・
    /// 無い / 文字列でないは必須の文（ja / en）
    #[test]
    fn only_login_is_accepted() {
        for en in [false, true] {
            assert_eq!(wiki_chat_auth_action(&json!({"action": "login"}), en), Ok(()));
        }
        for bad in ["logout", "LOGIN", ""] {
            assert_eq!(
                wiki_chat_auth_action(&json!({"action": bad}), false),
                Err(format!("wiki_chat_auth: action は login だけです: {bad}"))
            );
            assert_eq!(wiki_chat_auth_action(&json!({"action": bad}), true), Err(format!("wiki_chat_auth: action must be login: {bad}")));
        }
        for a in [json!({}), json!({"action": 1})] {
            assert_eq!(wiki_chat_auth_action(&a, false), Err("wiki_chat_auth: action（login）は必須です".into()));
            assert_eq!(wiki_chat_auth_action(&a, true), Err("wiki_chat_auth: action (login) is required".into()));
        }
    }

    /// ログアウトを実行する道は GUI のコードに残っていない（wiki_chat_auth は action を検査してからログインだけ・`claude auth logout` を
    /// 起こす関数は無い）
    #[test]
    fn no_logout_path_is_left() {
        use super::parity::{fn_body, squash};
        let cmd = squash(&fn_body(include_str!("main.rs"), "async fn wiki_chat_auth("));
        assert!(cmd.starts_with(&squash("async fn wiki_chat_auth(")) && cmd.contains("wiki_chat_auth_action(&obj_args(args),ui_en(&app))?;"), "{cmd}");
        assert!(cmd.contains("wiki_chat::open_login_terminal(&cli,&app_dir)?;") && !cmd.contains("logout"), "{cmd}");
        for src in [include_str!("main.rs"), include_str!("wiki_chat.rs"), include_str!("wiki_chat_gui.rs"), include_str!("wiki_gui.rs")] {
            let code = src.split("#[cfg(test)]").next().unwrap();
            let code: String = code.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
            assert!(!code.contains("cli_logout") && !code.contains("\"logout\""), "ログアウトの道が残っている");
        }
    }
}

#[cfg(test)]
mod capabilities {
    /// 許可する権限（capabilities/default.json）と、使う所（根拠）。csp: null で vault の HTML を描くので、使う物だけを許す
    const ALLOWED: &[(&str, &str)] = &[
        ("core:event:allow-listen", "@tauri-apps/api/event の listen・webview の onDragDropEvent（どちらも plugin:event|listen）"),
        ("core:event:allow-unlisten", "listen / onDragDropEvent の戻り値の解除（plugin:event|unlisten）"),
        ("core:webview:allow-internal-toggle-devtools", "debug ビルドで Tauri が差し込む ⌘⌥I（webview/scripts/toggle-devtools.js）。release では差し込まれない"),
        ("dialog:allow-open", "@tauri-apps/plugin-dialog の open（PDF・CSL・移行元・画像の選択）"),
        ("dialog:allow-save", "@tauri-apps/plugin-dialog の save（移行の結果 JSON）"),
    ];

    /// 画面が import してよい Tauri の JS API と、それが要る権限（新しく使うなら権限と ALLOWED も足す）。
    /// api/core の invoke（このアプリの command）・convertFileSrc（asset URL を作るだけ）は権限が要らない
    const UI_API: &[(&str, &str, &[&str])] = &[
        ("api/core", "invoke", &[]),
        ("api/core", "convertFileSrc", &[]),
        ("api/core", "InvokeArgs", &[]),
        ("api/event", "listen", &["core:event:allow-listen", "core:event:allow-unlisten"]),
        ("api/webview", "getCurrentWebview", &["core:event:allow-listen", "core:event:allow-unlisten"]),
        ("plugin-dialog", "open", &["dialog:allow-open"]),
        ("plugin-dialog", "save", &["dialog:allow-save"]),
    ];

    #[test]
    fn only_the_used_permissions_are_granted() {
        let v: serde_json::Value = serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        let got: Vec<&str> = v["permissions"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        assert_eq!(got, ALLOWED.iter().map(|a| a.0).collect::<Vec<_>>());
        assert_eq!(v["windows"], serde_json::json!(["main"]));
    }

    /// app/src（.ts / .svelte / .js / .mts / .cts）の Tauri の import は UI_API の物だけ（引用符は " と ' のどちらも。逆引用符
    /// `` `@tauri-apps/…` ``（動的な import(`…`) の形）は名前付きでない import として落とす）。webview は onDragDropEvent だけ・
    /// __TAURI_INTERNALS__ や `plugin:` の直の invoke は無い（権限の表と画面が食い違ったら落ちる）。
    /// 限界: getCurrentWebview() を変数に入れてから呼ぶ別のメソッドは見えない（`getCurrentWebview().x` の形だけを見る）・
    /// .mjs は試験と試験の土台（test-harness.mjs と *.test.mjs = 配布物に入らない）なので見ない
    #[test]
    fn the_ui_uses_only_covered_apis() {
        fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if matches!(p.extension().and_then(|x| x.to_str()), Some("ts" | "svelte" | "js" | "mts" | "cts")) {
                    out.push(p);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        assert!(files.len() > 50, "走査が壊れている: {}", files.len());
        let named = regex::Regex::new(r#"import\s*\{([^}]*)\}\s*from\s*["']@tauri-apps/([^"']+)["']"#).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for f in &files {
            let s = std::fs::read_to_string(f).unwrap();
            let all = ["\"@tauri-apps/", "'@tauri-apps/", "`@tauri-apps/"].iter().map(|q| s.matches(q).count()).sum::<usize>();
            let mut ok = 0;
            for c in named.captures_iter(&s) {
                ok += 1;
                for n in c[1].split(',').map(str::trim).filter(|n| !n.is_empty()) {
                    let n = n.trim_start_matches("type ").split_whitespace().next().unwrap();
                    let (_, _, perms) = UI_API
                        .iter()
                        .find(|(m, name, _)| *m == &c[2] && *name == n)
                        .unwrap_or_else(|| panic!("{}: @tauri-apps/{} の {n} は表に無い", f.display(), &c[2]));
                    for p in *perms {
                        assert!(ALLOWED.iter().any(|a| a.0 == *p), "{}: {n} に要る {p} を許していない", f.display());
                    }
                    seen.insert(format!("{}:{n}", &c[2]));
                }
            }
            assert_eq!(ok, all, "{}: 名前付きでない @tauri-apps の import がある", f.display());
            assert!(!s.contains("__TAURI_INTERNALS__") && !s.contains("\"plugin:"), "{}: Tauri の内部を直に呼ぶ", f.display());
            for c in regex::Regex::new(r"getCurrentWebview\(\)\s*\.(\w+)").unwrap().captures_iter(&s) {
                assert_eq!(&c[1], "onDragDropEvent", "{}", f.display());
            }
        }
        // 許している権限は、画面が使う API（か Tauri が差し込む devtools の ⌘⌥I）のどれかが要る物だけ
        for (perm, _) in ALLOWED {
            let used = *perm == "core:webview:allow-internal-toggle-devtools"
                || UI_API.iter().any(|(m, n, ps)| ps.contains(perm) && seen.contains(&format!("{m}:{n}")));
            assert!(used, "{perm} を使う所が無い = 外せる: {seen:?}");
        }
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

    /// open に渡すフォルダと toast: 今の保存先を開く・開く間はロックを離す・Drive 未検出は storage + toast（ja / en）で
    /// Drive 側に何も作らない。open は差し替え = Finder は開かない。OPENREF_MOUNT を読むのはこの crate の試験でここだけ
    #[test]
    fn reveal_storage_opens_the_current_folder() {
        use std::path::{Path, PathBuf};
        let top = std::env::temp_dir().join(format!(
            "openref-reveal-{}-{}",
            std::process::id(),
            openref_core::keys::new_key()
        ));
        let (app, mount, none) = (top.join("app"), top.join("mount"), top.join("none"));
        let db = openref_core::db::Db::open(&app.join("openref.sqlite")).unwrap();
        std::fs::create_dir(app.join("storage")).unwrap();
        std::fs::create_dir_all(mount.join("All Papers")).unwrap();
        let db = std::sync::Mutex::new(db);
        let reveal = |mode: &str, mount_env: &Path, en: bool| {
            db.lock()
                .unwrap()
                .set_setting("pdf_write_mode", mode)
                .unwrap();
            std::env::set_var("OPENREF_MOUNT", mount_env);
            let mut opened = None;
            let toast = super::reveal_storage_with(&db, en, |p| {
                assert!(db.try_lock().is_ok(), "open の間はロックを離す");
                opened = Some(PathBuf::from(p));
                Ok(())
            });
            (opened.expect("open を呼ぶ"), toast)
        };
        assert_eq!(
            reveal("managed", &mount, false),
            (app.join("storage"), None)
        );
        assert_eq!(
            reveal("drive", &mount, false),
            (mount.join("All Papers"), None)
        );
        let ja = "Google Drive が見つからないため、新しい PDF は Mac 内に保存されています";
        let en = "Google Drive was not found, so new PDFs are being saved on this Mac";
        assert_eq!(
            reveal("drive", &none, false),
            (app.join("storage"), Some(ja.to_string()))
        );
        assert_eq!(
            reveal("drive", &none, true),
            (app.join("storage"), Some(en.to_string()))
        );
        assert!(!none.exists(), "見るだけ = Drive 側に何も作らない");
        std::env::remove_var("OPENREF_MOUNT");
        std::fs::remove_dir_all(&top).ok();
    }

    /// 保存先を開けなければ toast に理由が出て、親のパスは出ない（Drive のパスはアカウント名を含む）。
    /// DB の置き場を消してあるので open は失敗する = Finder は開かない
    #[test]
    #[cfg(target_os = "macos")]
    fn reveal_storage_failure_drops_the_path() {
        let top = std::env::temp_dir().join(format!(
            "openref-reveal-{}-{}",
            std::process::id(),
            openref_core::keys::new_key()
        ));
        let dir = top.join("GoogleDrive-account").join("OpenRefData");
        let db = openref_core::db::Db::open(&dir.join("openref.sqlite")).unwrap();
        std::fs::remove_dir_all(&top).unwrap();
        let db = std::sync::Mutex::new(db);
        for (en, prefix) in [
            (false, "✗ 保存先のフォルダを開けません: "),
            (true, "✗ Could not open the storage folder: "),
        ] {
            let msg = super::reveal_storage_folder(&db, en).unwrap();
            assert!(msg.starts_with(prefix), "{msg}");
            assert!(msg.contains("OpenRefData"), "理由はファイル名まで: {msg}");
            assert!(
                !msg.contains("GoogleDrive-account"),
                "親のパスを出さない: {msg}"
            );
        }
    }

    /// Finder で表示（ワークスペースの木・本棚のカード）に失敗したときの文は、ファイル名までで親のパスを出さない
    /// （Drive のパスはアカウント名を含む）。存在しないパスなので Finder は開かない
    #[test]
    #[cfg(target_os = "macos")]
    fn reveal_failure_drops_the_path() {
        let p = std::path::Path::new("/nonexistent/GoogleDrive-account/openref-reveal-test.pdf");
        let e = super::reveal_in_finder(p).unwrap_err();
        assert!(e.contains("openref-reveal-test.pdf"), "理由はファイル名まで: {e}");
        assert!(!e.contains("GoogleDrive-account") && !e.contains('/'), "親のパスを出さない: {e}");
        assert!(!e.contains('\n'));
    }

    /// open に渡す引数の並び（open は実行しない）: Finder で表示 = `-R <path>`・ログイン = `-a Terminal <script>`
    #[test]
    fn open_arguments() {
        use std::ffi::OsStr;
        let p = std::path::Path::new("/x/y.pdf");
        assert_eq!(super::reveal_args(p), [OsStr::new("-R"), OsStr::new("/x/y.pdf")]);
        let s = std::path::Path::new("/a/claude-login.command");
        assert_eq!(
            super::wiki_chat::login_open_args(s),
            [OsStr::new("-a"), OsStr::new("Terminal"), OsStr::new("/a/claude-login.command")]
        );
    }

    /// `/usr/bin/open` を起こすのは open_cmd の 1 か所。Finder で表示はどれも reveal_in_finder を通る（= パスを落とす）
    #[test]
    fn open_runs_in_one_place() {
        use super::parity::{fn_body, squash};
        let body = |s: &'static str| s.split("#[cfg(test)]").next().unwrap();
        let srcs = [
            body(include_str!("main.rs")),
            body(include_str!("migration.rs")),
            body(include_str!("shelf_gui.rs")),
            body(include_str!("wiki_asset.rs")),
            body(include_str!("wiki_chat.rs")),
            body(include_str!("wiki_chat_gui.rs")),
            body(include_str!("wiki_chat_live.rs")),
            body(include_str!("wiki_chat_log.rs")),
            body(include_str!("wiki_gui.rs")),
        ];
        let n: usize = srcs.iter().map(|s| s.matches("\"/usr/bin/open\"").count()).sum();
        assert_eq!(n, 1, "/usr/bin/open が {n} か所");
        assert!(squash(&fn_body(srcs[0], "pub(crate) fn open_cmd")).contains("Command::new(\"/usr/bin/open\")"));
        assert!(squash(&fn_body(srcs[0], "pub(crate) fn reveal_in_finder(")).contains("open_cmd(&reveal_args(path))"));
        assert!(squash(&fn_body(srcs[4], "pub fn open_login_terminal(")).contains("crate::open_cmd(&login_open_args(&p))"));
        for (src, head, want) in [
            (srcs[0], "async fn wiki_open_path(", "if reveal { return reveal_in_finder(&abs).map(|()| true); }"),
            (srcs[2], "pub(crate) fn shelf_reveal_book(", "reveal_in_finder(&p)"),
            (srcs[4], "pub fn open_login_terminal(", ".map_err(|e| format!(\"open -a Terminal: {}\", crate::drop_dir_part(e)))"),
        ] {
            assert!(squash(&fn_body(src, head)).contains(&squash(want)), "{head}: {want}");
        }
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

#[cfg(test)]
mod cite {
    use openref_core::resolver::{AddInput, Resolved};
    use serde_json::json;
    use std::sync::Mutex;

    fn resolved(csl: serde_json::Value, retracted: Option<&str>) -> Resolved {
        Resolved { csl, retracted: retracted.map(String::from), source: "pubmed", warnings: vec![] }
    }

    /// 蔵書: DOI だけの論文 A・PMID と DOI の論文 B（撤回）
    fn db() -> (Mutex<openref_core::Db>, String, String) {
        let mut db = openref_core::Db::open_in_memory().unwrap();
        let a = openref_core::ingest::add_item(
            &mut db,
            &resolved(json!({"type": "article-journal", "title": "Alpha", "DOI": "10.1000/ABC-1", "issued": {"date-parts": [[2020]]}}), None),
        )
        .unwrap()
        .key;
        let b = openref_core::ingest::add_item(
            &mut db,
            &resolved(json!({"type": "article-journal", "title": "Beta", "DOI": "10.2000/b", "PMID": "123456", "issued": {"date-parts": [[2021]]}}), Some("Retracted")),
        )
        .unwrap()
        .key;
        (Mutex::new(db), a, b)
    }

    #[test]
    fn normalizes_doi_urls() {
        let n = super::cite_id_input;
        assert_eq!(n("  https://doi.org/10.1000%2FABC-1?utm=x#top "), "https://doi.org/10.1000/ABC-1");
        assert_eq!(n("https://dx.doi.org/10.1000/abc-1#frag"), "https://dx.doi.org/10.1000/abc-1");
        assert_eq!(n("<https://doi.org/10.1000/abc-1>"), "https://doi.org/10.1000/abc-1");
        assert_eq!(n("doi: 10.1000/abc-1"), "10.1000/abc-1");
        assert_eq!(n("DOI:10.1000/abc-1"), "10.1000/abc-1");
        assert_eq!(n("https://pubmed.ncbi.nlm.nih.gov/123456/?from=x"), "https://pubmed.ncbi.nlm.nih.gov/123456/?from=x");
        assert_eq!(n("liver"), "liver");
        // 大小・前置き・末尾の句読点（parse_input は区別する = ここで揃える）
        assert_eq!(n("HTTPS://DOI.ORG/10.1000/ABC-1"), "https://doi.org/10.1000/ABC-1");
        assert_eq!(n("https://Pubmed.NCBI.nlm.nih.gov/123456/"), "https://pubmed.ncbi.nlm.nih.gov/123456/");
        assert_eq!(n("Pmid: 123456"), "123456");
        assert_eq!(n("10.1000/abc-1."), "10.1000/abc-1");
        assert_eq!(n("（https://doi.org/10.1000/abc-1）。"), "（https://doi.org/10.1000/abc-1）", "全角の括弧は外さない（URL ではない = 照合しない）");
        assert_eq!(n("https://doi.org/10.1016/S0140-6736(20)30183-5)."), "https://doi.org/10.1016/S0140-6736(20)30183-5", "DOI の中の括弧は残す");
        // 先頭が非 ASCII でも panic しない（byte 4 が字の境目でない）
        assert_eq!(n("日本語の題名"), "日本語の題名");
        assert_eq!(n("é: x"), "é: x");
        assert_eq!(n("ドイ:"), "ドイ:");
    }

    /// 照合: DOI・doi.org の URL（%xx・?・#・大小）・PubMed の URL・PMID は 1 件に・蔵書に無い識別子は item なし・語は None
    #[test]
    fn lookup_is_offline_and_structured() {
        let (db, a, b) = db();
        let db = db.lock().unwrap();
        let key = |s: &str| super::cite_lookup_in(&db, s).unwrap().expect(s).item.map(|i| i.key);
        assert_eq!(key("10.1000/abc-1"), Some(a.clone()));
        assert_eq!(key("https://doi.org/10.1000%2Fabc-1?utm_source=x#sec"), Some(a.clone()));
        assert_eq!(key("doi:10.1000/ABC-1"), Some(a.clone()));
        assert_eq!(key("https://pubmed.ncbi.nlm.nih.gov/123456/"), Some(b.clone()));
        assert_eq!(key("PMID: 123456"), Some(b.clone()));
        assert_eq!(key("https://doi.org/10.2000/b"), Some(b.clone()));
        assert_eq!(key("10.9999/none"), None);
        assert!(super::cite_lookup_in(&db, "liver cirrhosis").unwrap().is_none());
        assert!(super::cite_lookup_in(&db, "日本語の題名").unwrap().is_none(), "非 ASCII の語で panic しない");
        assert_eq!(key("HTTPS://DOI.ORG/10.1000%2FABC-1"), Some(a.clone()), "scheme と host の大文字");
        assert_eq!(key("https://doi.org/10.1000/abc-1."), Some(a.clone()), "末尾の句読点");
        assert_eq!(key("Pmid: 123456"), Some(b.clone()), "PMID の前置きの大小");
        assert!(super::cite_lookup_in(&db, "https://example.com/x").unwrap().is_none());
        let hit = super::cite_lookup_in(&db, "123456").unwrap().unwrap().item.unwrap();
        assert!(hit.retracted, "撤回の印が候補に出る");
        assert_eq!(hit.year, Some(2021));
        // ゴミ箱の item は照合しない（add_item の重複判定と同じ）
        openref_core::edit::trash(&db, &a).unwrap();
        assert_eq!(key("10.1000/abc-1"), None);
    }

    /// 登録: 既にあれば resolver を呼ばない・PubMed の URL で引いた書誌の DOI が既存と一致 = 既存の key（増やさない）・
    /// 無ければ登録して created = true・resolver の失敗は Err（何も書かない）
    #[test]
    fn register_returns_existing_key_structurally() {
        let (db, a, b) = db();
        let count = || -> i64 { db.lock().unwrap().conn.query_row("SELECT count(*) FROM items", [], |r| r.get(0)).unwrap() };
        let never = |_: &AddInput| -> openref_core::Result<Resolved> { panic!("蔵書にあるのに resolver を呼んだ") };
        let r = super::cite_register_with(&db, "https://doi.org/10.2000%2Fb", never).unwrap();
        assert_eq!((r.item.key.as_str(), r.created), (b.as_str(), false));
        // A は DOI だけ。PubMed の URL（PMID 999）を貼った = 照合では見つからない → resolver の書誌の DOI が A と一致 → Duplicate → A
        let pubmed = |p: &AddInput| {
            assert_eq!(p, &AddInput::Pmid("999".into()));
            Ok(resolved(json!({"type": "article-journal", "title": "Alpha", "DOI": "10.1000/abc-1", "PMID": "999"}), None))
        };
        let r = super::cite_register_with(&db, "https://pubmed.ncbi.nlm.nih.gov/999/", pubmed).unwrap();
        assert_eq!((r.item.key.as_str(), r.created), (a.as_str(), false));
        assert_eq!(count(), 2, "重複で増やさない");
        let fresh = |_: &AddInput| {
            let mut r = resolved(json!({"type": "article-journal", "title": "Gamma", "DOI": "10.3000/c"}), None);
            r.warnings.push("w".into());
            Ok(r)
        };
        let r = super::cite_register_with(&db, "10.3000/c", fresh).unwrap();
        assert!(r.created);
        assert_eq!(r.item.title, "Gamma");
        assert_eq!(r.warnings, vec!["w".to_string()]);
        assert_eq!(count(), 3);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["created"], true);
        assert!(v["key"].is_string() && v.get("item").is_none(), "一覧の行のキーが上位にある: {v}");
        let fail = |_: &AddInput| -> openref_core::Result<Resolved> { Err(openref_core::CoreError::Http("down".into())) };
        assert!(super::cite_register_with(&db, "10.4000/d", fail).unwrap_err().contains("down"));
        assert!(super::cite_register_with(&db, "liver", never).is_err(), "識別子でなければ resolver を呼ばない");
        assert_eq!(count(), 3);
    }

    /// 実 DB（本番の .backup の写し・pdf_write_mode = managed に直した物）で照合と登録。登録は本物の PubMed / Crossref に出る。
    /// OPENREF_DB=<写し> OPENREF_CITE_HAVE=<蔵書にある論文の PubMed の URL> OPENREF_CITE_NEW=<蔵書に無い論文の doi.org の URL>
    /// OPENREF_CITE_NEW_PMID=<その PMID> cargo test -p openref-app -- --ignored cite::real_db
    #[test]
    #[ignore]
    fn real_db_lookup_and_register() {
        let env = |k: &str| std::env::var(k).unwrap();
        let db = Mutex::new(openref_core::Db::open(std::path::Path::new(&env("OPENREF_DB"))).unwrap());
        let count = || -> i64 { db.lock().unwrap().conn.query_row("SELECT count(*) FROM items WHERE deleted_at IS NULL", [], |r| r.get(0)).unwrap() };
        let n0 = count();
        let have = super::cite_lookup_in(&db.lock().unwrap(), &env("OPENREF_CITE_HAVE")).unwrap().unwrap().item.expect("蔵書にある");
        eprintln!("have: {} {} {:?}", have.key, have.title, have.year);
        let new = env("OPENREF_CITE_NEW");
        assert!(super::cite_lookup_in(&db.lock().unwrap(), &new).unwrap().unwrap().item.is_none(), "蔵書に無いはず");
        let a = super::cite_register_with(&db, &new, openref_core::resolver::resolve).unwrap();
        eprintln!("added: {} {} {:?} created={} warnings={:?}", a.item.key, a.item.title, a.item.year, a.created, a.warnings);
        assert!(a.created);
        assert_eq!(count(), n0 + 1);
        let again = super::cite_register_with(&db, &new, |_: &openref_core::resolver::AddInput| -> openref_core::Result<openref_core::resolver::Resolved> { panic!("照合で見つかるはず") }).unwrap();
        assert_eq!((again.item.key.as_str(), again.created), (a.item.key.as_str(), false));
        let pm = format!("https://pubmed.ncbi.nlm.nih.gov/{}/", env("OPENREF_CITE_NEW_PMID"));
        let by_pm = super::cite_register_with(&db, &pm, openref_core::resolver::resolve).unwrap();
        assert_eq!((by_pm.item.key.as_str(), by_pm.created), (a.item.key.as_str(), false), "同じ論文の PubMed の URL で増やさない");
        assert_eq!(count(), n0 + 1);
        let hit = super::cite_lookup_in(&db.lock().unwrap(), &format!("{new}?utm_source=x#abs")).unwrap().unwrap().item.unwrap();
        assert_eq!(hit.key, a.item.key);
    }
}
