//! openref-connectd — Zotero Connector 互換 API。
//! 既製 Zotero Connector 拡張（+ translator 群）を OpenRef の取込口として流用する。
//! localhost:23119 のみ bind。Word アドイン配信（HTTPS）も本デーモンが担う。
//!
//! プロトコルは逆行分析ベース（非公式）。未知エンドポイントは 200 {} で寛容に受け、
//! stderr にログして観察できるようにする。

mod jsc;
mod word;
mod word_v2;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use openref_core::{datadir, edit, ingest, model, pdffetch, pdfingest, resolver, search, tags, zotero, Db};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct St {
    pub db: Mutex<Db>,
    /// sessionID → (connector item id → openref item key)
    sessions: Mutex<HashMap<String, HashMap<String, String>>>,
}

impl St {
    /// DB のロック。poison（ロック中の panic）でも中身を取り出して続ける: rusqlite の Transaction は unwind で rollback
    /// されるので接続はそのまま使える（`lock().unwrap()` だと 1 回の panic で以後の全リクエストが失敗する）
    pub fn db(&self) -> std::sync::MutexGuard<'_, Db> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, HashMap<String, String>>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }
}

const ZHEADERS: [(&str, &str); 2] = [
    ("X-Zotero-Version", "7.0.15"),
    ("X-Zotero-Connector-API-Version", "3"),
];

fn zjson(status: StatusCode, v: Value) -> impl IntoResponse {
    (status, ZHEADERS, Json(v))
}

#[tokio::main]
async fn main() {
    // ureq(ring) と axum-server(aws-lc-rs) が両方 rustls の provider feature を有効化するため、
    // プロセス既定 provider を明示しないと rustls が panic する
    let _ = rustls::crypto::ring::default_provider().install_default();

    let db_path = match datadir::db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("openref-connectd: {e}");
            std::process::exit(1);
        }
    };
    let db = match Db::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("openref-connectd: DB open 失敗 {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };
    let st = Arc::new(St {
        db: Mutex::new(db),
        sessions: Mutex::new(HashMap::new()),
    });

    let app = Router::new()
        .route("/connector/ping", get(ping_get).post(ping_post))
        .route("/connector/getSelectedCollection", post(get_selected_collection))
        .route("/connector/saveItems", post(save_items))
        .route("/connector/saveAttachment", post(save_attachment))
        .route("/connector/saveStandaloneAttachment", post(save_standalone_attachment))
        .route("/connector/hasAttachmentResolvers", post(has_attachment_resolvers))
        .route("/connector/saveAttachmentFromResolver", post(save_attachment_from_resolver))
        .route("/connector/saveSnapshot", post(save_snapshot))
        .route("/connector/updateSession", post(update_session))
        .route("/connector/sessionProgress", post(session_progress))
        .route("/connector/delaySync", post(ok_empty))
        .route("/connector/getClientHostnames", post(ok_empty))
        // ---- 自作 Capture 拡張用 API（Zotero 非依存の 1 クリック取込）----
        .route("/capture/status", get(capture_status))
        .route("/capture/collections", get(capture_collections))
        .route("/capture/save", post(capture_save))
        .route("/capture/attachment", post(capture_attachment))
        .route("/capture/fetch-oa", post(capture_fetch_oa))
        .route("/capture/pdf", post(capture_pdf))
        .fallback(fallback)
        // axum 既定の 2MB では図の多い論文 PDF が 413 で弾かれる（localhost bind のみなので緩めて安全）
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .with_state(st.clone());

    // 検証用: OPENREF_CONNECTD_PORT / OPENREF_WORD_PORT で待受ポートを変えられる（既定 23119 / 23120）。
    // 本番デーモンを止めずに scratch DB で新ビルドを実弾検証するためのもの（拡張・Word は既定ポート固定）
    let port = env_port("CONNECTD_PORT", 23119);
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "openref-connectd: port {port} を bind できません（Zotero 本体が起動中？）: {e}"
            );
            std::process::exit(1);
        }
    };
    eprintln!(
        "openref-connectd: ready on 127.0.0.1:{port} (db={})",
        db_path.display()
    );

    // Word アドイン配信（HTTPS :23120）。cert 準備に失敗しても connector（:23119）は生かす。
    let word_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        match word_https(st.clone(), &db_path) {
            Ok(fut) => Box::pin(fut),
            Err(e) => {
                eprintln!("openref-connectd: Word 配信（:23120）を起動できません: {e}");
                Box::pin(std::future::pending())
            }
        };

    let connector_fut = async move {
        axum::serve(listener, app).await.unwrap();
    };
    tokio::join!(connector_fut, word_fut);
}

fn env_port(name: &str, default: u16) -> u16 {
    openref_core::env_os(name)
        .and_then(|v| v.to_str()?.trim().parse().ok())
        .unwrap_or(default)
}

fn word_https(
    st: Arc<St>,
    db_path: &std::path::Path,
) -> Result<impl std::future::Future<Output = ()> + Send, String> {
    let app_dir = db_path
        .parent()
        .ok_or("DB パスから親ディレクトリを解決できません")?
        .to_path_buf();
    let tls = word::ensure_cert(&app_dir)?;
    let word_port = env_port("WORD_PORT", 23120);
    let router = word::router(st, word_port);
    Ok(async move {
        let cfg = match axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key).await
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("openref-connectd: TLS 設定に失敗（Word 配信は無効）: {e}");
                return;
            }
        };
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], word_port));
        eprintln!("openref-connectd: Word add-in ready on https://localhost:{word_port}");
        if let Err(e) = axum_server::bind_rustls(addr, cfg)
            .serve(router.into_make_service())
            .await
        {
            eprintln!("openref-connectd: Word 配信エラー: {e}");
        }
    })
}

async fn ping_get() -> impl IntoResponse {
    (
        ZHEADERS,
        Html("<html><head><title>OpenRef</title></head><body>OpenRef is running (Zotero Connector compatible)</body></html>"),
    )
}

/// Connector はこの prefs で挙動を決める（拡張の itemSaver.js を実読して確定）:
/// - `supportsAttachmentUpload: true` … **ブラウザ側が PDF バイト列を saveAttachment へ送る**。
///   これが false/欠落だと connector は「Zotero 本体が自力 DL する」前提になり、
///   `items = data.items`（saveItems の応答）で undefined を掴んで保存エラーになる。
///   paywall 論文の PDF が取れるのはこの経路だけ（ブラウザの認証済み session を使うため）。
/// - `downloadAssociatedFiles: true` … PDF 添付を有効化（false だと PDF が捨てられる）。
/// - `automaticSnapshots: false` … HTML スナップショットは保存しない（scope = 論文のみ）。
async fn ping_post() -> impl IntoResponse {
    zjson(
        StatusCode::OK,
        json!({
            "prefs": {
                "reportActiveURL": false,
                "automaticSnapshots": false,
                "downloadAssociatedFiles": true,
                "supportsAttachmentUpload": true,
                "supportsTagsAutocomplete": false,
                "translatorsHash": "openref"
            }
        }),
    )
}

async fn get_selected_collection(State(st): State<Arc<St>>) -> impl IntoResponse {
    let db = st.db();
    let mut targets = vec![json!({ "id": "L1", "name": "OpenRef", "level": 0 })];
    if let Ok(rows) = search::collections(&db) {
        // 親→子の順に並べる（深さ計算）
        fn depth(rows: &[search::CollectionRow], id: Option<i64>) -> usize {
            match id {
                None => 0,
                Some(i) => rows
                    .iter()
                    .find(|r| r.id == i)
                    .map(|r| 1 + depth(rows, r.parent_id))
                    .unwrap_or(1),
            }
        }
        fn push_children(
            rows: &[search::CollectionRow],
            parent: Option<i64>,
            out: &mut Vec<Value>,
        ) {
            for r in rows.iter().filter(|r| r.parent_id == parent) {
                out.push(json!({
                    "id": format!("C{}", r.id),
                    "name": r.name,
                    "level": depth(rows, Some(r.id))
                }));
                push_children(rows, Some(r.id), out);
            }
        }
        push_children(&rows, None, &mut targets);
    }
    zjson(
        StatusCode::OK,
        json!({
            "libraryID": 1,
            "libraryName": "OpenRef",
            "collectionID": null,
            "id": "L1",
            "name": "OpenRef",
            "editable": true,
            // filesEditable が false だと connector は PDF アップロードを丸ごと skip する
            "filesEditable": true,
            "targets": targets
        }),
    )
}

async fn save_items(State(st): State<Arc<St>>, body: Bytes) -> impl IntoResponse {
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("saveItems: JSON parse 失敗: {e}");
            return zjson(StatusCode::BAD_REQUEST, json!({ "error": "bad json" }));
        }
    };
    let session = payload
        .get("sessionID")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let target_collection = payload
        .get("target")
        .and_then(|v| v.as_str())
        .and_then(parse_collection_target);
    let empty = vec![];
    let items = payload
        .get("items")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);

    let mut saved = 0usize;
    for zitem in items {
        let Some(ci) = zotero::zotero_to_csl(zitem) else {
            eprintln!("saveItems: title なし item を skip");
            continue;
        };
        let pmid = model::s(&ci.csl, "PMID").map(str::to_string);
        let doi = model::s(&ci.csl, "DOI").map(str::to_lowercase);

        let (key, is_new) = {
            let mut db = st.db();
            match search::find_by_ids(&db, pmid.as_deref(), doi.as_deref()) {
                Ok(Some((_id, key))) => {
                    eprintln!("saveItems: 既存 {key} に一致（dedup）");
                    (key, false)
                }
                _ => {
                    let resolved = resolver::Resolved {
                        csl: ci.csl.clone(),
                        retracted: None,
                        source: "connector",
                    };
                    match ingest::add_item(&mut db, &resolved) {
                        Ok(added) => (added.key, true),
                        Err(e) => {
                            eprintln!("saveItems: 登録失敗: {e}");
                            continue;
                        }
                    }
                }
            }
        };
        saved += 1;

        if let Some(cid) = target_collection {
            let db = st.db();
            if let Err(e) = edit::add_to_collection(&db, &key, cid) {
                eprintln!("connector: {} をフォルダ(id {})へ追加できず: {}", key, cid, e);
            }
        }
        if let Some(cid_str) = &ci.connector_id {
            st.sessions()
                .entry(session.clone())
                .or_default()
                .insert(cid_str.clone(), key.clone());
        }

        // 非同期 enrich: PMID/DOI があれば PubMed/doi.org でリッチ書誌を補完
        if is_new && (pmid.is_some() || doi.is_some()) {
            let st2 = st.clone();
            let key2 = key.clone();
            let input = pmid
                .clone()
                .map(resolver::AddInput::Pmid)
                .or_else(|| doi.clone().map(resolver::AddInput::Doi))
                .unwrap();
            tokio::task::spawn_blocking(move || {
                if let Ok(richer) = resolver::resolve(&input) {
                    let mut db = st2.db();
                    let id: Option<i64> = db
                        .conn
                        .query_row("SELECT id FROM items WHERE key = ?1", [key2.as_str()], |r| r.get(0))
                        .ok();
                    if let Some(id) = id {
                        match ingest::merge_enrichment(&mut db, id, &richer.csl) {
                            Ok(true) => eprintln!("enrich: {key2} に abstract/MeSH 等を補完"),
                            Ok(false) => {}
                            Err(e) => eprintln!("enrich: {key2} 失敗: {e}"),
                        }
                    }
                }
            });
        }

        // 添付バイナリ POST が来なかった場合の保険: 30 秒後に translator が宣言した URL からサーバ側で直接 DL を試す。
        // サーバ側の取得なので fetch-pdf と同じ採否（`pdffetch::verdict`・宣言所在 = Declared）を通し、照合の印も添付行に書く。
        // 取得は core の `pdffetch::download_pdf`（https のみ・各ホップに SSRF ガード・200 MB 上限）。curl 子プロセスは使わない:
        // ここは spawn_blocking の中なので core の ureq を呼んでよく（`/capture/fetch-oa` も同じ場所から呼ぶ）、
        // cookie / リダイレクト / 上限に curl でしか満たせない要件もない。
        // ① DB 読取 → ② ロック外でネットワーク + 照合 → ③ 再ロックして添付（Mutex をネットワークの間に握らない・`fetch_pdf_*` と同じ）
        if is_new && !ci.pdf_urls.is_empty() {
            let st3 = st.clone();
            let key3 = key.clone();
            let urls = ci.pdf_urls.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let _ = tokio::task::spawn_blocking(move || {
                    let has_attachment = |db: &Db| -> bool {
                        db.conn
                            .query_row(
                                "SELECT COUNT(*) FROM attachments a JOIN items i ON i.id = a.item_id WHERE i.key = ?1",
                                [key3.as_str()],
                                |r| r.get::<_, i64>(0),
                            )
                            .unwrap_or(1)
                            > 0
                    };
                    let (csl, filename) = {
                        let db = st3.db();
                        if has_attachment(&db) {
                            return;
                        }
                        let it = match search::get_by_key(&db, &key3) {
                            Ok(Some(it)) => it,
                            Ok(None) => {
                                eprintln!("fallback DL: {key3} が見つからない（保存直後に消えた？）→ 中止");
                                return;
                            }
                            Err(e) => {
                                eprintln!("fallback DL: {key3} の読取に失敗: {e} → 中止");
                                return;
                            }
                        };
                        let f = pdffetch::suggest_filename(&it.csl);
                        (it.csl, f)
                    };
                    for u in urls {
                        let bytes = match pdffetch::download_pdf(&u, None, false) {
                            Ok(b) => b,
                            Err(e) => {
                                eprintln!("fallback DL: {key3} {u} → {e}");
                                continue;
                            }
                        };
                        let check = pdffetch::check_pdf_identity(&bytes, &csl);
                        let mark = match pdffetch::verdict(pdffetch::LocationKind::Declared, &check) {
                            pdffetch::Verdict::Attach { doi_check } => doi_check,
                            pdffetch::Verdict::Mismatch { found_doi } => {
                                eprintln!("fallback DL: {key3} {u} は別論文（DOI {found_doi}）→ 添付しない");
                                continue;
                            }
                            pdffetch::Verdict::Unverified { why } => {
                                eprintln!("fallback DL: {key3} {u} は{why}ため添付しない");
                                continue;
                            }
                        };
                        let db = st3.db();
                        if has_attachment(&db) {
                            eprintln!("fallback DL: {key3} は取得中に別経路で添付された → {u} の {} bytes は破棄", bytes.len());
                            return;
                        }
                        match edit::attach_auto_bytes(&db, &key3, &filename, &bytes) {
                            Ok(a) => {
                                if let Some(m) = mark {
                                    pdffetch::mark_doi_check(&db, a.id, m);
                                }
                                eprintln!(
                                    "fallback DL: {key3} に添付（{u}・{:?}・doi_check={}）",
                                    a.placement,
                                    mark.unwrap_or("null")
                                );
                                return;
                            }
                            Err(e) => eprintln!("fallback DL: {key3} {u} の添付に失敗: {e}"),
                        }
                    }
                })
                .await;
            });
        }
    }
    eprintln!("saveItems: session={session} saved={saved}");
    zjson(StatusCode::CREATED, json!({}))
}

fn parse_collection_target(t: &str) -> Option<i64> {
    t.strip_prefix('C').and_then(|s| s.parse::<i64>().ok())
}

async fn save_attachment(
    State(st): State<Arc<St>>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let session = q.get("sessionID").cloned().unwrap_or_else(|| "default".into());
    let meta: Value = headers
        .get("X-Metadata")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| json!({}));
    let content_type = meta
        .get("contentType")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let is_pdf = content_type.contains("pdf") || body.starts_with(b"%PDF");
    if !is_pdf {
        eprintln!("saveAttachment: PDF 以外（{content_type}）は skip");
        return zjson(StatusCode::CREATED, json!({}));
    }
    let parent = meta
        .get("parentItemID")
        .and_then(|v| v.as_str().map(String::from).or_else(|| v.as_i64().map(|n| n.to_string())));
    let key = {
        let sessions = st.sessions();
        let map = sessions.get(&session);
        match (map, &parent) {
            (Some(m), Some(p)) => m.get(p).cloned(),
            // parent 不明なら直近 session の唯一 item に付ける（単数保存の一般ケース）
            (Some(m), None) if m.len() == 1 => m.values().next().cloned(),
            _ => None,
        }
    };
    let Some(key) = key else {
        eprintln!("saveAttachment: parent item を解決できず skip（session={session}）");
        return zjson(StatusCode::CREATED, json!({}));
    };
    let db = st.db();
    let has: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM attachments a JOIN items i ON i.id = a.item_id WHERE i.key = ?1",
            [key.as_str()],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has > 0 {
        eprintln!("saveAttachment: {key} は既に添付あり skip");
        return zjson(StatusCode::CREATED, json!({}));
    }
    let filename = search::get_by_key(&db, &key)
        .ok()
        .flatten()
        .map(|it| pdffetch::suggest_filename(&it.csl))
        .unwrap_or_else(|| {
            meta.get("title")
                .and_then(|v| v.as_str())
                .map(|t| format!("{t}.pdf"))
                .unwrap_or_else(|| "attachment.pdf".into())
        });
    match edit::attach_auto_bytes(&db, &key, &filename, &body) {
        Ok(a) => {
            if let edit::Placement::ManagedFallback { reason, .. } = &a.placement {
                eprintln!("saveAttachment: ⚠ {reason}");
            }
            eprintln!("saveAttachment: {key} に {} bytes 添付", body.len());
            zjson(StatusCode::CREATED, json!({}))
        }
        Err(e) => {
            eprintln!("saveAttachment: 失敗 {e}");
            zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() }))
        }
    }
}

/// session の connector item id → openref key を引く
fn key_for(st: &St, session: &str, item_id: Option<&str>) -> Option<String> {
    let sessions = st.sessions();
    let map = sessions.get(session)?;
    match item_id {
        Some(p) => map.get(p).cloned(),
        None if map.len() == 1 => map.values().next().cloned(),
        _ => None,
    }
}

/// 誌面 PDF が取れなかった時に connector が問い合わせてくる: OA 経路の当てがあるか
async fn has_attachment_resolvers(State(st): State<Arc<St>>, body: Bytes) -> impl IntoResponse {
    let p: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let session = p.get("sessionID").and_then(|v| v.as_str()).unwrap_or("default");
    let item_id = p.get("itemID").and_then(|v| v.as_str());
    let has = key_for(&st, session, item_id)
        .and_then(|k| {
            let db = st.db();
            search::get_by_key(&db, &k).ok().flatten()
        })
        .map(|it| {
            model::s(&it.csl, "PMCID").is_some() || model::s(&it.csl, "DOI").is_some()
        })
        .unwrap_or(false);
    zjson(StatusCode::OK, json!(has))
}

/// 上記が true の時に呼ばれる: OA（PMC / Unpaywall）から自力取得して添付名を返す
async fn save_attachment_from_resolver(
    State(st): State<Arc<St>>,
    body: Bytes,
) -> impl IntoResponse {
    let p: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let session = p.get("sessionID").and_then(|v| v.as_str()).unwrap_or("default").to_string();
    let item_id = p.get("itemID").and_then(|v| v.as_str()).map(str::to_string);
    let Some(key) = key_for(&st, &session, item_id.as_deref()) else {
        return zjson(StatusCode::INTERNAL_SERVER_ERROR, json!("item を解決できません"));
    };
    let st2 = st.clone();
    let res = tokio::task::spawn_blocking(move || {
        // ① DB 読取 → ② ロック外でネットワーク → ③ 再ロックして添付（連鎖の間 /capture/status 等を止めない）
        let plan = {
            let db = st2.db();
            pdffetch::fetch_pdf_prepare(&db, &key)?
        };
        let dl = pdffetch::fetch_pdf_download(&plan)?;
        let db = st2.db();
        pdffetch::fetch_pdf_attach(&db, &plan, dl).map(|f| f.filename)
    })
    .await;
    match res {
        Ok(Ok(filename)) => {
            eprintln!("resolver: OA から添付 {filename}");
            zjson(StatusCode::CREATED, json!(filename))
        }
        Ok(Err(e)) => {
            eprintln!("resolver: 取得できず {e}");
            zjson(StatusCode::INTERNAL_SERVER_ERROR, json!(e.to_string()))
        }
        Err(e) => zjson(StatusCode::INTERNAL_SERVER_ERROR, json!(e.to_string())),
    }
}

/// PDF ページを直接保存した場合（親 item なし）。本文から DOI/PMID を拾って書誌確定を試みる。
async fn save_standalone_attachment(
    State(st): State<Arc<St>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let meta: Value = headers
        .get("X-Metadata")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| json!({}));
    if !body.starts_with(b"%PDF") {
        return zjson(StatusCode::CREATED, json!({}));
    }
    let title = meta
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled PDF")
        .to_string();
    let url = meta.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let st2 = st.clone();
    // body（axum の Bytes）をそのまま blocking 側へ渡す（複製もファイルへの書き出しもしない）
    let res = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let mut db = st2.db();
        // /capture/pdf と同じ報告経路（理由・警告・要確認ラベル・import_note・Err のログ）
        let opts = pdfingest::IngestOpts { title_search: true, stub_on_fail: false, csl_filename: true };
        let src_name = format!("{}.pdf", safe_filename_stem(&strip_pdf_suffix(&title)));
        let (outcome, warnings, stub_reason, fail, info_title, md5) =
            match pdfingest::ingest_pdf_bytes_report(&mut db, &body, &src_name, None, &opts) {
                Ok(r) => (Some(r.outcome), r.warnings, r.reason, None, r.info_title, Some(r.md5)),
                Err(e) => (None, Vec::new(), None, Some(e.to_string()), None, None),
            };
        for w in &warnings {
            eprintln!("saveStandaloneAttachment: ⚠ {w}");
        }
        match outcome {
            Some(pdfingest::PdfIngestOutcome::Added { key })
            | Some(pdfingest::PdfIngestOutcome::AttachedToExisting { key })
            | Some(pdfingest::PdfIngestOutcome::AttachedAdditional { key })
            | Some(pdfingest::PdfIngestOutcome::DuplicateFile { key })
            | Some(pdfingest::PdfIngestOutcome::NeedsReview { key }) => Ok(key),
            Some(pdfingest::PdfIngestOutcome::NoIdentifier) | None => {
                if let Some(e) = &fail {
                    eprintln!("saveStandaloneAttachment: 識別子を解決できず → 要確認 stub へ: {e}");
                }
                let note = stub_note(&fail, &stub_reason);
                // 取込が Err（冒頭を解析できない等）で /Title・md5 を得ていないときだけ解析・計算する
                let info_title = if fail.is_some() { openref_core::pdftools::info_title_bytes(&body) } else { info_title };
                let md5 = md5.unwrap_or_else(|| pdffetch::md5_hex(&body));
                capture_stub(&mut db, info_title.as_deref(), &body, &md5, Some(&title), Some(&url), "connector-standalone", &note)
            }
        }
    })
    .await;
    match res {
        Ok(Ok(key)) => {
            eprintln!("saveStandaloneAttachment: {key} として保存");
            zjson(StatusCode::CREATED, json!({}))
        }
        Ok(Err(e)) => {
            eprintln!("saveStandaloneAttachment: 失敗 {e}");
            zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e }))
        }
        Err(e) => zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() })),
    }
}

async fn save_snapshot(State(st): State<Arc<St>>, body: Bytes) -> impl IntoResponse {
    let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(url);
    if title.is_empty() {
        return zjson(StatusCode::CREATED, json!({}));
    }
    let csl = json!({
        "type": "webpage",
        "title": title,
        "URL": url,
        "custom": { "source": "connector-snapshot" }
    });
    let mut db = st.db();
    let resolved = resolver::Resolved { csl, retracted: None, source: "connector" };
    match ingest::add_item(&mut db, &resolved) {
        Ok(a) => eprintln!("saveSnapshot: webpage {} 登録（snapshot 本体は非保存・scope=論文のみ）", a.key),
        Err(e) => eprintln!("saveSnapshot: {e}"),
    }
    zjson(StatusCode::CREATED, json!({}))
}

async fn update_session(State(st): State<Arc<St>>, body: Bytes) -> impl IntoResponse {
    let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    if let (Some(session), Some(target)) = (
        payload.get("sessionID").and_then(|v| v.as_str()),
        payload
            .get("target")
            .and_then(|v| v.as_str())
            .and_then(parse_collection_target),
    ) {
        let keys: Vec<String> = st.sessions()
            .get(session)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        let db = st.db();
        for k in keys {
            if let Err(e) = edit::add_to_collection(&db, &k, target) {
                eprintln!("connector: {} をフォルダ(id {})へ追加できず: {}", k, target, e);
            }
        }
        eprintln!("updateSession: session={session} → C{target}");
    }
    zjson(StatusCode::OK, json!({}))
}

// ---------------- Capture 拡張 API ----------------
// 設計: 拡張は「識別子（DOI/PMID）と PDF バイト列」だけを届ける薄い層。
// 書誌の正確さは resolver（PubMed 第一・doi.org 補完）が担う = translator 不要。

async fn capture_status(State(st): State<Arc<St>>) -> impl IntoResponse {
    let n: i64 = {
        let db = st.db();
        db.conn
            .query_row("SELECT COUNT(*) FROM items WHERE deleted_at IS NULL", [], |r| r.get(0))
            .unwrap_or(0)
    };
    zjson(
        StatusCode::OK,
        json!({ "ok": true, "app": "openref", "version": env!("CARGO_PKG_VERSION"), "items": n }),
    )
}

async fn capture_collections(State(st): State<Arc<St>>) -> impl IntoResponse {
    let db = st.db();
    let rows = search::collections(&db).unwrap_or_default();
    let path_of = |id: i64| -> String {
        let mut parts = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            if let Some(r) = rows.iter().find(|r| r.id == c) {
                parts.push(r.name.clone());
                cur = r.parent_id;
            } else {
                break;
            }
        }
        parts.reverse();
        parts.join("/")
    };
    let out: Vec<Value> = rows
        .iter()
        .map(|r| json!({ "id": r.id, "path": path_of(r.id), "n_items": r.n_items }))
        .collect();
    zjson(StatusCode::OK, json!({ "collections": out }))
}

/// `/capture/save` と `/capture/pdf` の共通戻り値（popup がそのまま表示する）。
/// ゴミ箱内の item（`get_by_key` が返さない）は trashed=true の最小形で返す（md5 重複の相手がゴミ箱のとき）。
fn summarize_item(db: &Db, key: &str, status: &str, retraction: &Option<String>) -> Result<Value, String> {
    let Some(it) = search::get_by_key(db, key).map_err(|e| e.to_string())? else {
        return Ok(json!({ "key": key, "status": status, "trashed": true }));
    };
    let has_pdf: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM attachments WHERE item_id = ?1",
            [it.id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(json!({
        "key": key,
        "status": status,
        "title": model::s(&it.csl, "title"),
        "first_author": model::first_author_family(&it.csl),
        "year": model::year(&it.csl),
        "journal": model::s(&it.csl, "container-title-short")
            .or_else(|| model::s(&it.csl, "container-title")),
        // 確定後の DOI（popup が DOI 起点の PDF 経路に使う。ページ抽出の DOI より resolver の値を優先）
        "doi": model::s(&it.csl, "DOI"),
        "has_pdf": has_pdf > 0,
        "retraction": retraction,
    }))
}

/// {doi?, pmid?, url?, title?, collection_id?} → 書誌確定して登録（既存なら existing を返す）
async fn capture_save(State(st): State<Arc<St>>, body: Bytes) -> impl IntoResponse {
    let p: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let clean = |k: &str| {
        p.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let doi = clean("doi").map(|d| d.to_lowercase());
    let pmid = clean("pmid");
    let title = clean("title");
    let url = clean("url");
    let collection_id = p.get("collection_id").and_then(|v| v.as_i64());

    let st2 = st.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let mut db = st2.db();
        // dedup 先行（登録済みなら resolver を呼ばない）
        if let Ok(Some((_id, key))) = search::find_by_ids(&db, pmid.as_deref(), doi.as_deref()) {
            if let Some(cid) = collection_id {
                if let Err(e) = edit::add_to_collection(&db, &key, cid) {
                    eprintln!("capture: {} をフォルダ(id {})へ追加できず: {}", key, cid, e);
                }
            }
            return summarize_item(&db, &key, "existing", &None);
        }

        let input = pmid
            .clone()
            .map(resolver::AddInput::Pmid)
            .or_else(|| doi.clone().map(resolver::AddInput::Doi));
        let resolved = match input {
            Some(i) => resolver::resolve(&i).map_err(|e| e.to_string())?,
            None => {
                // 識別子ゼロ: 捏造せず、ページ情報だけで document を作る
                let t = title.ok_or("DOI / PMID もタイトルも見つかりません")?;
                resolver::Resolved {
                    csl: json!({
                        "type": "document",
                        "title": t,
                        "URL": url,
                        "custom": { "source": "capture" }
                    }),
                    retracted: None,
                    source: "capture",
                }
            }
        };
        let retraction = resolved.retracted.clone();
        match ingest::add_item(&mut db, &resolved) {
            Ok(added) => {
                if let Some(cid) = collection_id {
                    if let Err(e) = edit::add_to_collection(&db, &added.key, cid) {
                        eprintln!("capture: {} をフォルダ(id {})へ追加できず: {}", added.key, cid, e);
                    }
                }
                eprintln!("capture: 新規 {}", added.key);
                summarize_item(&db, &added.key, "created", &retraction)
            }
            Err(openref_core::CoreError::Duplicate { existing_key, .. }) => {
                if let Some(cid) = collection_id {
                    if let Err(e) = edit::add_to_collection(&db, &existing_key, cid) {
                        eprintln!("capture: {} をフォルダ(id {})へ追加できず: {}", existing_key, cid, e);
                    }
                }
                summarize_item(&db, &existing_key, "existing", &None)
            }
            Err(e) => Err(e.to_string()),
        }
    })
    .await;
    match res {
        Ok(Ok(v)) => zjson(StatusCode::OK, v),
        Ok(Err(e)) => zjson(StatusCode::UNPROCESSABLE_ENTITY, json!({ "error": e })),
        Err(e) => zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() })),
    }
}

/// ?key=K&source=<meta|self|atypon|link|…>&via=<page|doi-tab> + body=PDF バイト列
/// （ページ内 fetch で取得した paywall PDF が届く本命経路。背景タブ経路の着地先でもある）。
/// 順序: ① DB 読取（item の CSL・md5 重複なら即 skipped）→ ② ロック外で **冒頭 2 頁の DOI 照合** → ③ 再ロックして添付。
/// 採否は `pdffetch::verdict`（サーバ側連鎖と同じ 1 箇所。所在 = `source=link`（ページ上の .pdf リンク拾い）ならヒューリスティック・
/// meta / self / atypon なら宣言所在）をこの経路の HTTP に写す:
/// - Mismatch（別 DOI だけがある）→ 409 + found_doi で拒否
/// - Unverified（ヒューリスティック所在で目的 DOI を確認できない・照合不能を含む）→ 422（投稿規定や補足資料を本体として付けない）
/// - Attach → 添付して `doi_check` に印（Match = 'match' / 宣言所在の Unknown = 'unknown' / Unchecked = NULL・未照合で警告なし）
async fn capture_attachment(
    State(st): State<Arc<St>>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> impl IntoResponse {
    let Some(key) = q.get("key").cloned() else {
        return zjson(StatusCode::BAD_REQUEST, json!({ "error": "key 必須" }));
    };
    if !body.starts_with(b"%PDF") {
        return zjson(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "error": format!("PDF ではありません（{} bytes）", body.len()) }),
        );
    }
    let source = q.get("source").cloned().unwrap_or_default();
    let via = q.get("via").cloned().unwrap_or_else(|| "page".into());
    let kind = if source == "link" { pdffetch::LocationKind::Heuristic } else { pdffetch::LocationKind::Declared };
    let st2 = st.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<(StatusCode, Value), String> {
        // md5 はここで 1 度だけ取り、③ の重複判定にもそのまま渡す（照合印は添付行 id で書く）
        let md5 = pdffetch::md5_hex(&body);
        // ① DB 読取（短く）: item の CSL と、同一ファイル（md5）が既に付いているか
        let (csl, filename) = {
            let db = st2.db();
            let Some(it) = search::get_by_key(&db, &key).map_err(|e| e.to_string())? else {
                return Ok((StatusCode::NOT_FOUND, json!({ "error": format!("item {key} が見つかりません") })));
            };
            let dup: i64 = db
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM attachments WHERE item_id = ?1 AND md5 = ?2",
                    (it.id, md5.as_str()),
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if dup > 0 {
                eprintln!("capture: {key} は同一 PDF（md5 一致）が添付済み → スキップ");
                return Ok((
                    StatusCode::OK,
                    json!({ "status": "skipped", "reason": "同一ファイルが添付済み（md5 一致）" }),
                ));
            }
            let f = pdffetch::suggest_filename(&it.csl);
            (it.csl, f)
        };
        // ② 同一性照合（ロック外・CPU）→ 採否は `pdffetch::verdict`
        let check = pdffetch::check_pdf_identity(&body, &csl);
        let mark: Option<&str> = match pdffetch::verdict(kind, &check) {
            pdffetch::Verdict::Mismatch { found_doi } => {
                eprintln!("capture: {key} へ届いた PDF（source={source} via={via}）は別論文（DOI {found_doi}）→ 添付しない（409）");
                return Ok((
                    StatusCode::CONFLICT,
                    json!({
                        "status": "mismatch",
                        "found_doi": found_doi,
                        "error": format!("取得した PDF は別論文（DOI {found_doi}）でした。意図的なら OpenRef へドロップして取り込んでください"),
                    }),
                ));
            }
            pdffetch::Verdict::Unverified { why } => {
                eprintln!("capture: {key} へ届いた PDF はページ上の .pdf リンク由来（source=link via={via}）で{why} → 添付しない（422）");
                return Ok((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    json!({
                        "status": "unverified",
                        "error": format!("ページ上の .pdf リンクから取った PDF は{why}ため添付しません（投稿規定や補足資料の可能性）。PDF を開いた状態で Capture するか、DL して OpenRef へドロップしてください"),
                    }),
                ));
            }
            pdffetch::Verdict::Attach { doi_check } => {
                if let pdffetch::IdentityCheck::Unchecked { reason } = &check {
                    eprintln!("capture: {key} の同一性照合は不能（{reason}）→ 未照合のまま添付");
                }
                doi_check
            }
        };
        // ③ 再ロックして添付。md5 の意味論は GUI 一括取込の attach_or_skip と同じ:
        // 未添付なら添付 / 同一ファイル（md5 一致）ならスキップ / 別ファイルなら 2 本目として添付
        let db = st2.db();
        match pdfingest::attach_or_skip_with_md5(&db, &key, &filename, &body, &md5) {
            Ok((outcome, attached)) => {
                if let Some(edit::Placement::ManagedFallback { reason, .. }) = attached.as_ref().map(|a| &a.placement) {
                    eprintln!("capture: ⚠ {reason}");
                }
                let status = match outcome {
                    pdfingest::PdfIngestOutcome::DuplicateFile { .. } => {
                        eprintln!("capture: {key} は同一 PDF（md5 一致）が添付済み → スキップ");
                        return Ok((
                            StatusCode::OK,
                            json!({ "status": "skipped", "reason": "同一ファイルが添付済み（md5 一致）" }),
                        ));
                    }
                    pdfingest::PdfIngestOutcome::AttachedAdditional { .. } => "attached_additional",
                    _ => "attached",
                };
                if let (Some(m), Some(a)) = (mark, &attached) {
                    pdffetch::mark_doi_check(&db, a.id, m);
                }
                eprintln!(
                    "capture: {key} に {} bytes 添付（{status}・source={source} via={via}・doi_check={}）",
                    body.len(),
                    mark.unwrap_or("null")
                );
                Ok((
                    StatusCode::CREATED,
                    json!({ "status": status, "bytes": body.len(), "filename": filename, "doi_check": mark }),
                ))
            }
            Err(e) => Err(e.to_string()),
        }
    })
    .await;
    match res {
        Ok(Ok((code, v))) => zjson(code, v),
        Ok(Err(e)) => zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e })),
        Err(e) => zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() })),
    }
}

/// body = PDF バイト列、query = collection_id / title / url（ヒント）。**PDF タブからの取込の本命経路**。
/// 冒頭テキストの DOI/PMID → resolver → 新規登録 or 既存 item へ添付（GUI の一括取込と同じ
/// `pdfingest::ingest_pdf_report`: md5 一致なら duplicate、本文 DOI は題名ガード付き、識別子が無ければタイトル検索まで試す）。
/// 識別子が無い・本文の識別子を解決できない（doi.org 404 等）ときは、従来どおりの capture stub（type document・題名は
/// Info /Title が妥当ならそれ、無ければタブ題名・URL・custom.source=capture）を作って添付し、ラベル「要確認」を付ける
/// （PDF を失わない = save_standalone_attachment と同じ方針）。
/// 戻り値は `/capture/save` と同形。status ∈ added | attached | attached_additional | duplicate | stub
async fn capture_pdf(
    State(st): State<Arc<St>>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> impl IntoResponse {
    if !body.starts_with(b"%PDF") {
        return zjson(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "error": format!("PDF ではありません（{} bytes）", body.len()) }),
        );
    }
    let collection_id = q.get("collection_id").and_then(|s| s.trim().parse::<i64>().ok());
    let clean = |k: &str| q.get(k).map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string);
    let title_hint = clean("title");
    let url_hint = clean("url");
    let st2 = st.clone();
    // body（axum の Bytes）をそのまま blocking 側へ渡す: 複製せず・一時ファイルにも落とさない
    // （取込・添付・索引はバイト列から。pdftotext fallback が要るときだけ core が一時ファイルを作って消す）
    let res = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        capture_pdf_ingest(&st2, &body, title_hint, url_hint, collection_id)
    })
    .await;
    match res {
        Ok(Ok(v)) => zjson(StatusCode::OK, v),
        Ok(Err(e)) => {
            eprintln!("capture: pdf 取込失敗: {e}");
            zjson(StatusCode::UNPROCESSABLE_ENTITY, json!({ "error": e }))
        }
        Err(e) => zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() })),
    }
}

fn capture_pdf_ingest(
    st: &St,
    bytes: &[u8],
    title_hint: Option<String>,
    url_hint: Option<String>,
    collection_id: Option<i64>,
) -> Result<Value, String> {
    use pdfingest::PdfIngestOutcome as O;
    let mut db = st.db();
    let opts = pdfingest::IngestOpts { title_search: true, stub_on_fail: false, csl_filename: true };
    // 元ファイル名はタブ題名から（csl_filename=true なので添付名には使われない。stub の題名は capture_stub が決める）
    let src_name = format!("{}.pdf", safe_filename_stem(&title_hint.as_deref().map(strip_pdf_suffix).unwrap_or_default()));
    let (outcome, warnings, stub_reason, fail, info_title, md5) =
        match pdfingest::ingest_pdf_bytes_report(&mut db, bytes, &src_name, None, &opts) {
            Ok(r) => (Some(r.outcome), r.warnings, r.reason, None, r.info_title, Some(r.md5)),
            Err(e) => (None, Vec::new(), None, Some(e.to_string()), None, None),
        };
    for w in &warnings {
        eprintln!("capture: ⚠ {w}");
    }
    // 応答の reason: stub になった理由 / 解決失敗の理由 / duplicate の相手が要確認 stub という案内
    let mut reason: Option<String> = None;
    let (status, key) = match outcome {
        Some(O::Added { key }) => ("added", key),
        Some(O::AttachedToExisting { key }) => ("attached", key),
        Some(O::AttachedAdditional { key }) => ("attached_additional", key),
        Some(O::DuplicateFile { key }) => {
            if pdfingest::is_review_stub(&db, &key) {
                reason = Some("同一ファイルが要確認 stub に添付済み。その item の詳細 → 編集から DOI/PMID を引き直せます".into());
            }
            ("duplicate", key)
        }
        // stub_on_fail=false なので到達しないが、網羅のため
        Some(O::NeedsReview { key }) => ("needs_review", key),
        Some(O::NoIdentifier) | None => {
            // 識別子ゼロ、本文の識別子を解決できない（doi.org 404・NCBI 障害）、または題名ガードで不採用:
            // 捏造せず、判明している情報だけで document を作って添付を保全する（PDF を失わない）
            if let Some(e) = &fail {
                eprintln!("capture: pdf の識別子を解決できず → 要確認 stub へ: {e}");
            }
            let note = stub_note(&fail, &stub_reason);
            reason = Some(note.clone());
            // 取込が Err（冒頭を解析できない等）で /Title・md5 を得ていないときだけ解析・計算する
            let info_title = if fail.is_some() { openref_core::pdftools::info_title_bytes(bytes) } else { info_title };
            let md5 = md5.unwrap_or_else(|| pdffetch::md5_hex(bytes));
            let key = capture_stub(&mut db, info_title.as_deref(), bytes, &md5, title_hint.as_deref(), url_hint.as_deref(), "capture", &note)?;
            ("stub", key)
        }
    };
    // ゴミ箱内の item（md5 重複の相手）には触らない: フォルダ所属も撤回表示もしない
    let live = search::get_by_key(&db, &key).map_err(|e| e.to_string())?;
    let retraction = match (&live, collection_id) {
        (Some(it), cid) => {
            if let Some(cid) = cid {
                if let Err(e) = edit::add_to_collection(&db, &key, cid) {
                    eprintln!("capture: {} をフォルダ(id {})へ追加できず: {}", key, cid, e);
                }
            }
            search::retraction_of(&db, it.id).unwrap_or(None)
        }
        (None, _) => None,
    };
    eprintln!("capture: pdf {} bytes → {status} {key}{}", bytes.len(), reason.as_deref().map(|r| format!("（{r}）")).unwrap_or_default());
    let mut v = summarize_item(&db, &key, status, &retraction)?;
    if let (Some(o), Some(r)) = (v.as_object_mut(), reason) {
        o.insert("reason".into(), json!(r));
    }
    Ok(v)
}

/// stub の import_note: 解決失敗の理由 > pdfingest の理由（題名ガード不採用 / 識別子なし / テキスト層なし）> 既定文
fn stub_note(fail: &Option<String>, stub_reason: &Option<String>) -> String {
    match (fail, stub_reason) {
        (Some(e), _) => format!("本文の識別子を解決できませんでした（{e}）。DOI/PMID を入力して書誌を引き直してください"),
        (None, Some(r)) => r.clone(),
        (None, None) => "識別子なし（PDF 本文に DOI/PMID が無く、タイトル検索も未一致。DOI/PMID を入力して書誌を引き直してください）".to_string(),
    }
}

/// 識別子が無い / 解決できない PDF の stub（type document・捏造しない）を作って添付し、「要確認」ラベルを付ける。
/// `/capture/pdf`（PDF タブ）と Zotero Connector の standalone 経路で共通。題名は Info /Title（取込の冒頭解析で得たもの =
/// `info_title`。解析し直さない）が妥当ならそれ、無ければヒント（タブ題名）から .pdf を落としたもの。
/// `md5` = `bytes` の 16 進 md5（取込で 1 度計算したもの）。戻り値は item key
fn capture_stub(
    db: &mut Db,
    info_title: Option<&str>,
    bytes: &[u8],
    md5: &str,
    title_hint: Option<&str>,
    url_hint: Option<&str>,
    source: &str,
    note: &str,
) -> Result<String, String> {
    let title = info_title
        .filter(|t| pdfingest::plausible_title(t))
        .map(str::to_string)
        .or_else(|| title_hint.map(strip_pdf_suffix).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| "Untitled PDF".into());
    let csl = json!({
        "type": "document",
        "title": title,
        "URL": url_hint.filter(|u| !u.is_empty()),
        "custom": { "source": source, "import_note": note }
    });
    let resolved = resolver::Resolved { csl, retracted: None, source: "capture" };
    let added = ingest::add_item(db, &resolved).map_err(|e| e.to_string())?;
    let filename = format!("{}.pdf", safe_filename_stem(&title));
    if let edit::Placement::ManagedFallback { reason, .. } =
        edit::attach_auto_with_md5(db, &added.key, &filename, bytes, md5).map_err(|e| e.to_string())?.placement
    {
        eprintln!("{source}: ⚠ {reason}");
    }
    // 一覧から見つけられるよう「要確認」ラベル（GUI 一括取込の要確認 stub と同じ色・同じ名）
    match tags::ensure_tag(db, pdfingest::REVIEW_TAG, Some(pdfingest::REVIEW_TAG_COLOR)) {
        Ok(tid) => {
            if let Err(e) = tags::attach_tag(db, &added.key, tid) {
                eprintln!("{source}: ⚠ 要確認ラベル付与失敗 {e}");
            }
        }
        Err(e) => eprintln!("{source}: ⚠ 要確認ラベル作成失敗 {e}"),
    }
    eprintln!("{source}: 要確認 stub {}「{}」（{note}）", added.key, model::truncate_chars(&title, 60));
    Ok(added.key)
}

/// タブ題名のヒントから拡張子を落とす（Chrome の PDF タブ題名はファイル名のことが多い）
fn strip_pdf_suffix(t: &str) -> String {
    let t = t.trim();
    let l = t.to_lowercase();
    if l.ends_with(".pdf") {
        t[..t.len() - 4].trim().to_string()
    } else {
        t.to_string()
    }
}

/// managed 添付のファイル名用（Drive モードでは drive_layout が CSL から命名するので使われない）
fn safe_filename_stem(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' => ' ',
            c => c,
        })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = model::truncate_chars(&s, 140);
    let s = s.trim_end_matches('.').trim().to_string();
    if s.is_empty() { "attachment".into() } else { s }
}

/// {key} → サーバ側の連鎖（PMC → Unpaywall → Crossref link[] → doi.org 着地ページ）で取得を試みる（ページ内 fetch 不発時の fallback）。
/// 各段の結果はエラー文とログに全部出る。
async fn capture_fetch_oa(State(st): State<Arc<St>>, body: Bytes) -> impl IntoResponse {
    let p: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let Some(key) = p.get("key").and_then(|v| v.as_str()).map(str::to_string) else {
        return zjson(StatusCode::BAD_REQUEST, json!({ "error": "key 必須" }));
    };
    let st2 = st.clone();
    let key2 = key.clone();
    let res = tokio::task::spawn_blocking(move || {
        // ① DB 読取 → ② ロック外でネットワーク（PMC → Unpaywall → Crossref → 着地ページ。数十秒〜数分）→ ③ 再ロックして添付
        let plan = {
            let db = st2.db();
            pdffetch::fetch_pdf_prepare(&db, &key2)?
        };
        let dl = pdffetch::fetch_pdf_download(&plan)?;
        let db = st2.db();
        pdffetch::fetch_pdf_attach(&db, &plan, dl).map(|f| (f.filename, f.source))
    })
    .await;
    match res {
        Ok(Ok((filename, source))) => {
            eprintln!("capture: fetch-oa {key} ✓ {source} {filename}");
            zjson(StatusCode::CREATED, json!({ "status": "fetched", "filename": filename, "source": source }))
        }
        Ok(Err(e)) => {
            // 失敗理由（pmc / unpaywall の tried 連鎖）を /tmp/openref-connectd.log に残す
            eprintln!("capture: fetch-oa {key} ✗ {e}");
            zjson(StatusCode::UNPROCESSABLE_ENTITY, json!({ "error": e.to_string() }))
        }
        Err(e) => zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() })),
    }
}

async fn session_progress() -> impl IntoResponse {
    zjson(StatusCode::OK, json!({ "done": true, "items": [] }))
}

async fn ok_empty() -> impl IntoResponse {
    zjson(StatusCode::OK, json!({}))
}

async fn fallback(uri: Uri) -> impl IntoResponse {
    eprintln!("(未実装エンドポイント) {uri} → 200 {{}}");
    zjson(StatusCode::OK, json!({}))
}
