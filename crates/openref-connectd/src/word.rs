//! Word 連携（OpenRef for Word = Startup テンプレート OpenRef.dotm + OpenRef Dialog.app）の HTTPS 配信。
//! 自己署名 cert（rcgen 生成・初回のみ user が Keychain 信頼）で https://localhost:23120 に
//! ダイアログのページ（引用の検索・設定・通知）と蔵書 API を配信する。VBA の API（/word/v2/*）は word_v2.rs。
//! アセットはビルド時にバイナリへ埋め込む（word/ ディレクトリが SSOT）。ページの fetch は同一オリジン（/api/*）。

use crate::St;
use axum::extract::{Query, State};
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
        // ダイアログ（引用の検索・設定・通知。OpenRef Dialog.app の WKWebView が開く）
        .route("/word/cite-dialog.html", get(|| async { asset(word_asset!("cite-dialog.html"), HTML) }))
        .route("/word/cite-dialog.js", get(|| async { asset(word_asset!("cite-dialog.js"), JS) }))
        .route("/word/settings-dialog.html", get(|| async { asset(word_asset!("settings-dialog.html"), HTML) }))
        .route("/word/settings-dialog.js", get(|| async { asset(word_asset!("settings-dialog.js"), JS) }))
        .route("/word/notice.html", get(|| async { asset(word_asset!("notice.html"), HTML) }))
        .route("/word/notice.js", get(|| async { asset(word_asset!("notice.js"), JS) }))
        .route("/word/dialog-common.js", get(|| async { asset(word_asset!("dialog-common.js"), JS) }))
        .route("/word/dialog.css", get(|| async { asset(word_asset!("dialog.css"), CSS) }))
        // 通知ダイアログのロゴ（PNG。Cache-Control ヘッダは付けない）
        .route("/word/assets/icon-80.png", get(|| async { ([("content-type", "image/png")], word_asset!("assets/icon-80.png")) }))
        .route("/api/status", get(api_status))
        .route("/api/styles", get(api_styles))
        .route("/api/search", get(api_search))
        .route("/api/items", post(api_items))
        .with_state(st.clone())
        // Startup テンプレート版（OpenRef.dotm・VBA）の API
        .merge(crate::word_v2::router(st, word_port, app_dir))
}

/// ページ・JS・CSS は no-store（connectd を更新したら次に開くダイアログから新しいものが動く）
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

/// 診断（VBA の ORCmd の Show Diagnostics）: デーモンの版・蔵書件数（ゴミ箱外）・DB の migration 版。
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
