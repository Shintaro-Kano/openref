//! Word の Startup テンプレート版（`OpenRef.dotm`・VBA）向け API `/word/v2/*`。
//! VBA は薄いクライアント: 文書（コンテンツコントロール・Document Variables）を操作し、整形・書誌・ダイアログはここに頼む。
//! - VBA 向けの応答は HTTP 200 + 行形式（1 行目 `OK` / `ERR<TAB>文言[<TAB>補足]`、以降 `<種別><TAB><欄>…`。各行は "\n" で終わる）。
//!   **TAB・LF と印字可能 ASCII（`\` `~` を除く）のみ**: 日本語ロケールの Mac Word は popen の出力を MacJapanese で読む
//!   （0x5C = ¥・0x7E = ‾）ため、欄は `%` でエスケープする（`%%` `%t` `%n` `%r`・それ以外は `%uXXXX` = UTF-16 単位。[`esc`]）
//! - ダイアログのページ・アプレット向け（params / result / next / closed）は JSON
//! - すべて `X-OpenRef: word` ヘッダ必須（無ければ 403。カスタムヘッダは CORS preflight を起こすので他サイトのページから呼べない）
//! - 整形 = citeproc を JavaScriptCore で（jsc.rs + word/dotm-render.js）。番号 = 文書順の初出順・書誌は蔵書の最新を優先し、
//!   蔵書に無い key は文書内の payload（snapshot）

use crate::{jsc, St};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as AxPath, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use openref_core::{citestyle, keys, search, Db};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

macro_rules! word_src {
    ($p:expr) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../word/", $p))
    };
}
// static = バイナリに 1 コピー（const だと使う場所ごとに埋め込まれる）
static CITEPROC_JS: &str = word_src!("vendor/citeproc.js");
static CITE_CORE_JS: &str = word_src!("cite-core.js");
const DOTM_RENDER_JS: &str = word_src!("dotm-render.js");

const DEFAULT_FONT: &str = "Century";
const DEFAULT_SIZE: f64 = 10.5;

/// ダイアログの窓の大きさ（pt。1440×900 前後の画面に合わせた値）
const SIZE_CITE: (u32, u32) = (640, 480);
const SIZE_SETTINGS: (u32, u32) = (600, 560);
const SIZE_NOTICE: (u32, u32) = (460, 270);
const SIZE_CONFIRM: (u32, u32) = (500, 340);
const SIZE_INFO: (u32, u32) = (600, 520);

/// 起動したアプレットが待ち行列のセッションを取りに来るまでの猶予。過ぎてもアプレットが動いていなければ起動し直す
pub const QUEUE_GRACE: Duration = Duration::from_secs(3);
/// 起動し直す回数の上限（超えたら ERR）
pub const MAX_LAUNCHES: u32 = 3;
/// 窓を開いているアプレットの生存確認の間隔（落ちた = 窓が消えた → cancel）
pub const PROBE_EVERY: Duration = Duration::from_secs(2);
/// VBA が受け取った結果を残す時間（取り直しに答える）
pub const KEEP_CONSUMED: Duration = Duration::from_secs(60);
/// 誰も poll しなくなったセッションを捨てるまで（Word を閉じた・VBA が止まった）
pub const KEEP_IDLE: Duration = Duration::from_secs(3600);

pub struct V2 {
    st: Arc<St>,
    dialogs: Mutex<Dialogs>,
    /// ダイアログのページとアプレットが使う起点（https://localhost:<Word のポート>）
    base: String,
    /// `OpenRef Dialog.app`（既定 = DB と同階層の word/。`OPENREF_DIALOG_APP` で差し替え）
    app: PathBuf,
}

impl V2 {
    fn dialogs(&self) -> std::sync::MutexGuard<'_, Dialogs> {
        self.dialogs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// アプレットを起動する（[`launch_applet`] を spawn_blocking で）
    async fn launch(&self) -> Result<(), UserErr> {
        let (app, base) = (self.app.clone(), self.base.clone());
        tokio::task::spawn_blocking(move || launch_applet(&app, &base))
            .await
            .unwrap_or_else(|e| Err(UserErr::new("ダイアログを表示できません。", Some(e.to_string()))))
    }
}

pub fn router(st: Arc<St>, word_port: u16, app_dir: &Path) -> Router {
    let app = dialog_app(app_dir);
    routes(Arc::new(V2 {
        st,
        dialogs: Mutex::new(Dialogs::default()),
        base: format!("https://localhost:{word_port}"),
        app,
    }))
}

fn routes(v2: Arc<V2>) -> Router {
    Router::new()
        .route("/word/v2/render", post(render))
        .route("/word/v2/prefs", post(prefs))
        .route("/word/v2/legacy", post(legacy))
        .route("/word/v2/log", post(log))
        .route("/word/v2/dialog", post(dialog_create))
        .route("/word/v2/dialog/next", get(dialog_next))
        .route("/word/v2/dialog/focus", post(dialog_focus))
        .route("/word/v2/dialog/:sid", get(dialog_poll))
        .route("/word/v2/dialog/:sid/params", get(dialog_params))
        .route("/word/v2/dialog/:sid/result", post(dialog_result))
        .route("/word/v2/dialog/:sid/closed", post(dialog_closed))
        .route_layer(middleware::from_fn(require_header))
        // 旧版文書の webextension パート（legacy）や引用の多い文書の render は既定の 2MB を超えうる
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(v2)
}

fn dialog_app(app_dir: &Path) -> PathBuf {
    match openref_core::env_os("DIALOG_APP") {
        Some(p) => PathBuf::from(p),
        None => app_dir.join("word").join("OpenRef Dialog.app"),
    }
}

// ---------- ヘッダ ----------

async fn require_header(req: Request, next: Next) -> Response {
    if req.headers().get("x-openref").is_some_and(|v| v.as_bytes() == b"word") {
        next.run(req).await
    } else {
        (
            StatusCode::FORBIDDEN,
            [("content-type", "text/plain; charset=us-ascii")],
            "ERR\tX-OpenRef: word header required",
        )
            .into_response()
    }
}

// ---------- 行形式 ----------

/// 欄のエスケープ。エスケープ文字は `%`: `%` → `%%`・TAB → `%t`・LF → `%n`・CR → `%r`、0x20〜0x7E 以外と
/// `\`（0x5C）・`~`（0x7E）は `%uXXXX`（UTF-16 単位・大文字 16 進）。日本語ロケールの Mac Word は VBA のソースも popen の出力も
/// MacJapanese で読む（0x5C = ¥・0x7E = ‾・U+005C は 0x80 に化ける）ので `\` と `~` は応答に出さない。
/// 結果は 0x20〜0x7E から `\` `~` を除いた文字だけ（生の TAB / 改行も含まない）。payload の JSON の `\` も `%u005C` で運ぶ
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%%"),
            '\t' => out.push_str("%t"),
            '\n' => out.push_str("%n"),
            '\r' => out.push_str("%r"),
            ' '..='}' if c != '\\' => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "%u{u:04X}");
                }
            }
        }
    }
    out
}

/// VBA 向けの応答本文に許すバイト: TAB・LF と 0x20〜0x7E から `\`（0x5C）・`~`（0x7E）を除いたもの
pub fn vba_safe(body: &str) -> bool {
    body.bytes().all(|b| b == b'\t' || b == b'\n' || ((0x20..=0x7D).contains(&b) && b != b'\\'))
}

/// 1 行 = 種別 + TAB 区切りの欄（欄はエスケープする）
pub fn line(kind: &str, fields: &[&str]) -> String {
    let mut s = kind.to_string();
    for f in fields {
        s.push('\t');
        s.push_str(&esc(f));
    }
    s
}

/// `OK` + 行（応答本文）。どの行も "\n" で終える
pub fn ok_text(lines: &[String]) -> String {
    let mut s = String::from("OK\n");
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s
}

/// `ERR<TAB>文言[<TAB>補足]`（利用者に見せる。補足 = 原因や対処の 1 行。VBA の Parse が読む）
pub fn err_text(msg: &str, detail: Option<&str>) -> String {
    match detail {
        Some(d) => format!("ERR\t{}\t{}\n", esc(msg), esc(d)),
        None => format!("ERR\t{}\n", esc(msg)),
    }
}

fn text(body: String) -> Response {
    debug_assert!(vba_safe(&body), "VBA 向けの応答は TAB・LF と \\ ~ を除く印字可能 ASCII のみ");
    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=us-ascii"), ("cache-control", "no-store")],
        body,
    )
        .into_response()
}

fn ok(lines: &[String]) -> Response {
    text(ok_text(lines))
}

/// 利用者に見せるエラー（文言 + 補足）
#[derive(Debug, Clone, PartialEq)]
pub struct UserErr {
    pub msg: String,
    pub detail: Option<String>,
}

impl UserErr {
    fn new(msg: impl Into<String>, detail: Option<String>) -> Self {
        Self { msg: msg.into(), detail }
    }
    fn text(&self) -> String {
        err_text(&self.msg, self.detail.as_deref())
    }
}

fn err(e: &UserErr) -> Response {
    text(e.text())
}

fn bad_request(detail: impl Into<String>) -> UserErr {
    UserErr::new("OpenRef（connectd）への要求を読めません。", Some(detail.into()))
}

/// 要求本文（VBA が文字列連結で作る JSON。非 ASCII は \uXXXX）。空なら {}。Content-Type は見ない（curl -d は form 扱いで送る）
fn parse_body(body: &[u8]) -> Result<Value, UserErr> {
    let b = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    if b.iter().all(|c| c.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    serde_json::from_slice(b).map_err(|e| bad_request(format!("JSON: {e}")))
}

/// 真偽の欄（VBA が "true" の文字列で送っても受ける）
fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1"),
        Some(Value::Number(n)) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
        _ => false,
    }
}

/// JS の parseFloat 相当（先頭の数値部分）
fn parse_float(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == digits_start || &t[digits_start..i] == "." {
        return None;
    }
    let mut end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let k = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > k {
            end = j;
        }
    }
    t[..end].parse().ok()
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => parse_float(s),
        _ => None,
    }
}

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

/// フォントの大きさ（数値 / parseFloat できる文字列で、正の有限の数のときだけ）
fn font_size_of(v: Option<&Value>) -> Option<f64> {
    num(v).filter(|x| x.is_finite() && *x > 0.0)
}

/// 要求の style（無い・文字列でない・空なら既定のスタイル）
fn style_of(req: &Value) -> &str {
    str_of(req, "style").filter(|s| !s.is_empty()).unwrap_or(citestyle::DEFAULT_STYLE)
}

/// フォントの大きさ等の数値 → 文字列（JS の Number の toString と同じ形: 10.5 / 12）
fn fmt_num(x: f64) -> String {
    format!("{x}")
}

// ---------- payload（{"items":[{"key":…,"csl":{…}}]}） ----------

/// payload（値・または JSON の文字列）→ [(key, csl)]。形が違えば None（key は空でない文字列・csl はオブジェクト）
pub fn payload_items(v: &Value) -> Option<Vec<(String, Value)>> {
    let parsed;
    let obj = match v {
        Value::String(s) => {
            parsed = serde_json::from_str::<Value>(s).ok()?;
            &parsed
        }
        other => other,
    };
    let items = obj.get("items")?.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let key = it.get("key")?.as_str()?;
        let csl = it.get("csl")?;
        if key.is_empty() || !csl.is_object() {
            return None;
        }
        out.push((key.to_string(), csl.clone()));
    }
    Some(out)
}

fn payload_value(items: &[(String, Value)]) -> Value {
    json!({ "items": items.iter().map(|(k, c)| json!({"key": k, "csl": c})).collect::<Vec<_>>() })
}

/// CSL から abstract を除く（文書に持つ snapshot を小さくする。Document Variables は 1 個 65,000 文字まで）
pub fn strip_abstract(csl: &Value) -> Value {
    let mut c = csl.clone();
    if let Some(o) = c.as_object_mut() {
        o.remove("abstract");
    }
    c
}

/// ASCII だけの JSON（非 ASCII は \uXXXX = UTF-16 単位）。VBA は payload を不透明な文字列として保存・送信するだけ
/// （行に載せるときは [`esc`] が JSON の `\` も `%u005C` にする）
pub fn ascii_json(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_else(|_| "null".into());
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                let _ = write!(out, "\\u{u:04x}");
            }
        }
    }
    out
}

/// 選んだ key（順序どおり・重複なし）から payload を作る: 蔵書（ゴミ箱外）の書誌 → 無ければ snapshot（snapshot に同じ key が
/// 複数あれば後の方が勝つ）。csl から abstract を除く。どちらにも無い key があれば Err(その key 一覧)
pub fn build_payload(db: &Db, keys: &[String], snapshot: &[(String, Value)]) -> Result<Value, Vec<String>> {
    let mut items = Vec::new();
    let mut missing = Vec::new();
    for k in keys {
        let fresh = match search::get_by_key(db, k) {
            Ok(Some(it)) if it.csl.is_object() => Some(it.csl),
            Ok(_) => None,
            Err(e) => {
                eprintln!("word v2: {k} の書誌を読めません: {e}");
                None
            }
        };
        match fresh.or_else(|| snapshot.iter().rev().find(|(sk, _)| sk == k).map(|(_, c)| c.clone())) {
            Some(c) => items.push((k.clone(), strip_abstract(&c))),
            None => missing.push(k.clone()),
        }
    }
    if missing.is_empty() {
        Ok(payload_value(&items))
    } else {
        Err(missing)
    }
}

/// 筆頭著者 + 年（著者 = 筆頭の family → literal → "?"・年が無ければ著者だけ）
pub fn label_of(csl: &Value) -> String {
    let a0 = csl.get("author").and_then(|a| a.get(0));
    let truthy_str = |v: Option<&Value>| match v {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) if n.as_f64() != Some(0.0) => Some(n.to_string()),
        _ => None,
    };
    let a = a0
        .and_then(|a| truthy_str(a.get("family")).or_else(|| truthy_str(a.get("literal"))))
        .unwrap_or_else(|| "?".into());
    let y = truthy_str(csl.get("issued").and_then(|i| i.get("date-parts")).and_then(|d| d.get(0)).and_then(|d| d.get(0)));
    match y {
        Some(y) => format!("{a} {y}"),
        None => a,
    }
}

// ---------- 旧版文書の document settings（移行） ----------

fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        // 文字参照の終わりの ';' をバイトで探す（文字列のスライスは 12 バイト目が日本語の途中に当たると panic する。
        // ';' は ASCII なのでその位置で切るのは安全）
        let Some(semi) = tail.as_bytes()[..tail.len().min(12)].iter().position(|&b| b == b';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let ch = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `<we:property name="…" value="…"/>` を文書順に拾う（名前空間の接頭辞は問わない・XML の断片でもよい・
/// 属性値の中の `>` は飛ばす）。値は文字参照を戻した文字列
pub fn we_properties(xml: &str) -> Vec<(String, String)> {
    let b = xml.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < n && !(b[j].is_ascii_whitespace() || b[j] == b'/' || b[j] == b'>') {
            j += 1;
        }
        let name = &xml[start..j];
        let local = name.rsplit(':').next().unwrap_or(name);
        if local != "property" {
            i = j.max(start);
            continue;
        }
        // 属性
        let mut attrs: HashMap<String, String> = HashMap::new();
        loop {
            while j < n && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= n || b[j] == b'>' || b[j] == b'/' {
                break;
            }
            let a0 = j;
            while j < n && !(b[j] == b'=' || b[j].is_ascii_whitespace() || b[j] == b'>' || b[j] == b'/') {
                j += 1;
            }
            let aname = xml[a0..j].to_string();
            while j < n && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= n || b[j] != b'=' {
                continue;
            }
            j += 1;
            while j < n && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= n || (b[j] != b'"' && b[j] != b'\'') {
                break;
            }
            let q = b[j];
            let v0 = j + 1;
            let Some(len) = b[v0..].iter().position(|&c| c == q) else {
                j = n;
                break;
            };
            attrs.insert(aname, xml_unescape(&xml[v0..v0 + len]));
            j = v0 + len + 1;
        }
        if let (Some(k), Some(v)) = (attrs.remove("name"), attrs.remove("value")) {
            out.push((k, v));
        }
        i = j;
    }
    out
}

fn valid_cite_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|c| c.is_ascii_alphanumeric())
}

/// 旧版（Office JS アドイン）の document settings（webextension パートの `<we:properties>`。値は JSON を属性に入れたもの）→
/// Document Variables。`refract-*` のうち v2 が使うものだけ: 引用の payload（abstract を除いた ASCII の JSON）・
/// style・doi-always（true/false）・font-name・font-size（数値の文字列）。台帳 `refract-index` は v2 では持たないので落とす。
/// 同じ名前が複数あれば最初のもの（壊れた値・空の payload は書かない）
pub fn legacy_vars(xml: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, raw) in we_properties(xml) {
        if !name.starts_with("refract-") || out.iter().any(|(n, _)| n == &name) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let val = if let Some(id) = name.strip_prefix("refract-cite-") {
            match payload_items(&v) {
                Some(items) if valid_cite_id(id) && !items.is_empty() => {
                    let items: Vec<(String, Value)> = items.iter().map(|(k, c)| (k.clone(), strip_abstract(c))).collect();
                    ascii_json(&payload_value(&items))
                }
                _ => continue,
            }
        } else {
            match name.as_str() {
                "refract-style" => match v.as_str() {
                    Some(s) if citestyle::valid_id(s) => s.to_string(),
                    _ => continue,
                },
                "refract-doi-always" => match v.as_bool() {
                    Some(b) => b.to_string(),
                    None => continue,
                },
                "refract-font-name" => match v.as_str() {
                    Some(s) if !s.trim().is_empty() => s.to_string(),
                    _ => continue,
                },
                "refract-font-size" => match font_size_of(Some(&v)) {
                    Some(x) => fmt_num(x),
                    None => continue,
                },
                _ => continue,
            }
        };
        out.push((name, val));
    }
    out
}

// ---------- 整形（citeproc on JavaScriptCore） ----------

pub struct Rendered {
    /// 引用ごとの (本文, run)（要求の cites と同順）
    pub cites: Vec<(String, String)>,
    /// 文献リストの (本文, run)。entry ごとに "\n"
    pub bib: (String, String),
    /// 文献数（文書順の初出順に並べた key の数）
    pub items: usize,
    pub warnings: Vec<String>,
    /// JavaScriptCore の準備（context + citeproc 等の評価）/ 整形の所要時間
    pub load_ms: u128,
    pub render_ms: u128,
}

fn js_lit(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// citeproc で整形する（word/dotm-render.js の render）。cites = 文書順の payload（{items:[{key,csl}]}）・
/// fresh = 蔵書にあった key の CSL。locale = 言語 → XML（取れなければ None = en-US の語彙で代用）
pub fn render_js(
    style_xml: &str,
    doi_always: bool,
    cites: &[Value],
    fresh: &Map<String, Value>,
    locale: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<Rendered, String> {
    let t0 = Instant::now();
    let ctx = jsc::Context::new()?;
    ctx.eval(CITEPROC_JS, "citeproc.js")?;
    ctx.eval(CITE_CORE_JS, "cite-core.js")?;
    ctx.eval(DOTM_RENDER_JS, "dotm-render.js")?;
    let load_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let en = citestyle::LOCALE_EN_US;
    let langs: Vec<String> = serde_json::from_str(&ctx.eval(
        &format!("JSON.stringify(OpenRefDotm.locales({}, {}))", js_lit(style_xml), js_lit(en)),
        "locales",
    )?)
    .map_err(|e| format!("locale の一覧を読めません: {e}"))?;
    let mut locales = Map::new();
    locales.insert("en-US".into(), json!(en));
    for l in langs {
        if locales.contains_key(&l) {
            continue;
        }
        if let Some(x) = locale(&l) {
            locales.insert(l, json!(x));
        }
    }
    let input = json!({
        "styleXml": style_xml,
        "locales": locales,
        "doiAlways": doi_always,
        "cites": cites,
        "fresh": fresh,
    });
    let src = format!(
        "(function () {{ try {{ return JSON.stringify({{ ok: OpenRefDotm.render({input}) }}); }} \
         catch (e) {{ return JSON.stringify({{ error: String((e && e.message) || e), stack: String((e && e.stack) || '') }}); }} }})()"
    );
    let res: Value = serde_json::from_str(&ctx.eval(&src, "render")?).map_err(|e| format!("整形の結果を読めません: {e}"))?;
    if let Some(e) = res.get("error").and_then(|v| v.as_str()) {
        let stack = res.get("stack").and_then(|v| v.as_str()).unwrap_or("");
        eprintln!("word v2 render: citeproc の例外: {e}\n{stack}");
        return Err(format!("citeproc: {e}"));
    }
    let r = &res["ok"];
    let pair = |v: &Value| (v["text"].as_str().unwrap_or("").to_string(), v["runs"].as_str().unwrap_or("").to_string());
    let out_cites: Vec<(String, String)> = r["cites"].as_array().map(|a| a.iter().map(pair).collect()).unwrap_or_default();
    if out_cites.len() != cites.len() {
        return Err(format!("整形の結果の数が合いません（引用 {} / 結果 {}）", cites.len(), out_cites.len()));
    }
    Ok(Rendered {
        cites: out_cites,
        bib: pair(&r["bib"]),
        items: r["items"].as_u64().unwrap_or(0) as usize,
        warnings: r["warnings"]
            .as_array()
            .map(|a| a.iter().filter_map(|w| w.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        load_ms,
        render_ms: t1.elapsed().as_millis(),
    })
}

/// render の本体（spawn_blocking の中）: 蔵書の最新を引く（DB ロックはこの間だけ）→ スタイル・locale（未キャッシュなら取得）→
/// citeproc → 行。cites = 検証済みの payload（文書順）
fn render_doc(st: &St, style: &str, doi_always: bool, cites: &[Vec<(String, Value)>]) -> Result<Vec<String>, UserErr> {
    let t0 = Instant::now();
    let mut keys: Vec<&str> = Vec::new();
    for c in cites {
        for (k, _) in c {
            if !keys.contains(&k.as_str()) {
                keys.push(k);
            }
        }
    }
    let (fresh, dir) = {
        let db = st.db();
        let mut m = Map::new();
        for k in &keys {
            match search::get_by_key(&db, k) {
                Ok(Some(it)) => {
                    m.insert(k.to_string(), it.csl);
                }
                Ok(None) => {}
                Err(e) => eprintln!("word v2 render: {k} の書誌を読めません（snapshot で整形）: {e}"),
            }
        }
        (m, citestyle::styles_dir(&db))
    };
    // スタイルが取れなければここで失敗 = 文書は無傷
    let style_xml = citestyle::style_xml(&dir, style)
        .map_err(|e| UserErr::new(format!("引用スタイル {style} を読み込めません。"), Some(e.to_string())))?;
    let payloads: Vec<Value> = cites.iter().map(|c| payload_value(c)).collect();
    let r = render_js(&style_xml, doi_always, &payloads, &fresh, &mut |lang| citestyle::locale_xml(&dir, lang).ok())
        .map_err(|e| UserErr::new("引用を整形できません。", Some(e)))?;
    for w in r.warnings.iter().take(5) {
        eprintln!("word v2 render: citeproc: {w}");
    }
    let mut lines: Vec<String> = r.cites.iter().map(|(t, runs)| line("C", &[t, runs])).collect();
    lines.push(line("B", &[&r.bib.0, &r.bib.1]));
    lines.push(line("S", &[style, &r.items.to_string()]));
    eprintln!(
        "word v2 render: 引用 {} 箇所・文献 {}（蔵書 {} / snapshot {}）・{style} · 準備 {} ms + 整形 {} ms · 計 {} ms",
        cites.len(),
        r.items,
        fresh.len(),
        keys.len() - fresh.len(),
        r.load_ms,
        r.render_ms,
        t0.elapsed().as_millis()
    );
    Ok(lines)
}

/// `POST /word/v2/render` `{"style","doiAlways","cites":[<payload>…]}` → `C<TAB>本文<TAB>run`（cites と同順）… `B` `S`。
/// 引用 0 件は `S<TAB>style<TAB>0` だけ（文書に触れない）
async fn render(State(v2): State<Arc<V2>>, body: Bytes) -> Response {
    let req = match parse_body(&body) {
        Ok(v) => v,
        Err(e) => return err(&e),
    };
    let style = str_of(&req, "style")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(citestyle::DEFAULT_STYLE)
        .to_string();
    if !citestyle::valid_id(&style) {
        return err(&UserErr::new(format!("スタイル id が不正です: {style}"), None));
    }
    let doi_always = truthy(req.get("doiAlways"));
    let raw: &[Value] = match req.get("cites") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(a)) => a,
        Some(_) => return err(&bad_request("cites が配列ではありません")),
    };
    let mut cites = Vec::with_capacity(raw.len());
    for (i, c) in raw.iter().enumerate() {
        match payload_items(c) {
            Some(items) if !items.is_empty() => cites.push(items),
            _ => {
                return err(&UserErr::new(
                    format!("文書の {} 番目の引用のデータを読めません。", i + 1),
                    Some("引用データが壊れています。その引用を Add/Edit で付け直してください。".into()),
                ))
            }
        }
    }
    if cites.is_empty() {
        return ok(&[line("S", &[&style, "0"])]);
    }
    let st = v2.st.clone();
    match tokio::task::spawn_blocking(move || render_doc(&st, &style, doi_always, &cites)).await {
        Ok(Ok(lines)) => ok(&lines),
        Ok(Err(e)) => {
            eprintln!("word v2 render: {} {}", e.msg, e.detail.as_deref().unwrap_or(""));
            err(&e)
        }
        Err(e) => err(&UserErr::new("引用を整形できません。", Some(e.to_string()))),
    }
}

/// `POST /word/v2/prefs` → `P<TAB>app 既定 style<TAB>doi 0/1`（新規文書の初期値）
async fn prefs(State(v2): State<Arc<V2>>) -> Response {
    let p = citestyle::prefs(&v2.st.db());
    let style = p["style"].as_str().unwrap_or(citestyle::DEFAULT_STYLE).to_string();
    let doi = if p["doi_always"].as_bool() == Some(true) { "1" } else { "0" };
    ok(&[line("P", &[&style, doi])])
}

/// `POST /word/v2/legacy` `{"xml":"<we:webextension…>…"}` → `V<TAB>変数名<TAB>値`
async fn legacy(body: Bytes) -> Response {
    let req = match parse_body(&body) {
        Ok(v) => v,
        Err(e) => return err(&e),
    };
    let Some(xml) = str_of(&req, "xml") else {
        return err(&bad_request("xml がありません"));
    };
    let vars = legacy_vars(xml);
    eprintln!("word v2 legacy: 旧版の文書設定から {} 個（XML {} 文字）", vars.len(), xml.chars().count());
    ok(&vars.iter().map(|(n, v)| line("V", &[n, v])).collect::<Vec<_>>())
}

/// `POST /word/v2/log` `{"level","message"}` → connectd のログ（/tmp/openref-connectd.log）へ 1 行
async fn log(body: Bytes) -> Response {
    let req = parse_body(&body).unwrap_or_else(|_| json!({}));
    let level = str_of(&req, "level").unwrap_or("info");
    let msg: String = str_of(&req, "message")
        .unwrap_or("")
        .chars()
        .take(2000)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let level: String = level.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
    eprintln!("word(dotm) {level}: {msg}");
    ok(&[])
}

// ---------- ダイアログ（dialog・OpenRef Dialog.app） ----------

/// 結果の行を作るのに要る、セッションの種類ごとの情報
#[derive(Debug, Clone)]
pub enum Kind {
    /// edit = 既存の引用の編集（delete が効く）。snapshot = 既存の payload の items（蔵書に無い key の書誌）
    Cite { edit: bool, snapshot: Vec<(String, Value)> },
    /// 開いたときの値（apply が欄を省いたときに使う）
    Settings { style: String, doi_always: bool },
    Notice,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub kind: Kind,
    pub page: &'static str,
    pub tab: Option<String>,
    pub d: Value,
    pub width: u32,
    pub height: u32,
    pub title: &'static str,
}

fn style_title(st: &St, id: &str) -> String {
    let dir = citestyle::styles_dir(&st.db());
    citestyle::installed(&dir)
        .iter()
        .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(id))
        .and_then(|s| s.get("title").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_else(|| id.to_string())
}

/// VBA の要求 → ページの d（ページの d は connectd が作る = VBA はラベルも style の表示名も作らない）
pub fn dialog_spec(req: &Value, style_title: &dyn Fn(&str) -> String) -> Result<Spec, UserErr> {
    let kind = str_of(req, "kind").unwrap_or("");
    match kind {
        "cite" => {
            let edit = str_of(req, "mode") == Some("edit");
            // 読めない引用データは「データなし」として開く（ダイアログで付け直すか削除できる）
            let mut broken = false;
            let snapshot = match req.get("existing") {
                None | Some(Value::Null) => Vec::new(),
                Some(v) => payload_items(v).unwrap_or_else(|| {
                    broken = true;
                    Vec::new()
                }),
            };
            let style = style_of(req);
            let items: Vec<Value> = if edit {
                snapshot.iter().map(|(k, c)| json!({"key": k, "label": label_of(c)})).collect()
            } else {
                Vec::new()
            };
            let mut d = json!({
                "mode": if edit { "edit" } else { "add" },
                "items": items,
                "style": { "id": style, "title": style_title(style) },
            });
            if truthy(req.get("missing")) || (edit && broken) {
                d["missing"] = json!(true);
            }
            Ok(Spec {
                kind: Kind::Cite { edit, snapshot },
                page: "cite-dialog.html",
                tab: None,
                d,
                width: SIZE_CITE.0,
                height: SIZE_CITE.1,
                title: "OpenRef — 引用",
            })
        }
        "settings" => {
            let tab = if str_of(req, "tab") == Some("doc") { "doc" } else { "style" };
            let style = style_of(req).to_string();
            let doi_always = truthy(req.get("doiAlways"));
            let font_name = str_of(req, "fontName").map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_FONT);
            let font_size = font_size_of(req.get("fontSize")).unwrap_or(DEFAULT_SIZE);
            let cites = num(req.get("cites")).filter(|x| x.is_finite() && *x >= 0.0).unwrap_or(0.0) as i64;
            let d = json!({
                "style": style,
                "doiAlways": doi_always,
                "fontName": font_name,
                "fontSize": font_size,
                "cites": cites,
                "pinned": truthy(req.get("pinned")),
            });
            Ok(Spec {
                kind: Kind::Settings { style, doi_always },
                page: "settings-dialog.html",
                tab: Some(tab.into()),
                d,
                width: SIZE_SETTINGS.0,
                height: SIZE_SETTINGS.1,
                title: if tab == "doc" { "OpenRef — 文書の設定" } else { "OpenRef — 引用スタイル" },
            })
        }
        "notice" => {
            let nk = match str_of(req, "notice") {
                Some("confirm") => "confirm",
                Some("info") => "info",
                _ => "error",
            };
            // 空・null の欄は渡さない
            let mut d = json!({ "kind": nk });
            for k in ["title", "message", "detail", "ok", "danger", "copy"] {
                match req.get(k) {
                    None | Some(Value::Null) => {}
                    Some(Value::String(s)) if s.is_empty() => {}
                    Some(v) => d[k] = v.clone(),
                }
            }
            let (w, h) = match nk {
                "confirm" => SIZE_CONFIRM,
                "info" => SIZE_INFO,
                _ => SIZE_NOTICE,
            };
            Ok(Spec { kind: Kind::Notice, page: "notice.html", tab: None, d, width: w, height: h, title: "OpenRef" })
        }
        "" => Err(bad_request("kind がありません")),
        other => Err(bad_request(format!("不明なダイアログの種類: {other}"))),
    }
}

/// encodeURIComponent と同じ（UTF-8 のバイトを %XX）
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// ダイアログの URL: `<base>/word/<page>?host=dotm&sid=<sid>[&tab=…]&d=<params JSON>`（Office の URL 長制限は無い）
pub fn dialog_url(base: &str, sid: &str, spec: &Spec) -> String {
    let mut u = format!("{base}/word/{}?host=dotm&sid={}", spec.page, pct(sid));
    if let Some(t) = &spec.tab {
        let _ = write!(u, "&tab={}", pct(t));
    }
    let _ = write!(u, "&d={}", pct(&spec.d.to_string()));
    u
}

/// ページの結果メッセージ（{type:…}）→ VBA への応答本文（`OK` + `R` 行、または `ERR`）
pub fn result_text(db: &Db, kind: &Kind, msg: &Value) -> String {
    let ty = str_of(msg, "type").unwrap_or("");
    let r = |fields: &[&str]| ok_text(&[line("R", fields)]);
    match kind {
        Kind::Cite { edit, snapshot } => match ty {
            "insert" | "replace" => {
                let mut keys: Vec<String> = Vec::new();
                for k in msg.get("keys").and_then(|v| v.as_array()).into_iter().flatten() {
                    if let Some(k) = k.as_str().filter(|k| !k.is_empty()) {
                        if !keys.iter().any(|x| x == k) {
                            keys.push(k.to_string());
                        }
                    }
                }
                if keys.is_empty() {
                    return err_text("引用する文献が選ばれていません。", None);
                }
                match build_payload(db, &keys, snapshot) {
                    Ok(p) => r(&["ok", &ascii_json(&p)]),
                    Err(missing) => err_text(
                        &format!("蔵書に見つからない文献があります: {}", missing.join(", ")),
                        Some("ゴミ箱に移した・削除した文献は引用できません。"),
                    ),
                }
            }
            "delete" if *edit => r(&["delete"]),
            // cancel・× で閉じた・不正なメッセージ = キャンセル
            _ => r(&["cancel"]),
        },
        Kind::Settings { style, doi_always } => {
            if ty != "apply" {
                return r(&["cancel"]);
            }
            let style = match msg.get("style") {
                None | Some(Value::Null) => style.clone(),
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
            };
            if !citestyle::valid_id(&style) {
                return err_text(&format!("スタイル id が不正です: {style}"), None);
            }
            let doi = match msg.get("doiAlways") {
                Some(Value::Bool(b)) => *b,
                _ => *doi_always,
            };
            // 既定: 空のフォント名 = Century・不正な大きさ = 10.5
            let name = match msg.get("fontName") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.trim().to_string(),
                Some(v) => v.to_string(),
            };
            let name = if name.is_empty() { DEFAULT_FONT.to_string() } else { name };
            let size = font_size_of(msg.get("fontSize")).unwrap_or(DEFAULT_SIZE);
            r(&["apply", &style, if doi { "1" } else { "0" }, &name, &fmt_num(size)])
        }
        Kind::Notice => match ty {
            "ok" => r(&["ok"]),
            "close" => r(&["close"]),
            _ => r(&["cancel"]),
        },
    }
}

fn cancel_text() -> String {
    ok_text(&[line("R", &["cancel"])])
}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// アプレットが取りに来るのを待つ（since = 起動 or 最後に生存を確かめた時刻）
    Queued { since: Instant, launches: u32 },
    /// アプレットが窓を開いている（probed = 最後に生存を確かめた時刻）
    Open { probed: Instant },
    /// 結果あり（text = VBA への応答本文）。consumed = VBA が最初に受け取った時刻
    Done { text: String, at: Instant, consumed: Option<Instant> },
}

pub struct Session {
    pub sid: String,
    pub spec: Spec,
    pub url: String,
    pub phase: Phase,
    /// VBA が最後に poll した時刻
    pub polled: Instant,
}

/// VBA の poll 1 回分の結果
#[derive(Debug, PartialEq)]
pub enum Poll {
    Unknown,
    Pending,
    /// アプレットの生存確認が要る（呼び手が確かめて poll し直す）
    Probe,
    /// アプレットを起動し直す（呼び手が起動して relaunched を呼ぶ）
    Relaunch,
    Done(String),
}

/// ダイアログのセッション（作成順）。状態遷移: Queued →（アプレットの next）→ Open →（ページの result / アプレットの closed /
/// アプレットが消えた）→ Done →（VBA が受け取って KEEP_CONSUMED 後）削除
#[derive(Default)]
pub struct Dialogs {
    list: Vec<Session>,
}

impl Dialogs {
    fn get_mut(&mut self, sid: &str) -> Option<&mut Session> {
        self.list.iter_mut().find(|s| s.sid == sid)
    }

    pub fn create(&mut self, sid: String, spec: Spec, url: String, now: Instant) {
        self.sweep(now);
        self.list.push(Session { sid, spec, url, phase: Phase::Queued { since: now, launches: 1 }, polled: now });
    }

    pub fn remove(&mut self, sid: &str) {
        self.list.retain(|s| s.sid != sid);
    }

    /// アプレットの `next`: 最も古い待ち行列のセッションを開く
    pub fn next(&mut self, now: Instant) -> Option<&Session> {
        self.sweep(now);
        let s = self.list.iter_mut().find(|s| matches!(s.phase, Phase::Queued { .. }))?;
        s.phase = Phase::Open { probed: now };
        Some(s)
    }

    pub fn kind(&self, sid: &str) -> Option<Kind> {
        self.list.iter().find(|s| s.sid == sid).map(|s| s.spec.kind.clone())
    }

    pub fn params(&self, sid: &str) -> Option<Value> {
        self.list
            .iter()
            .find(|s| s.sid == sid)
            .map(|s| json!({ "page": s.spec.page, "tab": s.spec.tab, "d": s.spec.d }))
    }

    /// 結果を入れる。None = 不明なセッション / Some(false) = 既に結果あり（最初の結果が勝つ）
    pub fn set_result(&mut self, sid: &str, text: String, now: Instant) -> Option<bool> {
        let s = self.get_mut(sid)?;
        if matches!(s.phase, Phase::Done { .. }) {
            return Some(false);
        }
        s.phase = Phase::Done { text, at: now, consumed: None };
        Some(true)
    }

    /// アプレットの `closed`（窓が結果なしで閉じた）= cancel。結果があれば何もしない
    pub fn closed(&mut self, sid: &str, now: Instant) -> Option<bool> {
        self.set_result(sid, cancel_text(), now)
    }

    /// VBA の poll。alive = アプレットの生存確認の結果（Probe を返したときだけ呼び手が渡す）
    pub fn poll(&mut self, sid: &str, now: Instant, alive: Option<bool>) -> Poll {
        self.sweep(now);
        let Some(s) = self.get_mut(sid) else {
            return Poll::Unknown;
        };
        s.polled = now;
        match &mut s.phase {
            Phase::Done { text, consumed, .. } => {
                consumed.get_or_insert(now);
                Poll::Done(text.clone())
            }
            Phase::Queued { since, launches } => {
                if now.duration_since(*since) < QUEUE_GRACE {
                    return Poll::Pending;
                }
                match alive {
                    None => Poll::Probe,
                    // 動いている（前の窓を処理中・起動の途中）→ 次の確認まで待つ
                    Some(true) => {
                        *since = now;
                        Poll::Pending
                    }
                    Some(false) if *launches < MAX_LAUNCHES => Poll::Relaunch,
                    Some(false) => {
                        let t = err_text("ダイアログを表示できません。", Some("OpenRef Dialog.app が起動しません。"));
                        s.phase = Phase::Done { text: t.clone(), at: now, consumed: Some(now) };
                        Poll::Done(t)
                    }
                }
            }
            Phase::Open { probed } => {
                if now.duration_since(*probed) < PROBE_EVERY {
                    return Poll::Pending;
                }
                match alive {
                    None => Poll::Probe,
                    Some(true) => {
                        *probed = now;
                        Poll::Pending
                    }
                    // 窓を開いたままアプレットが消えた（落ちた・強制終了）= × で閉じたのと同じ
                    Some(false) => {
                        let t = cancel_text();
                        s.phase = Phase::Done { text: t.clone(), at: now, consumed: Some(now) };
                        Poll::Done(t)
                    }
                }
            }
        }
    }

    /// Relaunch の後: 起動できたら猶予を数え直す。起動できなければ ERR
    pub fn relaunched(&mut self, sid: &str, now: Instant, res: Result<(), UserErr>) {
        let Some(s) = self.get_mut(sid) else {
            return;
        };
        if let Phase::Queued { since, launches } = &mut s.phase {
            match res {
                Ok(()) => {
                    *since = now;
                    *launches += 1;
                }
                Err(e) => s.phase = Phase::Done { text: e.text(), at: now, consumed: None },
            }
        }
    }

    /// 結果待ちのセッションがあるか（focus でアプレットを前面に出すか）
    pub fn has_active(&self) -> bool {
        self.list.iter().any(|s| !matches!(s.phase, Phase::Done { .. }))
    }

    fn sweep(&mut self, now: Instant) {
        self.list.retain(|s| match &s.phase {
            Phase::Done { consumed: Some(c), .. } => now.duration_since(*c) < KEEP_CONSUMED,
            Phase::Done { at, consumed: None, .. } => now.duration_since(*at) < KEEP_IDLE,
            _ => now.duration_since(s.polled) < KEEP_IDLE,
        });
    }
}

/// アプレットを起動する（`/usr/bin/open -a <app> --args <base>`。動いていれば前面に出すだけ = 引数は渡らない）
fn launch_applet(app: &Path, base: &str) -> Result<(), UserErr> {
    let fail = |d: String| UserErr::new("ダイアログを表示できません。", Some(d));
    if !app.exists() {
        return Err(fail(format!("{} がありません。OpenRef for Word を設置し直してください。", app.display())));
    }
    let out = std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg(app)
        .arg("--args")
        .arg(base)
        .output()
        .map_err(|e| fail(format!("open を実行できません: {e}")))?;
    if !out.status.success() {
        return Err(fail(format!("open: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(())
}

/// アプレット（`<app>/Contents/MacOS/…`）のプロセスがあるか。判定できないときは「ある」（誤って cancel にしない）
fn applet_running(app: &Path) -> bool {
    let real = std::fs::canonicalize(app).unwrap_or_else(|_| app.to_path_buf());
    let needle = format!("{}/Contents/MacOS/", real.display());
    match std::process::Command::new("/bin/ps").args(["-axww", "-o", "args="]).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).lines().any(|l| l.contains(&needle)),
        _ => true,
    }
}

fn new_sid() -> String {
    format!("{}{}", keys::new_key(), keys::new_key()).to_ascii_lowercase()
}

/// `POST /word/v2/dialog`（kind ごとの JSON）→ `D<TAB>sid`。セッションを待ち行列に入れてからアプレットを起動する
async fn dialog_create(State(v2): State<Arc<V2>>, body: Bytes) -> Response {
    let req = match parse_body(&body) {
        Ok(v) => v,
        Err(e) => return err(&e),
    };
    let spec = match dialog_spec(&req, &|id| style_title(&v2.st, id)) {
        Ok(s) => s,
        Err(e) => return err(&e),
    };
    let sid = new_sid();
    let url = dialog_url(&v2.base, &sid, &spec);
    let page = spec.page;
    v2.dialogs().create(sid.clone(), spec, url, Instant::now());
    if let Err(e) = v2.launch().await {
        v2.dialogs().remove(&sid);
        eprintln!("word v2 dialog: {page} を開けません: {}", e.detail.as_deref().unwrap_or(""));
        return err(&e);
    }
    eprintln!("word v2 dialog: {sid} {page}");
    // 2 欄目 = アプレットのパス。VBA も前面の Word から open -a し直す（常駐プロセスからの起動は macOS のフォーカス横取り防止で
    // 前面・キー窓にならないことがある）
    ok(&[line("D", &[&sid, &v2.app.display().to_string()])])
}

/// `GET /word/v2/dialog/<sid>?wait=<ms>`（VBA の long-poll）→ `PENDING` / `R…` / `ERR`
async fn dialog_poll(
    State(v2): State<Arc<V2>>,
    AxPath(sid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let wait = q.get("wait").and_then(|w| w.trim().parse::<u64>().ok()).unwrap_or(0).min(30_000);
    let deadline = Instant::now() + Duration::from_millis(wait);
    loop {
        let mut alive = None;
        let step = loop {
            let p = v2.dialogs().poll(&sid, Instant::now(), alive);
            if p != Poll::Probe {
                break p;
            }
            let app = v2.app.clone();
            alive = Some(tokio::task::spawn_blocking(move || applet_running(&app)).await.unwrap_or(true));
        };
        match step {
            Poll::Unknown => {
                return err(&UserErr::new(
                    "ダイアログが見つかりません。",
                    Some("OpenRef（connectd）が再起動した可能性があります。もう一度操作してください。".into()),
                ))
            }
            Poll::Done(t) => return text(t),
            Poll::Relaunch => {
                eprintln!("word v2 dialog: {sid} をアプレットが取りに来ない → 起動し直す");
                let res = v2.launch().await;
                v2.dialogs().relaunched(&sid, Instant::now(), res);
            }
            Poll::Pending | Poll::Probe => {}
        }
        if Instant::now() >= deadline {
            return ok(&["PENDING".into()]);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `POST /word/v2/dialog/focus` → 結果待ちのダイアログがあればアプレットを前面へ（`open -a`）
async fn dialog_focus(State(v2): State<Arc<V2>>) -> Response {
    if v2.dialogs().has_active() {
        if let Err(e) = v2.launch().await {
            eprintln!("word v2 dialog: 前面に出せません: {}", e.detail.as_deref().unwrap_or(""));
        }
    }
    ok(&[])
}

/// `GET /word/v2/dialog/next`（アプレット）→ `{sid,url,width,height,title}` / 204
async fn dialog_next(State(v2): State<Arc<V2>>) -> Response {
    let mut d = v2.dialogs();
    match d.next(Instant::now()) {
        Some(s) => Json(json!({
            "sid": s.sid,
            "url": s.url,
            "width": s.spec.width,
            "height": s.spec.height,
            "title": s.spec.title,
        }))
        .into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "ダイアログが見つかりません" }))).into_response()
}

/// `GET /word/v2/dialog/<sid>/params`（ページ）→ `{page, tab, d}`
async fn dialog_params(State(v2): State<Arc<V2>>, AxPath(sid): AxPath<String>) -> Response {
    match v2.dialogs().params(&sid) {
        Some(p) => Json(p).into_response(),
        None => not_found(),
    }
}

/// `POST /word/v2/dialog/<sid>/result`（ページの結果メッセージ {type:…}）→ `{ok}`。
/// cite の insert / replace はここで payload を作る（蔵書 → snapshot。無ければ VBA には ERR）
async fn dialog_result(State(v2): State<Arc<V2>>, AxPath(sid): AxPath<String>, body: Bytes) -> Response {
    let msg: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let Some(kind) = v2.dialogs().kind(&sid) else {
        return not_found();
    };
    let st = v2.st.clone();
    let t = match tokio::task::spawn_blocking(move || result_text(&st.db(), &kind, &msg)).await {
        Ok(t) => t,
        Err(e) => err_text("ダイアログの結果を処理できません。", Some(&e.to_string())),
    };
    let head: String = t.lines().find(|l| *l != "OK").unwrap_or("").chars().take(40).collect();
    match v2.dialogs().set_result(&sid, t, Instant::now()) {
        Some(stored) => {
            eprintln!("word v2 dialog: {sid} → {head}{}", if stored { "" } else { "（結果は既にあるので無視）" });
            Json(json!({ "ok": true, "ignored": !stored })).into_response()
        }
        None => not_found(),
    }
}

/// `POST /word/v2/dialog/<sid>/closed`（アプレット: 窓が閉じた）→ 結果が無ければ cancel
async fn dialog_closed(State(v2): State<Arc<V2>>, AxPath(sid): AxPath<String>) -> Response {
    match v2.dialogs().closed(&sid, Instant::now()) {
        Some(changed) => {
            if changed {
                eprintln!("word v2 dialog: {sid} は結果なしで閉じた → cancel");
            }
            Json(json!({ "ok": true })).into_response()
        }
        None => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openref_core::{ingest, resolver};

    /// VBA 側の逆変換（テスト用。% エスケープ）
    fn unesc(s: &str) -> String {
        let mut units: Vec<u16> = Vec::new();
        let b: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < b.len() {
            if b[i] == '%' && i + 1 < b.len() {
                match b[i + 1] {
                    '%' => units.push('%' as u16),
                    't' => units.push('\t' as u16),
                    'n' => units.push('\n' as u16),
                    'r' => units.push('\r' as u16),
                    'u' => {
                        let h: String = b[i + 2..i + 6].iter().collect();
                        units.push(u16::from_str_radix(&h, 16).unwrap());
                        i += 6;
                        continue;
                    }
                    c => panic!("未知のエスケープ %{c}"),
                }
                i += 2;
            } else {
                let mut buf = [0u16; 2];
                units.extend_from_slice(b[i].encode_utf16(&mut buf));
                i += 1;
            }
        }
        String::from_utf16(&units).unwrap()
    }

    fn test_st() -> Arc<St> {
        Arc::new(St {
            db: Mutex::new(Db::open_in_memory().unwrap()),
            sessions: Mutex::new(Default::default()),
        })
    }

    fn add(st: &St, csl: Value) -> String {
        let mut db = st.db();
        ingest::add_item(&mut db, &resolver::Resolved { csl, retracted: None, source: "test" }).unwrap().key
    }

    fn k1() -> Value {
        json!({
            "type": "article-journal",
            "title": "Pancreatic cancer: changing epidemiology",
            "author": [{"family": "Stoffel", "given": "Elena M"}, {"family": "Goggins", "given": "Michael"}],
            "container-title": "Gastroenterology",
            "issued": {"date-parts": [[2023, 2]]},
            "volume": "164", "issue": "5", "page": "752-765",
            "DOI": "10.1053/j.gastro.2023.02.012",
            "abstract": "Long abstract that must not be stored in the document",
        })
    }

    fn k2() -> Value {
        json!({
            "type": "article-journal",
            "title": "小児の中枢神経感染症",
            "author": [{"family": "Corrêa", "given": "José"}],
            "container-title": "日本小児放射線学会雑誌",
            "issued": {"date-parts": [[2024]]},
            "volume": "44", "issue": "8", "page": "e230216",
        })
    }

    // ---------- 行形式 ----------

    #[test]
    fn esc_avoids_backslash_and_tilde_and_round_trips() {
        let cases = [
            "",
            "plain ASCII 123",
            "tab\there\nnewline\r\nCRLF",
            "back\\slash \\u0041 literal %u0041 %% 100%",
            "tilde ~user/x~ and ¥ ‾",
            "Corrêa José 小児の中枢神経感染症",
            "emoji \u{1F600} and NBSP\u{a0}",
            "ctrl \u{1}\u{1f}\u{7f} end",
            "\\\\\\t%t%n",
            r#"{"items":[{"key":"K","csl":{"title":"a \"q\" \\ é"}}]}"#,
        ];
        for s in cases {
            let e = esc(s);
            assert!(vba_safe(&e) && !e.contains('\t') && !e.contains('\n'), "{e}");
            assert!(!e.contains('\\') && !e.contains('~'), "{e}");
            assert_eq!(unesc(&e), s, "往復: {e}");
        }
        assert_eq!(esc("%"), "%%");
        assert_eq!(esc("\t\n\r"), "%t%n%r");
        assert_eq!(esc("a\\b~c"), "a%u005Cb%u007Ec");
        assert_eq!(esc("é"), "%u00E9");
        assert_eq!(esc("\u{1F600}"), "%uD83D%uDE00", "UTF-16 単位（サロゲートペア）");
        assert_eq!(esc("\u{7f}"), "%u007F");
        assert_eq!(line("C", &["(1)", "0:3:s"]), "C\t(1)\t0:3:s");
        assert_eq!(ok_text(&[line("P", &["vancouver", "0"])]), "OK\nP\tvancouver\t0\n");
        assert_eq!(err_text("小児", None), "ERR\t%u5C0F%u5150\n");
        assert_eq!(err_text("a", Some("b\tc")), "ERR\ta\tb%tc\n", "補足は 3 欄目");
        // payload（JSON の \u・\"）も同じエスケープで運ぶ = \ は %u005C
        let p = ascii_json(&json!({"items": [{"key": "K", "csl": {"title": "Corrêa \"q\" ~"}}]}));
        let l = line("R", &["ok", &p]);
        assert!(vba_safe(&l), "{l}");
        assert!(l.contains("%u005Cu00ea") && l.contains("%u007E"), "{l}");
        assert_eq!(unesc(l.split('\t').nth(2).unwrap()), p);
        assert!(!vba_safe("a\\b") && !vba_safe("a~b") && !vba_safe("é") && !vba_safe("a\rb") && vba_safe("a\tb\n"));
    }

    #[test]
    fn ascii_json_escapes_non_ascii_and_stays_json() {
        let v = json!({"t": "Corrêa \u{1F600} \"q\" \\ \n"});
        let s = ascii_json(&v);
        assert!(s.is_ascii(), "{s}");
        assert!(s.contains("\\u00ea") && s.contains("\\ud83d\\ude00"), "{s}");
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap(), v);
    }

    #[test]
    fn parse_float_like_js() {
        assert_eq!(parse_float("10.5"), Some(10.5));
        assert_eq!(parse_float(" 12pt"), Some(12.0));
        assert_eq!(parse_float("1e1x"), Some(10.0));
        assert_eq!(parse_float("abc"), None);
        assert_eq!(parse_float("."), None);
        assert_eq!(fmt_num(10.5), "10.5");
        assert_eq!(fmt_num(12.0), "12");
    }

    // ---------- payload ----------

    #[test]
    fn payload_strips_abstract_falls_back_to_snapshot_and_reports_missing() {
        let st = test_st();
        let key = add(&st, k1());
        let snap = vec![
            ("SNAP0001".to_string(), json!({"title": "old", "abstract": "x"})),
            ("SNAP0001".to_string(), json!({"title": "newer snapshot", "abstract": "y"})),
            (key.clone(), json!({"title": "snapshot of a library item"})),
        ];
        let db = st.db();
        let p = build_payload(&db, &[key.clone(), "SNAP0001".into()], &snap).unwrap();
        let items = payload_items(&p).unwrap();
        assert_eq!(items[0].0, key);
        assert_eq!(items[0].1["title"], "Pancreatic cancer: changing epidemiology", "蔵書の最新が勝つ");
        assert!(items[0].1.get("abstract").is_none(), "abstract は除く");
        assert_eq!(items[1].1["title"], "newer snapshot", "snapshot は後のものが勝つ");
        assert!(items[1].1.get("abstract").is_none());
        let e = build_payload(&db, &["NOPE0001".into(), key, "NOPE0002".into()], &snap).unwrap_err();
        assert_eq!(e, vec!["NOPE0001".to_string(), "NOPE0002".to_string()]);
    }

    #[test]
    fn payload_items_accepts_object_or_string_and_rejects_bad_shapes() {
        let p = json!({"items": [{"key": "A", "csl": {"title": "t"}}]});
        assert_eq!(payload_items(&p).unwrap().len(), 1);
        assert_eq!(payload_items(&json!(p.to_string())).unwrap()[0].0, "A");
        assert!(payload_items(&json!({"items": [{"key": "", "csl": {}}]})).is_none());
        assert!(payload_items(&json!({"items": [{"key": "A"}]})).is_none());
        assert!(payload_items(&json!({"nope": 1})).is_none());
        assert!(payload_items(&json!("not json")).is_none());
    }

    #[test]
    fn cite_result_builds_payload_from_library_or_errs() {
        let st = test_st();
        let key = add(&st, k1());
        let db = st.db();
        let edit = Kind::Cite { edit: true, snapshot: vec![("GONE0001".into(), k2())] };
        let t = result_text(&db, &edit, &json!({"type": "replace", "keys": [key, "GONE0001", key, ""]}));
        assert!(vba_safe(&t), "{t}");
        let mut ls = t.split('\n');
        assert_eq!(ls.next(), Some("OK"));
        let f: Vec<&str> = ls.next().unwrap().split('\t').collect();
        assert_eq!(&f[..2], &["R", "ok"]);
        let p: Value = serde_json::from_str(&unesc(f[2])).unwrap();
        let items = payload_items(&p).unwrap();
        assert_eq!(items.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), vec![key.as_str(), "GONE0001"]);
        assert_eq!(items[1].1["author"][0]["family"], "Corrêa", "非 ASCII は \\u で往復");
        assert!(items[0].1.get("abstract").is_none());
        let e = result_text(&db, &edit, &json!({"type": "insert", "keys": ["NOPE0001"]}));
        assert!(e.starts_with("ERR\t") && unesc(&e[4..]).contains("NOPE0001"), "{e}");
        assert!(result_text(&db, &edit, &json!({"type": "insert", "keys": []})).starts_with("ERR\t"));
        assert_eq!(result_text(&db, &edit, &json!({"type": "delete"})), "OK\nR\tdelete\n");
        let add_mode = Kind::Cite { edit: false, snapshot: vec![] };
        assert_eq!(result_text(&db, &add_mode, &json!({"type": "delete"})), "OK\nR\tcancel\n", "追加で delete は cancel");
        assert_eq!(result_text(&db, &add_mode, &json!({"type": "cancel"})), "OK\nR\tcancel\n");
        assert_eq!(result_text(&db, &add_mode, &Value::Null), "OK\nR\tcancel\n", "不正なメッセージ = cancel");
    }

    #[test]
    fn settings_and_notice_results() {
        let db = Db::open_in_memory().unwrap();
        let s = Kind::Settings { style: "radiology".into(), doi_always: true };
        assert_eq!(
            result_text(&db, &s, &json!({"type": "apply", "style": "vancouver", "doiAlways": false, "fontName": " 游明朝 ", "fontSize": 12})),
            format!("OK\nR\tapply\tvancouver\t0\t{}\t12\n", esc("游明朝"))
        );
        // 欄を省いたら開いたときの値・空のフォント名と不正な大きさは既定
        assert_eq!(
            result_text(&db, &s, &json!({"type": "apply", "fontName": "", "fontSize": "x"})),
            "OK\nR\tapply\tradiology\t1\tCentury\t10.5\n"
        );
        assert!(result_text(&db, &s, &json!({"type": "apply", "style": "../etc"})).starts_with("ERR\t"));
        assert_eq!(result_text(&db, &s, &json!({"type": "cancel"})), "OK\nR\tcancel\n");
        assert_eq!(result_text(&db, &Kind::Notice, &json!({"type": "ok"})), "OK\nR\tok\n");
        assert_eq!(result_text(&db, &Kind::Notice, &json!({"type": "close"})), "OK\nR\tclose\n");
        assert_eq!(result_text(&db, &Kind::Notice, &json!({"type": "cancel"})), "OK\nR\tcancel\n");
    }

    #[test]
    fn dialog_spec_builds_page_params() {
        let title = |id: &str| if id == "vancouver" { "Vancouver".to_string() } else { id.to_string() };
        let existing = json!({"items": [{"key": "K1", "csl": k1()}, {"key": "K2", "csl": {"author": [{"literal": "WHO"}]}}]});
        let s = dialog_spec(&json!({"kind": "cite", "mode": "edit", "style": "vancouver", "missing": false, "existing": existing.to_string()}), &title).unwrap();
        assert_eq!(s.page, "cite-dialog.html");
        assert_eq!(
            s.d,
            json!({"mode": "edit", "items": [{"key": "K1", "label": "Stoffel 2023"}, {"key": "K2", "label": "WHO"}], "style": {"id": "vancouver", "title": "Vancouver"}})
        );
        let a = dialog_spec(&json!({"kind": "cite", "mode": "add", "style": "radiology", "missing": true, "existing": null}), &title).unwrap();
        assert_eq!(a.d, json!({"mode": "add", "items": [], "style": {"id": "radiology", "title": "radiology"}, "missing": true}));
        let url = dialog_url("https://localhost:23120", "abc", &a);
        assert!(url.starts_with("https://localhost:23120/word/cite-dialog.html?host=dotm&sid=abc&d=%7B%22"), "{url}");
        let st = dialog_spec(&json!({"kind": "settings", "tab": "doc", "style": "vancouver", "doiAlways": "true", "fontName": "Century", "fontSize": 10.5, "cites": 3, "pinned": true}), &title).unwrap();
        assert_eq!(st.tab.as_deref(), Some("doc"));
        assert_eq!(st.d, json!({"style": "vancouver", "doiAlways": true, "fontName": "Century", "fontSize": 10.5, "cites": 3, "pinned": true}));
        assert!(dialog_url("https://x", "s", &st).contains("&tab=doc&d="));
        let n = dialog_spec(&json!({"kind": "notice", "notice": "confirm", "title": "確認", "message": "m", "detail": "", "ok": "削除", "danger": true, "copy": null}), &title).unwrap();
        assert_eq!(n.d, json!({"kind": "confirm", "title": "確認", "message": "m", "ok": "削除", "danger": true}));
        assert_eq!((n.width, n.height), SIZE_CONFIRM);
        assert!(dialog_spec(&json!({"kind": "bogus"}), &title).is_err());
        // 壊れた引用データは「データなし」で開く（エラーにしない = 付け直し・削除ができる）
        let b = dialog_spec(&json!({"kind": "cite", "mode": "edit", "existing": {"items": "x"}}), &title).unwrap();
        assert_eq!(b.d["missing"], json!(true));
    }

    #[test]
    fn label_first_author_and_year() {
        assert_eq!(label_of(&k1()), "Stoffel 2023");
        assert_eq!(label_of(&json!({"author": [{"family": "", "literal": "WHO"}], "issued": {"date-parts": [["2020"]]}})), "WHO 2020");
        assert_eq!(label_of(&json!({})), "?");
    }

    // ---------- 旧版文書 ----------

    #[test]
    fn xml_unescape_survives_multibyte_after_entities() {
        // 12 バイト目が日本語の途中に当たる並び（ASCII 1・2・4・5 文字 + 漢字・&amp; の直後に漢字）
        for (src, want) in [
            ("{&quot;title&quot;:&quot;CT検査の被ばく&quot;}", "{\"title\":\"CT検査の被ばく\"}"),
            ("&quot;X線&quot;", "\"X線\""),
            ("&quot;US所見&quot;", "\"US所見\""),
            ("&quot;A検&quot;", "\"A検\""),
            ("&quot;ABCDE検&quot;", "\"ABCDE検\""),
            ("&amp;画像&lt;", "&画像<"),
            ("&画像", "&画像"),
            ("& 画像診断の基礎", "& 画像診断の基礎"),
        ] {
            assert_eq!(xml_unescape(src), want, "{src}");
        }
    }

    #[test]
    fn broken_existing_payload_opens_the_dialog_as_missing() {
        let spec = dialog_spec(&json!({"kind": "cite", "mode": "edit", "style": "vancouver", "missing": false, "existing": "{not json"}), &|_| "Vancouver".into())
            .expect("読めない引用データでもダイアログは開く");
        assert_eq!(spec.d["missing"], json!(true));
        assert_eq!(spec.d["items"], json!([]));
    }

    #[test]
    fn legacy_reads_we_properties() {
        let cite = json!({"items": [{"key": "K1", "csl": k1()}, {"key": "K2", "csl": k2()}]});
        let attr = |v: &Value| v.to_string().replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        let xml = format!(
            r#"<pkg:part pkg:name="/word/webextensions/webextension1.xml"><pkg:xmlData>
<we:webextension xmlns:we="http://schemas.microsoft.com/office/webextensions/webextension/2010/11" id="{{3A1F}}">
<we:reference id="0b0b" version="1.0.0.0" store="developer" storeType="Registry"/>
<we:properties>
  <we:property name="refract-cite-ab12cd34" value="{cite}"/>
  <we:property name="refract-cite-zz99zz99" value="{{&quot;items&quot;:[]}}"/>
  <we:property name="refract-cite-brokenjs" value="{{not json"/>
  <we:property name="refract-index" value="[&quot;refract-cite-ab12cd34&quot;]"/>
  <we:property name="refract-style" value="&quot;radiology&quot;"/>
  <we:property name="refract-style" value="&quot;vancouver&quot;"/>
  <we:property name="refract-doi-always" value="true"/>
  <we:property value="&quot;&#28216;&#26126;&#26397;&quot;" name='refract-font-name'/>
  <we:property name="refract-font-size" value="10.5"/>
  <we:property name="other-addin-key" value="1"/>
</we:properties><we:bindings/><we:snapshot xmlns:r="x"/></we:webextension></pkg:xmlData></pkg:part>"#,
            cite = attr(&cite)
        );
        let vars = legacy_vars(&xml);
        let names: Vec<&str> = vars.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["refract-cite-ab12cd34", "refract-style", "refract-doi-always", "refract-font-name", "refract-font-size"]);
        let get = |n: &str| vars.iter().find(|(k, _)| k == n).unwrap().1.clone();
        let p = get("refract-cite-ab12cd34");
        assert!(p.is_ascii(), "payload は ASCII");
        let items = payload_items(&serde_json::from_str(&p).unwrap()).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].1.get("abstract").is_none(), "abstract は除く");
        assert_eq!(items[1].1["title"], "小児の中枢神経感染症");
        assert_eq!(get("refract-style"), "radiology", "同じ名前は最初のもの");
        assert_eq!(get("refract-doi-always"), "true");
        assert_eq!(get("refract-font-name"), "游明朝");
        assert_eq!(get("refract-font-size"), "10.5");
        // 断片（名前空間の宣言なし・接頭辞なし）でも読む
        assert_eq!(legacy_vars(r#"<property name="refract-style" value="&quot;radiology&quot;"/>"#), vec![("refract-style".to_string(), "radiology".to_string())]);
        assert!(legacy_vars("<we:properties></we:properties>").is_empty());
    }

    // ---------- ダイアログのセッション ----------

    fn spec() -> Spec {
        dialog_spec(&json!({"kind": "notice", "notice": "error", "message": "m"}), &|s: &str| s.to_string()).unwrap()
    }

    #[test]
    fn dialog_state_machine() {
        let t0 = Instant::now();
        let mut d = Dialogs::default();
        assert_eq!(d.poll("x", t0, None), Poll::Unknown);
        d.create("s1".into(), spec(), "u1".into(), t0);
        d.create("s2".into(), spec(), "u2".into(), t0);
        assert!(d.has_active());
        assert_eq!(d.poll("s1", t0, None), Poll::Pending);
        // アプレットは古い順に取る
        assert_eq!(d.next(t0).map(|s| s.sid.clone()), Some("s1".into()));
        assert_eq!(d.next(t0).map(|s| s.sid.clone()), Some("s2".into()));
        assert!(d.next(t0).is_none(), "待ち行列が空なら 204");
        // ページの結果 → VBA が受け取る（取り直しにも同じものを返す）
        assert_eq!(d.set_result("s1", "OK\nR\tok".into(), t0), Some(true));
        assert_eq!(d.set_result("s1", "OK\nR\tcancel".into(), t0), Some(false), "最初の結果が勝つ");
        assert_eq!(d.closed("s1", t0), Some(false), "結果の後の closed は何もしない");
        assert_eq!(d.poll("s1", t0, None), Poll::Done("OK\nR\tok".into()));
        assert_eq!(d.poll("s1", t0 + Duration::from_secs(10), None), Poll::Done("OK\nR\tok".into()));
        // × で閉じた = cancel
        assert_eq!(d.closed("s2", t0), Some(true));
        assert_eq!(d.poll("s2", t0, None), Poll::Done("OK\nR\tcancel\n".into()));
        // 受け取って KEEP_CONSUMED 後に消える
        let later = t0 + KEEP_CONSUMED + Duration::from_secs(11);
        assert_eq!(d.poll("s1", later, None), Poll::Unknown);
        assert!(!d.has_active());
        assert_eq!(d.set_result("nope", "x".into(), t0), None);
        assert_eq!(d.closed("nope", t0), None);
    }

    #[test]
    fn dialog_relaunches_then_gives_up_and_cancels_when_applet_vanishes() {
        let t0 = Instant::now();
        let mut d = Dialogs::default();
        d.create("q".into(), spec(), "u".into(), t0);
        let t1 = t0 + QUEUE_GRACE;
        // 猶予を過ぎたら生存確認を求める → 動いていれば待つ
        assert_eq!(d.poll("q", t1, None), Poll::Probe);
        assert_eq!(d.poll("q", t1, Some(true)), Poll::Pending);
        assert_eq!(d.poll("q", t1 + Duration::from_secs(1), None), Poll::Pending, "確認したばかり");
        // 動いていなければ起動し直す（上限まで）→ 上限を超えたら ERR
        let mut t = t1;
        for _ in 1..MAX_LAUNCHES {
            t += QUEUE_GRACE;
            assert_eq!(d.poll("q", t, Some(false)), Poll::Relaunch);
            d.relaunched("q", t, Ok(()));
        }
        t += QUEUE_GRACE;
        match d.poll("q", t, Some(false)) {
            Poll::Done(e) => assert!(e.starts_with("ERR\t"), "{e}"),
            p => panic!("{p:?}"),
        }
        // 起動に失敗したら ERR
        d.create("f".into(), spec(), "u".into(), t0);
        d.relaunched("f", t0, Err(UserErr::new("ダイアログを表示できません。", Some("open: だめ".into()))));
        assert!(matches!(d.poll("f", t0, None), Poll::Done(e) if e.starts_with("ERR\t")));
        // 窓を開いたままアプレットが消えた = cancel
        d.create("o".into(), spec(), "u".into(), t0);
        assert_eq!(d.next(t0).map(|s| s.sid.clone()), Some("o".into()));
        assert_eq!(d.poll("o", t0 + Duration::from_secs(1), None), Poll::Pending);
        let tp = t0 + PROBE_EVERY;
        assert_eq!(d.poll("o", tp, None), Poll::Probe);
        assert_eq!(d.poll("o", tp, Some(true)), Poll::Pending);
        assert_eq!(d.poll("o", tp + PROBE_EVERY, Some(false)), Poll::Done("OK\nR\tcancel\n".into()));
        // 誰も poll しないセッションは KEEP_IDLE で消える
        d.create("idle".into(), spec(), "u".into(), t0);
        assert_eq!(d.poll("idle", t0 + KEEP_IDLE + Duration::from_secs(1), None), Poll::Unknown);
    }

    // ---------- 整形（本物の citeproc on JavaScriptCore） ----------

    #[cfg(target_os = "macos")]
    #[test]
    fn render_vancouver_through_jsc() {
        let style = citestyle::builtin_xml("vancouver").unwrap();
        let p = |keys: &[&str]| {
            payload_value(&keys.iter().map(|k| (k.to_string(), if *k == "K1" { k1() } else { k2() })).collect::<Vec<_>>())
        };
        // 文書順 = [K2], [K1,K2], [K2]（複製 CC）。K1 は蔵書の最新（題名が違う）で整形される
        let mut fresh = Map::new();
        let mut k1f = k1();
        k1f["title"] = json!("Fresh title");
        fresh.insert("K1".into(), k1f);
        let mut asked = Vec::new();
        let t0 = Instant::now();
        let r = render_js(style, false, &[p(&["K2"]), p(&["K1", "K2"]), p(&["K2"])], &fresh, &mut |l| {
            asked.push(l.to_string());
            None
        })
        .unwrap();
        let ms = t0.elapsed().as_millis();
        assert!(asked.is_empty(), "vancouver は en-US だけ: {asked:?}");
        assert_eq!(r.cites, vec![("(1)".into(), "".into()), ("(1,2)".into(), "".into()), ("(1)".into(), "".into())]);
        assert_eq!(r.items, 2);
        let bib: Vec<&str> = r.bib.0.split('\n').collect();
        assert!(bib[0].starts_with("1. Corrêa J. 小児の中枢神経感染症. 日本小児放射線学会雑誌. 2024;44(8):e230216"), "{}", bib[0]);
        assert!(bib[1].starts_with("2. Stoffel EM, Goggins M. Fresh title. Gastroenterology."), "{}", bib[1]);
        eprintln!("render_vancouver_through_jsc: 準備 {} ms + 整形 {} ms（計 {ms} ms）", r.load_ms, r.render_ms);
        // AMA = 上付き + 斜体の誌名（run は UTF-16 単位）
        let ama = citestyle::builtin_xml("american-medical-association").unwrap();
        let r = render_js(ama, false, &[p(&["K2", "K1"])], &Map::new(), &mut |_| None).unwrap();
        assert_eq!(r.cites, vec![("1,2".into(), "0:3:s".into())]);
        let line2 = r.bib.0.split('\n').nth(1).unwrap();
        let off = r.bib.0.encode_utf16().count() - line2.encode_utf16().count() + line2.find("Gastroenterology").unwrap();
        assert!(r.bib.1.split(',').any(|x| x == format!("{off}:16:i")), "{} / {}", r.bib.0, r.bib.1);
        // 不正な CSL は Err（文書に触れない）
        assert!(render_js("<style", false, &[p(&["K1"])], &Map::new(), &mut |_| None).is_err());
    }

    // ---------- HTTP（ヘッダ・render・prefs・legacy・ダイアログの往復） ----------

    /// 素の HTTP/1.1 を 1 往復（テスト用。TLS なしの router に直接）。
    /// X-OpenRef ヘッダの無い 403 は、サーバが本文を読まずに応答して閉じる。受信バッファに未読の本文が残ったまま
    /// close するとカーネルが RST を送り、こちらの read が ECONNRESET になりうる（クライアント側の都合であって製品の不具合ではない）。
    /// → 要求は 1 回の write で送り（`write!` は断片ごとに write するので本文が遅れて届き、閉じた後の socket に当たって RST になる。
    /// これが主因）、それでも read が ConnectionReset で終わったら読めた分で解釈する（応答の頭が無ければ panic）。
    /// 送信後の shutdown(Write) はしない（hyper は半閉じを EOF と見て応答なしで切る）
    fn http(port: u16, method: &str, path: &str, header: bool, body: &str) -> (u16, String) {
        use std::io::{ErrorKind, Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let h = if header { "X-OpenRef: word\r\n" } else { "" };
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{h}Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        // 応答を返して先に閉じたサーバへの write は RST・EPIPE になりうる（応答は読める）ので、その 2 種は無視
        if let Err(e) = s.write_all(req.as_bytes()) {
            assert!(matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::BrokenPipe), "write: {e}");
        }
        let mut raw = Vec::new();
        if let Err(e) = s.read_to_end(&mut raw) {
            assert_eq!(e.kind(), ErrorKind::ConnectionReset, "read: {e}");
            assert!(raw.starts_with(b"HTTP/1.1 "), "RST で応答の頭が読めなかった: {e}: {:?}", String::from_utf8_lossy(&raw));
        }
        let raw = String::from_utf8(raw).unwrap();
        let (head, rest) = raw.split_once("\r\n\r\n").unwrap();
        let code: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
        // chunked なら解く
        let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
            let mut out = String::new();
            let mut r = rest;
            loop {
                let (n, tail) = r.split_once("\r\n").unwrap();
                let n = usize::from_str_radix(n.trim(), 16).unwrap();
                if n == 0 {
                    break;
                }
                out.push_str(&tail[..n]);
                r = &tail[n + 2..];
            }
            out
        } else {
            rest.to_string()
        };
        (code, body)
    }

    /// ダイアログの器は無い（起動すると ERR）。セッションの往復は v2 に直接入れて確かめる
    async fn serve(st: Arc<St>) -> (u16, Arc<V2>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let v2 = Arc::new(V2 {
            st,
            dialogs: Mutex::new(Dialogs::default()),
            base: format!("https://localhost:{port}"),
            app: PathBuf::from("/nonexistent/OpenRef Dialog.app"),
        });
        let app = routes(v2.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (port, v2)
    }

    async fn call(port: u16, method: &'static str, path: String, header: bool, body: String) -> (u16, String) {
        tokio::task::spawn_blocking(move || http(port, method, &path, header, &body)).await.unwrap()
    }

    /// VBA 向けのエンドポイント: 応答本文に 0x5C（\）・0x7E（~）・TAB と LF 以外の制御文字・非 ASCII が無いこと
    async fn vba(port: u16, method: &'static str, path: &str, body: String) -> String {
        let (code, b) = call(port, method, path.to_string(), true, body).await;
        assert_eq!(code, 200, "{path}: {b}");
        assert!(!b.bytes().any(|c| c == 0x5C || c == 0x7E), "{path}: \\ か ~ がある: {b}");
        assert!(vba_safe(&b), "{path}: {b}");
        b
    }

    /// \ と ~ を題名・URL に含む書誌（payload の JSON の \u・\" と合わせて、応答に 0x5C / 0x7E が出ないことを確かめる）
    fn k3() -> Value {
        json!({
            "type": "article-journal",
            "title": "C:\\temp ~ \"quoted\" 100% \u{1F600}",
            "author": [{"family": "Tilde~Back\\slash", "given": "Ä"}],
            "container-title": "J ~Test~",
            "issued": {"date-parts": [[2022]]},
            "URL": "https://example.org/~user/a\\b",
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_header_render_prefs_legacy_and_dialog_round_trip() {
        let st = test_st();
        let key = add(&st, k1());
        let key3 = add(&st, k3());
        let (port, v2) = serve(st.clone()).await;

        // ヘッダが無ければ 403（どの v2 エンドポイントも）
        for (m, p) in [("POST", "/word/v2/prefs"), ("POST", "/word/v2/render"), ("GET", "/word/v2/dialog/next"), ("POST", "/word/v2/dialog/x/result")] {
            let (code, body) = call(port, m, p.into(), false, "{}".into()).await;
            assert_eq!(code, 403, "{p}: {body}");
            assert!(vba_safe(&body), "{body}");
        }

        assert_eq!(vba(port, "POST", "/word/v2/prefs", "{}".into()).await, "OK\nP\tvancouver\t0\n");

        // render: 蔵書の key + snapshot だけの key（アクセント・和文）。要求は VBA と同じく非 ASCII を \u で
        let snap = ascii_json(&json!({"items": [{"key": "SNAP0001", "csl": k2()}]}));
        let mine = ascii_json(&json!({"items": [{"key": key, "csl": {"title": "stale"}}, {"key": "SNAP0001", "csl": k2()}]}));
        let req = format!(r#"{{"style":"vancouver","doiAlways":false,"cites":[{snap},{mine}]}}"#);
        assert!(req.is_ascii());
        let body = vba(port, "POST", "/word/v2/render", req).await;
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(lines[0], "OK");
        assert_eq!(lines[1], "C\t(1)\t");
        assert_eq!(lines[2], "C\t(1,2)\t");
        assert!(lines[3].starts_with("B\t1. Corr%u00EAa J. %u5C0F%u5150"), "{}", lines[3]);
        let bib = unesc(lines[3].split('\t').nth(1).unwrap());
        assert!(bib.contains("\n2. Stoffel EM, Goggins M. Pancreatic cancer: changing epidemiology."), "蔵書の最新: {bib}");
        assert_eq!(lines[4], "S\tvancouver\t2");
        assert_eq!(lines[5], "", "各行は改行で終わる");
        // \ と ~ を含む書誌（蔵書の最新）+ DOI / URL を常に含める
        let req = format!(
            r#"{{"style":"vancouver","doiAlways":true,"cites":[{}]}}"#,
            ascii_json(&json!({"items": [{"key": key3, "csl": {"title": "x"}}]}))
        );
        let body = vba(port, "POST", "/word/v2/render", req).await;
        let bib = unesc(body.split('\n').find(|l| l.starts_with("B\t")).unwrap().split('\t').nth(1).unwrap());
        assert!(bib.contains("C:\\temp ~") && bib.contains("https://example.org/~user/a\\b"), "{bib}");
        // 壊れた payload は ERR（文書に触れない）・引用 0 件は S だけ・壊れた JSON は ERR
        assert!(vba(port, "POST", "/word/v2/render", r#"{"cites":[{"items":[]}]}"#.into()).await.starts_with("ERR\t"));
        assert_eq!(vba(port, "POST", "/word/v2/render", r#"{"style":"radiology","cites":[]}"#.into()).await, "OK\nS\tradiology\t0\n");
        assert!(vba(port, "POST", "/word/v2/render", "{not json \\ ~".into()).await.starts_with("ERR\t"));

        // legacy（payload の \ ~ も %u で運ぶ）
        let cite = json!({"items": [{"key": "K3", "csl": k3()}]});
        let attr = cite.to_string().replace('&', "&amp;").replace('"', "&quot;");
        let xml = format!(
            r#"<we:properties><we:property name="refract-style" value="&quot;radiology&quot;"/><we:property name="refract-font-name" value="&quot;游明朝&quot;"/><we:property name="refract-cite-abcd1234" value="{attr}"/></we:properties>"#
        );
        let req = format!(r#"{{"xml":{}}}"#, ascii_json(&json!(xml)));
        let body = vba(port, "POST", "/word/v2/legacy", req).await;
        let ls: Vec<&str> = body.split('\n').collect();
        assert_eq!(&ls[..3], &["OK", "V\trefract-style\tradiology", "V\trefract-font-name\t%u6E38%u660E%u671D"]);
        let f: Vec<&str> = ls[3].split('\t').collect();
        assert_eq!(&f[..2], &["V", "refract-cite-abcd1234"]);
        let p = payload_items(&serde_json::from_str(&unesc(f[2])).unwrap()).unwrap();
        assert_eq!(p[0].1, k3(), "\\ ~ \" 非 ASCII が往復する");

        assert_eq!(vba(port, "POST", "/word/v2/log", r#"{"level":"warn","message":"test 小\nline"}"#.into()).await, "OK\n");
        assert_eq!(vba(port, "POST", "/word/v2/dialog/focus", "{}".into()).await, "OK\n", "結果待ちが無ければ何もしない");

        // ダイアログ: 器が無ければ作成は ERR（セッションは残さない）
        let body = vba(port, "POST", "/word/v2/dialog", r#"{"kind":"notice","notice":"info","message":"m"}"#.into()).await;
        assert!(body.starts_with("ERR\t") && unesc(&body[4..]).contains("Dialog.app"), "{body}");
        let (code, _) = call(port, "GET", "/word/v2/dialog/next".into(), true, String::new()).await;
        assert_eq!(code, 204);
        assert!(vba(port, "POST", "/word/v2/dialog", r#"{"kind":"nope"}"#.into()).await.starts_with("ERR\t"));
        assert!(vba(port, "GET", "/word/v2/dialog/none", String::new()).await.starts_with("ERR\t"), "不明な sid は ERR");

        // 起動を飛ばしてセッションを入れ、アプレット（next）→ ページ（params・result）→ VBA（poll）を往復する
        let spec = dialog_spec(
            &json!({"kind": "cite", "mode": "edit", "style": "vancouver", "missing": false, "existing": {"items": [{"key": "GONE0003", "csl": k3()}]}}),
            &|s: &str| s.to_string(),
        )
        .unwrap();
        let url = dialog_url(&v2.base, "sid1", &spec);
        v2.dialogs().create("sid1".into(), spec, url, Instant::now());
        assert_eq!(vba(port, "GET", "/word/v2/dialog/sid1?wait=100", String::new()).await, "OK\nPENDING\n");
        let (code, body) = call(port, "GET", "/word/v2/dialog/next".into(), true, String::new()).await;
        assert_eq!(code, 200);
        let next: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(next["sid"], "sid1");
        assert!(next["url"].as_str().unwrap().contains("/word/cite-dialog.html?host=dotm&sid=sid1&d="));
        assert_eq!((next["width"].as_u64(), next["height"].as_u64()), (Some(640), Some(480)));
        let (_, body) = call(port, "GET", "/word/v2/dialog/sid1/params".into(), true, String::new()).await;
        let params: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(params["d"]["mode"], "edit");
        assert_eq!(params["d"]["items"][0], json!({"key": "GONE0003", "label": "Tilde~Back\\slash 2022"}));
        // VBA の long-poll の最中にページの結果が届く（蔵書の key + 蔵書に無い key = 既存の snapshot）
        let poll = tokio::spawn(vba(port, "GET", "/word/v2/dialog/sid1?wait=3000", String::new()));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (_, body) =
            call(port, "POST", "/word/v2/dialog/sid1/result".into(), true, format!(r#"{{"type":"replace","keys":["{key}","{key3}","GONE0003"]}}"#)).await;
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["ok"], true);
        let body = poll.await.unwrap();
        let f: Vec<&str> = body.split('\n').nth(1).unwrap().split('\t').collect();
        assert_eq!(&f[..2], &["R", "ok"], "{body}");
        let p = payload_items(&serde_json::from_str(&unesc(f[2])).unwrap()).unwrap();
        assert_eq!(p.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), vec![key.as_str(), key3.as_str(), "GONE0003"]);
        assert!(p[0].1.get("abstract").is_none());
        assert_eq!(p[1].1["URL"], "https://example.org/~user/a\\b");
        assert_eq!(p[2].1, k3());
        // 窓が閉じた後の closed は何もしない・取り直しにも同じ結果
        call(port, "POST", "/word/v2/dialog/sid1/closed".into(), true, String::new()).await;
        assert_eq!(vba(port, "GET", "/word/v2/dialog/sid1", String::new()).await, body);
        let (code, _) = call(port, "POST", "/word/v2/dialog/nope/result".into(), true, "{}".into()).await;
        assert_eq!(code, 404);
    }
}
