//! Word アドイン配信。
//! Office アドインは HTTPS 必須のため、自己署名 cert（rcgen 生成・初回のみ user が Keychain 信頼）で
//! https://localhost:23120 にアドイン一式（リボンの関数ファイル・ダイアログ・citeproc 込み）と蔵書 API を配信する。
//! アセットはビルド時にバイナリへ埋め込む（word/ ディレクトリが SSOT）。ダイアログは host と同一オリジン必須。

use crate::St;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use openref_core::{citestyle, model, search};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

macro_rules! word_asset {
    ($p:expr) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../word/", $p)).as_slice()
    };
}

const HTML: &str = "text/html; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";

/// word_port = 待受ポート（Startup テンプレート版のダイアログの URL・アプレットの起点に使う）。
/// app_dir = DB の置き場（ダイアログの器 `word/OpenRef Dialog.app` の既定の場所）
pub fn router(st: Arc<St>, word_port: u16, app_dir: &std::path::Path) -> Router {
    Router::new()
        // 挿入 > アドイン から開いたときの入口（使い方と診断のみ）
        .route("/word/taskpane.html", get(|| async { asset(word_asset!("taskpane.html"), HTML) }))
        // リボン「OpenRef」タブの関数ファイル（ExecuteFunction）と Office 層
        .route("/word/commands.html", get(|| async { asset(word_asset!("commands.html"), HTML) }))
        .route("/word/commands.js", get(|| async { asset(word_asset!("commands.js"), JS) }))
        .route("/word/doc.js", get(|| async { asset(word_asset!("doc.js"), JS) }))
        // ダイアログ（引用の検索・設定・通知）
        .route("/word/cite-dialog.html", get(|| async { asset(word_asset!("cite-dialog.html"), HTML) }))
        .route("/word/cite-dialog.js", get(|| async { asset(word_asset!("cite-dialog.js"), JS) }))
        .route("/word/settings-dialog.html", get(|| async { asset(word_asset!("settings-dialog.html"), HTML) }))
        .route("/word/settings-dialog.js", get(|| async { asset(word_asset!("settings-dialog.js"), JS) }))
        .route("/word/notice.html", get(|| async { asset(word_asset!("notice.html"), HTML) }))
        .route("/word/notice.js", get(|| async { asset(word_asset!("notice.js"), JS) }))
        .route("/word/dialog-common.js", get(|| async { asset(word_asset!("dialog-common.js"), JS) }))
        .route("/word/dialog.css", get(|| async { asset(word_asset!("dialog.css"), CSS) }))
        .route("/word/cite-core.js", get(|| async { asset(crate::word_v2::CITE_CORE_JS.as_bytes(), JS) }))
        .route("/word/citeproc.js", get(|| async { asset(crate::word_v2::CITEPROC_JS.as_bytes(), JS) }))
        .route("/word/locales/:file", get(locale))
        .route("/word/manifest.xml", get(|| async { asset(word_asset!("manifest.xml"), "application/xml") }))
        .route("/word/styles/:id", get(style))
        .route("/word/assets/:name", get(icon))
        .route("/api/status", get(api_status))
        .route("/api/styles", get(api_styles))
        .route("/api/search", get(api_search))
        .route("/api/items", post(api_items))
        .with_state(st.clone())
        // Startup テンプレート版（OpenRef.dotm・VBA）の API
        .merge(crate::word_v2::router(st, word_port, app_dir))
}

/// Word の WebView は taskpane を強くキャッシュするため no-store を明示する
/// （これが無いと connectd を更新しても古い JS が動き続ける）。アイコン（icon()）には使わない
fn asset(bytes: &'static [u8], ctype: &'static str) -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            ("content-type", ctype),
            ("cache-control", "no-store, must-revalidate"),
        ],
        bytes,
    )
}

/// CSL（同梱 4 種 + app で導入したスタイル。dependent は親で解決済み）。
/// 他機で作った文書が未導入スタイルを指していても、ここで取得してキャッシュする
async fn style(State(st): State<Arc<St>>, AxPath(id): AxPath<String>) -> impl IntoResponse {
    let id = id.trim_end_matches(".csl").to_string();
    let dir = citestyle::styles_dir(&st.db());
    let r = tokio::task::spawn_blocking(move || citestyle::style_xml(&dir, &id)).await;
    xml_or_404(r)
}

/// locale（"et al." 等の語彙。en-US は同梱・他は取得してキャッシュ）。`/word/locales/ja-JP.xml`
async fn locale(State(st): State<Arc<St>>, AxPath(file): AxPath<String>) -> impl IntoResponse {
    let lang = file.trim_end_matches(".xml").to_string();
    let dir = citestyle::styles_dir(&st.db());
    let r = tokio::task::spawn_blocking(move || citestyle::locale_xml(&dir, &lang)).await;
    xml_or_404(r)
}

fn xml_or_404(r: Result<openref_core::Result<String>, tokio::task::JoinError>) -> axum::response::Response {
    match r {
        Ok(Ok(x)) => (
            StatusCode::OK,
            [("content-type", "application/xml"), ("cache-control", "no-store, must-revalidate")],
            x,
        )
            .into_response(),
        Ok(Err(e)) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn icon(AxPath(name): AxPath<String>) -> impl IntoResponse {
    let body: Option<&'static [u8]> = match name.as_str() {
        "icon-16.png" => Some(word_asset!("assets/icon-16.png")),
        "icon-32.png" => Some(word_asset!("assets/icon-32.png")),
        "icon-80.png" => Some(word_asset!("assets/icon-80.png")),
        // リボンのボタン（scripts/gen-word-icons.cjs が生成）
        "add-edit-16.png" => Some(word_asset!("assets/add-edit-16.png")),
        "add-edit-32.png" => Some(word_asset!("assets/add-edit-32.png")),
        "add-edit-80.png" => Some(word_asset!("assets/add-edit-80.png")),
        "update-16.png" => Some(word_asset!("assets/update-16.png")),
        "update-32.png" => Some(word_asset!("assets/update-32.png")),
        "update-80.png" => Some(word_asset!("assets/update-80.png")),
        "settings-16.png" => Some(word_asset!("assets/settings-16.png")),
        "settings-32.png" => Some(word_asset!("assets/settings-32.png")),
        "settings-80.png" => Some(word_asset!("assets/settings-80.png")),
        _ => None,
    };
    match body {
        // アイコンだけは Cache-Control を付けない（MS docs の Image 要素: 画像の応答に CACHE-CONTROL があると
        // Office が既定の画像に差し替える）。更新は manifest 側の URL（?v=）を変えて行う
        Some(b) => (StatusCode::OK, [("content-type", "image/png")], b).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// 診断（Word の Show Diagnostics・案内ページ）: デーモンの版・蔵書件数（ゴミ箱外）・DB の migration 版。
/// DB ロックは件数と user_version を読む間だけ
async fn api_status(State(st): State<Arc<St>>) -> impl IntoResponse {
    let (items, db_version) = {
        let db = st.db();
        let n = crate::live_item_count(&db);
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
        (n, v)
    };
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "items": items,
        "db_version": db_version,
    }))
}

/// Word のスタイル一覧 = 同梱 4 種 + app で導入したもの。current / doi_always = app の既定（新規文書の初期値）
async fn api_styles(State(st): State<Arc<St>>) -> impl IntoResponse {
    let db = st.db();
    let dir = citestyle::styles_dir(&db);
    let p = citestyle::prefs(&db);
    Json(json!({
        "styles": citestyle::installed(&dir),
        "current": p["style"],
        "doi_always": p["doi_always"],
    }))
}

async fn api_search(
    State(st): State<Arc<St>>,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let query = q.get("q").cloned().unwrap_or_default();
    let db = st.db();
    let rows = search::list_items(
        &db,
        &search::ListFilter {
            search: if query.trim().is_empty() { None } else { Some(query) },
            limit: 20,
            // 引用候補は書誌（title / 著者 / 誌名 / abstract）だけで引く。本文 hit（その著者を引用しているだけの論文）は
            // 抜粋を出せない Word 側では誤引用のもとになるので外す
            metadata_only: true,
            ..Default::default()
        },
    )
    .unwrap_or_default();
    let items: Vec<Value> = rows
        .iter()
        .map(|li| {
            let c = &li.item.csl;
            json!({
                "key": li.item.key,
                "title": model::s(c, "title"),
                "first_author": model::first_author_family(c),
                "year": model::year(c),
                "journal": model::s(c, "container-title-short").or_else(|| model::s(c, "container-title")),
                "csl": c,
            })
        })
        .collect();
    Json(json!({ "items": items }))
}

async fn api_items(State(st): State<Arc<St>>, body: axum::body::Bytes) -> impl IntoResponse {
    let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let keys: Vec<String> = payload
        .get("keys")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let db = st.db();
    let mut out = serde_json::Map::new();
    for k in keys.iter().take(500) {
        if let Ok(Some(item)) = search::get_by_key(&db, k) {
            out.insert(k.clone(), item.csl);
        }
    }
    Json(Value::Object(out))
}

// ---------- TLS（自己署名 localhost cert） ----------

pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// cert/key を生成 or 再利用。置き場所は DB と同じ Application Support 配下。
pub fn ensure_cert(app_dir: &std::path::Path) -> Result<TlsPaths, String> {
    let dir = app_dir.join("certs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cert_path = dir.join("openref-localhost.pem");
    let key_path = dir.join("openref-localhost.key.pem");
    if cert_path.exists() && key_path.exists() {
        return Ok(TlsPaths { cert: cert_path, key: key_path });
    }
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
        .map_err(|e| e.to_string())?;
    std::fs::write(&cert_path, ck.cert.pem()).map_err(|e| e.to_string())?;
    std::fs::write(&key_path, ck.key_pair.serialize_pem()).map_err(|e| e.to_string())?;
    eprintln!(
        "openref-connectd: 自己署名 cert を生成しました → {}\n  Word で使うには一度だけ信頼登録:\n  security add-trusted-cert -r trustRoot -k \"$HOME/Library/Keychains/login.keychain-db\" \"{}\"",
        cert_path.display(),
        cert_path.display()
    );
    Ok(TlsPaths { cert: cert_path, key: key_path })
}
