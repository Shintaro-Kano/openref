//! openref-connectd — ブラウザからの取込口（自作 Capture 拡張の `/capture/*`）と Word 配信（HTTPS）の常駐サービス。
//! localhost:23119（取込）/ :23120（Word）のみ bind。
//!
//! Zotero Connector 互換（`/connector/*`）は 3.0.0 で外した。`/connector` 配下は
//! すべて 404 + 理由を返す（`connector_gone`）= 未知のパスの 200 {}（`fallback`）に落とさない
//! （落とすと Zotero Connector に保存が成功したように見える）。それ以外の未知のパスは 200 {} で寛容に受け、
//! stderr にログして観察できるようにする。

#[cfg(test)]
mod capture_contract;
mod jsc;
mod word;
mod word_v2;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{any, get, post};
use axum::{Json, Router};
use openref_core::{datadir, edit, ingest, model, pdffetch, pdfingest, resolver, search, Db};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct St {
    pub db: Mutex<Db>,
}

impl St {
    /// DB のロック。poison（ロック中の panic）でも中身を取り出して続ける: rusqlite の Transaction は unwind で rollback
    /// されるので接続はそのまま使える（`lock().unwrap()` だと 1 回の panic で以後の全リクエストが失敗する）
    pub fn db(&self) -> std::sync::MutexGuard<'_, Db> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `/capture/*` の応答に付けるヘッダ（Zotero Connector 互換だった頃の名残。/capture/* の契約を変えないため残す）
const ZHEADERS: [(&str, &str); 2] = [
    ("X-Zotero-Version", "7.0.15"),
    ("X-Zotero-Connector-API-Version", "3"),
];

fn zjson(status: StatusCode, v: Value) -> impl IntoResponse {
    (status, ZHEADERS, Json(v))
}

/// 蔵書の件数（ゴミ箱外）。/capture/status の接続確認と Word の /api/status の診断表示。
/// 読めなければ Err（0 件に畳まない。どちらも接続自体は成功なので、呼び手は件数を null・理由を error にして返す）
fn live_item_count(db: &Db) -> openref_core::Result<i64> {
    Ok(db.conn.query_row("SELECT COUNT(*) FROM items WHERE deleted_at IS NULL", [], |r| r.get(0))?)
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
    });

    let app = connector_router(st.clone());

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

    let connector_fut = async move {
        axum::serve(listener, app).await.unwrap();
    };
    // Word アドイン配信（HTTPS :23120）を先頭に置く。join! は初回を先頭の future から poll するので（tokio の実装。docs に明記は無い）、
    // cert の準備・router の構築は connector の最初の poll より前に走る。
    // Word 配信が失敗して先に終わっても、join! は connector（:23119）の終わりまで待つ
    tokio::join!(word_https(st.clone(), &db_path), connector_fut);
}

/// :23119 の要求 body の上限（PDF の受け取り）。axum 既定の 2MB では図の多い論文 PDF が 413 で弾かれる（localhost bind のみなので緩めて安全）
const CONNECTOR_BODY_LIMIT: usize = 512 * 1024 * 1024;

/// :23119 の router（Capture 拡張の /capture/* + 外した /connector/* の 404）。main と契約テスト（tests）が同じものを使う
fn connector_router(st: Arc<St>) -> Router {
    Router::new()
        // ---- Zotero Connector 互換は 3.0.0 で外した: /connector 配下はどのメソッドでも 404 + 理由（fallback の 200 {} に落とさない）----
        .route("/connector", any(connector_gone))
        .route("/connector/", any(connector_gone))
        .route("/connector/*rest", any(connector_gone))
        // ---- 自作 Capture 拡張用 API（Zotero 非依存の 1 クリック取込）----
        .route("/capture/status", get(capture_status))
        .route("/capture/collections", get(capture_collections))
        .route("/capture/save", post(capture_save))
        .route("/capture/attachment", post(capture_attachment))
        .route("/capture/fetch-oa", post(capture_fetch_oa))
        .route("/capture/pdf", post(capture_pdf))
        .fallback(fallback)
        .layer(DefaultBodyLimit::max(CONNECTOR_BODY_LIMIT))
        .with_state(st)
}

fn env_port(name: &str, default: u16) -> u16 {
    openref_core::env_os(name)
        .and_then(|v| v.to_str()?.trim().parse().ok())
        .unwrap_or(default)
}

/// Word アドイン配信（HTTPS）。cert・TLS の準備に失敗したらログを残して終わる（:23119 の取込は止めない）
async fn word_https(st: Arc<St>, db_path: &std::path::Path) {
    let unavailable = |e: &str| eprintln!("openref-connectd: Word 配信（:23120）を起動できません: {e}");
    let Some(app_dir) = db_path.parent() else {
        return unavailable("DB パスから親ディレクトリを解決できません");
    };
    let tls = match word::ensure_cert(app_dir) {
        Ok(t) => t,
        Err(e) => return unavailable(&e),
    };
    let word_port = env_port("WORD_PORT", 23120);
    let router = word::router(st, word_port, app_dir);
    let cfg = match axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key).await {
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
}

/// item をフォルダへ入れる。失敗はログだけ（保存・取込そのものは成功のまま）。who = ログの接頭辞
fn add_to_folder(db: &Db, key: &str, cid: i64, who: &str) {
    if let Err(e) = edit::add_to_collection(db, key, cid) {
        eprintln!("{who}: {key} をフォルダ(id {cid})へ追加できず: {e}");
    }
}

/// サーバ側の連鎖（PMC → Unpaywall → Crossref → 着地ページ。数十秒〜数分）で PDF を取得して添付する。blocking（spawn_blocking の中で呼ぶ）。
/// ① DB 読取 → ② ロック外でネットワーク → ③ 再ロックして添付（連鎖の間 /capture/status 等を止めない）
fn fetch_pdf_unlocked(st: &St, key: &str) -> openref_core::Result<pdffetch::Fetched> {
    let plan = {
        let db = st.db();
        pdffetch::fetch_pdf_prepare(&db, key)?
    };
    let dl = pdffetch::fetch_pdf_download(&plan)?;
    let db = st.db();
    pdffetch::fetch_pdf_attach(&db, &plan, dl)
}

/// `/capture/pdf` の PDF 取込の経路の値（ログの文面・stub の custom.source）
struct PdfFrom<'a> {
    /// タブ / ページの題名（元ファイル名と stub の題名の既定）・URL（stub の URL）
    title: Option<&'a str>,
    url: Option<&'a str>,
    /// stub の custom.source（stub の行のログ接頭辞も）
    source: &'static str,
    /// 取込の行（警告・識別子を解決できなかった行）のログ接頭辞と、解決できなかった行の主語
    log: &'static str,
    fail_subject: &'static str,
}

/// `ingest_or_stub` の結果。status ∈ added | attached | attached_additional | duplicate | needs_review | stub
/// （needs_review は stub_on_fail=false なので来ないが、網羅のため）。note = stub の import_note
struct Ingested {
    key: String,
    status: &'static str,
    note: Option<String>,
}

/// PDF 取込（pdfingest の 3 段）→ 識別子が無い / 解決できなければ要確認 stub（`/capture/pdf`）。
/// ① md5 照合と ③ 登録・添付だけロックを取り、② 冒頭 2 頁の抽出・resolver・題名検索はロックの外（取込中も /capture/status・
/// Word・他の保存を止めない）。stub は ③ のロックの中で作る（③ の md5 再照合と stub 作成の間に同じ PDF の別の取込が割り込まない）。
/// 戻り値の guard = そのロック（呼び手は続けて要約・フォルダ所属に使う）。Err = DB / 添付の失敗だけ（抽出できない・
/// resolver の失敗は stub_on_fail=false なので `fail` に入って届き、stub になる）
fn ingest_or_stub<'a>(st: &'a St, bytes: &[u8], from: &PdfFrom) -> Result<(std::sync::MutexGuard<'a, Db>, Ingested), String> {
    use pdfingest::PdfIngestOutcome as O;
    let opts = pdfingest::IngestOpts { title_search: true, stub_on_fail: false, csl_filename: true };
    // 元ファイル名はタブ / ページの題名から（csl_filename=true なので添付名には使われない）
    let src_name = format!("{}.pdf", pdfingest::safe_filename_stem(&from.title.map(pdfingest::strip_pdf_suffix).unwrap_or_default()));
    let Ok((mut db, res)) =
        pdfingest::ingest_pdf_bytes_unlocked(|| Ok::<_, std::convert::Infallible>(st.db()), bytes, &src_name, None, &opts);
    let pdfingest::IngestReport { outcome, warnings, reason, fail, info_title, md5 } = res.map_err(|e| e.to_string())?;
    for w in &warnings {
        eprintln!("{}: ⚠ {w}", from.log);
    }
    let (key, status) = match outcome {
        O::Added { key } => (key, "added"),
        O::AttachedToExisting { key } => (key, "attached"),
        O::AttachedAdditional { key } => (key, "attached_additional"),
        O::DuplicateFile { key } => (key, "duplicate"),
        O::NeedsReview { key } => (key, "needs_review"),
        O::NoIdentifier => {
            // 識別子ゼロ、本文の識別子を解決できない（doi.org 404・NCBI 障害）、または題名ガードで不採用:
            // 捏造せず、判明している情報だけで document を作って添付を保全する（PDF を失わない）
            if let Some(e) = &fail {
                eprintln!("{}: {}識別子を解決できず → 要確認 stub へ: {e}", from.log, from.fail_subject);
            }
            let note = stub_note(&fail, &reason);
            let origin = pdfingest::StubOrigin::Page { title: from.title, url: from.url, source: from.source };
            let mut stub_warnings = Vec::new();
            let made = pdfingest::create_review_stub(&mut db, &origin, info_title.as_deref(), bytes, &md5, &note, &mut stub_warnings);
            for w in &stub_warnings {
                eprintln!("{}: ⚠ {w}", from.source);
            }
            let (key, title) = made.map_err(|e| e.to_string())?;
            eprintln!("{}: 要確認 stub {key}「{}」（{note}）", from.source, model::truncate_chars(&title, 60));
            return Ok((db, Ingested { key, status: "stub", note: Some(note) }));
        }
    };
    Ok((db, Ingested { key, status, note: None }))
}

// ---------------- Capture 拡張 API ----------------
// 設計: 拡張は「識別子（DOI/PMID）と PDF バイト列」だけを届ける薄い層。
// 書誌の正確さは resolver（PubMed 第一・doi.org 補完）が担う = translator 不要。

async fn capture_status(State(st): State<Arc<St>>) -> impl IntoResponse {
    let mut v = json!({ "ok": true, "app": "openref", "version": env!("CARGO_PKG_VERSION"), "items": null });
    match live_item_count(&st.db()) {
        Ok(n) => v["items"] = json!(n),
        Err(e) => {
            eprintln!("capture: status の件数を数えられません: {e}");
            v["error"] = json!(format!("件数を数えられません: {e}"));
        }
    }
    zjson(StatusCode::OK, v)
}

async fn capture_collections(State(st): State<Arc<St>>) -> impl IntoResponse {
    let db = st.db();
    let rows = match search::collections(&db) {
        Ok(rows) => rows,
        Err(e) => {
            // 空の一覧（= フォルダが無い）に畳まない。popup はフォルダ無しで保存を続けられる
            eprintln!("capture: フォルダ一覧を読めません: {e}");
            return zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() }));
        }
    };
    let paths = search::collection_paths(&rows);
    let path_of = |id: i64| -> String { paths[&id].clone() };
    let out: Vec<Value> = rows
        .iter()
        .map(|r| json!({ "id": r.id, "path": path_of(r.id), "n_items": r.n_items }))
        .collect();
    zjson(StatusCode::OK, json!({ "collections": out }))
}

/// `/capture/save` と `/capture/pdf` の共通戻り値（popup がそのまま表示する）。
/// ゴミ箱内の item（`get_by_key` が返さない）は trashed=true の最小形で返す（md5 重複の相手がゴミ箱のとき）。
/// 撤回の印は DB（retraction_flags）から引く = 新規・既存一致・Duplicate のどれでも同じ警告（popup の「撤回論文の可能性」）
fn summarize_item(db: &Db, key: &str, status: &str) -> Result<Value, String> {
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
    // 読めなければ応答は null のまま（保存は成功している）だが、黙って落とさずログに残す
    let retraction = search::retraction_of(db, it.id).unwrap_or_else(|e| {
        eprintln!("capture: {key} の撤回の印を読めず（応答は null）: {e}");
        None
    });
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

/// `/capture/save` の要求（trim 済み・空は None・DOI は小文字）
struct SaveReq {
    doi: Option<String>,
    pmid: Option<String>,
    title: Option<String>,
    url: Option<String>,
    collection_id: Option<i64>,
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
    let req = SaveReq {
        doi: clean("doi").map(|d| d.to_lowercase()),
        pmid: clean("pmid"),
        title: clean("title"),
        url: clean("url"),
        collection_id: p.get("collection_id").and_then(|v| v.as_i64()),
    };

    let st2 = st.clone();
    let res = tokio::task::spawn_blocking(move || capture_save_unlocked(&st2, req, resolver::resolve)).await;
    match res {
        Ok(Ok(v)) => zjson(StatusCode::OK, v),
        Ok(Err(e)) => zjson(StatusCode::UNPROCESSABLE_ENTITY, json!({ "error": e })),
        Err(e) => zjson(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() })),
    }
}

/// `/capture/save` の本体（blocking）。① ロックして dedup（登録済みなら resolver を呼ばない）→ ② **ロックを離して** resolver
/// （PubMed / doi.org・Crossref の門の列に並ぶ = 数秒〜数十秒。その間も Word・他の /capture/* を止めない）→ ③ 再ロックして登録。
/// ① と ③ の間に同じ識別子が別経路で入っていたら、③ の add_item が Duplicate を返す → existing（新しい分岐は要らない）
fn capture_save_unlocked(
    st: &St,
    req: SaveReq,
    resolve: impl FnOnce(&resolver::AddInput) -> openref_core::Result<resolver::Resolved>,
) -> Result<Value, String> {
    let mut db = st.db();
    let mut dup_retraction: Option<String> = None;
    // resolver の警告（PubMed の照合に失敗して doi.org で代用した等）。登録に使ったときだけ応答・ログへ
    let mut warnings: Vec<String> = Vec::new();
    let (key, status) = match search::find_by_ids(&db, req.pmid.as_deref(), req.doi.as_deref()) {
        Ok(Some((_id, key))) => (key, "existing"),
        _ => {
            drop(db);
            let input = req
                .pmid
                .clone()
                .map(resolver::AddInput::Pmid)
                .or_else(|| req.doi.clone().map(resolver::AddInput::Doi));
            let resolved = match input {
                Some(i) => resolve(&i).map_err(|e| e.to_string())?,
                None => {
                    // 識別子ゼロ: 捏造せず、ページ情報だけで document を作る
                    let t = req.title.ok_or("DOI / PMID もタイトルも見つかりません")?;
                    resolver::Resolved {
                        csl: json!({
                            "type": "document",
                            "title": t,
                            "URL": req.url,
                            "custom": { "source": "capture" }
                        }),
                        retracted: None,
                        source: "capture",
                        warnings: vec![],
                    }
                }
            };
            db = st.db();
            match ingest::add_item(&mut db, &resolved) {
                Ok(added) => {
                    warnings = resolved.warnings;
                    (added.key, "created")
                }
                Err(openref_core::CoreError::Duplicate { existing_key, .. }) => {
                    // ② の間に別経路（同じ connectd の別の /capture/* や GUI・CLI・MCP）が先に登録した: resolver が返した撤回を捨てない
                    if let Some(r) = &resolved.retracted {
                        dup_retraction = Some(r.clone());
                        record_retraction(&mut db, &existing_key, &resolved, r);
                    }
                    (existing_key, "existing")
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    };
    if let Some(cid) = req.collection_id {
        add_to_folder(&db, &key, cid, "capture");
    }
    if status == "created" {
        eprintln!("capture: 新規 {key}");
    }
    for w in &warnings {
        eprintln!("capture: {key} ⚠ {w}");
    }
    let mut v = summarize_item(&db, &key, status)?;
    // 題名ガードで印を書けなかったときも、resolver が撤回と言った警告は応答に出す（警告を黙って外さない）
    if let (Some(r), Some(o)) = (dup_retraction, v.as_object_mut()) {
        if o.get("retraction").is_some_and(Value::is_null) {
            o.insert("retraction".into(), json!(r));
        }
    }
    if let Some(o) = v.as_object_mut() {
        o.insert("warnings".into(), json!(warnings));
    }
    Ok(v)
}

/// ③ の Duplicate の相手に resolver の撤回の印を書く（merge_enrichment = 題名ガード付き・権威フィールドの補完も同じ応答で）。
/// 失敗・ガードでの却下はエラーにせずログへ（保存そのものは existing で成功）
fn record_retraction(db: &mut Db, key: &str, resolved: &resolver::Resolved, detail: &str) {
    match search::get_by_key(db, key) {
        Ok(Some(it)) => match ingest::merge_enrichment(db, it.id, resolved) {
            Ok(_) => eprintln!("capture: {key} に撤回の印: {detail}"),
            Err(e) => eprintln!("capture: {key} に撤回の印を書けず: {e}"),
        },
        Ok(None) => eprintln!("capture: {key} はゴミ箱内 → 撤回の印を書かない"),
        Err(e) => eprintln!("capture: {key} を読めず撤回の印を書けない: {e}"),
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
        return zjson(StatusCode::UNPROCESSABLE_ENTITY, not_pdf(&body));
    }
    let source = q.get("source").cloned().unwrap_or_default();
    let via = q.get("via").cloned().unwrap_or_else(|| "page".into());
    let kind = if source == "link" { pdffetch::LocationKind::Heuristic } else { pdffetch::LocationKind::Declared };
    let st2 = st.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<(StatusCode, Value), String> {
        // md5 はここで 1 度だけ取り、③ の重複判定にもそのまま渡す（照合印は添付行 id で書く）
        let md5 = pdffetch::md5_hex(&body);
        let skipped = || {
            eprintln!("capture: {key} は同一 PDF（md5 一致）が添付済み → スキップ");
            (StatusCode::OK, json!({ "status": "skipped", "reason": "同一ファイルが添付済み（md5 一致）" }))
        };
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
                return Ok(skipped());
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
        // ③ 再ロックして添付。md5 の意味論は GUI 一括取込(pdfingest の ③ → attach_or_skip_with_md5)と同じ:
        // 未添付なら添付 / 同一ファイル（md5 一致）ならスキップ / 別ファイルなら 2 本目として添付
        let db = st2.db();
        match pdfingest::attach_or_skip_with_md5(&db, &key, &filename, &body, &md5) {
            Ok((outcome, attached)) => {
                if let Some(edit::Placement::ManagedFallback { reason, .. }) = attached.as_ref().map(|a| &a.placement) {
                    eprintln!("capture: ⚠ {reason}");
                }
                let status = match outcome {
                    pdfingest::PdfIngestOutcome::DuplicateFile { .. } => return Ok(skipped()),
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

/// `/capture/attachment`・`/capture/pdf` で body が PDF でないときの 422 の本文
fn not_pdf(body: &[u8]) -> Value {
    json!({ "error": format!("PDF ではありません（{} bytes）", body.len()) })
}

/// body = PDF バイト列、query = collection_id / title / url（ヒント）。**PDF タブからの取込の本命経路**。
/// 冒頭テキストの DOI/PMID → resolver → 新規登録 or 既存 item へ添付（GUI の一括取込と同じ
/// pdfingest の 3 段 = `ingest_or_stub`: md5 一致なら duplicate、本文 DOI は題名ガード付き、識別子が無ければタイトル検索まで試す）。
/// 識別子が無い・本文の識別子を解決できない（doi.org 404 等）ときは、従来どおりの capture stub（type document・題名は
/// Info /Title が妥当ならそれ、無ければタブ題名・URL・custom.source=capture）を作って添付し、ラベル「要確認」を付ける
/// （PDF を失わない）。
/// 戻り値は `/capture/save` と同形。status ∈ added | attached | attached_additional | duplicate | stub
async fn capture_pdf(
    State(st): State<Arc<St>>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> impl IntoResponse {
    if !body.starts_with(b"%PDF") {
        return zjson(StatusCode::UNPROCESSABLE_ENTITY, not_pdf(&body));
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
    let from = PdfFrom { title: title_hint.as_deref(), url: url_hint.as_deref(), source: "capture", log: "capture", fail_subject: "pdf の" };
    let (db, Ingested { key, status, note }) = ingest_or_stub(st, bytes, &from)?;
    // 応答の reason: stub になった理由 / 解決失敗の理由（= stub の import_note）/ duplicate の相手が要確認 stub という案内
    let mut reason = note;
    if status == "duplicate" {
        match pdfingest::is_review_stub(&db, &key) {
            Ok(true) => reason = Some("同一ファイルが要確認 stub に添付済み。その item の詳細 → 編集から DOI/PMID を引き直せます".into()),
            Ok(false) => {}
            Err(e) => eprintln!("capture: {key} が要確認 stub かを確かめられません: {e}"),
        }
    }
    // ゴミ箱内の item（md5 重複の相手）には触らない: フォルダ所属もしない（撤回表示は summarize_item の trashed の最小形に無い）
    let live = search::get_by_key(&db, &key).map_err(|e| e.to_string())?;
    if let (Some(_), Some(cid)) = (&live, collection_id) {
        add_to_folder(&db, &key, cid, "capture");
    }
    eprintln!("capture: pdf {} bytes → {status} {key}{}", bytes.len(), reason.as_deref().map(|r| format!("（{r}）")).unwrap_or_default());
    let mut v = summarize_item(&db, &key, status)?;
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

/// {key} → サーバ側の連鎖（PMC → Unpaywall → Crossref link[] → doi.org 着地ページ）で取得を試みる（ページ内 fetch 不発時の fallback）。
/// 各段の結果はエラー文とログに全部出る。
async fn capture_fetch_oa(State(st): State<Arc<St>>, body: Bytes) -> impl IntoResponse {
    let p: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let Some(key) = p.get("key").and_then(|v| v.as_str()).map(str::to_string) else {
        return zjson(StatusCode::BAD_REQUEST, json!({ "error": "key 必須" }));
    };
    let st2 = st.clone();
    let key2 = key.clone();
    let res = tokio::task::spawn_blocking(move || fetch_pdf_unlocked(&st2, &key2).map(|f| (f.filename, f.source))).await;
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

/// 外した Zotero Connector 互換の口（`/connector` 配下）。404 + 理由の 1 行。ZHEADERS は付けない（Zotero を名乗らない）。
/// Zotero Connector には「Zotero が起動していない」と見え、保存したつもりで保存されていない状態にならない
async fn connector_gone(method: Method, uri: Uri) -> impl IntoResponse {
    eprintln!("(外した口) {method} {uri} → 404（Zotero Connector 互換は 3.0.0 で外した）");
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": "OpenRef は Zotero Connector 互換（/connector/*）を 3.0.0 で外しました。ブラウザからの取込は OpenRef Capture 拡張を使ってください"
        })),
    )
}

async fn fallback(uri: Uri) -> impl IntoResponse {
    eprintln!("(未実装エンドポイント) {uri} → 200 {{}}");
    zjson(StatusCode::OK, json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_st() -> St {
        St {
            db: Mutex::new(Db::open_in_memory().unwrap()),
        }
    }

    fn csl_for(doi: &str) -> Value {
        json!({ "type": "article-journal", "title": format!("Paper {doi}"), "DOI": doi,
                "author": [{ "family": "Brandao", "given": "A" }], "issued": { "date-parts": [[2026]] } })
    }

    fn resolved(doi: &str, retracted: Option<&str>) -> resolver::Resolved {
        resolver::Resolved { csl: csl_for(doi), retracted: retracted.map(str::to_string), source: "stub", warnings: vec![] }
    }

    fn req(doi: Option<&str>, title: Option<&str>) -> SaveReq {
        SaveReq { doi: doi.map(str::to_string), pmid: None, title: title.map(str::to_string), url: Some("https://example.org/x".into()), collection_id: None }
    }

    fn n_items(st: &St) -> i64 {
        st.db().conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0)).unwrap()
    }

    /// 新規: ② の resolver は DB のロックを握らずに呼ばれ（中で try_lock が取れる）、③ で登録して created を返す
    #[test]
    fn save_resolves_outside_the_lock() {
        let st = test_st();
        let v = capture_save_unlocked(&st, req(Some("10.1234/new"), None), |i| {
            assert!(matches!(i, resolver::AddInput::Doi(d) if d == "10.1234/new"));
            let g = st.db.try_lock().expect("② の間に DB のロックが握られている");
            drop(g);
            Ok(resolved("10.1234/new", None))
        })
        .unwrap();
        assert_eq!(v["status"], "created");
        assert_eq!(v["title"], "Paper 10.1234/new");
        assert_eq!(v["doi"], "10.1234/new");
        assert_eq!(v["has_pdf"], false);
        assert_eq!(v["warnings"], json!([]), "普段は空配列");
        assert_eq!(n_items(&st), 1);
    }

    /// resolver の警告（PubMed の照合に失敗 → doi.org で代用）は created の応答に載る。既存一致（resolver を呼ばない）は空配列
    #[test]
    fn save_reports_resolver_warnings() {
        let st = test_st();
        let v = capture_save_unlocked(&st, req(Some("10.1234/w"), None), |_| {
            let mut r = resolved("10.1234/w", None);
            r.warnings.push("PubMed の照合に失敗（x）→ doi.org の書誌で代用".into());
            Ok(r)
        })
        .unwrap();
        assert_eq!(v["status"], "created");
        assert_eq!(v["warnings"], json!(["PubMed の照合に失敗（x）→ doi.org の書誌で代用"]));
        let again = capture_save_unlocked(&st, req(Some("10.1234/w"), None), |_| panic!("resolver")).unwrap();
        assert_eq!(again["status"], "existing");
        assert_eq!(again["warnings"], json!([]));
    }

    /// 登録済みの DOI は ① で existing（resolver を呼ばない）
    #[test]
    fn save_existing_does_not_resolve() {
        let st = test_st();
        let first = capture_save_unlocked(&st, req(Some("10.1234/a"), None), |_| Ok(resolved("10.1234/a", None))).unwrap();
        let v = capture_save_unlocked(&st, req(Some("10.1234/a"), None), |_| panic!("登録済みなのに resolver を呼んだ")).unwrap();
        assert_eq!(v["status"], "existing");
        assert_eq!(v["key"], first["key"]);
        assert_eq!(n_items(&st), 1);
    }

    /// ① と ③ の間（② = ロックを離している間）に同じ DOI が別経路で入ったら、③ は Duplicate → その item の existing
    #[test]
    fn same_doi_between_check_and_commit_is_existing() {
        let st = test_st();
        let mut other = String::new();
        let v = capture_save_unlocked(&st, req(Some("10.1234/race"), None), |_| {
            let mut db = st.db();
            other = ingest::add_item(&mut db, &resolved("10.1234/race", None)).unwrap().key;
            Ok(resolved("10.1234/race", None))
        })
        .unwrap();
        assert_eq!(v["status"], "existing");
        assert_eq!(v["key"], other.as_str());
        assert_eq!(n_items(&st), 1);
    }

    /// 識別子なし: 題名があれば document（resolver を呼ばない）・題名も無ければ Err。resolver の失敗は Err のまま（登録しない）
    #[test]
    fn save_without_identifier_and_resolver_error() {
        let st = test_st();
        let v = capture_save_unlocked(&st, req(None, Some("Page title")), |_| panic!("識別子なしで resolver")).unwrap();
        assert_eq!(v["status"], "created");
        let it = search::get_by_key(&st.db(), v["key"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!(it.csl["type"], "document");
        assert_eq!(it.csl["URL"], "https://example.org/x");
        let e = capture_save_unlocked(&st, req(None, None), |_| panic!("resolver")).unwrap_err();
        assert_eq!(e, "DOI / PMID もタイトルも見つかりません");
        let e = capture_save_unlocked(&st, req(Some("10.1234/404"), None), |_| Err(openref_core::CoreError::Http("404".into()))).unwrap_err();
        assert!(e.contains("404"), "{e}");
        assert_eq!(n_items(&st), 1);
    }

    /// 撤回の印は経路によらず出る: 新規（resolver が撤回を返した）・既存一致（① の existing）・Duplicate（③ の existing）・
    /// 印の無い item は null。ゴミ箱の item（md5 重複の相手）は trashed の最小形のまま（retraction を出さない）
    #[test]
    fn retraction_is_reported_for_existing_items_too() {
        let st = test_st();
        let created = capture_save_unlocked(&st, req(Some("10.1234/retracted"), None), |_| Ok(resolved("10.1234/retracted", Some("Retraction in: X 2026")))).unwrap();
        assert_eq!(created["status"], "created");
        assert_eq!(created["retraction"], "Retraction in: X 2026");
        let existing = capture_save_unlocked(&st, req(Some("10.1234/retracted"), None), |_| panic!("resolver")).unwrap();
        assert_eq!(existing["status"], "existing");
        assert_eq!(existing["retraction"], "Retraction in: X 2026", "既存一致でも撤回の警告");
        // ③ の Duplicate: ② の間に同じ DOI が入った（印はその item に付いている）
        let st2 = test_st();
        let v = capture_save_unlocked(&st2, req(Some("10.1234/race-r"), None), |_| {
            let mut db = st2.db();
            ingest::add_item(&mut db, &resolved("10.1234/race-r", Some("Retracted"))).unwrap();
            Ok(resolved("10.1234/race-r", None))
        })
        .unwrap();
        assert_eq!(v["status"], "existing");
        assert_eq!(v["retraction"], "Retracted", "Duplicate でも撤回の警告");
        let plain = capture_save_unlocked(&st, req(Some("10.1234/plain"), None), |_| Ok(resolved("10.1234/plain", None))).unwrap();
        assert_eq!(plain["retraction"], Value::Null);
        let key = existing["key"].as_str().unwrap();
        edit::trash(&st.db(), key).unwrap();
        let t = summarize_item(&st.db(), key, "duplicate").unwrap();
        assert_eq!(t, json!({ "key": key, "status": "duplicate", "trashed": true }));
    }

    /// ② の間に同じ DOI が印なしで先に入り（同じ connectd の別の /capture/* や GUI・CLI・MCP）、resolver は撤回を返した: ③ の Duplicate で
    /// resolver の撤回を捨てず、応答にも DB（retraction_flags）にも残す。題名ガードで却下されても応答の警告は出す（DB には書かない）
    #[test]
    fn duplicate_keeps_retraction_from_resolver() {
        let st = test_st();
        let mut other = String::new();
        let v = capture_save_unlocked(&st, req(Some("10.1234/race-new"), None), |_| {
            let mut db = st.db();
            other = ingest::add_item(&mut db, &resolved("10.1234/race-new", None)).unwrap().key;
            Ok(resolved("10.1234/race-new", Some("Retraction in: Y 2026")))
        })
        .unwrap();
        assert_eq!(v["status"], "existing");
        assert_eq!(v["key"], other.as_str());
        assert_eq!(v["retraction"], "Retraction in: Y 2026");
        let id = search::get_by_key(&st.db(), &other).unwrap().unwrap().id;
        assert_eq!(search::retraction_of(&st.db(), id).unwrap().as_deref(), Some("Retraction in: Y 2026"));

        // 先に入った item の題名が違う（識別子の誤り）: merge_enrichment のガードで DB には書かないが、応答には警告
        let mut odd = String::new();
        let v = capture_save_unlocked(&st, req(Some("10.1234/race-odd"), None), |_| {
            let mut db = st.db();
            let mut csl = csl_for("10.1234/race-odd");
            csl["title"] = json!("Completely different hospital newsletter");
            odd = ingest::add_item(&mut db, &resolver::Resolved { csl, retracted: None, source: "test", warnings: vec![] }).unwrap().key;
            Ok(resolved("10.1234/race-odd", Some("Retracted")))
        })
        .unwrap();
        assert_eq!(v["key"], odd.as_str());
        assert_eq!(v["retraction"], "Retracted");
        let id = search::get_by_key(&st.db(), &odd).unwrap().unwrap().id;
        assert_eq!(search::retraction_of(&st.db(), id).unwrap(), None);
    }
}
