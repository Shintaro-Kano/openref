//! `/capture/*` の応答契約（Capture 拡張 = extension/popup.js が読む形）を cargo test で固定する。
//! main と同じ `connector_router` を一時ディレクトリの DB で 127.0.0.1 の空きポートに立て、素の HTTP/1.1 で叩く。
//! ネット不要の枝だけ（resolver を呼ぶ枝は `capture_save_unlocked` の resolve 引数で差し替える）。
//! 固定するもの = 各枝の HTTP status code・応答 JSON のキー集合・区分値（status / created / existing 等）。値の全一致は見ない。
//! - 拡張が読むキー（popup.js の grep）: status・key・doi・has_pdf・retraction・trashed・reason・doi_check・bytes・error・
//!   source（fetch-oa の成功 = ネットが要るので対象外）・title・first_author・year・journal・items・collections[].id / path / n_items
//!   （409 は HTTP の status code で見分け、found_doi は読まない）
//! - 拡張は読まないが応答の契約として固定するキー: found_doi・warnings・filename・ok・app・version
//! - :23119 の body 上限（`CONNECTOR_BODY_LIMIT`）: ちょうど上限の body は受け、1 byte 超えると 413

use super::*;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::PathBuf;

/// 一時ディレクトリの DB（添付は DB と同階層の storage/ = managed。in-memory DB は managed 添付ができない）。drop で消す
struct TempLib {
    dir: PathBuf,
    st: Arc<St>,
}

impl Drop for TempLib {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_lib(name: &str) -> TempLib {
    let dir = std::env::temp_dir().join(format!("openref-capture-contract-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Db::open(&dir.join("openref.sqlite")).unwrap();
    // 新しい DB は managed（drive だと Google Drive へ書く）
    assert_eq!(db.pdf_write_mode().unwrap(), "managed");
    let st = Arc::new(St { db: Mutex::new(db), sessions: Mutex::new(Sessions::default()) });
    TempLib { dir, st }
}

async fn serve(st: Arc<St>) -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = connector_router(st);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// 1 往復（Connection: close）。本文は JSON として解く（解けなければ生のまま panic に出す）
async fn call(port: u16, method: &'static str, path: &str, ctype: &'static str, body: Vec<u8>) -> (u16, Value) {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        req.extend_from_slice(&body);
        s.write_all(&req).unwrap();
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let at = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("応答にヘッダの終わりが無い");
        let head = String::from_utf8_lossy(&raw[..at]).to_string();
        let code: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
        let rest = &raw[at + 4..];
        let v: Value = serde_json::from_slice(rest)
            .unwrap_or_else(|e| panic!("{path}: 応答が JSON でない（{e}）: {head}\n{}", String::from_utf8_lossy(rest)));
        (code, v)
    })
    .await
    .unwrap()
}

async fn get_json(port: u16, path: &str) -> (u16, Value) {
    call(port, "GET", path, "application/json", Vec::new()).await
}

async fn post_json(port: u16, path: &str, body: Value) -> (u16, Value) {
    call(port, "POST", path, "application/json", body.to_string().into_bytes()).await
}

async fn post_pdf(port: u16, path: &str, bytes: &[u8]) -> (u16, Value) {
    call(port, "POST", path, "application/pdf", bytes.to_vec()).await
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap_or_else(|| panic!("object でない: {v}")).keys().cloned().collect()
}

fn set(ks: &[&str]) -> BTreeSet<String> {
    ks.iter().map(|k| k.to_string()).collect()
}

/// status code とキー集合の一致（ctx = 失敗時に枝を示す）
fn shape(ctx: &str, got: &(u16, Value), code: u16, ks: &[&str]) {
    assert_eq!(got.0, code, "{ctx}: status code: {}", got.1);
    assert_eq!(keys(&got.1), set(ks), "{ctx}: キー集合: {}", got.1);
}

/// `summarize_item` の形（/capture/save と /capture/pdf の共通戻り値）
const ITEM: [&str; 9] = ["key", "status", "title", "first_author", "year", "journal", "doi", "has_pdf", "retraction"];

fn with(base: &[&'static str], extra: &[&'static str]) -> Vec<&'static str> {
    base.iter().chain(extra).copied().collect()
}

fn add(st: &St, doi: Option<&str>, pmid: Option<&str>, title: &str) -> String {
    let mut csl = json!({ "type": "article-journal", "title": title,
        "author": [{ "family": "Brandao", "given": "A" }], "issued": { "date-parts": [[2026]] }, "container-title": "J Test" });
    if let Some(d) = doi {
        csl["DOI"] = json!(d);
    }
    if let Some(p) = pmid {
        csl["PMID"] = json!(p);
    }
    let mut db = st.db();
    ingest::add_item(&mut db, &resolver::Resolved { csl, retracted: None, source: "test", warnings: vec![] }).unwrap().key
}

fn pdf(text: &str) -> Vec<u8> {
    openref_core::pdftools::minimal_pdf(text, None)
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_status_and_collections() {
    let lib = temp_lib("status");
    let parent = openref_core::collections::create_collection(&lib.st.db(), "Parent", None).unwrap();
    let child = openref_core::collections::create_collection(&lib.st.db(), "Child", Some(parent)).unwrap();
    let key = add(&lib.st, Some("10.1234/s"), None, "Status paper");
    edit::add_to_collection(&lib.st.db(), &key, child).unwrap();
    let port = serve(lib.st.clone()).await;

    let r = get_json(port, "/capture/status").await;
    shape("status", &r, 200, &["ok", "app", "version", "items"]);
    assert_eq!(r.1["ok"], true);
    assert_eq!(r.1["app"], "openref");
    assert_eq!(r.1["items"], 1);

    let r = get_json(port, "/capture/collections").await;
    shape("collections", &r, 200, &["collections"]);
    let cols = r.1["collections"].as_array().unwrap();
    assert_eq!(cols.len(), 2);
    for c in cols {
        assert_eq!(keys(c), set(&["id", "path", "n_items"]), "{c}");
    }
    let c = cols.iter().find(|c| c["id"] == child).unwrap();
    assert_eq!(c["path"], "Parent/Child");
    assert_eq!(c["n_items"], 1);

    // DB が読めない: /capture/status は 200 のまま items=null + error（ok は true のまま = 接続自体は成功）
    // /capture/collections は 500 + {error}（空の一覧に畳まない）
    lib.st.db().conn.execute_batch("DROP TABLE items_fts; DROP TABLE collection_items; DROP TABLE collections; DROP TABLE items;").unwrap();
    let r = get_json(port, "/capture/status").await;
    shape("status（DB 不可）", &r, 200, &["ok", "app", "version", "items", "error"]);
    assert_eq!(r.1["ok"], true);
    assert_eq!(r.1["items"], Value::Null);
    let r = get_json(port, "/capture/collections").await;
    shape("collections（DB 不可）", &r, 500, &["error"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_save_contract() {
    let lib = temp_lib("save");
    let cid = openref_core::collections::create_collection(&lib.st.db(), "Inbox", None).unwrap();
    let k_doi = add(&lib.st, Some("10.1234/abc"), None, "Existing by DOI");
    let k_pmid = add(&lib.st, None, Some("31234567"), "Existing by PMID");
    let port = serve(lib.st.clone()).await;
    let save = with(&ITEM, &["warnings"]);

    // 既存一致（resolver を呼ばない）: 前後空白は capture_save の trim・大文字小文字は照合側（find_duplicate の normalize_doi）が吸収
    // + collection_id でフォルダへ
    let r = post_json(port, "/capture/save", json!({ "doi": " 10.1234/ABC ", "collection_id": cid })).await;
    shape("save existing(doi)", &r, 200, &save);
    assert_eq!(r.1["status"], "existing");
    assert_eq!(r.1["key"], k_doi.as_str());
    assert_eq!(r.1["doi"], "10.1234/abc");
    assert_eq!(r.1["has_pdf"], false);
    assert_eq!(r.1["retraction"], Value::Null);
    assert_eq!(r.1["warnings"], json!([]));
    let c = get_json(port, "/capture/collections").await;
    assert_eq!(c.1["collections"][0]["n_items"], 1, "{}", c.1);

    let r = post_json(port, "/capture/save", json!({ "pmid": "31234567" })).await;
    shape("save existing(pmid)", &r, 200, &save);
    assert_eq!(r.1["status"], "existing");
    assert_eq!(r.1["key"], k_pmid.as_str());

    // 識別子なし・題名あり: document を作る（resolver を呼ばない）
    let r = post_json(port, "/capture/save", json!({ "title": "Page title", "url": "https://example.org/p" })).await;
    shape("save created(題名のみ)", &r, 200, &save);
    assert_eq!(r.1["status"], "created");
    assert_eq!(r.1["doi"], Value::Null);
    assert_eq!(r.1["warnings"], json!([]));

    // 識別子も題名も無い・JSON でない本文 = 422 {error}
    shape("save 識別子なし", &post_json(port, "/capture/save", json!({ "url": "https://example.org/q" })).await, 422, &["error"]);
    let r = call(port, "POST", "/capture/save", "application/json", b"not json".to_vec()).await;
    shape("save JSON でない", &r, 422, &["error"]);

    // resolver を通る新規（継ぎ目で差し替え）: 同じキー集合 + warnings に resolver の警告
    let v = capture_save_unlocked(
        &lib.st,
        SaveReq { doi: Some("10.1234/new".into()), pmid: None, title: None, url: None, collection_id: None },
        |_| {
            Ok(resolver::Resolved {
                csl: json!({ "type": "article-journal", "title": "New", "DOI": "10.1234/new" }),
                retracted: Some("Retraction in: X".into()),
                source: "stub",
                warnings: vec!["PubMed の照合に失敗".into()],
            })
        },
    )
    .unwrap();
    shape("save created(resolver)", &(200, v.clone()), 200, &save);
    assert_eq!(v["status"], "created");
    assert_eq!(v["retraction"], "Retraction in: X");
    assert_eq!(v["warnings"], json!(["PubMed の照合に失敗"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_attachment_contract() {
    let lib = temp_lib("attachment");
    let key = add(&lib.st, Some("10.1234/att"), None, "Attachment paper");
    let no_doi = add(&lib.st, None, None, "Paper without DOI");
    let port = serve(lib.st.clone()).await;
    let att = ["status", "bytes", "filename", "doi_check"];
    let q = |k: &str, source: &str| format!("/capture/attachment?key={k}&source={source}&via=page");

    shape("attachment key なし", &post_pdf(port, "/capture/attachment", &pdf("x")).await, 400, &["error"]);
    shape("attachment PDF でない", &call(port, "POST", &q(&key, "meta"), "application/pdf", b"<html>".to_vec()).await, 422, &["error"]);
    shape("attachment 無い key", &post_pdf(port, &q("NOSUCHKEY", "meta"), &pdf("x")).await, 404, &["error"]);

    // 照合一致 → 201 attached・doi_check match
    let first = pdf("Attachment paper doi:10.1234/att");
    let r = post_pdf(port, &q(&key, "meta"), &first).await;
    shape("attachment attached", &r, 201, &att);
    assert_eq!(r.1["status"], "attached");
    assert_eq!(r.1["doi_check"], "match");
    assert_eq!(r.1["bytes"], first.len());
    // 同じファイル → 200 skipped
    let r = post_pdf(port, &q(&key, "meta"), &first).await;
    shape("attachment skipped", &r, 200, &["status", "reason"]);
    assert_eq!(r.1["status"], "skipped");
    // 別ファイル → 201 attached_additional
    let r = post_pdf(port, &q(&key, "self"), &pdf("Second copy https://doi.org/10.1234/att")).await;
    shape("attachment attached_additional", &r, 201, &att);
    assert_eq!(r.1["status"], "attached_additional");
    assert_eq!(r.1["doi_check"], "match");
    // 宣言所在で目的 DOI が見つからない → 添付して doi_check unknown
    let r = post_pdf(port, &q(&key, "meta"), &pdf("Declared location without any identifier")).await;
    shape("attachment unknown", &r, 201, &att);
    assert_eq!(r.1["doi_check"], "unknown");
    // 別 DOI だけ → 409 mismatch + found_doi
    let r = post_pdf(port, &q(&key, "meta"), &pdf("Other paper doi:10.9999/other")).await;
    shape("attachment mismatch", &r, 409, &["status", "found_doi", "error"]);
    assert_eq!(r.1["status"], "mismatch");
    assert_eq!(r.1["found_doi"], "10.9999/other");
    // ページ上の .pdf リンク（source=link）で目的 DOI を確認できない → 422 unverified
    let r = post_pdf(port, &q(&key, "link"), &pdf("Instructions for authors")).await;
    shape("attachment unverified", &r, 422, &["status", "error"]);
    assert_eq!(r.1["status"], "unverified");
    // item に DOI が無い（照合不能）・宣言所在 → 添付・doi_check null
    let r = post_pdf(port, &q(&no_doi, ""), &pdf("Paper without DOI body")).await;
    shape("attachment unchecked", &r, 201, &att);
    assert_eq!(r.1["status"], "attached");
    assert_eq!(r.1["doi_check"], Value::Null);

    // 添付後の /capture/save は has_pdf=true（popup が PDF 経路を飛ばす枝）
    let r = post_json(port, "/capture/save", json!({ "doi": "10.1234/att" })).await;
    shape("save has_pdf", &r, 200, &with(&ITEM, &["warnings"]));
    assert_eq!(r.1["has_pdf"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_pdf_contract() {
    let lib = temp_lib("pdf");
    let cid = openref_core::collections::create_collection(&lib.st.db(), "Inbox", None).unwrap();
    let key = add(&lib.st, Some("10.1234/p"), None, "Pdf paper");
    let port = serve(lib.st.clone()).await;

    shape("pdf PDF でない", &call(port, "POST", "/capture/pdf", "application/pdf", b"<html>".to_vec()).await, 422, &["error"]);

    // テキスト層なし（識別子も題名検索の材料も無い = ネットに出ない）→ 要確認 stub + reason・collection_id でフォルダへ
    let blank = pdf("");
    let r = post_pdf(port, &format!("/capture/pdf?collection_id={cid}&title=Tab%20title&url=https%3A%2F%2Fexample.org%2Fa.pdf"), &blank).await;
    shape("pdf stub", &r, 200, &with(&ITEM, &["reason"]));
    assert_eq!(r.1["status"], "stub");
    assert_eq!(r.1["has_pdf"], true);
    let stub = r.1["key"].as_str().unwrap().to_string();
    let c = get_json(port, "/capture/collections").await;
    assert_eq!(c.1["collections"][0]["n_items"], 1, "{}", c.1);
    // 同じ PDF → duplicate（相手が要確認 stub なので reason で案内）
    let r = post_pdf(port, "/capture/pdf", &blank).await;
    shape("pdf duplicate(stub)", &r, 200, &with(&ITEM, &["reason"]));
    assert_eq!(r.1["status"], "duplicate");
    assert_eq!(r.1["key"], stub.as_str());

    // 本文を読めない PDF → stub（解決失敗の理由を reason に）
    let r = post_pdf(port, "/capture/pdf", b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n").await;
    shape("pdf stub(抽出不可)", &r, 200, &with(&ITEM, &["reason"]));
    assert_eq!(r.1["status"], "stub");

    // 通常の item に添付済みの PDF → duplicate（reason なし）
    let body = pdf("Pdf paper doi:10.1234/p");
    assert_eq!(post_pdf(port, &format!("/capture/attachment?key={key}&source=meta"), &body).await.0, 201);
    let r = post_pdf(port, "/capture/pdf", &body).await;
    shape("pdf duplicate", &r, 200, &ITEM);
    assert_eq!(r.1["status"], "duplicate");
    assert_eq!(r.1["key"], key.as_str());

    // 相手がゴミ箱内 → trashed の最小形（+ 相手が要確認 stub なら reason）
    edit::trash(&lib.st.db(), &stub).unwrap();
    let r = post_pdf(port, "/capture/pdf", &blank).await;
    shape("pdf duplicate(ゴミ箱)", &r, 200, &["key", "status", "trashed", "reason"]);
    assert_eq!(r.1["status"], "duplicate");
    assert_eq!(r.1["trashed"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_fetch_oa_and_fallback_contract() {
    let lib = temp_lib("fetchoa");
    let key = add(&lib.st, Some("10.1234/oa"), None, "Oa paper");
    let port = serve(lib.st.clone()).await;

    shape("fetch-oa key なし", &post_json(port, "/capture/fetch-oa", json!({})).await, 400, &["error"]);
    shape("fetch-oa JSON でない", &call(port, "POST", "/capture/fetch-oa", "application/json", b"x".to_vec()).await, 400, &["error"]);
    // ネットに出る前に失敗する枝: 無い key・既に PDF がある item → 422 {error}
    shape("fetch-oa 無い key", &post_json(port, "/capture/fetch-oa", json!({ "key": "NOSUCHKEY" })).await, 422, &["error"]);
    assert_eq!(post_pdf(port, &format!("/capture/attachment?key={key}&source=meta"), &pdf("Oa paper doi:10.1234/oa")).await.0, 201);
    shape("fetch-oa 添付済み", &post_json(port, "/capture/fetch-oa", json!({ "key": key })).await, 422, &["error"]);

    // 未知のエンドポイントは 200 {}（寛容に受けてログ）
    shape("fallback", &get_json(port, "/capture/unknown").await, 200, &[]);
}

/// body 上限の要求を 1 本送り、status code だけ返す（413 の本文は JSON でない）。body は 1 MiB の塊を繰り返して書く
/// （上限ぶんを手元に確保しない）。サーバが 413 を返して読むのをやめたら書き込みは失敗しうる → 応答は別スレッドで読む
async fn post_sized(port: u16, path: &'static str, len: usize) -> u16 {
    tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut r = s.try_clone().unwrap();
        let reader = std::thread::spawn(move || {
            let mut raw = Vec::new();
            let _ = r.read_to_end(&mut raw);
            raw
        });
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/pdf\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
        );
        s.write_all(head.as_bytes()).unwrap();
        let mut chunk = vec![b' '; 1 << 20];
        chunk[..4].copy_from_slice(b"%PDF");
        let mut left = len;
        while left > 0 {
            let n = left.min(chunk.len());
            if s.write_all(&chunk[..n]).is_err() {
                break;
            }
            chunk[..4].copy_from_slice(b"    ");
            left -= n;
        }
        let raw = reader.join().unwrap();
        let head = String::from_utf8_lossy(&raw[..raw.len().min(64)]).to_string();
        head.split(' ').nth(1).unwrap_or_else(|| panic!("応答が無い: {head:?}")).parse().unwrap()
    })
    .await
    .unwrap()
}

/// :23119 の body 上限: ちょうど上限の body は extractor を通って handler に届き（key が無いので 400 = 413 ではない）、
/// 1 byte 超えると axum が 413。上限の削除・縮小をここで検出する（値は本番の定数をそのまま使う）
#[tokio::test(flavor = "multi_thread")]
async fn capture_body_limit() {
    let lib = temp_lib("limit");
    let port = serve(lib.st.clone()).await;
    assert_eq!(post_sized(port, "/capture/attachment", CONNECTOR_BODY_LIMIT).await, 400, "上限ちょうどは受ける");
    assert_eq!(post_sized(port, "/capture/attachment", CONNECTOR_BODY_LIMIT + 1).await, 413, "上限超えは 413");
}
