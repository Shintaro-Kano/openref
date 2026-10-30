//! PDF 自動取得。取得経路は「文書化された API」だけを使い、スクレイピングはしない。
//!
//! 判定と取得を分離する — どちらも **PMC AWS Open Data**（`pmc-oa-opendata` S3・無料・認証不要）:
//! - **判定** = S3 の ListObjectsV2（prefix `PMC<id>.`）で収載の有無と version を知り、同階層の
//!   `PMC<id>.<v>/PMC<id>.<v>.json` を読んで `is_pmc_openaccess` / `license_code` / `pdf_url`
//!   （`?md5=` 付き）を得る。KeyCount 0 = 未収載（OA サブセット外か未同期）、
//!   `is_pmc_openaccess: false` = author manuscript 等の TDM 限定（自動取得不可）と誠実に報告する。
//!   **本体 PDF は json の `pdf_url` だけを信じる**: `media_urls` には補足資料（`…-s001.pdf` / `MOESM1_ESM.pdf` /
//!   `mmc1.pdf`）の PDF が並び、本体 PDF の無い記事（JAMA 系・Lancet Glob Health・BMJ 等）で
//!   一覧の `.pdf` を拾うと補足資料を論文本体として添付してしまう。
//!   json が読めない異常系だけ一覧の `PMC<id>.<v>/PMC<id>.<v>.pdf` 完全一致で代替する。
//!   json の `pmcid` / `doi` は item の値と照合し、食い違えば添付しない（取り違え防止）。
//! - **取得** = json の `pdf_url`（`s3://pmc-oa-opendata/PMC<id>.<v>/PMC<id>.<v>.pdf?md5=…`）を
//!   https に直して GET し、**md5 を照合してから**添付する。
//!
//! 背景: NCBI は 2026-02 に FTP レイアウトを廃止して AWS Open Data へ移行し（旧 `oa_package/` は
//! 2026-08 に削除）、**2026-08-25 に PMC OA Web Service API（`oa.fcgi`）も廃止した**（全 ID で 404）。
//! oa.fcgi で OA 判定すると全 PMCID で判定段階に失敗し AWS 取得へ進めないので使わない。
//! S3 一覧 + json が唯一の判定根拠（legacy tar.gz 経路は持たない）。
//!
//! 続いて **Unpaywall**（`OPENREF_EMAIL` 設定時のみ opt-in）— 出版社ホストの OA PDF。その後 DOI 起点の経路 3・4（後述）。
//!
//! paywall 論文はサーバ側からは取得しない（できない）。その場合はブラウザの Capture 拡張
//! （user の認証済み session で取得）へ誠実に誘導する。
//!
//! **DOI 起点の連鎖（経路 3・4）・取得物の同一性照合・3 段 API（DB の Mutex をネットワークの間に握らない）**は
//! 本ファイル後半の `doi_fallback_download` 冒頭コメントと `fetch_pdf_prepare` / `fetch_pdf_download` / `fetch_pdf_attach` を参照。
//! 照合結果 → 採否（添付するか・`doi_check` の印は何か）は **`verdict` 1 関数**が全経路（connectd の `/capture/attachment`・
//! connector の保険 DL を含む）の正で、`verdict_table` テストが全組合せを固定する。

use crate::http::{fetch_agent as agent, fetch_agent_no_redirect, text};
use crate::{edit, model, search, CoreError, Db, Result};
use serde_json::Value;

#[derive(Debug)]
pub struct Fetched {
    pub source: String,
    pub filename: String,
    pub bytes_len: usize,
}

/// 3 段 API: connectd / Tauri は DB を Mutex で持つため、ネットワークの間（連鎖が長いと数分）に
/// ロックを握らないよう「① DB 読取 → ② ロック外でネットワーク → ③ 再ロックして添付」に分ける。
/// CLI / MCP（Mutex なし）は便宜関数 `fetch_pdf` でよい。
#[derive(Debug, Clone)]
pub struct FetchPlan {
    pub key: String,
    pub csl: Value,
}

/// ② の結果（まだ添付していない）。`doi_check` は添付行に書く照合結果（'match' / 'unknown' / None = 印なし）
#[derive(Debug)]
pub struct Downloaded {
    pub bytes: Vec<u8>,
    pub source: String,
    pub doi_check: Option<&'static str>,
}

/// ① item を読み、既に PDF 添付があれば拒否する（DB 読取のみ・短い）
pub fn fetch_pdf_prepare(db: &Db, key: &str) -> Result<FetchPlan> {
    let item = search::get_by_key(db, key)?
        .ok_or_else(|| CoreError::NotFound(format!("item {key}")))?;
    if has_pdf(db, item.id)? {
        return Err(CoreError::BadInput(format!(
            "item {key} には既に PDF 添付があります"
        )));
    }
    Ok(FetchPlan { key: key.to_string(), csl: item.csl })
}

fn has_pdf(db: &Db, item_id: i64) -> Result<bool> {
    let n: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM attachments WHERE item_id = ?1 AND kind = 'pdf'",
        [item_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// ② 連鎖を回す（DB に触らない）: PMC → Unpaywall → Crossref link[] → doi.org 着地ページ。
/// 取れた PDF は `check_pdf_identity` で目的の論文か照合し、採否は所在の種類ごとに `verdict` が決める（`judge` 経由）。
pub fn fetch_pdf_download(plan: &FetchPlan) -> Result<Downloaded> {
    let csl = &plan.csl;
    let mut tried: Vec<String> = Vec::new();

    // --- 経路 1: PMC AWS Open Data（S3 一覧 + json で OA 判定 → 同じバケットから取得）---
    if let Some(pmcid) = model::s(csl, "PMCID") {
        match pmc_oa_lookup(pmcid, model::s(csl, "DOI")) {
            Ok(OaLookup::Available { key: s3_key, license, md5 }) => {
                match aws_fetch_pdf(&s3_key, md5.as_deref()) {
                    // 採否は `verdict`（Pmc = PMCID + json の pmcid/doi 照合で同一性が担保済み → 別 DOI だけ拒否・Unknown は印なし）
                    Ok(bytes) => match judge(LocationKind::Pmc, csl, bytes, "pmc", format!("pmc-aws ({license})")) {
                        Ok(dl) => return Ok(dl),
                        Err(note) => tried.push(note),
                    },
                    Err(e) => tried.push(format!("pmc: {e}")),
                }
            }
            Ok(OaLookup::NotOpenAccess { license, manuscript }) => tried.push(format!(
                "pmc: {pmcid} は OA サブセット外（{}license {license}・自動取得不可）",
                if manuscript { "author manuscript・" } else { "" }
            )),
            Ok(OaLookup::NoPdf { license }) => {
                tried.push(format!("pmc: {pmcid} は OA（{license}）だが本体 PDF なし（XML / 補足資料のみ）"))
            }
            Ok(OaLookup::NotListed) => tried.push(format!(
                "pmc: {pmcid} は AWS Open Data に未収載（OA サブセット外か未同期）"
            )),
            Ok(OaLookup::Mismatch { reason }) => tried.push(format!("pmc: {pmcid} の収載 json が item と食い違う（{reason}）→ 添付しない")),
            Err(e) => tried.push(format!("pmc: {e}")),
        }
    } else {
        tried.push("pmc: PMCID なし".into());
    }

    // --- 経路 2: Unpaywall（opt-in）。文書化 API が宣言した所在（Declared → Unknown は印を付けて添付）。この段だけ http も許す ---
    match crate::env_os("EMAIL").and_then(|v| v.into_string().ok()) {
        Some(email) if !email.trim().is_empty() => {
            if let Some(doi) = model::s(csl, "DOI") {
                match unpaywall_pdf_url(doi, email.trim()) {
                    Ok(Some(url)) => match download_pdf(&url, None, true) {
                        Ok(bytes) => match judge(LocationKind::Declared, csl, bytes, "unpaywall", "unpaywall".into()) {
                            Ok(dl) => return Ok(dl),
                            Err(note) => tried.push(note),
                        },
                        Err(e) => tried.push(format!("unpaywall: {e}")),
                    },
                    Ok(None) => tried.push("unpaywall: OA 版なし".into()),
                    Err(e) => tried.push(format!("unpaywall: {e}")),
                }
            } else {
                tried.push("unpaywall: DOI なし".into());
            }
        }
        _ => tried.push("unpaywall: OPENREF_EMAIL 未設定（opt-in）".into()),
    }

    // --- 経路 3・4: DOI 起点（Crossref link → doi.org 着地ページ。本体は後半の doi_fallback_download）---
    if let Some(dl) = doi_fallback_download(csl, &mut tried) {
        return Ok(dl);
    }

    Err(CoreError::NotFound(format!(
        "PDF を自動取得できませんでした — {}。paywall 論文はサーバ側からは取得できません（設計どおり）。ブラウザで PDF を開いた状態で取り込んでください",
        tried.join(" / ")
    )))
}

/// ③ 添付して照合結果（`doi_check` = 'match' / 'unknown' / None = 印なし）を添付行に記録する。
/// ② の間にロックを離しているので、別経路で添付済みになっていたら拒否する
pub fn fetch_pdf_attach(db: &Db, plan: &FetchPlan, dl: Downloaded) -> Result<Fetched> {
    let item = search::get_by_key(db, &plan.key)?
        .ok_or_else(|| CoreError::NotFound(format!("item {}", plan.key)))?;
    if has_pdf(db, item.id)? {
        return Err(CoreError::Conflict(format!(
            "item {} には取得中に別経路で PDF が添付されました（{}・{} bytes は破棄）",
            plan.key,
            dl.source,
            dl.bytes.len()
        )));
    }
    let filename = suggest_filename(&plan.csl);
    let attached = edit::attach_auto_bytes(db, &plan.key, &filename, &dl.bytes)?;
    if let Some(mark) = dl.doi_check {
        mark_doi_check(db, attached.id, mark);
    }
    Ok(Fetched { source: dl.source, filename, bytes_len: dl.bytes.len() })
}

/// 便宜関数: ①②③を続けて行う（CLI / MCP など Mutex を持たない呼び手向け）
pub fn fetch_pdf(db: &Db, key: &str) -> Result<Fetched> {
    let plan = fetch_pdf_prepare(db, key)?;
    let dl = fetch_pdf_download(&plan)?;
    fetch_pdf_attach(db, &plan, dl)
}

/// AWS Open Data での OA 判定結果（`pmc_oa_lookup`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OaLookup {
    /// OA サブセット内で PDF あり。`key` は S3 オブジェクトキー、`md5` は json の `pdf_url` に付く検証値
    Available { key: String, license: String, md5: Option<String> },
    /// 収載はあるが OA サブセット外（author manuscript / TDM 限定ライセンス等）→ 自動取得不可
    NotOpenAccess { license: String, manuscript: bool },
    /// OA サブセット内だが PDF オブジェクトが無い（XML / txt のみの記事）
    NoPdf { license: String },
    /// バケットに無い（OA サブセット外か未同期）
    NotListed,
    /// 収載 json の pmcid / doi が item と食い違う（取り違え防止のため添付しない）
    Mismatch { reason: String },
}

pub const AWS_OA: &str = "https://pmc-oa-opendata.s3.amazonaws.com";
const AWS_BUCKET: &str = "pmc-oa-opendata";

/// PMCID の表記ゆれを吸収（`pmc123` / `123` → `PMC123`）。
pub fn normalize_pmcid(s: &str) -> String {
    let t = s.trim();
    let digits = t
        .strip_prefix("PMC")
        .or_else(|| t.strip_prefix("pmc"))
        .or_else(|| t.strip_prefix("Pmc"))
        .unwrap_or(t);
    format!("PMC{}", digits.trim())
}

/// S3 ListObjectsV2（prefix `PMC<id>.`）→ 最新 version の json を読み、OA 可否と PDF キーを返す。
/// `expect_doi` は item の DOI（json の doi と照合。None なら照合しない）。
/// ネットワークを使うのはここと `aws_fetch_pdf` だけ（解釈は `interpret_oa` で単体テスト可能）。
pub fn pmc_oa_lookup(pmcid: &str, expect_doi: Option<&str>) -> std::result::Result<OaLookup, String> {
    let pmcid = normalize_pmcid(pmcid);
    let keys = list_prefix(&format!("{pmcid}."))?;
    let Some(prefix) = latest_version_prefix(&keys, &pmcid) else {
        return Ok(OaLookup::NotListed);
    };
    let json_key = format!("{prefix}/{prefix}.json");
    let json = if keys.iter().any(|k| k == &json_key) {
        Some(text(agent().get(&format!("{AWS_OA}/{json_key}")).call(), |e| format!("{json_key} 取得失敗: {e}"))?)
    } else {
        None
    };
    Ok(interpret_oa(json.as_deref(), &keys, &prefix, &pmcid, expect_doi))
}

/// prefix 配下の全 Key（IsTruncated なら continuation-token で続きを辿る。1 記事で 1000 件超は想定外だが取りこぼさない）
fn list_prefix(prefix: &str) -> std::result::Result<Vec<String>, String> {
    let mut keys = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..10 {
        let mut req = agent().get(AWS_OA).query("list-type", "2").query("prefix", prefix);
        if let Some(t) = &token {
            req = req.query("continuation-token", t);
        }
        let xml = text(req.call(), |e| format!("S3 一覧取得失敗: {e}"))?;
        keys.extend(listing_keys(&xml));
        match listing_next_token(&xml) {
            Some(t) => token = Some(t),
            None => return Ok(keys),
        }
    }
    Err(format!("S3 一覧が長すぎます（prefix {prefix}・10 ページ超）"))
}

/// `<IsTruncated>true</IsTruncated>` のとき `<NextContinuationToken>` を返す
pub fn listing_next_token(xml: &str) -> Option<String> {
    if !xml.contains("<IsTruncated>true</IsTruncated>") {
        return None;
    }
    let t = xml.split("<NextContinuationToken>").nth(1)?.split("</NextContinuationToken>").next()?;
    // XML エスケープ（&amp; 等）を戻す
    Some(t.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\""))
}

/// S3 ListObjectsV2 の XML から全 Key を拾う（ネットワーク非依存＝テスト可能）
pub fn listing_keys(xml: &str) -> Vec<String> {
    xml.split("<Key>")
        .skip(1)
        .filter_map(|seg| seg.split("</Key>").next())
        .map(str::to_string)
        .collect()
}

/// 一覧の Key（`PMC<id>.<v>/…`）から最新 version のディレクトリ名（`PMC<id>.<v>`）を返す。
/// 該当 PMCID のキーが無ければ None（= 未収載）。
pub fn latest_version_prefix(keys: &[String], pmcid: &str) -> Option<String> {
    let want = format!("{pmcid}.");
    keys.iter()
        .filter_map(|k| k.split('/').next())
        .filter_map(|dir| {
            dir.strip_prefix(&want)
                .and_then(|v| v.parse::<u32>().ok())
                .map(|v| (v, dir.to_string()))
        })
        .max_by_key(|(v, _)| *v)
        .map(|(_, dir)| dir)
}

/// `s3://pmc-oa-opendata/<key>?md5=<hex>` → (`<key>`, Some(hex))。他バケット・不正形式は None。
pub fn s3_url_parts(url: &str) -> Option<(String, Option<String>)> {
    let rest = url.strip_prefix("s3://")?;
    let (bucket, path) = rest.split_once('/')?;
    if bucket != AWS_BUCKET || path.is_empty() {
        return None;
    }
    let (key, query) = match path.split_once('?') {
        Some((k, q)) => (k, Some(q)),
        None => (path, None),
    };
    let md5 = query.and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("md5="))
            .filter(|h| h.len() == 32 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|h| h.to_lowercase())
    });
    Some((key.to_string(), md5))
}

/// json（あれば）と一覧から OA 可否を決める（ネットワーク非依存＝テスト可能）。
/// - json の `pmcid` / `doi` が item と食い違う → Mismatch（添付しない）
/// - `is_pmc_openaccess` が明示的に false → NotOpenAccess（manuscript / TDM）
/// - 本体 PDF は json の `pdf_url` のみ（`media_urls` の補足 PDF は本体ではない）。無ければ NoPdf
/// - json が無い / 壊れている（異常系）ときだけ、一覧の `PMC<id>.<v>/PMC<id>.<v>.pdf` 完全一致で代替（license は unknown）
pub fn interpret_oa(
    json: Option<&str>,
    keys: &[String],
    prefix: &str,
    pmcid: &str,
    expect_doi: Option<&str>,
) -> OaLookup {
    let dir = format!("{prefix}/");
    let v: Option<Value> = json.and_then(|j| serde_json::from_str(j).ok());
    let Some(v) = v else {
        let body = format!("{prefix}/{prefix}.pdf");
        return if keys.iter().any(|k| k == &body) {
            OaLookup::Available { key: body, license: "unknown".into(), md5: None }
        } else {
            OaLookup::NoPdf { license: "unknown".into() }
        };
    };
    let license = v
        .get("license_code")
        .and_then(|l| l.as_str())
        .filter(|l| !l.trim().is_empty())
        .unwrap_or("unknown")
        .to_string();
    if let Some(j) = v.get("pmcid").and_then(|x| x.as_str()) {
        if normalize_pmcid(j) != normalize_pmcid(pmcid) {
            return OaLookup::Mismatch { reason: format!("json pmcid {j} ≠ {pmcid}") };
        }
    }
    if let (Some(want), Some(j)) = (expect_doi, v.get("doi").and_then(|x| x.as_str())) {
        if model::normalize_doi(j) != model::normalize_doi(want) {
            return OaLookup::Mismatch { reason: format!("json doi {j} ≠ item DOI {want}") };
        }
    }
    let manuscript = v.get("is_manuscript").and_then(|b| b.as_bool()).unwrap_or(false);
    if v.get("is_pmc_openaccess").and_then(|b| b.as_bool()) == Some(false) {
        return OaLookup::NotOpenAccess { license, manuscript };
    }
    match v.get("pdf_url").and_then(|u| u.as_str()).and_then(s3_url_parts) {
        Some((key, md5)) if key.starts_with(&dir) => OaLookup::Available { key, license, md5 },
        _ => OaLookup::NoPdf { license },
    }
}

/// AWS Open Data から PDF を取り、`%PDF` と（あれば）md5 を検証する。
fn aws_fetch_pdf(key: &str, expect_md5: Option<&str>) -> std::result::Result<Vec<u8>, String> {
    let bytes = fetch_bytes(&format!("{AWS_OA}/{key}"))?;
    if !bytes.starts_with(b"%PDF") {
        return Err(format!("{key}: PDF ではない応答（{} bytes）", bytes.len()));
    }
    if let Some(want) = expect_md5 {
        let got = md5_hex(&bytes);
        if got != want {
            return Err(format!("{key}: md5 不一致（期待 {want}・実際 {got}・転送破損の可能性）"));
        }
    }
    Ok(bytes)
}

fn unpaywall_pdf_url(doi: &str, email: &str) -> std::result::Result<Option<String>, String> {
    let sent = agent()
        .get(&format!("https://api.unpaywall.org/v2/{}", encode_doi_path(doi)))
        .query("email", email)
        .call();
    let body = text(sent, |e| e.to_string())?;
    let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(v.pointer("/best_oa_location/url_for_pdf")
        .and_then(|u| u.as_str())
        .map(String::from))
}

/// AWS S3（固定ホスト `AWS_OA`・URL は自前で組む）専用の GET。外部 URL 用の `http_get` と違い、SSRF ガードも手動リダイレクトも
/// 要らず（ureq 既定の追従）、403 を出版社の bot 対策とは読み替えない。外部 URL からの取得は `download_pdf`（= `http_get`）を使うこと
fn fetch_bytes(url: &str) -> std::result::Result<Vec<u8>, String> {
    let resp = agent().get(url).call().map_err(|e| e.to_string())?;
    let (bytes, truncated) = read_capped(resp.into_reader(), PDF_CAP).map_err(|e| e.to_string())?;
    if truncated {
        return Err(format!("上限 {} MB を超えたので取得しません", PDF_CAP / (1024 * 1024)));
    }
    Ok(bytes)
}

// ================= DOI 起点の連鎖（経路 3・4）=================
//
// PMCID が無い / PMC にも Unpaywall にも無い論文でも、DOI から出版社の PDF に届くことが多い
// （Ann Ital Chir・Elsevier の OA 論文など。DOI を手でクリックすれば PDF に届く）。
// そこで DOI を起点に 2 段を足す:
//
// 3. **Crossref `works/<doi>` の `link[]`** — `content-type: application/pdf` かつ
//    `intended-application` が text-mining 以外のもの（文書化された API・あれば）
// 4. **`https://doi.org/<doi>` の着地ページ** — HTTP リダイレクト（手動追従・各ホップを検査）+ meta refresh を
//    追従し、着地 HTML から PDF の所在を次の順で読む: `citation_pdf_url`（Highwire / Google Scholar 規約のメタタグ）→
//    `link[type=application/pdf]` → meta refresh（中継ページ）→ **同一ホストでパスが `.pdf` で終わる `<a href>` が
//    ちょうど 1 つ**のときだけそれ（ヒューリスティック）。ダウンロードして `%PDF` 検証
//
// **原則の緩和（明記）**: 書誌はページから一切読まない（PubMed / Crossref で機械確定）。
// 一方 **PDF の所在だけ**は、出版社が機械向けに宣言するメタタグ / リンクから読んでよい。
// ページの文章・著者・誌名を解釈しない = 書誌のスクレイピングではない。
//
// **取得物の同一性**: 取れた PDF は `check_pdf_identity`（冒頭 2 頁の本文に目的 DOI があるか）で
// 照合し、採否（添付するか・`doi_check` の印）は所在の種類 `LocationKind` ごとに **`verdict` 1 箇所**で決める（表はそこ・
// 全組合せは `verdict_table` テスト）。合本 `10.11152/mu-4311` の実例（p.1 は mu-4292）= Mismatch → どの経路でも添付しない。
// 連鎖の順序規則（`verdict` の外・この関数）: 経路 3 / 4 の宣言所在で Match が出なかった候補は保留し、後の段に Match が出ればそちらを採る。
// `/capture/attachment`（宣言所在 = meta / self / atypon・ヒューリスティック = source=link → Mismatch 409 / Unverified 422）と
// connector の保険 DL（translator が宣言した URL = Declared）も同じ `verdict` を通す。
//
// **安全**: 取得先 URL は https のみ（Unpaywall が返す URL だけ http も可）・loopback / private / link-local / CGNAT /
// localhost を `url::Url` のホスト解釈（数値形 IPv4 `2130706433` / `0x7f.0.0.1` / `127.1` も正規化される）で拒否
// （`citation_pdf_url` に `http://127.0.0.1:23119/…` が書かれていても connectd が自分を呼ばない）。
// DOI は URL パスとして percent-encode する（`#` `<` `>` `;` を含む SICI 形で fragment に切れないように）。
//
// 各段の結果は `tried` に積み、全滅時はエラー文に全部出す。cookie なし・UA は `agent()` のまま。
// bot 対策で HTML / 403 が返る出版社（Elsevier 等）は 4 で誠実に失敗し、拡張の背景タブ経路
// （ブラウザの認証済み session で取得）へ誘導する。

/// 経路 3・4 を順に試す（DB に触らない）。取れて同一性方針を通ったら `Some(Downloaded)`、
/// どの段でも取れなければ理由を `tried` に積んで `None`（呼び出し側が全体のエラー文を組む）。
fn doi_fallback_download(csl: &Value, tried: &mut Vec<String>) -> Option<Downloaded> {
    let Some(doi) = model::s(csl, "DOI") else {
        tried.push("crossref/doi.org: DOI なし".into());
        return None;
    };
    // 宣言された所在で Match が出なかった添付候補（経路 4 で Match が出なければこれを添付。印は `verdict` が決める）
    let mut held: Option<Downloaded> = None;

    // 経路 3: Crossref works/<doi> の link[]（application/pdf・text-mining 以外）= 宣言所在
    match crossref_pdf_links(doi) {
        Ok(urls) if urls.is_empty() => tried.push("crossref: link[] に PDF なし".into()),
        Ok(urls) => {
            for url in urls {
                match download_pdf(&url, None, false) {
                    Ok(bytes) => match judge(LocationKind::Declared, csl, bytes, "crossref", "crossref-link".into()) {
                        Ok(dl) if dl.doi_check == Some("match") => return Some(dl),
                        Ok(dl) if held.is_none() => held = Some(dl),
                        Ok(_) => tried.push(format!("crossref: {url} も DOI 未確認（先の候補を保持）")),
                        Err(note) => tried.push(note),
                    },
                    Err(e) => tried.push(format!("crossref: {url} → {e}")),
                }
            }
        }
        Err(e) => tried.push(format!("crossref: {e}")),
    }

    // 経路 4: doi.org 着地ページの citation_pdf_url（PDF の所在のみ・書誌は読まない）。所在の種類は `lp.kind`（宣言 / ヒューリスティック）
    match landing_page_pdf(doi) {
        Ok(lp) => match judge(lp.kind, csl, lp.bytes, "doi.org", format!("publisher-page ({})", lp.host)) {
            Ok(dl) if dl.doi_check == Some("match") => return Some(dl),
            Ok(dl) if held.is_none() => held = Some(dl),
            Ok(_) => tried.push("doi.org: 着地ページの PDF も DOI 未確認（Crossref の候補を採用）".into()),
            Err(note) => tried.push(note),
        },
        Err(e) => tried.push(format!("doi.org: {e}")),
    }

    held
}

/// 連鎖の 4 段が共有する採否の適用: 取れたバイト列を照合し `verdict` に通す。添付候補なら `Downloaded`（`doi_check` = 添付行に
/// 書く印。'unknown' のときは source に「・DOI 未確認」を添える）。添付しないなら `tried` に積む 1 行（「<stage>: …（n bytes・添付せず）」）を Err で返す
fn judge(
    kind: LocationKind,
    csl: &Value,
    bytes: Vec<u8>,
    stage: &str,
    source: String,
) -> std::result::Result<Downloaded, String> {
    match verdict(kind, &check_pdf_identity(&bytes, csl)) {
        Verdict::Attach { doi_check } => {
            let source = if doi_check == Some("unknown") { format!("{source}・DOI 未確認") } else { source };
            Ok(Downloaded { bytes, source, doi_check })
        }
        Verdict::Mismatch { found_doi } => {
            Err(format!("{stage}: 別論文（DOI {found_doi}）の PDF でした（{} bytes・添付せず）", bytes.len()))
        }
        Verdict::Unverified { why } => Err(format!(
            "{stage}: {source} の .pdf リンク（ヒューリスティック所在）は{why}（{} bytes・添付せず）",
            bytes.len()
        )),
    }
}

// ---------------- 採否（照合結果 × 所在の種類 → 添付するか・印は何か） ----------------

/// 取得した PDF の所在の種類 = その所在の同一性をどこまで外部が担保しているか。`verdict` が採否を決める軸
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationKind {
    /// PMC AWS Open Data（PMCID + 収載 json の pmcid / doi 照合で同一性が担保済み）
    Pmc,
    /// 出版社 / 文書化 API が宣言した所在（Unpaywall・Crossref `link[]`・`citation_pdf_url` / `link[type=application/pdf]`・
    /// `/capture/attachment` の meta / self / atypon・connector translator の宣言 URL）
    Declared,
    /// ヒューリスティックで選んだ所在（同一ホストの `.pdf` リンク 1 本規則・`/capture/attachment` の `source=link`）
    Heuristic,
}

/// `verdict` の結果 = 取れた PDF をどうするか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 添付する。`doi_check` は添付行に書く印: Some("match") / Some("unknown")（GUI に「DOI 未確認」）/ None（印なし = 未照合）
    Attach { doi_check: Option<&'static str> },
    /// 添付しない: 1 頁目に別の DOI だけがある（合本・別論文・補遺）。連鎖は次の段へ・`/capture/attachment` は 409
    Mismatch { found_doi: String },
    /// 添付しない: ヒューリスティック所在で目的 DOI を確認できない（照合不能を含む）。`why` は述語
    /// （「本文にこの文献の DOI が見つからない」/「照合できない（…）」）。連鎖は次の段へ・`/capture/attachment` は 422
    Unverified { why: String },
}

/// 照合結果（`check_pdf_identity`）と所在の種類から採否を決める **唯一の場所**。
/// 全組合せは `verdict_table` テストが固定する。方針を変えるときは表・この関数・テストを一緒に直す:
///
/// | 所在 ＼ 照合 | Match | Mismatch | Unknown（印字なし） | Unchecked（照合不能） |
/// |---|---|---|---|---|
/// | Pmc | 添付 'match' | 拒否 | 添付 印なし | 添付 印なし |
/// | Declared | 添付 'match' | 拒否 | 添付 'unknown' | 添付 印なし |
/// | Heuristic | 添付 'match' | 拒否 | 拒否 | 拒否 |
///
/// 核 = 「目的 DOI の印字が無い（Unknown）PDF は、宣言所在なら印を付けて添付・ヒューリスティック所在なら添付しない」。
/// Unchecked（item に DOI が無い・本文を抽出できない）は照合そのものが無かったので印を付けず「別論文の可能性」の警告も出さない。
pub fn verdict(kind: LocationKind, check: &IdentityCheck) -> Verdict {
    match (kind, check) {
        (_, IdentityCheck::Match) => Verdict::Attach { doi_check: Some("match") },
        (_, IdentityCheck::Mismatch { found_doi }) => Verdict::Mismatch { found_doi: found_doi.clone() },
        (LocationKind::Heuristic, IdentityCheck::Unknown) => {
            Verdict::Unverified { why: "本文にこの文献の DOI が見つからない".into() }
        }
        (LocationKind::Heuristic, IdentityCheck::Unchecked { reason }) => {
            Verdict::Unverified { why: format!("照合できない（{reason}）") }
        }
        (LocationKind::Declared, IdentityCheck::Unknown) => Verdict::Attach { doi_check: Some("unknown") },
        (LocationKind::Pmc, IdentityCheck::Unknown) | (_, IdentityCheck::Unchecked { .. }) => {
            Verdict::Attach { doi_check: None }
        }
    }
}

/// 添付ファイルの md5（16 進）。取込・添付・索引はこれを 1 度だけ取って使い回す（md5 crate は core の依存）
pub fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

/// 直前に添付した行（`edit::Attached::id`・未記入のもの）に照合結果を書く。`/capture/attachment` からも使う。
pub fn mark_doi_check(db: &Db, attachment_id: i64, mark: &str) {
    if let Err(e) = db.conn.execute(
        "UPDATE attachments SET doi_check = ?1 WHERE id = ?2 AND doi_check IS NULL",
        rusqlite::params![mark, attachment_id],
    ) {
        // 握りつぶすと「DOI 未確認」の表示が黙って出ないままになる
        eprintln!("mark_doi_check: 添付 {attachment_id} に {mark} を書けず: {e}");
    }
}

// ---------------- 取得物の同一性（目的の論文か） ----------------

/// `check_pdf_identity` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityCheck {
    /// 本文に目的 DOI がある（1 頁目、または 1 頁目に別 DOI が無いときの 2 頁目）
    Match,
    /// 目的 DOI が無く、1 頁目に別の DOI だけがある（合本・別論文・補遺）
    Mismatch { found_doi: String },
    /// DOI で照合を試みたが、本文に目的 DOI も別 DOI も無かった（DOI の印字が無い誌・表記ゆれ・切れた自己候補・
    /// 1 頁目に別 DOI があり目的 DOI は 2 頁目にだけある合本の 2 本目）→ 添付行には 'unknown'
    Unknown,
    /// 照合そのものができなかった: item に DOI が無い / 一時ファイルに書けない / 抽出できない・テキスト層なし
    /// → 添付行は NULL（未照合）のまま。「別論文の可能性」の警告は出さない
    Unchecked { reason: String },
}

/// 取得した PDF が目的の論文かを、冒頭 2 頁の本文で照合する。PMC 経路・`/capture/attachment` からも呼べる共通ヘルパ。
/// バイト列を直接解析する（一時ファイルは pdftotext fallback が要るときだけ `pdftools` が作る）
pub fn check_pdf_identity(bytes: &[u8], csl: &Value) -> IdentityCheck {
    let Some(doi) = model::s(csl, "DOI") else {
        return IdentityCheck::Unchecked { reason: "item に DOI なし".into() };
    };
    let pages = match crate::pdftools::extract_pages_bytes(bytes, None, 1, 2) {
        Ok(ex) => ex.pages,
        Err(e) => return IdentityCheck::Unchecked { reason: format!("本文を抽出できない（{e}）") },
    };
    identity_from_pages(&pages, doi)
}

/// 本文ページ列に対する照合（ネットワーク・ファイル非依存＝テスト可能）。
/// 照合前に合字（NFKC）と Unicode ハイフン（U+2010/2011/2012/2013/2212 → '-'・U+00AD は除去）を正規化する。
/// - Match: 1 頁目に目的 DOI（空白・改行を詰めて探し、直後が区切りであることを確認 = 前方一致は不可）。
///   2 頁目だけにある場合は、1 頁目に別 DOI が無いときに限り Match（合本の 2 本目は Unknown）
/// - Mismatch: 目的 DOI が無く、**1 頁目**に別の DOI がある（参考文献が 2 頁目に始まる短報を誤って弾かないため 1 頁目に限る）
/// - Unknown: それ以外。別 DOI と数えないもの = 表記ゆれ（同じ接頭辞で数字列が同じ）・目的 DOI の前方部分（行末で切れた自己候補）
pub fn identity_from_pages(pages: &[String], target_doi: &str) -> IdentityCheck {
    let target = printed_doi(target_doi);
    if target.is_empty() {
        return IdentityCheck::Unchecked { reason: "item に DOI なし".into() };
    }
    if pages.is_empty() || pages.iter().all(|p| p.trim().is_empty()) {
        return IdentityCheck::Unchecked { reason: "本文にテキスト層なし".into() };
    }
    let p1 = normalize_page(&pages[0]);
    if contains_doi(&p1, &target) {
        return IdentityCheck::Match;
    }
    let others_p1: Vec<String> = dois_in_text(&p1)
        .into_iter()
        .filter(|d| !same_or_similar(d, &target))
        .collect();
    let on_p2 = pages
        .get(1)
        .map(|p| contains_doi(&normalize_page(p), &target))
        .unwrap_or(false);
    if on_p2 {
        return if others_p1.is_empty() { IdentityCheck::Match } else { IdentityCheck::Unknown };
    }
    match others_p1.first() {
        None => IdentityCheck::Unknown,
        Some(found) => IdentityCheck::Mismatch { found_doi: found.clone() },
    }
}

/// 合字を NFKC で分解し、Unicode ハイフン類を '-' に寄せ、soft hyphen を落とす（小文字化はしない）
fn normalize_page(p: &str) -> String {
    crate::fulltext::normalize_text(p)
        .chars()
        .filter(|&c| c != '\u{00AD}')
        .map(|c| match c {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2212}' => '-',
            c => c,
        })
        .collect()
}

fn is_doi_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.' | '(' | ')' | ':' | ';' | '<' | '>')
}

/// 空白・改行を詰めた本文の中に target があり、その直後が区切り（末尾・空白・DOI の構成文字以外。
/// `.` `;` `)` `:` `,` はその次が区切りなら区切り）であるかを確認する。`10.11152/mu-430` を `mu-4308` に一致させない。
fn contains_doi(page: &str, target: &str) -> bool {
    let lower = page.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut sq: Vec<char> = Vec::with_capacity(chars.len());
    let mut idx: Vec<usize> = Vec::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_whitespace() {
            sq.push(c);
            idx.push(i);
        }
    }
    let t: Vec<char> = target.chars().collect();
    if t.is_empty() || sq.len() < t.len() {
        return false;
    }
    let boundary_at = |pos: usize| -> bool {
        match chars.get(pos) {
            None => true,
            Some(&c) if c.is_whitespace() || !is_doi_char(c) => true,
            Some(&c) if matches!(c, '.' | ';' | ')' | ':' | ',') => match chars.get(pos + 1) {
                None => true,
                Some(&n) => n.is_whitespace() || !is_doi_char(n),
            },
            Some(_) => false,
        }
    };
    for s in 0..=sq.len() - t.len() {
        if sq[s..s + t.len()] == t[..] && boundary_at(idx[s + t.len() - 1] + 1) {
            return true;
        }
    }
    false
}

/// 生テキスト（空白あり）から DOI 候補を全部拾う（正規表現は取込と同じ `pdfingest::doi_re`。
/// `pdfingest::find_dois_in_text` は件数の上限と正規化が違うので使わない）
fn dois_in_text(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in crate::pdfingest::doi_re().captures_iter(text) {
        let d = printed_doi(&c[1]);
        let Some((_, suffix)) = d.split_once('/') else { continue };
        if suffix.len() < 2 || out.iter().any(|x| x == &d) {
            continue;
        }
        out.push(d);
    }
    out
}

/// 本文に印字された DOI との比較形: `model::normalize_doi` + 末尾の句読点（文末の `.` 等）を落とす
fn printed_doi(s: &str) -> String {
    model::normalize_doi(s).trim_end_matches(['.', ',', ';', ')', ']']).to_string()
}

/// 候補 `d` を「別 DOI」と数えないか: 同一 / 表記ゆれ（同じ接頭辞で数字列が同じ。`10.5812/iranjradiol-138586` と
/// 本文印字 `ijradiol-138586`）/ 目的 DOI の前方部分（行末で `10.x/abc-` と切れた自己候補）
fn same_or_similar(d: &str, target: &str) -> bool {
    d == target || similar_doi(d, target) || (d.len() >= 8 && target.starts_with(d))
}

fn similar_doi(a: &str, b: &str) -> bool {
    let (Some((pa, sa)), Some((pb, sb))) = (a.split_once('/'), b.split_once('/')) else {
        return false;
    };
    if pa != pb {
        return false;
    }
    let da: String = sa.chars().filter(|c| c.is_ascii_digit()).collect();
    let dbg: String = sb.chars().filter(|c| c.is_ascii_digit()).collect();
    !da.is_empty() && da == dbg
}

// ---------------- HTTP（手動リダイレクト・取得先の検査） ----------------

const PDF_CAP: u64 = 200 * 1024 * 1024;
/// 着地 HTML の読み込み上限（ScienceDirect の bot 対策ページは 1.2 MB ある）
const HTML_CAP: u64 = 4 * 1024 * 1024;
/// HTTP リダイレクトの追従上限（各ホップで `url_allowed` を通す）
const MAX_REDIRECTS: usize = 5;
/// meta refresh の追従上限（linkinghub.elsevier.com は refresh → 301 → sciencedirect の 2 段）
const META_REFRESH_HOPS: usize = 2;
/// これより長い遅延の meta refresh は中継ではない（`1800;url=/logout` 等）
const META_REFRESH_MAX_DELAY: u64 = 10;

struct Page {
    url: String,
    content_type: String,
    body: Vec<u8>,
    /// 本文が上限（`cap`・PDF は `PDF_CAP`）で切れた。HTML は切れたまま使ってよい（所在を読むだけ）・PDF は `pdf_body` が拒否する
    truncated: bool,
}

/// 本文を `cap` まで読む。超えていれば `cap` に切り詰めて `truncated = true`（呼び手が「切れたまま使う」か「失敗にする」かを決める）。
/// 黙って切り詰めた PDF は `%PDF` 検査を通って壊れたまま添付されうるので、PDF は必ず `pdf_body` を通す
fn read_capped(reader: impl std::io::Read, cap: u64) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let mut body = Vec::new();
    reader.take(cap + 1).read_to_end(&mut body)?;
    let truncated = body.len() as u64 > cap;
    if truncated {
        body.truncate(cap as usize);
    }
    Ok((body, truncated))
}

/// 取得先として許す URL か。https（`allow_http` なら http も）・host は loopback / private / link-local / CGNAT /
/// localhost / IPv6 リテラル以外。`url::Url` でホストを解釈するので数値形 IPv4（`2130706433` `0x7f.0.0.1` `0177.0.0.1`
/// `127.1` `127.0.0.1.`）も 127.0.0.1 として弾ける。SSRF 対策: 着地ページの `citation_pdf_url` が内部アドレス
/// （connectd 自身の 127.0.0.1:23119 等）を指しても辿らない。
pub fn url_allowed(url: &str, allow_http: bool) -> std::result::Result<(), String> {
    let u = url::Url::parse(url.trim()).map_err(|e| format!("URL を解釈できません（{e}: {}）", truncate_url(url)))?;
    match u.scheme() {
        "https" => {}
        "http" if allow_http => {}
        s => return Err(format!("{s} の取得先は辿りません（{}）", truncate_url(url))),
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(format!("userinfo 付きの取得先は辿りません（{}）", truncate_url(url)));
    }
    match u.host() {
        None => Err(format!("ホストの無い取得先（{}）", truncate_url(url))),
        Some(url::Host::Ipv6(_)) => Err(format!("IPv6 リテラルの取得先は辿りません（{}）", truncate_url(url))),
        Some(url::Host::Ipv4(ip)) => {
            let o = ip.octets();
            let bad = ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
                || o[0] == 0
                || o[0] >= 224
                || (o[0] == 100 && (64..=127).contains(&o[1])); // CGNAT 100.64/10
            if bad {
                Err(format!("内部アドレスへの取得先は辿りません（{ip}）"))
            } else {
                Ok(())
            }
        }
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            if d.is_empty() || d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local") {
                Err(format!("ローカルホストへの取得先は辿りません（{d}）"))
            } else if !d.contains('.') {
                // 公開ホストは必ずドットを含む。単一ラベル（intranet / x 等）は内部名とみなす
                Err(format!("内部名の可能性がある取得先は辿りません（{d}）"))
            } else {
                Ok(())
            }
        }
    }
}

fn truncate_url(u: &str) -> String {
    model::truncate_chars(u, 80)
}

/// DOI を URL のパスとして percent-encode する（`/` は残す。popup の encodeURIComponent と同じ集合を素通し）
pub fn encode_doi_path(doi: &str) -> String {
    let mut out = String::with_capacity(doi.len() + 8);
    for b in doi.trim().bytes() {
        let keep = b.is_ascii_alphanumeric() || b"-_.!~*'()/".contains(&b);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// GET（リダイレクトは手動で最大 5 回・各ホップを `url_allowed` で検査）。本文は `cap`（application/pdf なら PDF_CAP）まで。
fn http_get(
    start: &str,
    referer: Option<&str>,
    accept: &str,
    cap: u64,
    allow_http: bool,
) -> std::result::Result<Page, String> {
    let mut url = start.to_string();
    for _ in 0..=MAX_REDIRECTS {
        url_allowed(&url, allow_http)?;
        let mut req = fetch_agent_no_redirect().get(&url).set("Accept", accept);
        if let Some(r) = referer {
            req = req.set("Referer", r);
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code @ (403 | 429), _)) => {
                return Err(format!(
                    "出版社ページ {} が HTTP {code} を返しました（拡張外からの取得を拒否・bot 対策 → ブラウザ経路へ）",
                    host_of(&url)
                ))
            }
            Err(ureq::Error::Status(code, _)) => return Err(format!("{} が HTTP {code}", host_of(&url))),
            Err(e) => return Err(e.to_string()),
        };
        let status = resp.status();
        if (300..400).contains(&status) {
            let Some(loc) = resp.header("Location") else {
                return Err(format!("{} が HTTP {status}（Location なし）", host_of(&url)));
            };
            url = resolve_url(&url, loc).ok_or_else(|| format!("Location を解釈できません（{loc}）"))?;
            continue;
        }
        let content_type = resp.content_type().to_ascii_lowercase();
        let limit = if content_type == "application/pdf" { PDF_CAP } else { cap };
        let (body, truncated) = read_capped(resp.into_reader(), limit).map_err(|e| e.to_string())?;
        return Ok(Page { url, content_type, body, truncated });
    }
    Err(format!("リダイレクトが {MAX_REDIRECTS} 回を超えました（{}）", truncate_url(start)))
}

/// 外部 URL から PDF をダウンロードして `%PDF` を検証する（`http_get` = 手動リダイレクト・各ホップに SSRF ガード `url_allowed`・
/// 本文 200 MB 上限）。`referer` は着地ページ（出版社ホストが要求することがある）。`allow_http` は Unpaywall（文書化 API が返す URL）だけ true。
/// connectd の connector 保険 DL（translator が宣言した URL）もこれを使う（curl 子プロセスは使わない。理由は connectd 側のコメント）
pub fn download_pdf(url: &str, referer: Option<&str>, allow_http: bool) -> std::result::Result<Vec<u8>, String> {
    pdf_body(http_get(url, referer, "application/pdf,*/*;q=0.8", PDF_CAP, allow_http)?)
}

/// 取得した応答を PDF として受け入れる: 上限（`PDF_CAP`）で切れていれば失敗（切れた PDF は `%PDF` 検査を通って壊れたまま添付されうる。
/// connectd が使っていた curl の `--max-filesize` と同じく添付しない）→ `%PDF` 検証
fn pdf_body(page: Page) -> std::result::Result<Vec<u8>, String> {
    if page.truncated {
        return Err(format!(
            "PDF が上限 {} MB を超えたので取得しません（{}）",
            PDF_CAP / (1024 * 1024),
            host_of(&page.url)
        ));
    }
    if !page.body.starts_with(b"%PDF") {
        return Err(format!(
            "PDF ではない応答（{}・{} bytes・ログイン要求や bot 対策ページの可能性 → ブラウザ経路へ）",
            page.content_type,
            page.body.len()
        ));
    }
    Ok(page.body)
}

// ---------------- 経路 3: Crossref link[] ----------------

fn crossref_pdf_links(doi: &str) -> std::result::Result<Vec<String>, String> {
    // 書誌検索・doi.org と同じ Crossref の門（同時 1 本 + 1.1 s 間隔）を通す。本文の受信まで門の中
    let body = crate::resolver::with_crossref_gate(|pace| {
        pace();
        text(agent().get(&format!("https://api.crossref.org/works/{}", encode_doi_path(doi))).call(), |e| match e {
            ureq::Error::Status(404, _) => format!("{doi} は Crossref 未登録"),
            e => e.to_string(),
        })
    })?;
    let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(select_crossref_pdf_links(&v))
}

/// `works/<doi>` 応答の `message.link[]` から、`content-type: application/pdf` かつ
/// `intended-application` が text-mining 以外で、取得先として許される URL を順に返す（ネットワーク非依存＝テスト可能）。
pub fn select_crossref_pdf_links(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let links = v.pointer("/message/link").and_then(|l| l.as_array());
    for l in links.into_iter().flatten() {
        let ct = l.get("content-type").and_then(|x| x.as_str()).unwrap_or("");
        let app = l.get("intended-application").and_then(|x| x.as_str()).unwrap_or("");
        let url = l.get("URL").and_then(|x| x.as_str()).unwrap_or("");
        if ct.eq_ignore_ascii_case("application/pdf")
            && !app.eq_ignore_ascii_case("text-mining")
            && url_allowed(url, false).is_ok()
            && !out.iter().any(|u| u == url)
        {
            out.push(url.to_string());
        }
    }
    out
}

// ---------------- 経路 4: doi.org 着地ページ ----------------

/// 経路 4 の結果。`kind` = 所在の種類（`LocationKind::Declared` = メタタグ / link、`Heuristic` = .pdf リンク 1 本規則）
pub struct LandingPdf {
    pub bytes: Vec<u8>,
    pub host: String,
    pub kind: LocationKind,
}

/// 経路 4 の本体: doi.org → 着地ページ → PDF の所在 → ダウンロード。
/// 書誌には一切使わない（PDF の URL だけを読む）。
pub fn landing_page_pdf(doi: &str) -> std::result::Result<LandingPdf, String> {
    let mut url = format!("https://doi.org/{}", encode_doi_path(doi));
    let mut hops = 0;
    loop {
        let page = http_get(
            &url,
            None,
            "text/html,application/xhtml+xml,application/pdf;q=0.9,*/*;q=0.8",
            HTML_CAP,
            false,
        )?;
        let host = host_of(&page.url);
        if page.body.starts_with(b"%PDF") {
            // DOI が PDF そのものに解決した（content-type が違って HTML の上限で切れていたら取り直す。PDF の上限超過は `pdf_body` が拒否）
            let bytes = if page.truncated && page.content_type != "application/pdf" {
                download_pdf(&page.url, None, false)?
            } else {
                pdf_body(page)?
            };
            return Ok(LandingPdf { bytes, host, kind: LocationKind::Declared });
        }
        let html = String::from_utf8_lossy(&page.body);
        match locate_pdf(&html, &page.url) {
            Located::Pdf(pdf_url, kind) => {
                return match download_pdf(&pdf_url, Some(&page.url), false) {
                    Ok(bytes) => Ok(LandingPdf { bytes, host, kind }),
                    Err(e) => Err(format!("{host} の {} → {e}", truncate_url(&pdf_url))),
                };
            }
            Located::Refresh(next) if hops < META_REFRESH_HOPS => {
                hops += 1;
                url = next;
            }
            Located::Refresh(_) => {
                return Err(format!("中継ページ {host} の meta refresh が {META_REFRESH_HOPS} 段を超えました"));
            }
            Located::Rejected(reason) => return Err(format!("着地ページ {host}: {reason}")),
            Located::None => {
                return Err(format!(
                    "着地ページ {host} に PDF の所在（citation_pdf_url / .pdf リンク）がありません（{}・{} bytes。出版社が拡張外からの取得に本文ページを出さない bot 対策の可能性 → ブラウザ経路へ）",
                    page.content_type,
                    page.body.len()
                ))
            }
        }
    }
}

/// 着地 HTML から読めた「PDF の所在」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// PDF の URL と、宣言された所在かヒューリスティックか
    Pdf(String, LocationKind),
    /// 中継ページ: meta refresh の飛び先
    Refresh(String),
    /// 所在は書かれていたが取得先として許されない（http・内部アドレス）
    Rejected(String),
    None,
}

/// 着地 HTML から **PDF の所在だけ**を読む。順: `citation_pdf_url` メタタグ → `link[type=application/pdf]` →
/// meta refresh（中継ページ。フッターの `.pdf` を拾わないよう `.pdf` 探索より先）→ 同一ホストでパスが `.pdf` で終わる
/// `<a href>` が **ちょうど 1 つ**のときだけそれ（ヒューリスティック）。
/// HTML コメント・`<script>` `<style>` `<noscript>` の中は無視・`<base href>` を尊重・書誌は読まない。
pub fn locate_pdf(html: &str, page_url: &str) -> Located {
    let html = strip_comments(html);
    let html = strip_blocks(&html, &["script", "style", "noscript"]);
    let tags = scan_tags(&html);
    let base = tags
        .iter()
        .find(|(n, _)| n == "base")
        .and_then(|(_, a)| attr(a, "href"))
        .and_then(|h| resolve_url(page_url, h))
        .unwrap_or_else(|| page_url.to_string());
    let accept = |u: String, kind: LocationKind| match url_allowed(&u, false) {
        Ok(()) => Located::Pdf(u, kind),
        Err(e) => Located::Rejected(e),
    };
    for (name, attrs) in &tags {
        if name == "meta" {
            let key = attr(attrs, "name").or_else(|| attr(attrs, "property"));
            if key.map(|k| k.eq_ignore_ascii_case("citation_pdf_url")).unwrap_or(false) {
                if let Some(u) = attr(attrs, "content").and_then(|c| resolve_url(&base, c)) {
                    return accept(u, LocationKind::Declared);
                }
            }
        }
    }
    for (name, attrs) in &tags {
        if name == "link"
            && attr(attrs, "type")
                .map(|t| t.eq_ignore_ascii_case("application/pdf"))
                .unwrap_or(false)
        {
            if let Some(u) = attr(attrs, "href").and_then(|h| resolve_url(&base, h)) {
                return accept(u, LocationKind::Declared);
            }
        }
    }
    if let Some(next) = meta_refresh_url_in(&tags, &base) {
        return match url_allowed(&next, false) {
            Ok(()) => Located::Refresh(next),
            Err(e) => Located::Rejected(e),
        };
    }
    let host = host_of(page_url);
    let mut pdf_links: Vec<String> = Vec::new();
    for (name, attrs) in &tags {
        if name == "a" {
            if let Some(u) = attr(attrs, "href").and_then(|h| resolve_url(&base, h)) {
                let path = u.split(['?', '#']).next().unwrap_or("");
                if path.to_ascii_lowercase().ends_with(".pdf")
                    && host_of(&u) == host
                    && !pdf_links.iter().any(|x| x == &u)
                {
                    pdf_links.push(u);
                }
            }
        }
    }
    match pdf_links.len() {
        1 => accept(pdf_links.remove(0), LocationKind::Heuristic),
        _ => Located::None,
    }
}

/// `<meta http-equiv="refresh" content="N; url=...">` の飛び先（linkinghub.elsevier.com 等）。
/// 遅延 N が META_REFRESH_MAX_DELAY 秒を超えるもの（logout タイマー等）は中継とみなさない。
fn meta_refresh_url_in(tags: &[(String, Vec<(String, String)>)], base: &str) -> Option<String> {
    for (name, attrs) in tags {
        if name == "meta"
            && attr(attrs, "http-equiv")
                .map(|v| v.eq_ignore_ascii_case("refresh"))
                .unwrap_or(false)
        {
            let content = attr(attrs, "content")?;
            let delay: u64 = content
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            if delay > META_REFRESH_MAX_DELAY {
                continue;
            }
            let idx = content.to_ascii_lowercase().find("url")?;
            let rest = content[idx + 3..].trim_start().trim_start_matches('=').trim();
            let rest = rest.trim_matches(|c| c == '\'' || c == '"').trim();
            return resolve_url(base, rest);
        }
    }
    None
}

/// `<!-- … -->` を落とす（コメント内に残った古い citation_pdf_url を拾わないため）。
/// 閉じない `<!--` はコメントとして扱わず、以降をそのまま残す（script 内の文字列などで全文を捨てないため）。
fn strip_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<!--") {
        match rest[i + 4..].find("-->") {
            Some(j) => {
                out.push_str(&rest[..i]);
                rest = &rest[i + 4 + j + 3..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// `<tag …>…</tag>` の中身を落とす（script / style / noscript。大小無視）
fn strip_blocks(html: &str, tags: &[&str]) -> String {
    let mut s = html.to_string();
    for tag in tags {
        let lower = s.to_ascii_lowercase();
        let open = format!("<{tag}");
        let close = format!("</{tag}");
        let mut out = String::with_capacity(s.len());
        let mut pos = 0;
        while let Some(i) = lower[pos..].find(&open) {
            let start = pos + i;
            // `<script` の直後がタグ名の続き（<scripts>）でないことを確認
            let after = lower[start + open.len()..].chars().next();
            if !matches!(after, Some(c) if c.is_ascii_whitespace() || c == '>' || c == '/') {
                out.push_str(&s[pos..start + open.len()]);
                pos = start + open.len();
                continue;
            }
            out.push_str(&s[pos..start]);
            match lower[start..].find(&close) {
                Some(j) => {
                    let end = start + j;
                    let gt = lower[end..].find('>').map(|k| end + k + 1).unwrap_or(lower.len());
                    pos = gt;
                }
                None => {
                    pos = lower.len();
                }
            }
        }
        out.push_str(&s[pos..]);
        s = out;
    }
    s
}

/// `<meta …>` / `<link …>` / `<a …>` / `<base …>` を (タグ名, 属性) に分解する。
/// 引用符の中の `>` を正しく扱う手書きスキャナ（属性順・引用符・大小は問わない）。
fn scan_tags(html: &str) -> Vec<(String, Vec<(String, String)>)> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j == i + 1 {
            i += 1;
            continue;
        }
        let name = html[i + 1..j].to_ascii_lowercase();
        let wanted = matches!(name.as_str(), "meta" | "link" | "a" | "base");
        // 属性を '>' まで読む（引用符の中は飛ばす）
        let mut attrs = Vec::new();
        let mut k = j;
        loop {
            while k < b.len() && (b[k].is_ascii_whitespace() || b[k] == b'/') {
                k += 1;
            }
            if k >= b.len() || b[k] == b'>' {
                break;
            }
            let ns = k;
            while k < b.len() && !b[k].is_ascii_whitespace() && b[k] != b'=' && b[k] != b'>' && b[k] != b'/' {
                k += 1;
            }
            let aname = html[ns..k].to_ascii_lowercase();
            let mut m = k;
            while m < b.len() && b[m].is_ascii_whitespace() {
                m += 1;
            }
            let mut val = String::new();
            if m < b.len() && b[m] == b'=' {
                m += 1;
                while m < b.len() && b[m].is_ascii_whitespace() {
                    m += 1;
                }
                if m < b.len() && (b[m] == b'"' || b[m] == b'\'') {
                    let q = b[m];
                    let vs = m + 1;
                    let mut ve = vs;
                    while ve < b.len() && b[ve] != q {
                        ve += 1;
                    }
                    val = html[vs..ve.min(b.len())].to_string();
                    k = (ve + 1).min(b.len());
                } else {
                    let vs = m;
                    let mut ve = m;
                    while ve < b.len() && !b[ve].is_ascii_whitespace() && b[ve] != b'>' {
                        ve += 1;
                    }
                    val = html[vs..ve].to_string();
                    k = ve;
                }
            } else {
                k = m;
            }
            if !aname.is_empty() && wanted {
                attrs.push((aname, decode_entities(&val)));
            }
        }
        if wanted {
            out.push((name, attrs));
        }
        i = (k + 1).max(j);
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

/// 文字実体参照（名前付きの基本形 + 数値 `&#NN;` / `&#xHH;`）を復号する
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..end];
        let decoded = match ent {
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "lt" => Some('<'),
            "gt" => Some('>'),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| {
                    if let Some(h) = n.strip_prefix(['x', 'X']) {
                        u32::from_str_radix(h, 16).ok()
                    } else {
                        n.parse::<u32>().ok()
                    }
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// 相対 URL を base に対して解決する（http/https のみ・絶対 URL はスキームの大小を問わない）。
/// `?query` だけの相対 URL はページのパスに対して解決し、`#fragment` だけは同じページなので None。
pub fn resolve_url(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("mailto:") || lower.starts_with("data:") {
        return None;
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(href.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if let Some(h) = href.strip_prefix("//") {
        return Some(format!("{scheme}://{h}"));
    }
    if href.starts_with('/') {
        return Some(format!("{scheme}://{host}{href}"));
    }
    let path = path.split(['?', '#']).next().unwrap_or("/");
    if href.starts_with('?') {
        return Some(format!("{scheme}://{host}{path}{href}"));
    }
    let dir = &path[..=path.rfind('/').unwrap_or(0)];
    Some(format!("{scheme}://{host}{dir}{href}"))
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|r| r.split(['/', '?', '#']).next())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// "Family et al. YYYY - Title.pdf"（Paperpile 命名互換・cutover 時の Drive レイアウトを見据える）
pub fn suggest_filename(csl: &Value) -> String {
    let fam = model::first_author_family(csl);
    let multi = csl
        .get("author")
        .and_then(|a| a.as_array())
        .map(|a| a.len() > 1)
        .unwrap_or(false);
    let year = model::year(csl).map(|y| y.to_string()).unwrap_or_default();
    let title = model::s(csl, "title").unwrap_or("untitled");
    let head = if multi { format!("{fam} et al.") } else { fam };
    let name = format!("{head} {year} - {title}");
    let name: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' => ' ',
            c => c,
        })
        .collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = model::truncate_chars(&name, 140);
    // タイトル末尾のピリオド（PubMed 由来）で "..pdf" にならないようにする
    format!("{}.pdf", name.trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filename_shape() {
        let csl = json!({
            "title": "Pancreatic Cancer: Changing / Epidemiology?",
            "author": [{"family": "Stoffel"}, {"family": "Brand"}],
            "issued": {"date-parts": [[2023]]}
        });
        assert_eq!(
            suggest_filename(&csl),
            "Stoffel et al. 2023 - Pancreatic Cancer Changing Epidemiology.pdf"
        );
        // PubMed タイトルの末尾ピリオドで "..pdf" にしない
        let csl2 = json!({
            "title": "Novel Strategy for the Management of Cervical Multicystic Diseases.",
            "author": [{"family": "Yoshino"}, {"family": "Kobayashi"}],
            "issued": {"date-parts": [[2023]]}
        });
        assert!(suggest_filename(&csl2).ends_with("Multicystic Diseases.pdf"));
    }

    // 実バケットの応答（PMC13677031 = Radiol Case Rep 2026・CC BY）
    const LISTING_OA: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>pmc-oa-opendata</Name><Prefix>PMC13677031.</Prefix><KeyCount>8</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated><Contents><Key>PMC13677031.1/PMC13677031.1.json</Key><Size>1138</Size></Contents><Contents><Key>PMC13677031.1/PMC13677031.1.pdf</Key><ETag>&quot;81995e647a1f9940bb34dd309e484f23&quot;</ETag><Size>3333023</Size></Contents><Contents><Key>PMC13677031.1/PMC13677031.1.txt</Key></Contents><Contents><Key>PMC13677031.1/PMC13677031.1.xml</Key></Contents><Contents><Key>PMC13677031.1/gr1.webp</Key></Contents></ListBucketResult>"#;
    const JSON_OA: &str = r#"{"pmcid": "PMC13677031", "version": 1, "pmid": null, "doi": "10.1016/j.radcr.2026.08.005", "mid": null, "title": "Capillary hemangioma of the breast presenting as a new mass on screening mammography: A case report with imaging-pathology correlation", "citation": "Radiol Case Rep. 2026 Oct 9;21(12):6462–7. doi: 10.1016/j.radcr.2026.08.005", "is_pmc_openaccess": true, "is_manuscript": false, "is_historical_ocr": false, "is_retracted": false, "license_code": "CC BY", "pdf_url": "s3://pmc-oa-opendata/PMC13677031.1/PMC13677031.1.pdf?md5=81995e647a1f9940bb34dd309e484f23", "text_url": "s3://pmc-oa-opendata/PMC13677031.1/PMC13677031.1.txt?md5=4c37c912c8a73381dc8b5d88444c6b0b", "xml_url": "s3://pmc-oa-opendata/PMC13677031.1/PMC13677031.1.xml?md5=5f7d4b48cdb74577d6499db51c7d82fb", "media_urls": ["s3://pmc-oa-opendata/PMC13677031.1/gr1.webp?md5=d55b73b9948f85742914b66ca5d2a838"]}"#;
    // 同（PMC10243302 = Gastroenterology author manuscript・OA サブセット外・TDM）— 一覧に .pdf が無い
    const LISTING_MANUSCRIPT: &str = r#"<ListBucketResult><Name>pmc-oa-opendata</Name><Prefix>PMC10243302.</Prefix><KeyCount>3</KeyCount><Contents><Key>PMC10243302.1/PMC10243302.1.json</Key></Contents><Contents><Key>PMC10243302.1/PMC10243302.1.txt</Key></Contents><Contents><Key>PMC10243302.1/PMC10243302.1.xml</Key></Contents></ListBucketResult>"#;
    const JSON_MANUSCRIPT: &str = r#"{"pmcid": "PMC10243302", "version": 1, "pmid": 36804602, "doi": "10.1053/j.gastro.2023.02.012", "mid": "NIHMS1901494", "title": "Pancreatic Cancer: Changing Epidemiology and New Approaches to Risk Assessment, Early Detection, and Prevention", "citation": "Gastroenterology. Author manuscript; Available in PMC 2023 Jun 6.", "is_pmc_openaccess": false, "is_manuscript": true, "is_historical_ocr": false, "is_retracted": false, "license_code": "TDM", "xml_url": "s3://pmc-oa-opendata/PMC10243302.1/PMC10243302.1.xml?md5=4634181677271ea6d10c8b39de3d1026", "text_url": "s3://pmc-oa-opendata/PMC10243302.1/PMC10243302.1.txt?md5=7f3c8f3b8ac9b2314163b39f81e9338d"}"#;
    // 未収載（KeyCount 0）
    const LISTING_EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>pmc-oa-opendata</Name><Prefix>PMC99999999999.</Prefix><KeyCount>0</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated></ListBucketResult>"#;

    #[test]
    fn aws_listing_parsing() {
        let keys = listing_keys(LISTING_OA);
        assert_eq!(keys.len(), 5);
        assert_eq!(keys[1], "PMC13677031.1/PMC13677031.1.pdf");
        assert!(listing_keys(LISTING_EMPTY).is_empty());
        // 最新 version を選ぶ（.1 と .2 が並ぶとき）・別 PMCID（prefix の桁違い）は混ぜない
        let keys: Vec<String> = [
            "PMC1.1/PMC1.1.json",
            "PMC1.1/PMC1.1.pdf",
            "PMC1.2/PMC1.2.json",
            "PMC1.2/PMC1.2.pdf",
            "PMC12.1/PMC12.1.pdf",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(latest_version_prefix(&keys, "PMC1").as_deref(), Some("PMC1.2"));
        assert_eq!(latest_version_prefix(&keys, "PMC12").as_deref(), Some("PMC12.1"));
        assert_eq!(latest_version_prefix(&keys, "PMC123"), None);
        assert_eq!(latest_version_prefix(&[], "PMC1"), None);
    }

    #[test]
    fn s3_url_parsing() {
        assert_eq!(
            s3_url_parts("s3://pmc-oa-opendata/PMC13677031.1/PMC13677031.1.pdf?md5=81995E647A1F9940BB34DD309E484F23"),
            Some(("PMC13677031.1/PMC13677031.1.pdf".into(), Some("81995e647a1f9940bb34dd309e484f23".into())))
        );
        assert_eq!(
            s3_url_parts("s3://pmc-oa-opendata/PMC1.1/PMC1.1.pdf"),
            Some(("PMC1.1/PMC1.1.pdf".into(), None))
        );
        // md5 が壊れていても key は使う（検証は省略）
        assert_eq!(s3_url_parts("s3://pmc-oa-opendata/a/b.pdf?md5=zz").unwrap().1, None);
        assert_eq!(s3_url_parts("s3://other-bucket/a/b.pdf"), None);
        assert_eq!(s3_url_parts("https://pmc-oa-opendata.s3.amazonaws.com/a.pdf"), None);
        assert_eq!(normalize_pmcid(" pmc13677031 "), "PMC13677031");
        assert_eq!(normalize_pmcid("13677031"), "PMC13677031");
    }

    // 同（PMC13000617 = JAMA Netw Open 2026・OA CC BY-NC-ND だが本体 PDF 無し・補足資料 s001/s002.pdf のみ）
    // 一覧の .pdf を拾うと s001.pdf（169 KB の Supplemental Online Content）を本体として添付してしまう例
    const LISTING_SUPP_ONLY: &str = r#"<ListBucketResult><Name>pmc-oa-opendata</Name><Prefix>PMC13000617.</Prefix><KeyCount>5</KeyCount><IsTruncated>false</IsTruncated><Contents><Key>PMC13000617.1/PMC13000617.1.json</Key></Contents><Contents><Key>PMC13000617.1/PMC13000617.1.txt</Key></Contents><Contents><Key>PMC13000617.1/PMC13000617.1.xml</Key></Contents><Contents><Key>PMC13000617.1/jamanetwopen-e262307-s001.pdf</Key></Contents><Contents><Key>PMC13000617.1/jamanetwopen-e262307-s002.pdf</Key></Contents></ListBucketResult>"#;
    const JSON_SUPP_ONLY: &str = r#"{"pmcid": "PMC13000617", "version": 1, "pmid": 41848736, "doi": "10.1001/jamanetworkopen.2026.2307", "mid": null, "title": "Women\u2019s Health Primary Care Practitioners and Attrition From Veterans Health Administration Care", "citation": "JAMA Netw Open. 2026 Mar 18;9(3):e262307. doi: 10.1001/jamanetworkopen.2026.2307", "is_pmc_openaccess": true, "is_manuscript": false, "is_historical_ocr": false, "is_retracted": false, "license_code": "CC BY-NC-ND", "text_url": "s3://pmc-oa-opendata/PMC13000617.1/PMC13000617.1.txt?md5=1c1bb72d06571ec36a6f3fa37c931677", "xml_url": "s3://pmc-oa-opendata/PMC13000617.1/PMC13000617.1.xml?md5=e4fb39890c4fdf5b5101ddcc5b631cda", "media_urls": ["s3://pmc-oa-opendata/PMC13000617.1/jamanetwopen-e262307-s001.pdf?md5=fae577c7953b0064e1064300d8227378", "s3://pmc-oa-opendata/PMC13000617.1/jamanetwopen-e262307-s002.pdf?md5=7bd9d24e139310049fb80a3ffaad2661"]}"#;

    #[test]
    fn oa_json_interpretation() {
        // OA（CC BY）+ pdf_url → Available（md5 付き）。DOI 照合も通る
        let keys = listing_keys(LISTING_OA);
        let p = latest_version_prefix(&keys, "PMC13677031").unwrap();
        assert_eq!(
            interpret_oa(Some(JSON_OA), &keys, &p, "PMC13677031", Some("10.1016/j.radcr.2026.08.005")),
            OaLookup::Available {
                key: "PMC13677031.1/PMC13677031.1.pdf".into(),
                license: "CC BY".into(),
                md5: Some("81995e647a1f9940bb34dd309e484f23".into()),
            }
        );
        // author manuscript（is_pmc_openaccess=false・TDM）→ NotOpenAccess
        let keys_m = listing_keys(LISTING_MANUSCRIPT);
        let pm = latest_version_prefix(&keys_m, "PMC10243302").unwrap();
        assert_eq!(
            interpret_oa(Some(JSON_MANUSCRIPT), &keys_m, &pm, "PMC10243302", None),
            OaLookup::NotOpenAccess { license: "TDM".into(), manuscript: true }
        );
        // OA だが pdf_url が無く、一覧には補足資料の .pdf だけ → NoPdf（補足を本体にしない）
        let keys_s = listing_keys(LISTING_SUPP_ONLY);
        let ps = latest_version_prefix(&keys_s, "PMC13000617").unwrap();
        assert_eq!(
            interpret_oa(Some(JSON_SUPP_ONLY), &keys_s, &ps, "PMC13000617", Some("10.1001/jamanetworkopen.2026.2307")),
            OaLookup::NoPdf { license: "CC BY-NC-ND".into() }
        );
        // json があって pdf_url が無ければ、一覧に本体名の .pdf があっても json を信じて NoPdf
        let oa_no_pdf = r#"{"is_pmc_openaccess": true, "is_manuscript": false, "license_code": "CC0"}"#;
        assert_eq!(
            interpret_oa(Some(oa_no_pdf), &keys, &p, "PMC13677031", None),
            OaLookup::NoPdf { license: "CC0".into() }
        );
        // pdf_url が別ディレクトリ（別 version / 別記事）を指す → 本体とみなさない
        let odd = r#"{"is_pmc_openaccess": true, "license_code": "CC BY", "pdf_url": "s3://pmc-oa-opendata/PMC99.1/PMC99.1.pdf?md5=00000000000000000000000000000000"}"#;
        assert_eq!(interpret_oa(Some(odd), &keys, &p, "PMC13677031", None), OaLookup::NoPdf { license: "CC BY".into() });
        // json の pmcid / doi が item と食い違う → Mismatch（添付しない）
        assert!(matches!(
            interpret_oa(Some(JSON_OA), &keys, &p, "PMC13677032", None),
            OaLookup::Mismatch { .. }
        ));
        assert!(matches!(
            interpret_oa(Some(JSON_OA), &keys, &p, "PMC13677031", Some("10.1001/other")),
            OaLookup::Mismatch { .. }
        ));
        // DOI の表記ゆれ（doi.org URL・doi: 接頭辞・大文字）は一致とみなす
        assert!(matches!(
            interpret_oa(Some(JSON_OA), &keys, &p, "PMC13677031", Some("https://doi.org/10.1016/J.RADCR.2026.08.005")),
            OaLookup::Available { .. }
        ));
        assert!(matches!(
            interpret_oa(Some(JSON_OA), &keys, &p, "PMC13677031", Some("doi:10.1016/j.radcr.2026.08.005")),
            OaLookup::Available { .. }
        ));
        assert!(matches!(
            interpret_oa(Some(JSON_OA), &keys, &p, "PMC13677031", Some("http://dx.doi.org/10.1016/j.radcr.2026.08.005")),
            OaLookup::Available { .. }
        ));
        // json 自体が無い（異常系）→ 一覧の本体名（PMC<id>.<v>.pdf）完全一致のみ・license unknown
        assert_eq!(
            interpret_oa(None, &keys, &p, "PMC13677031", None),
            OaLookup::Available { key: "PMC13677031.1/PMC13677031.1.pdf".into(), license: "unknown".into(), md5: None }
        );
        // 同・補足資料（MOESM1_ESM.pdf は辞書順で本体より先）しか無い一覧 → NoPdf
        let keys_e: Vec<String> = ["PMC10000338.1/41390_2023_2527_MOESM1_ESM.pdf", "PMC10000338.1/PMC10000338.1.txt"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(interpret_oa(None, &keys_e, "PMC10000338.1", "PMC10000338", None), OaLookup::NoPdf { license: "unknown".into() });
        // 同・補足と本体が並ぶ一覧 → 本体だけ
        let keys_b: Vec<String> = ["PMC10000338.2/41390_2023_2527_MOESM1_ESM.pdf", "PMC10000338.2/PMC10000338.2.pdf"]
            .iter().map(|s| s.to_string()).collect();
        assert!(matches!(interpret_oa(None, &keys_b, "PMC10000338.2", "PMC10000338", None),
            OaLookup::Available { key, .. } if key == "PMC10000338.2/PMC10000338.2.pdf"));
        // 壊れた json → json 無しと同じ扱い
        assert!(matches!(interpret_oa(Some("not json"), &keys, &p, "PMC13677031", None), OaLookup::Available { .. }));
        // 継続トークン
        assert_eq!(listing_next_token("<R><IsTruncated>false</IsTruncated></R>"), None);
        assert_eq!(
            listing_next_token("<R><IsTruncated>true</IsTruncated><NextContinuationToken>abc&amp;d=</NextContinuationToken></R>").as_deref(),
            Some("abc&d=")
        );
    }

    /// 実弾（ネットワーク）: `cargo test -p openref-core pmc_oa_live -- --ignored --nocapture`
    /// PMC13677031（Radiol Case Rep 2026・CC BY）が Available で md5 付き、PDF が取れて md5 一致。
    /// PMC10243302（author manuscript）は NotOpenAccess、存在しない ID は NotListed。
    /// 版は固定しない（PMC は .2〜.4 や `PMC10775174.319` のような改版があるので一覧の最新版を使う）。
    #[test]
    #[ignore]
    fn pmc_oa_live() {
        let keys = list_prefix("PMC13677031.").unwrap();
        let prefix = latest_version_prefix(&keys, "PMC13677031").unwrap();
        let r = pmc_oa_lookup("PMC13677031", Some("10.1016/j.radcr.2026.08.005")).unwrap();
        eprintln!("PMC13677031 ({prefix}): {r:?}");
        let OaLookup::Available { key, license, md5 } = r else { panic!("Available を期待") };
        assert_eq!(key, format!("{prefix}/{prefix}.pdf"), "本体 PDF は PMC<id>.<v>.pdf");
        assert_eq!(license, "CC BY");
        assert!(md5.is_some(), "pdf_url の md5 が付くはず");
        let bytes = aws_fetch_pdf(&key, md5.as_deref()).unwrap();
        eprintln!("fetched {} bytes (md5 verified)", bytes.len());
        assert!(bytes.len() > 1_000_000);
        // DOI が食い違えば添付しない
        assert!(matches!(pmc_oa_lookup("PMC13677031", Some("10.9999/wrong")).unwrap(), OaLookup::Mismatch { .. }));

        // 本体 PDF 無し・補足資料のみ（JAMA Netw Open）→ NoPdf
        let r = pmc_oa_lookup("PMC13000617", Some("10.1001/jamanetworkopen.2026.2307")).unwrap();
        eprintln!("PMC13000617: {r:?}");
        assert!(matches!(r, OaLookup::NoPdf { .. }));

        let r = pmc_oa_lookup("PMC10243302", None).unwrap();
        eprintln!("PMC10243302: {r:?}");
        assert!(matches!(r, OaLookup::NotOpenAccess { manuscript: true, .. }));

        let r = pmc_oa_lookup("PMC99999999999", None).unwrap();
        eprintln!("PMC99999999999: {r:?}");
        assert_eq!(r, OaLookup::NotListed);
    }
}

/// DOI 起点の連鎖（経路 3・4）のテスト。上の `tests` とは別モジュール（先行カードが PMC 部のテストを
/// 書き換えても衝突しないように末尾に置く）。
#[cfg(test)]
mod doi_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn crossref_link_selection() {
        // Ann Ital Chir 10.62713/aic.4729 の実応答: similarity-checking のみ → PDF なし
        let aic = json!({"message": {"link": [
            {"URL": "https://www.annaliitalianidichirurgia.it/journal/AIC/97/10/10.62713/aic.4729",
             "content-type": "unspecified", "content-version": "vor", "intended-application": "similarity-checking"}
        ]}});
        assert!(select_crossref_pdf_links(&aic).is_empty());
        // Elsevier 10.1016/j.radcr.2026.08.005 の実応答: text-mining（XML / plain・API キー要）のみ → PDF なし
        let els = json!({"message": {"link": [
            {"URL": "https://api.elsevier.com/content/article/PII:S1930043326007533?httpAccept=text/xml",
             "content-type": "text/xml", "content-version": "vor", "intended-application": "text-mining"},
            {"URL": "https://api.elsevier.com/content/article/PII:S1930043326007533?httpAccept=text/plain",
             "content-type": "text/plain", "content-version": "vor", "intended-application": "text-mining"}
        ]}});
        assert!(select_crossref_pdf_links(&els).is_empty());
        // application/pdf でも text-mining 用は除外・unspecified / similarity-checking は採用・重複は 1 回・http は弾く
        let mixed = json!({"message": {"link": [
            {"URL": "https://x.org/tdm.pdf", "content-type": "application/pdf", "intended-application": "text-mining"},
            {"URL": "https://x.org/a.pdf", "content-type": "application/pdf", "intended-application": "unspecified"},
            {"URL": "https://x.org/a.pdf", "content-type": "application/pdf", "intended-application": "similarity-checking"},
            {"URL": "http://x.org/plain.pdf", "content-type": "application/pdf", "intended-application": "unspecified"},
            {"URL": "https://x.org/b.pdf", "content-type": "APPLICATION/PDF", "intended-application": "unspecified"}
        ]}});
        assert_eq!(
            select_crossref_pdf_links(&mixed),
            vec!["https://x.org/a.pdf".to_string(), "https://x.org/b.pdf".to_string()]
        );
        // link なし
        assert!(select_crossref_pdf_links(&json!({"message": {}})).is_empty());
    }

    fn declared(u: &str) -> Located {
        Located::Pdf(u.to_string(), LocationKind::Declared)
    }
    fn heuristic(u: &str) -> Located {
        Located::Pdf(u.to_string(), LocationKind::Heuristic)
    }

    #[test]
    fn landing_html_pdf_location() {
        let base = "https://www.annaliitalianidichirurgia.it/journal/AIC/97/10/10.62713/aic.4729";
        // 実ページの形: citation_pdf_url が別ホスト（storage.）を指す → そのまま採用（宣言された所在）
        let aic = r#"<html><head><meta name="citation_title" content="Painless …"><meta name="citation_pdf_url" content="https://storage.annaliitalianidichirurgia.it/AIC/2109889745379975200/application/AIC-97-10-1766-1772.pdf"></head><body></body></html>"#;
        assert_eq!(
            locate_pdf(aic, base),
            declared("https://storage.annaliitalianidichirurgia.it/AIC/2109889745379975200/application/AIC-97-10-1766-1772.pdf")
        );
        // 属性順が逆・単引用符・大文字・&amp; / &#x2F; 符号化・相対パス
        let rev = r#"<META CONTENT='&#x2F;pdf&#x2F;1.pdf?a=1&amp;b=2' NAME='Citation_PDF_URL'>"#;
        assert_eq!(locate_pdf(rev, "https://pub.example.org/article/1"), declared("https://pub.example.org/pdf/1.pdf?a=1&b=2"));
        // HTML コメント内の古い citation_pdf_url は無視し、本物を採る
        let commented = r#"<!-- <meta name="citation_pdf_url" content="/old/stale.pdf"> --><meta name="citation_pdf_url" content="/new/real.pdf">"#;
        assert_eq!(locate_pdf(commented, "https://pub.example.org/a"), declared("https://pub.example.org/new/real.pdf"));
        // 閉じない <!-- は以降を捨てない（script 内の文字列など）
        let unclosed = r#"<script>var s = "<!--";</script><meta name="citation_pdf_url" content="/p.pdf">"#;
        assert_eq!(locate_pdf(unclosed, "https://pub.example.org/a"), declared("https://pub.example.org/p.pdf"));
        // <script> / <style> / <noscript> の中は無視（noscript 内の meta refresh も中継とみなさない）
        let scripted = r#"<script>document.write('<meta name="citation_pdf_url" content="/js.pdf">')</script><noscript><meta http-equiv="refresh" content="0;url=/nojs"></noscript><a href="/files/only.pdf">p</a>"#;
        assert_eq!(locate_pdf(scripted, "https://pub.example.org/a"), heuristic("https://pub.example.org/files/only.pdf"));
        // 属性値の中の '>' でタグを切らない
        let gt = r#"<meta name="description" content="a > b"><meta name="citation_pdf_url" content="/p.pdf">"#;
        assert_eq!(locate_pdf(gt, "https://pub.example.org/a"), declared("https://pub.example.org/p.pdf"));
        // <base href> を尊重
        let based = r#"<base href="https://cdn.example.org/files/"><meta name="citation_pdf_url" content="x.pdf">"#;
        assert_eq!(locate_pdf(based, "https://pub.example.org/a"), declared("https://cdn.example.org/files/x.pdf"));
        // 次点: link[type=application/pdf]
        let link = r#"<link rel="alternate" type="application/pdf" href="//pub.example.org/x.pdf">"#;
        assert_eq!(locate_pdf(link, "https://pub.example.org/a"), declared("https://pub.example.org/x.pdf"));
        // 中継ページ: meta refresh はフッターの .pdf より優先
        let relay = r#"<meta http-equiv="refresh" content="0;URL=/retrieve/next"><a href="/legal/privacy.pdf">privacy</a>"#;
        assert_eq!(
            locate_pdf(relay, "https://linkinghub.example.org/retrieve/pii/S1"),
            Located::Refresh("https://linkinghub.example.org/retrieve/next".into())
        );
        // 遅延の長い refresh（logout タイマー）は中継ではない → .pdf 探索へ
        let logout = r#"<meta http-equiv="refresh" content="1800;url=/logout"><a href="/files/paper.pdf">p</a>"#;
        assert_eq!(locate_pdf(logout, "https://pub.example.org/a"), heuristic("https://pub.example.org/files/paper.pdf"));
        // 最後: 同一ホストでパスが .pdf で終わる a[href] が **ちょうど 1 つ**のときだけ（他ホストは数えない）
        let one = r#"<a href="https://other.example.net/evil.pdf">x</a><a href="/files/paper.PDF?download=1">PDF</a>"#;
        assert_eq!(locate_pdf(one, "https://pub.example.org/article/1"), heuristic("https://pub.example.org/files/paper.PDF?download=1"));
        // 2 つあれば選ばない（投稿規定 Instructions-for-Authors.pdf を本体と取り違えないため）
        let two = r#"<a href="/docs/Instructions-for-Authors.pdf">i</a><a href="/files/paper.pdf">p</a>"#;
        assert_eq!(locate_pdf(two, "https://pub.example.org/article/1"), Located::None);
        // 同じ URL の重複は 1 つと数える
        let dup = r#"<a href="/files/paper.pdf">p</a><a href="/files/paper.pdf">again</a>"#;
        assert_eq!(locate_pdf(dup, "https://pub.example.org/a"), heuristic("https://pub.example.org/files/paper.pdf"));
        // 所在なし（本文に .pdf という語があっても拾わない）・javascript: は無視
        assert_eq!(locate_pdf("<p>download the .pdf here</p><a href=\"/login\">login</a>", base), Located::None);
        assert_eq!(locate_pdf(r#"<a href="javascript:openPdf('x.pdf')">x</a>"#, base), Located::None);
        // SSRF: 内部アドレス・http・数値形 IPv4 は所在として拒否（辿らない）
        for bad in [
            r#"<meta name="citation_pdf_url" content="http://127.0.0.1:23119/capture/status">"#,
            r#"<meta name="citation_pdf_url" content="https://10.0.0.5/x.pdf">"#,
            r#"<meta name="citation_pdf_url" content="https://2130706433:23120/x.pdf">"#,
            r#"<meta name="citation_pdf_url" content="https://0x7f.0.0.1/x.pdf">"#,
        ] {
            assert!(matches!(locate_pdf(bad, base), Located::Rejected(_)), "{bad}");
        }
    }

    /// 生の HTML から meta refresh の飛び先を読む（`locate_pdf` と同じ前処理 → `meta_refresh_url_in`）
    fn meta_refresh_url(html: &str, base: &str) -> Option<String> {
        let html = strip_comments(html);
        let html = strip_blocks(&html, &["script", "style", "noscript"]);
        meta_refresh_url_in(&scan_tags(&html), base)
    }

    #[test]
    fn meta_refresh_parsing() {
        // linkinghub.elsevier.com の実応答: 単引用符・&amp;・相対パス
        let lh = r#"<meta HTTP-EQUIV="REFRESH" content="2; url='/retrieve/articleSelectSinglePerm?Redirect=https%3A%2F%2Fwww.sciencedirect.com%2Fscience%2Farticle%2Fpii%2FS1930043326007533%3Fvia%253Dihub&amp;key=c3696a0a'"/>"#;
        assert_eq!(
            meta_refresh_url(lh, "https://linkinghub.elsevier.com/retrieve/pii/S1930043326007533").as_deref(),
            Some("https://linkinghub.elsevier.com/retrieve/articleSelectSinglePerm?Redirect=https%3A%2F%2Fwww.sciencedirect.com%2Fscience%2Farticle%2Fpii%2FS1930043326007533%3Fvia%253Dihub&key=c3696a0a")
        );
        // "0;URL=https://…" 形
        assert_eq!(
            meta_refresh_url(r#"<meta http-equiv="refresh" content="0;URL=https://pdf.example.org/s.pdf?sig=1">"#, "https://a.org/").as_deref(),
            Some("https://pdf.example.org/s.pdf?sig=1")
        );
        // refresh でない meta・遅延 1800 秒・noscript 内は無視
        assert!(meta_refresh_url(r#"<meta http-equiv="Content-Type" content="text/html">"#, "https://a.org/").is_none());
        assert!(meta_refresh_url(r#"<meta http-equiv="refresh" content="1800;url=/logout">"#, "https://a.org/").is_none());
        assert!(meta_refresh_url(r#"<noscript><meta http-equiv="refresh" content="0;url=/x"></noscript>"#, "https://a.org/").is_none());
    }

    #[test]
    fn url_resolution_and_encoding() {
        assert_eq!(resolve_url("https://h.org/a/b", "/x").as_deref(), Some("https://h.org/x"));
        assert_eq!(resolve_url("https://h.org/a/b", "x").as_deref(), Some("https://h.org/a/x"));
        assert_eq!(resolve_url("https://h.org", "x").as_deref(), Some("https://h.org/x"));
        assert_eq!(resolve_url("https://h.org/a", "//c.org/y").as_deref(), Some("https://c.org/y"));
        assert_eq!(resolve_url("https://h.org/a", "HTTP://c.org/y").as_deref(), Some("HTTP://c.org/y"));
        // query だけの相対 URL はページのパスに対して・fragment だけは同じページ
        assert_eq!(resolve_url("https://h.org/a/page?old=1#f", "?x=1").as_deref(), Some("https://h.org/a/page?x=1"));
        assert!(resolve_url("https://h.org/a/page", "#top").is_none());
        assert!(resolve_url("https://h.org/a", "  ").is_none());
        assert_eq!(host_of("https://WWW.Example.org/p?q#f"), "www.example.org");
        assert_eq!(host_of("https://h.org"), "h.org");
        // DOI の percent-encode: SICI 形の < > ; # は符号化、/ と英数・-._() は素通し
        assert_eq!(encode_doi_path("10.62713/aic.4729"), "10.62713/aic.4729");
        assert_eq!(
            encode_doi_path("10.1002/(SICI)1097-0142(19960415)77:8<1498::AID-CNCR11>3.0.CO;2-7"),
            "10.1002/(SICI)1097-0142(19960415)77%3A8%3C1498%3A%3AAID-CNCR11%3E3.0.CO%3B2-7"
        );
        assert_eq!(encode_doi_path("10.1000/a#b c"), "10.1000/a%23b%20c");
        assert_eq!(decode_entities("a&amp;b&#39;c&#x2F;d&lt;&unknown;"), "a&b'c/d<&unknown;");
    }

    #[test]
    fn ssrf_guard() {
        assert!(url_allowed("https://www.sciencedirect.com/x.pdf", false).is_ok());
        assert!(url_allowed("HTTPS://Pub.Example.org/x.pdf", false).is_ok()); // 大文字スキームは正規化される
        assert!(url_allowed("https://172.32.0.1/x.pdf", false).is_ok()); // private ではない
        assert!(url_allowed("https://100.128.0.1/x.pdf", false).is_ok()); // CGNAT の外
        // Unpaywall（文書化 API の返す URL）だけ http を許す。ガードは同じ
        assert!(url_allowed("http://pub.example.org/x.pdf", true).is_ok());
        assert!(url_allowed("http://127.0.0.1/x.pdf", true).is_err());
        for bad in [
            "http://pub.example.org/x.pdf", // https 以外（降格）
            "https://127.0.0.1:23119/capture/status",
            "https://2130706433:23120/x",  // 10 進 1 語
            "https://0x7f.0.0.1/x",        // 16 進
            "https://0177.0.0.1/x",        // 8 進
            "https://127.1/x",             // 短縮
            "https://127.0.0.1./x",        // 末尾ドット
            "https://localhost/x",
            "https://localhost./x",
            "https://openref.localhost/x",
            "https://printer.local/x",
            "https://10.1.2.3/x",
            "https://172.16.0.1/x",
            "https://192.168.1.1/x",
            "https://169.254.169.254/latest",
            "https://100.64.0.1/x", // CGNAT
            "https://0.0.0.0/x",
            "https://[::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://user@evil.example.org/x",
            "https://user:pw@evil.example.org/x",
            "ftp://pub.example.org/x.pdf",
            "https:///x", // url crate は host "x" と解釈する → 単一ラベルは内部名として拒否
            "https://intranet/x",
        ] {
            assert!(url_allowed(bad, false).is_err(), "{bad} は拒否されるべき");
        }
    }

    #[test]
    fn pdf_identity_rules() {
        let pg = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // 合本 10.11152/mu-4311 の pdftotext 1〜2 頁: p.1 = mu-4292, p.2 = mu-4308, 目的 mu-4311 は p.8
        let combined = pg(&[
            "Med Ultrason\n2023, Vol. 25, No 4, 472-473, DOI: 10.11152/mu-4292,\nCorresponding author: X",
            "Med Ultrason\n2023, Vol. 25, No 4, 473-474, DOI: 10.11152/mu-4308,\nCorresponding author: D",
        ]);
        assert_eq!(identity_from_pages(&combined, "10.11152/mu-4311"), IdentityCheck::Mismatch { found_doi: "10.11152/mu-4292".into() });
        assert_eq!(identity_from_pages(&combined, "10.11152/mu-4292"), IdentityCheck::Match);
        // 2 頁目にだけ目的 DOI があり 1 頁目に別 DOI → Unknown（合本の 2 本目を全頁添付しない）
        assert_eq!(identity_from_pages(&combined, "10.11152/mu-4308"), IdentityCheck::Unknown);
        // 前方一致は Match にしない（mu-430 ≠ mu-4308）→ p.1 の別 DOI で Mismatch
        assert_eq!(identity_from_pages(&combined, "10.11152/mu-430"), IdentityCheck::Mismatch { found_doi: "10.11152/mu-4292".into() });
        assert_eq!(identity_from_pages(&combined, "10.11152/mu-429"), IdentityCheck::Mismatch { found_doi: "10.11152/mu-4292".into() });
        // 表記ゆれ（実物 iranjradiol の p.1 印字 `doi.org/10.5812/ijradiol-138586.`）→ Unknown
        let pages = pg(&["Iran J Radiol. 2024\nhttps://doi.org/10.5812/ijradiol-138586.\nAbstract"]);
        assert_eq!(identity_from_pages(&pages, "10.5812/iranjradiol-138586"), IdentityCheck::Unknown);
        // 一致: そのまま / 大文字 / doi.org URL 形 + 文末ピリオド / 改行で割れている / 2 頁目（1 頁目に別 DOI なし）
        assert_eq!(identity_from_pages(&pg(&["Ann Ital Chir 2026\nhttps://doi.org/10.62713/AIC.4729.\nAbstract"]), "10.62713/aic.4729"), IdentityCheck::Match);
        assert_eq!(identity_from_pages(&pg(&["Radiology Case Reports\nDOI 10.1016/j.radcr.\n2026.08.005"]), "10.1016/j.radcr.2026.08.005"), IdentityCheck::Match);
        assert_eq!(identity_from_pages(&pg(&["Title page without doi", "doi:10.62713/aic.4729"]), "10.62713/aic.4729"), IdentityCheck::Match);
        // 本番 8X43BJPH の形: 目的 DOI と、行末で切れた別 DOI 候補 `10.1007/s00256-022-` が同じ 1 頁目にある → Match
        let real = pg(&["23456789)1 3\n\nhttps://doi.org/10.1007/s00256-022-04005-9\n\nTEST YOURSELF: ANS … found at doi: 10.1007/s00256-022-\n04004-w\n\n *  Fabian"]);
        assert_eq!(identity_from_pages(&real, "10.1007/s00256-022-04005-9"), IdentityCheck::Match);
        // 行末で切れた **自己**候補だけがある（目的 DOI の前方部分）→ 別 DOI と数えず Unknown
        let truncated = pg(&["see doi: 10.1007/s00256-022-\nand nothing else"]);
        assert_eq!(identity_from_pages(&truncated, "10.1007/s00256-022-04005-9"), IdentityCheck::Unknown);
        // 合字（ﬁ）と Unicode ハイフン（U+2011）・soft hyphen を正規化してから照合
        let ligated = pg(&["DOI: 10.1234/\u{FB01}bro\u{2011}sis\u{00AD}.2024"]);
        assert_eq!(identity_from_pages(&ligated, "10.1234/fibro-sis.2024"), IdentityCheck::Match);
        // DOI の印字が無い誌 → Unknown
        assert_eq!(identity_from_pages(&pg(&["No identifiers here", "Nor here"]), "10.62713/aic.4729"), IdentityCheck::Unknown);
        // 別 DOI が 2 頁目（参考文献）にだけある短報は Mismatch にしない
        assert_eq!(identity_from_pages(&pg(&["Short report, no doi printed", "References\n1. … 10.1148/radiol.2020200000"]), "10.62713/aic.4729"), IdentityCheck::Unknown);
        // 1 頁目に目的 DOI と別 DOI（引用）が両方あれば Match
        assert_eq!(identity_from_pages(&pg(&["doi:10.62713/aic.4729\nsee also 10.1148/radiol.1"]), "10.62713/aic.4729"), IdentityCheck::Match);
        // item に DOI が無い / 空ページ → Unknown
        // 照合不能 = Unchecked（添付行は NULL のまま・警告なし）: item に DOI が無い / 空ページ / テキスト層なし
        assert!(matches!(identity_from_pages(&pg(&["doi:10.62713/aic.4729"]), ""), IdentityCheck::Unchecked { .. }));
        assert!(matches!(identity_from_pages(&[], "10.62713/aic.4729"), IdentityCheck::Unchecked { .. }));
        assert!(matches!(identity_from_pages(&pg(&["", "  \n"]), "10.62713/aic.4729"), IdentityCheck::Unchecked { .. }));
        assert!(matches!(check_pdf_identity(b"%PDF-1.4 not really", &json!({"title": "x"})), IdentityCheck::Unchecked { .. }));
        // 抽出失敗（PDF でないバイト列）→ Unchecked。照合はバイト列を直接解析するので一時ファイルを作らない
        // （pdftotext fallback に落ちるときだけ pdftools が作って Drop で消す = pdftools のテストで名前ごと確かめる。
        // temp_dir の同接頭辞を数える検査は並行テストと競合するのでやめた）
        assert!(matches!(check_pdf_identity(b"not a pdf at all", &json!({"DOI": "10.62713/aic.4729"})), IdentityCheck::Unchecked { .. }));
        // 正規化
        assert_eq!(printed_doi("https://doi.org/10.62713/AIC.4729."), "10.62713/aic.4729");
        // 境界: 直後が `,` / 末尾 / `.`+改行 は区切り、英数は不可
        assert!(contains_doi("DOI: 10.11152/mu-4292,", "10.11152/mu-4292"));
        assert!(contains_doi("DOI: 10.11152/mu-4292", "10.11152/mu-4292"));
        assert!(contains_doi("DOI: 10.11152/mu-4292.\nNext", "10.11152/mu-4292"));
        assert!(!contains_doi("DOI: 10.11152/mu-4292,", "10.11152/mu-429"));
        assert!(!contains_doi("DOI: 10.1000/abc.def", "10.1000/abc"));
    }

    /// 採否の全組合せ（所在 3 × 照合 4 = 12）を固定する。方針を変えるときはこの表と `verdict` の doc 表を一緒に直す
    #[test]
    fn verdict_table() {
        use IdentityCheck as C;
        use LocationKind::*;
        let mismatch = || C::Mismatch { found_doi: "10.11152/mu-4292".into() };
        let unchecked = || C::Unchecked { reason: "本文にテキスト層なし".into() };
        let attach = |m: Option<&'static str>| Verdict::Attach { doi_check: m };
        let rejected = || Verdict::Mismatch { found_doi: "10.11152/mu-4292".into() };
        let table: Vec<(LocationKind, C, Verdict)> = vec![
            (Pmc, C::Match, attach(Some("match"))),
            (Pmc, mismatch(), rejected()),
            (Pmc, C::Unknown, attach(None)),
            (Pmc, unchecked(), attach(None)),
            (Declared, C::Match, attach(Some("match"))),
            (Declared, mismatch(), rejected()),
            (Declared, C::Unknown, attach(Some("unknown"))),
            (Declared, unchecked(), attach(None)),
            (Heuristic, C::Match, attach(Some("match"))),
            (Heuristic, mismatch(), rejected()),
            (Heuristic, C::Unknown, Verdict::Unverified { why: "本文にこの文献の DOI が見つからない".into() }),
            (Heuristic, unchecked(), Verdict::Unverified { why: "照合できない（本文にテキスト層なし）".into() }),
        ];
        assert_eq!(table.len(), 3 * 4);
        for (kind, check, want) in &table {
            assert_eq!(&verdict(*kind, check), want, "{kind:?} × {check:?}");
        }
    }

    /// 連鎖の 4 段が共有する `judge`: 実バイト列（`pdftools::minimal_pdf`）で照合 → `verdict` → `Downloaded` か `tried` の 1 行。ネットワークなし
    #[test]
    fn judge_builds_candidate_or_note() {
        let pdf = |text: &str| crate::pdftools::minimal_pdf(text, None);
        let csl = json!({"DOI": "10.62713/aic.4729"});
        // Match → 'match'・source そのまま（所在の種類によらない）
        for kind in [LocationKind::Pmc, LocationKind::Declared, LocationKind::Heuristic] {
            let dl = judge(kind, &csl, pdf("Ann Ital Chir doi: 10.62713/aic.4729"), "s", "src".into()).unwrap();
            assert_eq!((dl.doi_check, dl.source.as_str()), (Some("match"), "src"), "{kind:?}");
        }
        // Declared × Unknown → 'unknown'・source に「・DOI 未確認」
        let dl = judge(LocationKind::Declared, &csl, pdf("no identifiers printed"), "unpaywall", "unpaywall".into()).unwrap();
        assert_eq!((dl.doi_check, dl.source.as_str()), (Some("unknown"), "unpaywall・DOI 未確認"));
        // Pmc × Unknown → 印なし・source そのまま
        let dl = judge(LocationKind::Pmc, &csl, pdf("no identifiers printed"), "pmc", "pmc-aws (cc by)".into()).unwrap();
        assert_eq!((dl.doi_check, dl.source.as_str()), (None, "pmc-aws (cc by)"));
        // Mismatch → tried の 1 行（文言固定・どの所在でも）
        let bytes = pdf("Med Ultrason DOI: 10.11152/mu-4292, Corresponding");
        let n = bytes.len();
        let note = judge(LocationKind::Declared, &csl, bytes, "crossref", "crossref-link".into()).unwrap_err();
        assert_eq!(note, format!("crossref: 別論文（DOI 10.11152/mu-4292）の PDF でした（{n} bytes・添付せず）"));
        // Heuristic × Unknown → 添付しない
        let bytes = pdf("Instructions for authors");
        let n = bytes.len();
        let note = judge(LocationKind::Heuristic, &csl, bytes, "doi.org", "publisher-page (x.org)".into()).unwrap_err();
        assert_eq!(
            note,
            format!("doi.org: publisher-page (x.org) の .pdf リンク（ヒューリスティック所在）は本文にこの文献の DOI が見つからない（{n} bytes・添付せず）")
        );
        // Unchecked（item に DOI なし / PDF でない）: Pmc・Declared は印なしで添付・Heuristic は添付しない
        let nodoi = json!({"title": "x"});
        let dl = judge(LocationKind::Declared, &nodoi, pdf("whatever"), "crossref", "crossref-link".into()).unwrap();
        assert_eq!((dl.doi_check, dl.source.as_str()), (None, "crossref-link"));
        let dl = judge(LocationKind::Pmc, &csl, b"not a pdf".to_vec(), "pmc", "pmc-aws (cc by)".into()).unwrap();
        assert_eq!((dl.doi_check, dl.source.as_str()), (None, "pmc-aws (cc by)"));
        let note = judge(LocationKind::Heuristic, &nodoi, pdf("whatever"), "doi.org", "p".into()).unwrap_err();
        assert!(note.contains("は照合できない（item に DOI なし）（"), "{note}");
    }

    /// 上限超過の PDF は切り詰めて添付せず失敗にする: cap + 1 byte → `read_capped` が truncated → `pdf_body` が Err
    #[test]
    fn over_cap_pdf_is_rejected() {
        let cap = 16u64;
        let page = |body: Vec<u8>, truncated: bool| Page {
            url: "https://x.org/a.pdf".into(),
            content_type: "application/pdf".into(),
            body,
            truncated,
        };
        // ちょうど cap → そのまま
        let (body, truncated) = read_capped(std::io::Cursor::new(b"%PDF-1.4 exactly".to_vec()), cap).unwrap();
        assert_eq!((body.len() as u64, truncated), (cap, false));
        assert!(pdf_body(page(body, truncated)).is_ok());
        // cap + 1 byte → 切り詰め + truncated → 添付しない（Err）
        let (body, truncated) = read_capped(std::io::Cursor::new(b"%PDF-1.4 exactly+".to_vec()), cap).unwrap();
        assert_eq!((body.len() as u64, truncated), (cap, true));
        let e = pdf_body(page(body, truncated)).unwrap_err();
        assert!(e.contains("上限 200 MB を超えたので取得しません（x.org）"), "{e}");
        // PDF でない応答は従来どおり
        assert!(pdf_body(page(b"<html>".to_vec(), false)).unwrap_err().contains("PDF ではない応答"));
    }

    /// 実弾（ネットワーク）: `cargo test -p openref-core --lib -- --ignored landing_page_pdf_live --nocapture`
    /// Ann Ital Chir（OA・CC BY）は doi.org → citation_pdf_url → PDF → 同一性 Match。
    /// Elsevier は結果を表示するだけ（第三者の bot 対策に依存するため失敗側は assert しない）。
    #[test]
    #[ignore]
    fn landing_page_pdf_live() {
        let lp = landing_page_pdf("10.62713/aic.4729").expect("AIC は取れるはず");
        assert!(lp.bytes.starts_with(b"%PDF"));
        assert!(lp.bytes.len() > 100_000, "{} bytes", lp.bytes.len());
        assert_eq!(lp.host, "www.annaliitalianidichirurgia.it");
        assert_eq!(lp.kind, LocationKind::Declared);
        eprintln!("AIC: {} bytes from {}", lp.bytes.len(), lp.host);
        let id = check_pdf_identity(&lp.bytes, &json!({"DOI": "10.62713/aic.4729"}));
        eprintln!("AIC identity: {id:?}");
        assert_eq!(id, IdentityCheck::Match);
        // 別論文の DOI を目的にすると Mismatch になる（合本検出の実弾）
        let other = check_pdf_identity(&lp.bytes, &json!({"DOI": "10.62713/aic.9999"}));
        eprintln!("AIC vs other DOI: {other:?}");
        assert!(matches!(other, IdentityCheck::Mismatch { .. }));

        match landing_page_pdf("10.1016/j.radcr.2026.08.005") {
            Ok(lp) => eprintln!("Elsevier: {} bytes from {}", lp.bytes.len(), lp.host),
            Err(e) => eprintln!("Elsevier: {e}"),
        }
        assert!(crossref_pdf_links("10.62713/aic.4729").unwrap().is_empty());
        assert!(crossref_pdf_links("10.1016/j.radcr.2026.08.005").unwrap().is_empty());
    }

    /// 実弾（既存 DB の読み取りのみ）: 保存済み本文（fulltext・冒頭 2 頁）に同一性照合を再適用し、Match 率と Mismatch 0 を確認する。
    /// `OPENREF_PROD_DB=<path> cargo test -p openref-core --lib -- --ignored identity_prod_recheck --nocapture`
    /// （未設定なら ~/Library/Application Support/OpenRef/openref.sqlite を read-only で開く）
    #[test]
    #[ignore]
    fn identity_prod_recheck() {
        let path = crate::env_os("PROD_DB").map(std::path::PathBuf::from).unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("Library/Application Support/OpenRef/openref.sqlite")
        });
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("read-only open");
        let mut stmt = conn
            .prepare(
                "SELECT i.key, json_extract(i.csl,'$.DOI'), f.text FROM items i
                 JOIN attachments a ON a.item_id = i.id JOIN fulltext f ON f.attachment_id = a.id
                 WHERE i.deleted_at IS NULL AND a.kind = 'pdf'",
            )
            .unwrap();
        let rows: Vec<(String, Option<String>, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        let (mut m, mut u, mut mm, mut nodoi) = (0, 0, 0, 0);
        for (key, doi, text) in &rows {
            let Some(doi) = doi else { nodoi += 1; continue };
            let pages: Vec<String> = text.split('\u{c}').take(2).map(str::to_string).collect();
            match identity_from_pages(&pages, doi) {
                IdentityCheck::Match => m += 1,
                IdentityCheck::Unknown => {
                    u += 1;
                    eprintln!("Unknown  {key} {doi}");
                }
                IdentityCheck::Mismatch { found_doi } => {
                    mm += 1;
                    eprintln!("MISMATCH {key} {doi} found={found_doi}");
                }
                IdentityCheck::Unchecked { reason } => {
                    u += 1;
                    eprintln!("Unchecked {key} {doi} ({reason})");
                }
            }
        }
        eprintln!("prod recheck: total {} (DOI なし {nodoi}) Match {m} Unknown/Unchecked {u} Mismatch {mm}", rows.len());
        assert_eq!(mm, 0, "本番蔵書で誤拒否があってはならない");
        assert!(m > 0);
    }

    /// 実弾（手元の実 PDF）: `OPENREF_QA_PDF_DIR=<dir> cargo test -p openref-core --lib -- --ignored identity_qa_pdfs --nocapture`
    /// 合本 mu_4311.pdf（p.1 = mu-4292・p.2 = mu-4308・目的 mu-4311 は p.8）と表記ゆれ iranjradiol を実バイト列で確認する。
    #[test]
    #[ignore]
    fn identity_qa_pdfs() {
        let Some(dir) = crate::env_os("QA_PDF_DIR") else {
            eprintln!("OPENREF_QA_PDF_DIR 未設定のためスキップ");
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let combined = std::fs::read(dir.join("10_11152_mu_4311.pdf")).expect("combined pdf");
        let run = |doi: &str| check_pdf_identity(&combined, &json!({"DOI": doi}));
        let r4311 = run("10.11152/mu-4311");
        let r4292 = run("10.11152/mu-4292");
        let r4308 = run("10.11152/mu-4308");
        let r430 = run("10.11152/mu-430");
        let r429 = run("10.11152/mu-429");
        eprintln!("combined: 4311={r4311:?} 4292={r4292:?} 4308={r4308:?} 430={r430:?} 429={r429:?}");
        assert!(matches!(r4311, IdentityCheck::Mismatch { .. }));
        assert_eq!(r4292, IdentityCheck::Match);
        assert_eq!(r4308, IdentityCheck::Unknown);
        assert_ne!(r430, IdentityCheck::Match);
        assert_ne!(r429, IdentityCheck::Match);
        let iran = std::fs::read(dir.join("10_5812_iranjradiol_138586.pdf")).expect("iran pdf");
        let ri = check_pdf_identity(&iran, &json!({"DOI": "10.5812/iranjradiol-138586"}));
        eprintln!("iranjradiol: {ri:?}");
        assert_ne!(ri, IdentityCheck::Mismatch { found_doi: "10.5812/ijradiol-138586".into() });
        // 手元の実 PDF 全部: 自分の DOI で Mismatch が出ないこと
        let pairs = [
            ("10_1186_s13244_026_02425_y", "10.1186/s13244-026-02425-y"),
            ("10_1186_s43055_026_01875_0", "10.1186/s43055-026-01875-0"),
            ("10_14366_usg_22037", "10.14366/usg.22037"),
            ("10_1590_0100_3984_2017_0092", "10.1590/0100-3984.2017.0092"),
            ("10_22575_interventionalradiology_2024_0064", "10.22575/interventionalradiology.2024-0064"),
            ("10_2463_mrms_mp_2025_0208", "10.2463/mrms.mp.2025-0208"),
            ("10_25259_jcis_27_2025", "10.25259/jcis_27_2025"),
            ("10_32552_2024_actamedica_984", "10.32552/2024.actamedica.984"),
            ("10_4102_sajr_v30i1_3506", "10.4102/sajr.v30i1.3506"),
            ("10_4274_dir_2026_260001", "10.4274/dir.2026.260001"),
            ("10_5114_pjr_188257", "10.5114/pjr/188257"),
            ("10_5334_jbsr_4226", "10.5334/jbsr.4226"),
            ("10_62713_aic_4729", "10.62713/aic.4729"),
            ("10_7759_cureus_118154", "10.7759/cureus.118154"),
        ];
        let mut match_n = 0;
        for (stem, doi) in pairs {
            let Ok(bytes) = std::fs::read(dir.join(format!("{stem}.pdf"))) else { continue };
            let r = check_pdf_identity(&bytes, &json!({"DOI": doi}));
            eprintln!("{doi:45} {r:?}");
            assert!(!matches!(r, IdentityCheck::Mismatch { .. }), "{doi} が誤拒否");
            if r == IdentityCheck::Match {
                match_n += 1;
            }
        }
        eprintln!("round-1 PDFs: Match {match_n}/{}", pairs.len());
    }
}
