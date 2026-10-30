//! Word 連携（OpenRef for Word = Startup テンプレート OpenRef.dotm + OpenRef Dialog.app）の HTTPS 配信。
//! 自己署名 cert（rcgen 生成・初回のみ user が Keychain 信頼）で https://localhost:23120 に
//! ダイアログのページ（引用の検索・設定・通知）と蔵書 API を配信する。VBA の API（/word/v2/*）は word_v2.rs。
//! アセットはビルド時にバイナリへ埋め込む（word/ ディレクトリが SSOT）。ページの fetch は同一オリジン（/api/*）。

use crate::St;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
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

/// DB が読めないときの応答 = 500 + `{error}`（ダイアログの「検索できません: HTTP 500」に出る。/api/status だけは別扱い）。
/// 「該当なし」「蔵書に見つかりません」「0 件」に化けさせない（CLAUDE.md §4-3）
fn db_error(what: &str, e: impl std::fmt::Display) -> Response {
    let msg = format!("{what}: {e}");
    eprintln!("openref-connectd: {msg}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg }))).into_response()
}

/// 診断（VBA の ORCmd の Show Diagnostics）: デーモンの版・蔵書件数（ゴミ箱外）・DB の migration 版。
/// DB ロックは件数と user_version を読む間だけ。
/// VBA の GetRaw は curl -f で、500 だと本文（原因）が捨てられて「接続できません」と誤診される → ここだけは DB が
/// 読めなくても 200 を返し、読めなかった値を null・`"ok": false`・`"error"` で示す（診断はこの JSON をそのまま表示する）。
/// 健全時の応答は従来とバイト一致
async fn api_status(State(st): State<Arc<St>>) -> Response {
    let (items, db_version) = {
        let db = st.db();
        (
            crate::live_item_count(&db).map_err(|e| e.to_string()),
            db.conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string()),
        )
    };
    let mut body = json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "items": items.as_ref().ok(),
        "db_version": db_version.as_ref().ok(),
    });
    let errs: Vec<String> = [("cannot read item count", items.err()), ("cannot read db version", db_version.err())]
        .into_iter()
        .filter_map(|(what, e)| e.map(|e| format!("{what}: {e}")))
        .collect();
    if !errs.is_empty() {
        // 診断は VBA が MacJapanese で読んで表示する（CLAUDE.md §6）= 印字可能な ASCII だけにする
        // （非 ASCII・制御文字・JSON で \ が付く " と、MacJapanese で化ける \ ~ は ? に）
        let msg: String = errs
            .join(" / ")
            .chars()
            .map(|c| if (c.is_ascii_graphic() || c == ' ') && !matches!(c, '\\' | '~' | '"') { c } else { '?' })
            .collect();
        eprintln!("openref-connectd: /api/status: {msg}");
        body["ok"] = json!(false);
        body["error"] = json!(msg);
    }
    Json(body).into_response()
}

/// Word のスタイル一覧 = 同梱 4 種 + app で導入したもの。current / doi_always = app の既定（新規文書の初期値）
async fn api_styles(State(st): State<Arc<St>>) -> impl IntoResponse {
    let db = st.db();
    let dir = citestyle::styles_dir(&db);
    let p = match citestyle::prefs(&db) {
        Ok(p) => p,
        // 既定スタイルに畳まない（ダイアログが app の既定と違うものを「既定」と表示する）
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    (StatusCode::OK, Json(json!({
        "styles": citestyle::installed(&dir),
        "current": p["style"],
        "doi_always": p["doi_always"],
    })))
}

async fn api_search(
    State(st): State<Arc<St>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let query = q.get("q").cloned().unwrap_or_default();
    let db = st.db();
    let listed = search::list_items(
        &db,
        &search::ListFilter {
            search: if query.trim().is_empty() { None } else { Some(query) },
            limit: 20,
            // 引用候補は書誌（title / 著者 / 誌名 / abstract）だけで引く。本文 hit（その著者を引用しているだけの論文）は
            // 抜粋を出せない Word 側では誤引用のもとになるので外す
            metadata_only: true,
            ..Default::default()
        },
    );
    let rows = match listed {
        Ok(r) => r,
        Err(e) => return db_error("検索できません", e),
    };
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
    Json(json!({ "items": items })).into_response()
}

async fn api_items(State(st): State<Arc<St>>, body: axum::body::Bytes) -> Response {
    let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let keys: Vec<String> = payload
        .get("keys")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let db = st.db();
    let mut out = serde_json::Map::new();
    for k in keys.iter().take(500) {
        // 蔵書に無い key（Ok(None)）は応答に載せない = ダイアログが「蔵書に見つかりません」と出す。DB のエラーは 500
        match search::get_by_key(&db, k) {
            Ok(Some(item)) => {
                out.insert(k.clone(), item.csl);
            }
            Ok(None) => {}
            Err(e) => return db_error("蔵書を読めません", e),
        }
    }
    Json(Value::Object(out)).into_response()
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

#[cfg(test)]
mod tests {
    use super::*;
    use openref_core::{ingest, resolver, Db};
    use std::io::{Read, Write};
    use std::sync::Mutex;

    fn test_st() -> Arc<St> {
        Arc::new(St {
            db: Mutex::new(Db::open_in_memory().unwrap()),
            sessions: Mutex::new(Default::default()),
        })
    }

    async fn serve(st: Arc<St>) -> u16 {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = router(st, port, std::path::Path::new("/nonexistent"));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        port
    }

    /// 素の HTTP/1.1 を 1 往復（TLS なしの router に直接）。本文は解かず生のまま返す（JSON の部分一致だけ見る）。
    /// 要求は 1 回の write で送る
    async fn call(port: u16, method: &'static str, path: String, body: String) -> (u16, String) {
        tokio::task::spawn_blocking(move || {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let req = format!(
                "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            s.write_all(req.as_bytes()).unwrap();
            let mut raw = Vec::new();
            let _ = s.read_to_end(&mut raw);
            let raw = String::from_utf8_lossy(&raw).to_string();
            let (head, rest) = raw.split_once("\r\n\r\n").unwrap();
            (head.split(' ').nth(1).unwrap().parse().unwrap(), rest.to_string())
        })
        .await
        .unwrap()
    }

    /// /api/status の応答のキー集合（整列済み）。値は各テストが見る
    fn status_keys(b: &str) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(b).unwrap();
        let mut ks: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        ks.sort();
        ks
    }

    fn add_one(st: &St) -> String {
        let csl = json!({
            "type": "article-journal",
            "title": "Pancreatic cancer: changing epidemiology",
            "author": [{"family": "Stoffel", "given": "Elena M"}],
            "container-title": "Gastroenterology",
            "issued": {"date-parts": [[2023, 2]]},
        });
        let mut db = st.db();
        ingest::add_item(&mut db, &resolver::Resolved { csl, retracted: None, source: "test", warnings: vec![] }).unwrap().key
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_ok_on_healthy_db() {
        let st = test_st();
        let key = add_one(&st);
        let port = serve(st).await;
        let (c, b) = call(port, "GET", "/api/status".into(), String::new()).await;
        assert_eq!(c, 200, "{b}");
        assert!(b.contains("\"items\":1") && b.contains("\"db_version\":") && !b.contains("\"db_version\":null") && b.contains("\"ok\":true") && !b.contains("error"), "{b}");
        assert_eq!(status_keys(&b), ["db_version", "items", "ok", "version"], "{b}");
        let (c, b) = call(port, "GET", "/api/search?q=pancreatic".into(), String::new()).await;
        assert_eq!(c, 200, "{b}");
        assert!(b.contains(&key), "{b}");
        // 該当なしは 200 の空配列（DB エラーとは別）
        let (c, b) = call(port, "GET", "/api/search?q=zzzznothing".into(), String::new()).await;
        assert_eq!(c, 200, "{b}");
        assert!(b.contains("\"items\":[]"), "{b}");
        // 蔵書に無い key は 200 で応答に載らない（ダイアログが「蔵書に見つかりません」と出す枝）
        let body = json!({"keys": [key, "no-such-key"]}).to_string();
        let (c, b) = call(port, "POST", "/api/items".into(), body).await;
        assert_eq!(c, 200, "{b}");
        assert!(b.contains(&key) && !b.contains("no-such-key"), "{b}");
        let (c, b) = call(port, "GET", "/api/styles".into(), String::new()).await;
        assert_eq!(c, 200, "{b}");
    }

    /// DB が読めないとき（テーブルが無い）は 200 の「該当なし」「0 件」でなく 500 + {error}
    #[tokio::test(flavor = "multi_thread")]
    async fn api_returns_500_when_db_is_broken() {
        let st = test_st();
        let key = add_one(&st);
        st.db().conn.execute_batch("DROP TABLE items_fts; DROP TABLE items;").unwrap();
        let port = serve(st).await;
        for (m, path, body) in [
            ("GET", "/api/search?q=pancreatic", String::new()),
            ("GET", "/api/search?q=", String::new()),
            ("POST", "/api/items", json!({"keys": [key]}).to_string()),
        ] {
            let (c, b) = call(port, m, path.into(), body).await;
            assert_eq!(c, 500, "{path}: {b}");
            assert!(b.contains("\"error\""), "{path}: {b}");
            assert!(!b.contains("\"items\":[]"), "{path}: {b}");
        }
        // /api/status だけは 200 のまま（VBA の curl -f が本文を捨てるため）。読めない値は null・ok:false・error で示す
        let (c, b) = call(port, "GET", "/api/status".into(), String::new()).await;
        assert_eq!(c, 200, "{b}");
        assert!(b.contains("\"ok\":false") && b.contains("\"error\":\"cannot read item count: ") && b.contains("\"items\":null"), "{b}");
        assert!(b.contains("\"version\"") && b.contains("\"db_version\":"), "{b}");
        assert_eq!(status_keys(&b), ["db_version", "error", "items", "ok", "version"], "{b}");
    }

    /// settings が読めない DB では /api/styles は既定に畳まず 500（prefs の Err）
    #[tokio::test(flavor = "multi_thread")]
    async fn api_styles_returns_500_when_settings_is_broken() {
        let st = test_st();
        st.db().conn.execute_batch("DROP TABLE settings;").unwrap();
        let port = serve(st).await;
        let (c, b) = call(port, "GET", "/api/styles".into(), String::new()).await;
        assert_eq!(c, 500, "{b}");
        assert!(b.contains("\"error\""), "{b}");
    }
}
