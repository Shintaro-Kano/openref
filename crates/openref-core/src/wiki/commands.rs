//! 読みの command: `wiki_status` `wiki_list` `wiki_read` `wiki_backlinks` `wiki_search` `wiki_tags` `wiki_inbox`。
//! 登録表（crate::commands）の関数は `Ctx::from_db` で設定と app_dir を読んだら Db を手放し、ここの関数（Db を要しない）を呼ぶ。
//! GUI の Tauri 包みも同じ: Db のロックは `Ctx::from_db` の間だけ。

use super::codes;
use super::index::{self, default_title, sha256_hex, with_index, Fresh, Index, ListQuery, SearchQuery};
use super::scan::{self, Kind};
use crate::{CoreError, Db, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// 無効のときのエラー文（`wiki_status` 以外の全部）
pub const DISABLED: &str = "OpenNote は無効です（Settings › OpenNote）";

/// 登録表の関数が Db から読む設定（これを読んだら Db は要らない）
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub enabled: bool,
    pub root: Option<String>,
    /// None = in-memory の Db（索引の置き場が無い）
    pub app_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// GUI が root を監視していて、監視を張った後の stat 走査も済んでいる（変更は監視が索引に取り込む）= 入口の鮮度規則
    /// （5 秒ごとの stat 走査）を省く（`freshen`）。登録表（MCP / CLI）は常に false = 今どおり
    pub watched: bool,
}

impl Ctx {
    /// 設定 `wiki_enabled`（"1" = 有効）・`wiki_root` と `db.app_dir()`・環境変数 HOME
    pub fn from_db(db: &Db) -> Result<Ctx> {
        Ok(Ctx {
            enabled: db.setting("wiki_enabled")?.as_deref() == Some("1"),
            root: db.setting("wiki_root")?.filter(|s| !s.trim().is_empty()),
            app_dir: db.app_dir(),
            home: std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from),
            watched: false,
        })
    }

    /// 使える状態なら (索引のパス, canonical な root)
    pub fn ready(&self) -> Result<(PathBuf, PathBuf)> {
        if !self.enabled {
            return Err(CoreError::wiki(codes::UNAVAILABLE, DISABLED));
        }
        let app_dir = self
            .app_dir
            .as_deref()
            .ok_or_else(|| CoreError::wiki(codes::UNAVAILABLE, "in-memory の DB では OpenNote を使えません（索引の置き場が無い）"))?;
        let root = self.root.as_deref().ok_or_else(|| {
            CoreError::wiki(codes::UNAVAILABLE, "ワークスペースのフォルダが未設定です（GUI の Settings › OpenNote › 有効 でフォルダを選ぶ）")
        })?;
        let home = self.home.as_deref().ok_or_else(|| CoreError::wiki(codes::UNAVAILABLE, "HOME が未設定のためワークスペースのフォルダを検査できません"))?;
        let root = super::check_root(Path::new(root), home, Some(app_dir))?;
        Ok((app_dir.join(index::INDEX_FILE), root))
    }

    /// ホストのファイルのパス p（絶対か vault 相対）がワークスペースの中の通常のファイルなら canonical なパスを返す（symlink を解いてから
    /// root の配下かを見る = ワークスペースの中の symlink で外を指すものは外）。無効・root 未設定・実在しない・フォルダ・ワークスペースの外・
    /// vault 相対で `.` 始まりの区画を含むもの（.git / .obsidian / .trash 等 = wiki の他のコマンドと同じ）は Err。
    /// MCP の許可一覧で起動したセッションの import_pdf 等が使う
    pub fn host_path_in_vault(&self, p: &Path) -> Result<PathBuf> {
        use super::paths::Reject;
        let (_, root) = self.ready()?;
        // vault 相対に `..` は使わせない（wiki の他のコマンドと同じ）。残り（絶対パスの `..`・隠し区画）は canonicalize の後に実体で判定
        let opts = super::paths::Opts { allow_dir: false, judge_by_canonical: true };
        super::paths::resolve_existing(&root, p, opts).map(|(_, canon)| canon).map_err(|r| match r {
            Reject::Outside | Reject::Escapes | Reject::Root => bad(format!("ワークスペース（{}）の外です: {}", root.display(), p.display())),
            Reject::DotDot => bad(format!("パスに `..` は使えません: {}", p.display())),
            // DotStart は judge_by_canonical では出ない（字面の `.` 始まりも実体で判定 = Hidden）
            Reject::DotStart(rel) | Reject::Hidden(rel) => bad(format!("`.` で始まるフォルダ・ファイルの中は使えません: {rel}")),
            Reject::Missing(e) => bad(format!("{} を開けません: {e}", p.display())),
            Reject::NotFile(rel) => bad(format!("通常のファイルではありません: {rel}")),
        })
    }
}

pub(crate) fn bad(msg: impl Into<String>) -> CoreError {
    CoreError::BadInput(msg.into())
}

pub(crate) fn opt_str(a: &Value, k: &str) -> Result<Option<String>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone()).filter(|s| !s.trim().is_empty())),
        Some(_) => Err(bad(format!("{k} は文字列で渡してください"))),
    }
}

fn opt_int(a: &Value, k: &str) -> Result<Option<i64>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| bad(format!("{k} は整数で渡してください"))),
    }
}

/// `tag`（文字列 1 つ）と `tags`（文字列の配列）を合わせた tag の列（AND で使う）
fn tag_args(a: &Value) -> Result<Vec<String>> {
    let mut out: Vec<String> = opt_str(a, "tag")?.into_iter().collect();
    match a.get("tags") {
        None | Some(Value::Null) => {}
        Some(Value::Array(v)) => {
            for x in v {
                let t = x.as_str().ok_or_else(|| bad("tags は文字列の配列で渡してください"))?;
                if !t.trim().is_empty() {
                    out.push(t.trim().to_string());
                }
            }
        }
        Some(_) => return Err(bad("tags は文字列の配列で渡してください")),
    }
    Ok(out)
}

/// 索引を鮮度規則で更新してから f（作り直し中ならエラー）
fn fresh<T>(ctx: &Ctx, mut f: impl FnMut(&mut Index) -> Result<T>) -> Result<T> {
    let (ip, root) = ctx.ready()?;
    with_index(&ip, &root, |ix| {
        if let Fresh::Rebuilding(p) = freshen(ctx, ix)? {
            return Err(rebuilding(p));
        }
        f(ix)
    })
}

/// 入口の鮮度規則（`refresh_if_stale`）。GUI が監視中（ctx.watched）なら stat 走査を省く。ただし索引に作り直しの旗
/// （meta `rebuilding`・生死は問わない）があれば今どおり refresh_if_stale（旗の判定 = 他のプロセスの作り直し中は Rebuilding）に任せる。
/// 最終走査（meta `last_scan`）が無い = 一度も走査していない索引（壊れた索引の退避・root や schema の変更・作り直しの後）も省かない
fn freshen(ctx: &Ctx, ix: &mut Index) -> Result<Fresh> {
    if ctx.watched && ix.meta("rebuilding")?.is_none() && ix.meta("last_scan")?.is_some() {
        return Ok(Fresh::Ok);
    }
    ix.refresh_if_stale()
}

pub(crate) fn rebuilding(p: u32) -> CoreError {
    CoreError::wiki(codes::REBUILDING, format!("OpenNote の索引を作り直し中です（{p}%）。しばらくしてから呼び直してください"))
}

/// 引数のパス（vault 相対かワークスペースの中の絶対パス）→ (vault 相対, 実体の絶対パス)。
/// 拒否: `..`・`.` 始まりの区画・ワークスペースの外（symlink の先も canonicalize して判定）・実在しない・ファイルでない
pub fn resolve_path(root: &Path, p: &str) -> Result<(String, PathBuf)> {
    resolve_entry(root, p, false)
}

/// resolve_path と同じ検査で、通常ファイルに加えてディレクトリも返す（GUI の wiki_open_path が iWork のパッケージ形式
/// `.key` / `.pages` / `.numbers` を Finder で表示するため。読む系の command は resolve_path を使う）
pub fn resolve_entry(root: &Path, p: &str, allow_dir: bool) -> Result<(String, PathBuf)> {
    use super::paths::Reject;
    let p: String = p.trim().nfc().collect();
    let opts = super::paths::Opts { allow_dir, judge_by_canonical: false };
    super::paths::resolve_existing(root, Path::new(&p), opts).map_err(|r| match r {
        Reject::Outside => bad(format!("ワークスペースの外のパスは読めません: {p}")),
        Reject::DotDot | Reject::DotStart(_) => bad(format!("パスに `..`・`.` 始まりの区画・root は使えません: {p}")),
        Reject::Missing(_) => CoreError::NotFound(format!("ワークスペースのファイル {p}")),
        Reject::Escapes => bad(format!("ワークスペースの外を指しています（symlink）: {p}")),
        Reject::Root | Reject::Hidden(_) => bad(format!("このパスは読めません: {p}")),
        Reject::NotFile(_) => bad(format!("ファイルではありません: {p}")),
    })
}

/// 索引でのこのファイルの表記。完全一致はそのまま、大小だけ違う候補は canonicalize が abs と同じとき（同じ実体）だけ採る
/// （大小を区別するボリュームでは `A.md` と `a.md` が別ファイルでありうる = 別ファイルのメタを混ぜない）
pub(crate) fn index_path_for(ix: &Index, rel: &str, abs: &Path) -> Result<Option<String>> {
    Ok(match ix.lookup_path(rel)? {
        Some(p) if p == rel => Some(p),
        Some(p) if std::fs::canonicalize(ix.root().join(&p)).ok().as_deref() == Some(abs) => Some(p),
        _ => None,
    })
}

// ---------------- command ----------------

/// 状態: 有効か・root・件数（ページ・種類別・リンク・未解決・ambiguous・inbox・frontmatter）・最終走査。無効でもエラーにしない
pub fn status(ctx: &Ctx, _a: &Value) -> Result<Value> {
    let mut out = json!({"enabled": ctx.enabled, "root": ctx.root});
    if !ctx.enabled {
        return Ok(out);
    }
    let (ip, root) = match ctx.ready() {
        Ok(v) => v,
        Err(e) => {
            out["error"] = json!(e.to_string());
            return Ok(out);
        }
    };
    out["root"] = json!(root.to_string_lossy());
    out["index_path"] = json!(ip.to_string_lossy());
    // このプロセスで壊れた索引を退避して作り直した（索引が壊れていたので作り直しました）
    if let Some((at, moved)) = index::recovered() {
        out["recovered"] = json!({"at": at, "moved_to": moved, "message": "索引が壊れていたので作り直しました"});
    }
    // 自プロセスが作り直し中（索引を握っている）なら待たずに進捗だけ返す
    if index::busy() {
        if let Some(p) = index::progress() {
            out["rebuilding"] = json!({"percent": p});
            return Ok(out);
        }
    }
    with_index(&ip, &root, |ix| {
        match freshen(ctx, ix)? {
            Fresh::Rebuilding(p) => out["rebuilding"] = json!({"percent": p}),
            Fresh::Ok => out["rebuilding"] = Value::Null,
        }
        if let Value::Object(m) = ix.stats()? {
            for (k, v) in m {
                out[k] = v;
            }
        }
        Ok(())
    })?;
    Ok(out)
}

/// ページの一覧（パス順）
pub fn list(ctx: &Ctx, a: &Value) -> Result<Value> {
    let limit = opt_int(a, "limit")?.unwrap_or(1000);
    if limit < 1 {
        return Err(bad("limit は 1 以上"));
    }
    let kind = opt_str(a, "kind")?;
    if let Some(k) = kind.as_deref() {
        if !["md", "html", "media", "other"].contains(&k) {
            return Err(bad(format!("kind は md / html / media / other のどれか: {k}")));
        }
    }
    let q = ListQuery {
        prefix: opt_str(a, "prefix")?.map(|s| s.nfc().collect::<String>().trim_start_matches('/').to_string()),
        kind,
        tags: tag_args(a)?,
        limit: limit as usize,
    };
    fresh(ctx, |ix| {
        let (total, rows) = ix.list(&q)?;
        Ok(json!({
            "total": total,
            "count": rows.len(),
            "pages": rows.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
        }))
    })
}

/// 1 ページを読む。md / html = 本文 + 解析済みのメタ + sha256・それ以外 = パス・サイズ・種類だけ
pub fn read(ctx: &Ctx, a: &Value) -> Result<Value> {
    let path = opt_str(a, "path")?.ok_or_else(|| bad("wiki_read: 引数 path は必須です"))?;
    let max_chars = opt_int(a, "max_chars")?.unwrap_or(200_000);
    if max_chars < 0 {
        return Err(bad("max_chars は 0 以上"));
    }
    let (ip, root) = ctx.ready()?;
    let (rel, abs) = resolve_path(&root, &path)?;
    // 索引のロックの中では表記・行・リンクだけ取る（本文の read・解析・sha は外 = その間に監視・検索・バックリンクを待たせない）。
    // ファイルの書込はもとから索引のロックの外なので、ロックは行と本文の一致を守っていない（中で読んでも同じ窓がある）
    let (rel, st, title, links) = with_index(&ip, &root, |ix| {
        if let Fresh::Rebuilding(p) = freshen(ctx, ix)? {
            return Err(rebuilding(p));
        }
        // 大小だけ違う指定（APFS は大小無視）は索引の表記に揃える（同じ実体のときだけ）
        let rel = index_path_for(ix, &rel, &abs)?.unwrap_or_else(|| rel.clone());
        // 索引の行が今のファイルと違えば（5 秒以内の書換え）このファイルだけ取り込み直す
        let st = scan::stat(&abs);
        let row = ix.page(&rel)?;
        if row.as_ref().map(|r| (r.mtime_ns, r.size)) != st {
            ix.refresh_paths(&[rel.as_str()])?;
        }
        let kind = Kind::of(&rel);
        let size = st.map_or(0, |s| s.1);
        if !kind.has_body() || size > index::BODY_MAX {
            return Ok((rel, st, None, Vec::new()));
        }
        let title = ix.page(&rel)?.map(|r| r.title);
        let links = ix.links_from(&rel)?;
        Ok((rel, st, title, links))
    })?;
    let kind = Kind::of(&rel);
    let (mtime_ns, size) = st.unwrap_or((0, 0));
    let mut out = json!({
        "path": rel,
        "abs_path": abs.to_string_lossy(),
        "kind": kind.as_str(),
        "size": size,
        "mtime": mtime_ns / 1_000_000_000,
    });
    if !kind.has_body() {
        let ext = rel.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        out["media"] = json!(scan::media_kind(ext));
        out["title"] = json!(default_title(&rel, kind));
        return Ok(out);
    }
    if size > index::BODY_MAX {
        return Err(bad(format!("{rel} は大きすぎて読めません（{size} バイト・上限 {} バイト）", index::BODY_MAX)));
    }
    // ロックを離した後に消えた・読めなくなった = 従来と同じ read のエラー。読むのは上限 + 1 バイトまで（stat の後に増えた
    // ファイルを丸ごと読まない）= 超えていたら stat で超えていたときと同じエラー
    #[cfg(test)]
    BEFORE_BODY.with(|h| h.borrow().as_ref().map(|f| f(&abs)));
    let bytes = read_file_capped(&abs, index::BODY_MAX as u64)?;
    if bytes.len() as i64 > index::BODY_MAX {
        let size = std::fs::metadata(&abs).map_or(bytes.len() as u64, |m| m.len());
        return Err(bad(format!("{rel} は大きすぎて読めません（{size} バイト・上限 {} バイト）", index::BODY_MAX)));
    }
    let (parsed, body) = body_fields(kind, &bytes, max_chars as usize);
    out["title"] = json!(title.unwrap_or_else(|| default_title(&rel, kind)));
    out["tags"] = json!(parsed.tags);
    out["items"] = json!(parsed.items);
    out["frontmatter"] = parsed.frontmatter.as_ref().map(|f| f.to_json()).unwrap_or(Value::Null);
    out["links"] = Value::Array(links.iter().map(|l| l.to_json()).collect());
    out["sha256"] = json!(sha256_hex(&bytes));
    put_fields(&mut out, body);
    Ok(out)
}

/// md / html の本文の欄（wiki_read と GUI の参照フォルダの html が使う）: chars = 全体の字数・truncated = max_chars を超えるか・
/// content = 先頭 max_chars 字（UTF-8 として不正なバイトは置換文字）。parse_bytes の結果（題・tags・items・frontmatter）も返す
pub fn body_fields(kind: Kind, bytes: &[u8], max_chars: usize) -> (super::parse_md::Parsed, serde_json::Map<String, Value>) {
    let parsed = index::parse_bytes(kind, bytes);
    let text = String::from_utf8_lossy(bytes);
    let chars = text.chars().count();
    let mut m = serde_json::Map::new();
    m.insert("chars".into(), json!(chars));
    m.insert("truncated".into(), json!(chars > max_chars));
    m.insert("content".into(), json!(text.chars().take(max_chars).collect::<String>()));
    (parsed, m)
}

/// fields を out（object）に順に足す（同じ鍵は上書き = `out[k] = v` と同じ）
pub fn put_fields(out: &mut Value, fields: serde_json::Map<String, Value>) {
    for (k, v) in fields {
        out[k] = v;
    }
}

/// 画像を agent に渡す上限 = MCP の image の data（base64）の長さ（Anthropic API の画像 5 MB は base64 の長さで測られる前提）
pub const IMAGE_B64_MAX: usize = 5 * 1024 * 1024;
/// 同じ上限の生データのバイト数（base64 は 3 バイト → 4 字 = 3,932,160 バイト）
pub const IMAGE_MAX: u64 = (IMAGE_B64_MAX / 4 * 3) as u64;
/// wiki_read で本文を抽出する PDF の上限（抽出はファイル全体をメモリに読む。超えたら path と text_error だけ）
pub const PDF_MAX: i64 = 200 * 1024 * 1024;
/// 応答の予約キー: `[{mimeType, data（base64）}]`。openref-mcp が抜いて MCP の image content にする
/// （text に base64 を二重に流さない）・CLI は「<画像 N バイト・path>」の 1 行に落とす
pub const MCP_IMAGES: &str = "_mcp_images";

/// 登録表（MCP / CLI）の `wiki_read`: `read` に加えて、画像（中身が png / jpeg / gif / webp・IMAGE_MAX 以下）は
/// `_mcp_images` に載せ、PDF は抽出テキスト（頁区切り \f・索引には入れない）を content に入れる。
/// それ以外の画像（svg / avif / heic 等・大きすぎるもの）・動画・音声は path とサイズだけ（`image` に理由）。
/// GUI は画像・PDF を asset protocol で直接描くので `read` を使う（base64 も抽出も要らない）
pub fn read_agent(ctx: &Ctx, a: &Value) -> Result<Value> {
    // PDF の頁の指定は読む前に検査する（負・逆順は丸めない）
    let from_arg = opt_int(a, "from_page")?;
    let to_arg = opt_int(a, "to_page")?;
    for (k, v) in [("from_page", from_arg), ("to_page", to_arg)] {
        if v.is_some_and(|v| v < 1) {
            return Err(bad(format!("{k} は 1 以上")));
        }
    }
    if let (Some(f), Some(t)) = (from_arg, to_arg) {
        if t < f {
            return Err(bad(format!("to_page（{t}）が from_page（{f}）より前です")));
        }
    }
    let mut out = read(ctx, a)?;
    let abs = PathBuf::from(out["abs_path"].as_str().unwrap_or_default());
    let size = out["size"].as_i64().unwrap_or(0);
    match out["media"].as_str() {
        Some("image") => {
            let too_big = |n: u64| json!(format!("大きすぎるので画像としては渡しません（{n} バイト・上限 {IMAGE_MAX} バイト = base64 で 5 MB）。abs_path を参照"));
            if size as u64 > IMAGE_MAX {
                out["image"] = too_big(size as u64);
                return Ok(out);
            }
            // 上限 + 1 バイトまでしか読まない（stat の後に大きくなったファイルを丸ごと読まない）
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(std::fs::File::open(&abs)?, IMAGE_MAX + 1), &mut bytes)?;
            if bytes.len() as u64 > IMAGE_MAX {
                out["image"] = too_big(bytes.len() as u64);
                return Ok(out);
            }
            let mime = match super::edit::sniff_ext(&bytes) {
                Some("png") => "image/png",
                Some("jpg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                _ => {
                    out["image"] = json!("画像として渡せるのは中身が png / jpeg / gif / webp のものだけです（svg / avif / heic 等は abs_path を参照）");
                    return Ok(out);
                }
            };
            use base64::Engine;
            out[MCP_IMAGES] = json!([{"mimeType": mime, "data": base64::engine::general_purpose::STANDARD.encode(&bytes)}]);
        }
        Some("pdf") => {
            if size > PDF_MAX {
                out["text_error"] = json!(format!("大きすぎるので本文を抽出しません（{size} バイト・上限 {PDF_MAX} バイト）"));
                return Ok(out);
            }
            // 総頁数は文書を読むだけで取り（本文は抽出しない）、指定の範囲だけ抽出する（返す範囲は実際に切り出した頁）。
            // 内蔵の解析が読めない文書（pdftotext だけが読める）は総頁数が分からない = pages は null
            let bytes = std::fs::read(&abs)?;
            let total = crate::pdftools::page_count(&bytes);
            let max_chars = opt_int(a, "max_chars")?.unwrap_or(200_000).max(0) as usize;
            let from = from_arg.unwrap_or(1) as usize;
            let to = to_arg.map(|t| t as usize).unwrap_or(usize::MAX).min(total.unwrap_or(usize::MAX));
            out["pages"] = json!(total);
            if total.is_some_and(|n| from > n) {
                pdf_out_of_range(&mut out);
                return Ok(out);
            }
            let ex = match crate::pdftools::extract_pages_bytes(&bytes, Some(&abs), from, to) {
                Ok(ex) => ex,
                Err(e) => {
                    // 要約 = 原因の行（Syntax Error を含む行）を優先・無ければ最初の行
                    let msg = e.to_string();
                    let line = msg.lines().find(|l| l.contains("Syntax Error")).or_else(|| msg.lines().next()).unwrap_or_default();
                    out["text_error"] = json!(line.chars().take(300).collect::<String>());
                    return Ok(out);
                }
            };
            if ex.pages.is_empty() {
                pdf_out_of_range(&mut out);
                return Ok(out);
            }
            let text = ex.pages.join("\u{c}");
            let chars = text.chars().count();
            out["backend"] = json!(ex.backend);
            out["from_page"] = json!(from);
            out["to_page"] = json!(from + ex.pages.len() - 1);
            out["chars"] = json!(chars);
            out["truncated"] = json!(chars > max_chars);
            out["content"] = json!(text.chars().take(max_chars).collect::<String>());
        }
        _ => {}
    }
    Ok(out)
}

/// PDF の頁の指定が範囲外 = 空（from_page / to_page は null・総頁数は pages）
fn pdf_out_of_range(out: &mut Value) {
    out["from_page"] = Value::Null;
    out["to_page"] = Value::Null;
    out["out_of_range"] = json!(true);
    out["chars"] = json!(0);
    out["truncated"] = json!(false);
    out["content"] = json!("");
}

/// そのページ（path）か item（key）を参照するページ。一致した行の抜粋付き
pub fn backlinks(ctx: &Ctx, a: &Value) -> Result<Value> {
    let path = opt_str(a, "path")?;
    let key = opt_str(a, "key")?;
    let limit = opt_int(a, "limit")?.unwrap_or(200).max(1) as usize;
    if path.is_some() == key.is_some() {
        return Err(bad("wiki_backlinks: path か key のどちらか 1 つを渡してください"));
    }
    let (_, root) = ctx.ready()?;
    // path は read と同じ解決（ワークスペースの中・`..` なし・symlink の先・実在）を通し、索引の表記に揃える。解けなければエラー
    let given = match &path {
        Some(p) => Some(resolve_path(&root, p)?),
        None => None,
    };
    let (root, target, total, hits) = fresh(ctx, |ix| {
        let target = match given.clone() {
            Some((rel, abs)) => {
                if index_path_for(ix, &rel, &abs)?.is_none() {
                    // 5 秒以内にできたファイル = 索引にまだ無い → このファイルだけ取り込む
                    ix.refresh_paths(&[rel.as_str()])?;
                }
                Some(index_path_for(ix, &rel, &abs)?.ok_or_else(|| CoreError::NotFound(format!("索引にないページ {rel}")))?)
            }
            None => None,
        };
        // (ページ, 抜粋に探す文字列, リンクの種類)
        let mut hits: Vec<(index::PageRow, Vec<String>, Vec<String>)> = Vec::new();
        match (&target, &key) {
            (Some(t), _) => {
                for l in ix.links_to(t)? {
                    if let Some(h) = hits.iter_mut().find(|h| h.0.path == l.src) {
                        h.1.push(l.target);
                        if !h.2.contains(&l.kind) {
                            h.2.push(l.kind);
                        }
                        continue;
                    }
                    if let Some(p) = ix.page(&l.src)? {
                        hits.push((p, vec![l.target], vec![l.kind]));
                    }
                }
            }
            (None, Some(k)) => {
                for p in ix.pages_with_item(k)? {
                    hits.push((p, vec![k.clone()], vec!["openref".into()]));
                }
            }
            _ => unreachable!(),
        }
        let total = hits.len();
        hits.truncate(limit);
        Ok((ix.root().to_path_buf(), target, total, hits))
    })?;
    // 抜粋は索引のロックの外でファイルを読み直して作る（search と同じ。ロックを離した後に消えた・読めないページは抜粋 null = 従来の読めないときと同じ）
    let pages: Vec<Value> = hits
        .into_iter()
        .map(|(p, needles, kinds)| {
            let excerpt = excerpt(&root.join(&p.path), &needles);
            json!({"path": p.path, "title": p.title, "kind": p.kind, "link_kinds": kinds, "excerpt": excerpt})
        })
        .collect();
    Ok(json!({"target": target.or_else(|| key.clone()), "total": total, "count": pages.len(), "pages": pages}))
}

/// 検索: 題名の部分一致（完全一致 → 前方一致 → 全語を含む）が上・続いて全文（FTS・和文可）。抜粋付き。tag / tags は AND の絞込
pub fn search(ctx: &Ctx, a: &Value) -> Result<Value> {
    let query = opt_str(a, "query")?.unwrap_or_default();
    let tags = tag_args(a)?;
    let limit = opt_int(a, "limit")?.unwrap_or(50);
    if !(1..=1000).contains(&limit) {
        return Err(bad("limit は 1〜1000"));
    }
    let terms = index::query_terms(&query);
    if terms.is_empty() && tags.is_empty() {
        // 文は BadInput と同じ（前置き付き）・code で画面が「0 件」と扱う
        let msg = bad("wiki_search: query（英数字か和文を含む語）か tag / tags を渡してください").to_string();
        return Err(CoreError::wiki(codes::BAD_QUERY, msg));
    }
    let q = SearchQuery { query: query.clone(), tags: tags.clone(), limit: limit as usize };
    let (root, total, hits) = fresh(ctx, |ix| {
        let (total, hits) = ix.search(&q)?;
        Ok((ix.root().to_path_buf(), total, hits))
    })?;
    // 抜粋は索引のロックの外でファイルを読み直して作る（索引は本文の写しを持たない）
    let pages: Vec<Value> = hits
        .iter()
        .map(|h| {
            let mut o = h.row.to_json();
            o["match"] = json!(match (h.title_match.is_some(), h.body_match) {
                (true, true) => "title+body",
                (true, false) => "title",
                _ => "body",
            });
            o["snippet"] = match (terms.is_empty(), Kind::of(&h.row.path).has_body()) {
                (false, true) => json!(snippet_of(&root.join(&h.row.path), &terms)),
                _ => Value::Null,
            };
            if let Some(o) = o.as_object_mut() {
                o.remove("items");
            }
            o
        })
        .collect();
    Ok(json!({"query": query, "tags": tags, "total": total, "count": pages.len(), "pages": pages}))
}

/// tag の一覧と件数（ページ数の多い順）
pub fn tags(ctx: &Ctx, a: &Value) -> Result<Value> {
    let limit = opt_int(a, "limit")?.unwrap_or(500);
    if limit < 1 {
        return Err(bad("limit は 1 以上"));
    }
    fresh(ctx, |ix| {
        let (total, rows) = ix.tags(limit as usize)?;
        Ok(json!({
            "total": total,
            "count": rows.len(),
            "tags": rows.iter().map(|(t, n)| json!({"tag": t, "count": n})).collect::<Vec<_>>(),
        }))
    })
}

/// inbox（`<root>/ingest/` の中のファイル。下位フォルダも・`.` 始まりと symlink は除く = 索引と同じ）。
/// ingest/ が無ければ空。OpenRef は素材を動かさない（移すのは skill の wiki_move）。
/// `dir` = "ingest"（既定・未処理の素材・パス順）か "chat-log"（チャットの記録・新しい順 = 名前の日時の降順）
pub fn inbox(ctx: &Ctx, a: &Value) -> Result<Value> {
    let limit = opt_int(a, "limit")?.unwrap_or(1000);
    if limit < 1 {
        return Err(bad("limit は 1 以上"));
    }
    let dir = match opt_str(a, "dir")?.as_deref().unwrap_or("ingest") {
        "ingest" => "ingest",
        "chat-log" => "chat-log",
        other => return Err(bad(format!("dir は ingest か chat-log: {other}"))),
    };
    let newest_first = dir == "chat-log";
    fresh(ctx, |ix| {
        // chat-log は名前が日時なので、全件をパス順に取って逆にし limit で切る（新しいログが落ちない）
        let want = if newest_first { usize::MAX } else { limit as usize };
        let (total, mut rows) = ix.list(&ListQuery { prefix: Some(format!("{dir}/")), limit: want, ..Default::default() })?;
        if newest_first {
            rows.reverse();
            rows.truncate(limit as usize);
        }
        let files: Vec<Value> = rows
            .iter()
            .map(|r| {
                let name = r.path.rsplit('/').next().unwrap_or(&r.path);
                let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
                json!({
                    "path": r.path,
                    "name": name,
                    "kind": r.kind,
                    "media": scan::media_kind(&ext),
                    "size": r.size,
                    "mtime": r.mtime_ns / 1_000_000_000,
                })
            })
            .collect();
        Ok(json!({"dir": format!("{dir}/"), "total": total, "count": files.len(), "files": files}))
    })
}

/// 抜粋の前後の字数（前は一致の行の頭がこの範囲にあれば行の頭から）
const SNIP_BEFORE: usize = 40;
const SNIP_AFTER: usize = 160;

/// 結果のページの抜粋: 全文検索に入れた本文（index::parse_bytes の text）で最初に語が現れる所の前後を切り出し、
/// 語の出現を U+E000 … U+E001 で囲む（蔵書の fulltext_snippet と同じ印）。大小は無視。語がそのまま見つからなければ
/// 語を英数字の区切りで分けた部分（"PI RADS" に対する "PI-RADS" 等）で探し、それも無ければ本文の先頭
pub fn snippet_of(abs: &Path, terms: &[String]) -> Option<String> {
    let meta = std::fs::metadata(abs).ok()?;
    if meta.len() as i64 > index::BODY_MAX {
        return None;
    }
    let bytes = read_file(abs).ok()?;
    let text = match Kind::of(&abs.to_string_lossy()) {
        Kind::Md => md_plain(&super::parse_md::search_text(&String::from_utf8_lossy(&bytes))),
        k => index::parse_bytes(k, &bytes).text,
    };
    Some(snippet(&text, terms))
}

/// snippet_of の本体（ファイルに触らない）
pub fn snippet(text: &str, terms: &[String]) -> String {
    let low = |c: char| c.to_lowercase().next().unwrap_or(c);
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|&c| low(c)).collect();
    let mut needles: Vec<Vec<char>> = terms.iter().map(|t| t.chars().map(low).collect()).filter(|n: &Vec<char>| !n.is_empty()).collect();
    let find = |n: &[char], from: usize| -> Option<usize> {
        if n.is_empty() || lower.len() < n.len() {
            return None;
        }
        (from..=lower.len() - n.len()).find(|&i| lower[i..i + n.len()] == *n)
    };
    let first_of = |ns: &[Vec<char>]| ns.iter().filter_map(|n| find(n, 0)).min();
    let mut first = first_of(&needles);
    if first.is_none() {
        needles = terms
            .iter()
            .flat_map(|t| t.split(|c: char| !c.is_alphanumeric()).map(|p| p.chars().map(low).collect::<Vec<char>>()).collect::<Vec<_>>())
            .filter(|n| !n.is_empty())
            .collect();
        first = first_of(&needles);
    }
    let at = first.unwrap_or(0);
    let mut start = at.saturating_sub(SNIP_BEFORE);
    // 一致の行の頭が窓の中にあれば行の頭から（前の行の残りを見せない）
    if let Some(nl) = chars[start..at].iter().rposition(|&c| c == '\n') {
        start += nl + 1;
    }
    let end = (at + SNIP_AFTER).min(chars.len());
    // 窓の中の出現に印（重なりは先の物を採る）
    let mut marks: Vec<(usize, usize)> = Vec::new();
    if first.is_some() {
        let mut i = start;
        while i < end {
            match needles.iter().filter(|n| i + n.len() <= lower.len() && lower[i..i + n.len()] == **n).map(|n| n.len()).max() {
                Some(len) => {
                    marks.push((i, (i + len).min(end)));
                    i += len;
                }
                None => i += 1,
            }
        }
    }
    let mut out = String::new();
    if start > 0 && chars[start - 1] != '\n' {
        out.push('…');
    }
    let mut mi = 0;
    let mut space = false;
    for (i, &c) in chars.iter().enumerate().take(end).skip(start) {
        if mi < marks.len() && marks[mi].0 == i {
            if space && !out.is_empty() && !out.ends_with('…') {
                out.push(' ');
            }
            space = false;
            out.push(crate::fulltext::HL_START);
        }
        if c.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() && !out.ends_with('…') && !out.ends_with(crate::fulltext::HL_START) {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
        if mi < marks.len() && marks[mi].1 == i + 1 {
            out.push(crate::fulltext::HL_END);
            mi += 1;
        }
    }
    if end < chars.len() {
        out.push('…');
    }
    // md の記法のうち読みの邪魔になる物だけ落とす（wikilink の括弧・強調・HTML のタグとコメント）
    strip_tags(&out.replace("[[", "").replace("]]", "").replace("**", ""))
}

/// md の本文を抜粋向けの平文にする（印を付ける前に通す = 一致箇所の印の位置はずれない）:
/// 行頭の見出し記号・引用の `>`（入れ子も）・callout の `[!type]`・リスト記号・タスクの `[ ]` を落とし、表の区切り行と水平線は捨てる。
/// `[text](url)` / `![alt](url)` は text / alt、`[[a|b]]` は b（無ければ a）、`![[x|300]]` は x、表の `|` は空白、
/// 強調 `**` `__` `~~` とインラインコードの `` ` `` は落とし、コードフェンスの行（``` / ~~~）は行ごと捨てる（中身は残す）
pub fn md_plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split('\n') {
        let mut l = line.trim_start();
        if l.starts_with("```") || l.starts_with("~~~") {
            out.push('\n');
            continue;
        }
        while let Some(r) = l.strip_prefix('>') {
            l = r.trim_start();
        }
        if l.starts_with("[!") {
            if let Some(e) = l.find(']') {
                l = l[e + 1..].trim_start_matches(['+', '-']).trim_start();
            }
        }
        let hashes = l.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && (l.len() == hashes || l.as_bytes()[hashes] == b' ') {
            l = l[hashes..].trim_start();
        }
        if let Some(r) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).or_else(|| l.strip_prefix("+ ")) {
            l = r.trim_start();
        } else {
            let d = l.bytes().take_while(u8::is_ascii_digit).count();
            if (1..=9).contains(&d) && (l[d..].starts_with(". ") || l[d..].starts_with(") ")) {
                l = l[d + 2..].trim_start();
            }
        }
        for t in ["[ ] ", "[x] ", "[X] "] {
            if let Some(r) = l.strip_prefix(t) {
                l = r;
            }
        }
        // 表の区切り行（|---|:--:|）と水平線（--- *** ___）は捨てる
        let t = l.trim();
        if t.len() >= 3 && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && t.contains('-')
            || t.len() >= 3 && (t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
        {
            out.push('\n');
            continue;
        }
        md_inline(l, &mut out);
        out.push('\n');
    }
    out
}

fn md_inline(l: &str, out: &mut String) {
    let c: Vec<char> = l.chars().collect();
    let at = |i: usize, pat: &str| pat.chars().enumerate().all(|(k, p)| c.get(i + k) == Some(&p));
    let find = |from: usize, pat: &str| (from..c.len()).find(|&j| at(j, pat));
    let mut i = 0;
    while i < c.len() {
        // ![[x|300]] → x ・ [[a|b]] → b（無ければ a）
        let emb = c[i] == '!' && at(i + 1, "[[");
        if emb || at(i, "[[") {
            let s = if emb { i + 3 } else { i + 2 };
            if let Some(e) = find(s, "]]") {
                let inner: String = c[s..e].iter().collect::<String>().replace("\\|", "|");
                let (a, b) = match inner.split_once('|') {
                    Some((a, b)) => (a, Some(b)),
                    None => (inner.as_str(), None),
                };
                let shown = if emb { a } else { b.filter(|b| !b.trim().is_empty()).unwrap_or(a) };
                out.push_str(shown.trim());
                i = e + 2;
                continue;
            }
        }
        // [text](url) / ![alt](url) → text / alt
        let img = c[i] == '!' && c.get(i + 1) == Some(&'[');
        if c[i] == '[' || img {
            let s = if img { i + 2 } else { i + 1 };
            if let Some(e) = find(s, "]") {
                if c.get(e + 1) == Some(&'(') {
                    if let Some(pe) = find(e + 2, ")") {
                        out.extend(&c[s..e]);
                        i = pe + 1;
                        continue;
                    }
                }
            }
        }
        // 強調・取消線の記号とインラインコードの backtick
        if at(i, "**") || at(i, "__") || at(i, "~~") {
            i += 2;
            continue;
        }
        if c[i] == '`' {
            i += 1;
            continue;
        }
        out.push(if c[i] == '|' { ' ' } else { c[i] });
        i += 1;
    }
}

/// `<tag …>` `</tag>` `<!-- … -->` を落とす（中に一致の印を含むもの・閉じない `<` はそのまま）
fn strip_tags(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '<' && c.get(i + 1).is_some_and(|&n| n.is_ascii_alphabetic() || n == '/' || n == '!') {
            let end = c[i + 1..].iter().take(300).position(|&x| x == '>').map(|p| i + 1 + p);
            if let Some(e) = end {
                if !c[i + 1..e].iter().any(|&x| x == crate::fulltext::HL_START || x == crate::fulltext::HL_END || x == '<') {
                    i = e + 1;
                    continue;
                }
            }
        }
        // 落としたタグの前後の空白を 1 つに
        if !(c[i] == ' ' && out.ends_with(' ')) {
            out.push(c[i]);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
thread_local! {
    /// テスト用: read_file の呼び出し回数（このスレッド）= (with_index の中, 全部)
    static READS: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

/// テスト用のフック（読むファイルのパスを受け取る）
#[cfg(test)]
type PathHook = Box<dyn Fn(&Path)>;

#[cfg(test)]
thread_local! {
    /// テスト用: read が本文を読む直前（索引のロックを離した後）に呼ぶ（stat の後にファイルが増えた窓を作る）
    static BEFORE_BODY: std::cell::RefCell<Option<PathHook>> = const { std::cell::RefCell::new(None) };
}

/// 本文・抜粋のためにファイルを丸ごと読む（read・backlinks・search）。索引のロックの外で呼ぶ（テストが中の呼び出しを数える）
fn read_file(abs: &Path) -> std::io::Result<Vec<u8>> {
    count_read();
    std::fs::read(abs)
}

/// read_file と同じ（数え方も）で、先頭 max + 1 バイトまで（長さ > max = 上限を超えている）
fn read_file_capped(abs: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    count_read();
    let mut out = Vec::new();
    std::fs::File::open(abs)?.take(max.saturating_add(1)).read_to_end(&mut out)?;
    Ok(out)
}

fn count_read() {
    #[cfg(test)]
    READS.with(|c| {
        let (inside, all) = c.get();
        c.set((inside + index::in_index() as u32, all + 1));
    });
}

/// ファイルの中で needles（リンク先の書かれたまま = 索引の links.target か item の鍵）のどれかへのリンクが書かれた最初の行
/// （前後の空白を除き 200 字まで）。リンクの形で当てる（`link_line`）= `[[a]]` を探して "a" を含むだけの行を返さない。
/// 形で見つからなければ needle を含む最初の行（frontmatter の items: 等 = 従来と同じ）
fn excerpt(abs: &Path, needles: &[String]) -> Option<String> {
    let text = read_file(abs).ok()?;
    let text = String::from_utf8_lossy(&text);
    let line = link_line(&text, Kind::of(&abs.to_string_lossy()), needles)
        .or_else(|| text.lines().find(|l| needles.iter().any(|n| l.contains(n.as_str()))))?;
    Some(line.trim().chars().take(200).collect())
}

/// needles へのリンクが書かれた最初の行。md = 索引と同じ抽出器（parse_md::link_spans = コードブロック・コメントの中は出さない）が
/// target = needle のリンク（item の鍵なら key = needle の openref://）を出した所の行・html = 属性の値（`="needle"` / `='needle'`・
/// openref:// の `key=needle`）として書かれた行。それ以外の種類・見つからない = None
fn link_line<'a>(text: &'a str, kind: Kind, needles: &[String]) -> Option<&'a str> {
    use super::links::{openref_key, LinkKind};
    match kind {
        Kind::Md => {
            let hit = |s: &super::parse_md::Span| {
                needles.iter().any(|n| {
                    s.target == *n || s.kind == LinkKind::Openref && openref_key(&s.target).as_deref() == Some(n.as_str())
                })
            };
            let at = super::parse_md::link_spans(text).into_iter().filter(|s| hit(s)).map(|s| s.range.start).min()?;
            let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
            let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
            Some(&text[start..end])
        }
        Kind::Html => text.lines().find(|l| {
            needles.iter().any(|n| [format!("=\"{n}\""), format!("='{n}'"), format!("key={n}")].iter().any(|f| l.contains(f.as_str())))
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 偽ホームの下に vault と app_dir を作る（drop で消す）
    struct Env {
        base: PathBuf,
        ctx: Ctx,
        root: PathBuf,
        /// 索引（プロセス共有）を使うテストの直列化（index::test_serial・drop まで持つ）
        _serial: std::sync::MutexGuard<'static, ()>,
    }
    impl Drop for Env {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }
    fn env(name: &str) -> Env {
        let _serial = crate::wiki::index::test_serial();
        let base = std::env::temp_dir().join(format!("openref-wikicmd-{}-{}-{name}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(&base).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let home = base.join("home");
        let root = home.join("Documents/vault");
        let app = home.join("scratch-app");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        let ctx = Ctx { enabled: true, root: Some(root.to_string_lossy().into()), app_dir: Some(app), home: Some(home), watched: false };
        Env { base, ctx, root, _serial }
    }
    fn put(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// PDF の頁の指定が範囲外の応答 = 6 キー（from_page / to_page = null・out_of_range・chars 0・truncated false・content 空）。
    /// 他の鍵（path・pages 等）は残す
    #[test]
    fn pdf_out_of_range_six_keys() {
        let mut v = json!({"path": "p.pdf", "pages": 1, "chars": 5, "content": "x"});
        pdf_out_of_range(&mut v);
        let want = json!({"path": "p.pdf", "pages": 1, "from_page": null, "to_page": null, "out_of_range": true, "chars": 0, "truncated": false, "content": ""});
        assert_eq!(v, want);
        let e = env("oor");
        std::fs::write(e.root.join("p.pdf"), crate::pdftools::minimal_pdf("Hello", None)).unwrap();
        let r = read_agent(&e.ctx, &json!({"path": "p.pdf", "from_page": 3})).unwrap();
        for (k, want) in [("from_page", json!(null)), ("to_page", json!(null)), ("out_of_range", json!(true)), ("chars", json!(0)), ("truncated", json!(false)), ("content", json!("")), ("pages", json!(1))] {
            assert_eq!(r.get(k), Some(&want), "{k}: {r}");
        }
    }

    /// body_fields（chars・truncated・content）と parse_bytes は旧の read の組立てと同じ（md / html・壊れた UTF-8・max_chars の境界）
    #[test]
    fn body_fields_match_old() {
        let docs: [(Kind, &[u8]); 5] = [
            (Kind::Md, b"---\ntags: [a]\nitems: [K1]\n---\n# \xe9\xa1\x8c\n[[b]] #t\n"),
            (Kind::Md, b""),
            (Kind::Md, b"\xff\xfeabc\xe6\x97"),
            (Kind::Html, b"<title>T</title><meta name=\"wiki:tags\" content=\"x\"><p>\xe6\x9c\xac</p>"),
            (Kind::Html, b"<p>x</p>"),
        ];
        for (kind, b) in docs {
            let n = String::from_utf8_lossy(b).chars().count();
            for max in [0, 1, n.saturating_sub(1), n, n + 1, 200_000] {
                let (parsed, body) = body_fields(kind, b, max);
                let mut new = json!({"path": "p"});
                put_fields(&mut new, body);
                let text = String::from_utf8_lossy(b);
                let chars = text.chars().count();
                let mut old = json!({"path": "p"});
                old["chars"] = json!(chars);
                old["truncated"] = json!(chars > max);
                old["content"] = json!(text.chars().take(max).collect::<String>());
                assert_eq!(serde_json::to_string(&new).unwrap(), serde_json::to_string(&old).unwrap(), "{kind:?} {max}");
                let p = index::parse_bytes(kind, b);
                assert_eq!((parsed.title, parsed.tags, parsed.items), (p.title, p.tags, p.items));
            }
        }
    }

    /// 登録表の wiki_read: 画像（中身で判定・5 MB まで）は _mcp_images・svg / 偽装 / 大きすぎは path だけ・PDF は抽出テキスト・md は read と同じ
    #[test]
    fn read_agent_images_and_pdf() {
        let e = env("agent");
        let png = [&[0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..], &[0u8; 8]].concat();
        std::fs::create_dir_all(e.root.join("ingest")).unwrap();
        std::fs::write(e.root.join("ingest/a.png"), &png).unwrap();
        let r = read_agent(&e.ctx, &json!({"path": "ingest/a.png"})).unwrap();
        use base64::Engine;
        assert_eq!(r[MCP_IMAGES], json!([{"mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(&png)}]));
        assert!(r.get("image").is_none() && r.get("content").is_none());
        // GUI の read は載せない
        assert!(read(&e.ctx, &json!({"path": "ingest/a.png"})).unwrap().get(MCP_IMAGES).is_none());
        // 拡張子が jpg でも中身が png なら png として
        std::fs::write(e.root.join("ingest/b.jpg"), &png).unwrap();
        assert_eq!(read_agent(&e.ctx, &json!({"path": "ingest/b.jpg"})).unwrap()[MCP_IMAGES][0]["mimeType"], "image/png");
        put(&e.root, "ingest/c.svg", "<svg xmlns='http://www.w3.org/2000/svg'/>");
        put(&e.root, "ingest/d.png", "not a png");
        for p in ["ingest/c.svg", "ingest/d.png"] {
            let r = read_agent(&e.ctx, &json!({"path": p})).unwrap();
            assert!(r.get(MCP_IMAGES).is_none() && r["image"].as_str().unwrap().contains("png / jpeg"), "{p}: {r}");
            assert!(r["abs_path"].as_str().unwrap().ends_with(p));
        }
        // 境界: 生データ IMAGE_MAX（base64 でちょうど 5 MB）は画像・+1 バイトは path だけ
        assert_eq!(IMAGE_MAX, 3_932_160);
        let edge = [&png[..], &vec![0u8; IMAGE_MAX as usize - png.len()]].concat();
        std::fs::write(e.root.join("ingest/edge.png"), &edge).unwrap();
        let r = read_agent(&e.ctx, &json!({"path": "ingest/edge.png"})).unwrap();
        assert_eq!(r[MCP_IMAGES][0]["data"].as_str().unwrap().len(), IMAGE_B64_MAX);
        let big = [&edge[..], &[0u8][..]].concat();
        std::fs::write(e.root.join("ingest/big.png"), &big).unwrap();
        let r = read_agent(&e.ctx, &json!({"path": "ingest/big.png"})).unwrap();
        assert!(r.get(MCP_IMAGES).is_none() && r["image"].as_str().unwrap().contains("大きすぎる"), "{r}");
        // PDF = 抽出テキスト
        std::fs::write(e.root.join("ingest/p.pdf"), crate::pdftools::minimal_pdf("Hello vault pdf", None)).unwrap();
        let r = read_agent(&e.ctx, &json!({"path": "ingest/p.pdf"})).unwrap();
        assert!(r["content"].as_str().unwrap().contains("Hello vault pdf"), "{r}");
        assert_eq!((r["from_page"].as_i64(), r["to_page"].as_i64(), r["media"].as_str()), (Some(1), Some(1), Some("pdf")));
        assert_eq!(r["pages"], 1);
        // 範囲外 = 空・from / to は null・総頁数は pages
        let r = read_agent(&e.ctx, &json!({"path": "ingest/p.pdf", "from_page": 3})).unwrap();
        assert_eq!((r["content"].as_str(), r["from_page"].is_null(), r["out_of_range"].as_bool(), r["pages"].as_i64()), (Some(""), true, Some(true), Some(1)));
        // to は総頁数で切る
        let r = read_agent(&e.ctx, &json!({"path": "ingest/p.pdf", "to_page": 9})).unwrap();
        assert_eq!(r["to_page"], 1);
        // 負・0・逆順はエラー（丸めない）
        for a in [json!({"from_page": 0}), json!({"to_page": -1}), json!({"from_page": 2, "to_page": 1})] {
            let mut a = a;
            a["path"] = json!("ingest/p.pdf");
            assert!(read_agent(&e.ctx, &a).is_err(), "{a}");
        }
        // 壊れた PDF = 全体はエラーにせず path・size と text_error
        put(&e.root, "ingest/broken.pdf", "%PDF-1.4 garbage");
        let r = read_agent(&e.ctx, &json!({"path": "ingest/broken.pdf"})).unwrap();
        assert!(r["text_error"].as_str().is_some_and(|t| !t.is_empty()) && r.get("content").is_none() && r["size"] == 16, "{r}");
        // md は read と同じ
        put(&e.root, "a.md", "# A");
        assert_eq!(read_agent(&e.ctx, &json!({"path": "a.md"})).unwrap(), read(&e.ctx, &json!({"path": "a.md"})).unwrap());
    }

    /// ワークスペースの中だけ通す（symlink で外を指すもの・root そのもの・外・相対・無効は拒否）
    #[test]
    fn host_path_in_vault_rejects_outside_and_symlinks() {
        let e = env("hostpath");
        put(&e.root, "ingest/a.pdf", "%PDF");
        let ok = e.ctx.host_path_in_vault(&e.root.join("ingest/../ingest/a.pdf")).unwrap();
        assert_eq!(ok, e.root.join("ingest/a.pdf"));
        let outside = e.base.join("outside.pdf");
        std::fs::write(&outside, "%PDF").unwrap();
        std::os::unix::fs::symlink(&outside, e.root.join("ingest/link.pdf")).unwrap();
        put(&e.root, ".obsidian/x.pdf", "%PDF");
        put(&e.root, "sub/.trash/y.pdf", "%PDF");
        for p in [
            outside.clone(), e.root.join("ingest/link.pdf"), e.root.clone(), e.root.join("nope.pdf"), e.root.join("ingest"),
            PathBuf::from("ingest"), PathBuf::from("../outside.pdf"), PathBuf::from("ingest/../ingest/a.pdf"), PathBuf::from(".obsidian/x.pdf"), e.root.join("sub/.trash/y.pdf"),
        ] {
            assert!(e.ctx.host_path_in_vault(&p).is_err(), "{}", p.display());
        }
        // vault 相対も受ける（root と結合して同じ検査）
        assert_eq!(e.ctx.host_path_in_vault(Path::new("ingest/a.pdf")).unwrap(), ok);
        // ワークスペースの外の symlink がワークスペースの中を指すなら中（実体で判定）
        std::os::unix::fs::symlink(e.root.join("ingest/a.pdf"), e.base.join("in.pdf")).unwrap();
        assert!(e.ctx.host_path_in_vault(&e.base.join("in.pdf")).is_ok());
        assert_eq!(Ctx::default().host_path_in_vault(&ok).unwrap_err().to_string(), DISABLED);
    }

    #[test]
    fn disabled_and_unready() {
        let off = Ctx::default();
        assert_eq!(status(&off, &json!({})).unwrap(), json!({"enabled": false, "root": null}));
        for f in [list, read, backlinks] {
            assert_eq!(f(&off, &json!({"path": "a.md"})).unwrap_err().to_string(), DISABLED);
        }
        let mem = Ctx { enabled: true, root: Some("/x".into()), app_dir: None, home: Some("/Users/x".into()), watched: false };
        assert!(list(&mem, &json!({})).unwrap_err().to_string().contains("in-memory"));
        assert!(status(&mem, &json!({})).unwrap()["error"].as_str().unwrap().contains("in-memory"));
        let unset = Ctx { enabled: true, root: None, app_dir: Some("/tmp/x".into()), home: Some("/Users/x".into()), watched: false };
        assert!(list(&unset, &json!({})).unwrap_err().to_string().contains("未設定"));
    }

    /// エラーの契約: 画面が分岐する code を持ち、文（Display）は msg のまま = MCP / CLI の出力は変わらない。
    /// 文言は照合しない（改名・英訳で変わる物）
    #[test]
    fn error_codes() {
        let code = |e: &CoreError| match e {
            CoreError::Wiki { code, msg } => {
                assert_eq!(&e.to_string(), msg, "Display は msg そのもの（前置きを足さない）");
                Some(*code)
            }
            _ => None,
        };
        let r = rebuilding(42);
        assert_eq!(code(&r), Some(codes::REBUILDING));
        assert!(r.to_string().contains("42"), "進捗は文に残る");
        assert_eq!(code(&list(&Ctx::default(), &json!({})).unwrap_err()), Some(codes::UNAVAILABLE));
        let unset = Ctx { enabled: true, root: None, app_dir: Some("/tmp/x".into()), home: Some("/Users/x".into()), watched: false };
        assert_eq!(code(&list(&unset, &json!({})).unwrap_err()), Some(codes::UNAVAILABLE));
        // 語に英数字も和文も無い（tag も無い）= bad_query。文は従来の BadInput と同じ前置き
        let e = env("codes");
        put(&e.root, "a.md", "# A\n");
        for a in [json!({}), json!({"query": " - \""})] {
            let err = search(&e.ctx, &a).unwrap_err();
            assert_eq!(code(&err), Some(codes::BAD_QUERY), "{a}");
            assert!(err.to_string().starts_with(&CoreError::BadInput(String::new()).to_string()), "{err}");
        }
        // 他の入力の不備は BadInput のまま（code なし = 画面は素の文として出す）
        assert_eq!(code(&search(&e.ctx, &json!({"query": "x", "limit": 0})).unwrap_err()), None);
    }

    #[test]
    fn list_read_backlinks_roundtrip() {
        let e = env("rt");
        put(&e.root, "a.md", "---\ntitle: ページ A\ntags: [x]\n---\n[[b]] と ![[img.png|300]] と [論文](openref://open?key=KEY22222)\n");
        put(&e.root, "sub/b.md", "# B\n\n本文 [[a#見出し|A へ]] #y\n");
        put(&e.root, "img.png", "\u{89}PNG");
        put(&e.root, ".obsidian/app.json", "{}");
        put(&e.root, "ingest/new.pdf", "%PDF");
        let st = status(&e.ctx, &json!({})).unwrap();
        assert_eq!(st["pages"], 4, "{st}");
        assert_eq!(st["kinds"]["md"], 2);
        assert_eq!(st["kinds"]["media"], 2);
        assert_eq!(st["unresolved"], 0);
        assert_eq!(st["inbox"], 1);
        assert_eq!(st["frontmatter"]["pages"], 1);
        let l = list(&e.ctx, &json!({"kind": "md"})).unwrap();
        assert_eq!(l["count"], 2);
        assert_eq!(l["pages"][0]["title"], "ページ A");
        assert_eq!(l["pages"][1]["path"], "sub/b.md");
        assert_eq!(l["pages"][1]["title"], "B");
        assert_eq!(list(&e.ctx, &json!({"tag": "Y"})).unwrap()["pages"][0]["path"], "sub/b.md");
        assert_eq!(list(&e.ctx, &json!({"prefix": "sub/"})).unwrap()["count"], 1);
        assert_eq!(list(&e.ctx, &json!({"limit": 1})).unwrap()["total"], 4);
        let r = read(&e.ctx, &json!({"path": "a.md"})).unwrap();
        assert_eq!(r["title"], "ページ A");
        assert_eq!(r["items"], json!(["KEY22222"]));
        assert_eq!(r["links"][0], json!({"kind": "wiki", "target": "b", "path": "sub/b.md", "ambiguous": false}));
        assert_eq!(r["links"][1]["path"], "img.png");
        assert_eq!(r["links"][2]["kind"], "openref");
        assert_eq!(r["frontmatter"][0], json!({"key": "title", "value": "ページ A"}));
        assert_eq!(r["sha256"].as_str().unwrap().len(), 64);
        assert!(r["content"].as_str().unwrap().starts_with("---"));
        let r = read(&e.ctx, &json!({"path": "a.md", "max_chars": 3})).unwrap();
        assert_eq!((r["content"].as_str().unwrap(), r["truncated"].as_bool().unwrap()), ("---", true));
        let m = read(&e.ctx, &json!({"path": "img.png"})).unwrap();
        assert_eq!((m["kind"].as_str(), m["media"].as_str(), m["size"].as_i64()), (Some("media"), Some("image"), Some(5)));
        assert!(m.get("content").is_none());
        let b = backlinks(&e.ctx, &json!({"path": "a.md"})).unwrap();
        assert_eq!(b["count"], 1);
        assert_eq!(b["pages"][0]["path"], "sub/b.md");
        assert_eq!(b["pages"][0]["excerpt"], "本文 [[a#見出し|A へ]] #y");
        let k = backlinks(&e.ctx, &json!({"key": "KEY22222"})).unwrap();
        assert_eq!(k["pages"][0]["path"], "a.md");
        assert!(backlinks(&e.ctx, &json!({})).is_err());
        assert!(backlinks(&e.ctx, &json!({"path": "a.md", "key": "K"})).is_err());
    }

    #[test]
    fn read_rejects_outside_dotdot_symlink_and_hidden() {
        let e = env("deny");
        put(&e.root, "ok.md", "x");
        put(&e.root, ".obsidian/app.json", "{}");
        let outside = e.base.join("home/secret.md");
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, e.root.join("link.md")).unwrap();
        std::os::unix::fs::symlink(e.base.join("home"), e.root.join("homelink")).unwrap();
        let err = |p: &str| read(&e.ctx, &json!({"path": p})).unwrap_err().to_string();
        assert!(err(&outside.to_string_lossy()).contains("ワークスペースの外"));
        assert!(err("../secret.md").contains(".."));
        assert!(err("sub/../../secret.md").contains(".."));
        assert!(err("link.md").contains("ワークスペースの外"), "ワークスペースの外を指す symlink");
        assert!(err("homelink/secret.md").contains("ワークスペースの外"));
        assert!(err(".obsidian/app.json").contains("`.`"));
        assert!(err("nope.md").contains("not found"));
        assert!(err("").contains("必須"));
        // ワークスペースの中の絶対パスは可
        let abs = e.root.join("ok.md");
        assert_eq!(read(&e.ctx, &json!({"path": abs.to_string_lossy()})).unwrap()["path"], "ok.md");
    }

    #[test]
    fn freshness_and_refresh_paths() {
        let e = env("fresh");
        put(&e.root, "a.md", "[[b]]");
        let (ip, root) = e.ctx.ready().unwrap();
        let mut ix = Index::open(&ip, &root).unwrap();
        assert_eq!(ix.refresh_if_stale().unwrap(), Fresh::Ok);
        assert_eq!(ix.unresolved(10).unwrap().len(), 1);
        // 5 秒以内は走査しない → 新しいファイルは見えない。refresh_paths で取り込む
        put(&e.root, "b.md", "back");
        ix.refresh_if_stale().unwrap();
        assert!(ix.page("b.md").unwrap().is_none());
        assert_eq!(ix.refresh_paths(&[e.root.join("b.md")]).unwrap().added, ["b.md"]);
        assert!(ix.page("b.md").unwrap().is_some());
        assert_eq!(ix.unresolved(10).unwrap().len(), 0, "新しいファイルで他のページのリンクも解決し直す");
        std::fs::remove_file(e.root.join("b.md")).unwrap();
        assert_eq!(ix.refresh_paths(&["b.md"]).unwrap().removed, ["b.md"]);
        assert_eq!(ix.unresolved(10).unwrap().len(), 1);
        // 走査の再現性: 作り直しても同じ件数
        let n = |ix: &Index| (ix.stats().unwrap()["pages"].clone(), ix.stats().unwrap()["links"].clone());
        let before = n(&ix);
        ix.rebuild().unwrap();
        assert_eq!(n(&ix), before);
    }

    /// A-16: GUI が監視中（ctx.watched）なら入口の鮮度規則の stat 走査を省く（5 秒を過ぎても新しいファイルは見えない = 取り込むのは監視）。
    /// 作り直しの旗があれば今どおり（他の生きたプロセスの旗 = REBUILDING）。監視なし（watched = false）なら stat 走査が再開する
    #[test]
    fn watched_skips_stale_scan_unless_flag() {
        let e = env("watched");
        put(&e.root, "a.md", "x");
        let (ip, root) = e.ctx.ready().unwrap();
        let total = |ctx: &Ctx| list(ctx, &json!({})).unwrap()["total"].as_u64().unwrap();
        assert_eq!(total(&e.ctx), 1);
        let w = Ctx { watched: true, ..e.ctx.clone() };
        put(&e.root, "b.md", "[[a]]");
        let sql = |q: String| with_index(&ip, &root, |ix| Ok(ix.conn_for_test().execute_batch(&q)?)).unwrap();
        // 最終走査を古くする（5 秒の鮮度規則なら走査する状態）
        let age = || sql("UPDATE meta SET value = '0' WHERE key = 'last_scan'".into());
        age();
        assert_eq!(total(&w), 1, "監視中は stat 走査しない");
        assert_eq!(status(&w, &json!({})).unwrap()["pages"], 1);
        assert_eq!(backlinks(&w, &json!({"path": "a.md"})).unwrap()["total"], 0);
        assert_eq!(tags(&w, &json!({})).unwrap()["total"], 0);
        assert_eq!(read(&w, &json!({"path": "a.md"})).unwrap()["content"], "x");
        let last = || with_index(&ip, &root, |ix| ix.meta("last_scan")).unwrap();
        assert_eq!(last().as_deref(), Some("0"), "走査していない");
        // 旗（他の生きたプロセス = pid 1）があれば今どおり = 作り直し中
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        sql(format!("INSERT OR REPLACE INTO meta VALUES ('rebuilding', '1:{now}'); INSERT OR REPLACE INTO meta VALUES ('progress', '42');"));
        assert!(matches!(list(&w, &json!({})).unwrap_err(), CoreError::Wiki { code, .. } if code == codes::REBUILDING));
        assert_eq!(status(&w, &json!({})).unwrap()["rebuilding"], json!({"percent": 42}));
        // 死んだ旗（heartbeat から 10 分超）も refresh_if_stale に任せる = 引き継いで走査する。旗の行が残る間は今どおり
        sql(format!("UPDATE meta SET value = '1:{}' WHERE key = 'rebuilding'", now - 601));
        assert_eq!(total(&w), 2, "旗があれば鮮度規則（古い旗は引き継いで走査）");
        put(&e.root, "c.md", "[[a]]");
        age();
        assert_eq!(total(&w), 3, "旗の行が残る間は鮮度規則");
        // 旗が消えれば省く
        sql("DELETE FROM meta WHERE key IN ('rebuilding', 'progress')".into());
        put(&e.root, "d.md", "[[a]]");
        age();
        assert_eq!(total(&w), 3);
        // 監視が落ちた（watched = false）= stat 走査が再開する
        assert_eq!(total(&e.ctx), 4, "監視なしは 5 秒の鮮度規則で走査する");
        assert_eq!(backlinks(&e.ctx, &json!({"path": "a.md"})).unwrap()["total"], 3);
    }

    /// I1: backlinks の抜粋はリンクの形で当てる（needle を含むだけの行・コードブロックの中は選ばない）。md = 抽出器と同じ規則・
    /// item の鍵は openref:// の key・html は属性の値。形で見つからない（frontmatter の items 等）ときは needle を含む最初の行（従来）
    #[test]
    fn backlinks_excerpt_matches_link_form() {
        let e = env("excerpt");
        put(&e.root, "a.md", "A");
        put(&e.root, "w.md", "a を含む行\n```\n[[a]] はコード\n```\n本文 [[a|エー]] へ\n");
        put(&e.root, "m.md", "data.md と a.md の話\n見る: [x](a.md)\n");
        put(&e.root, "h.html", "<p>a.md の説明</p>\n<a href=\"a.md\">A</a>\n");
        put(&e.root, "k.md", "---\nitems: [ABCD2345]\n---\nABCD2345 の話\n[文献](openref://open?key=ABCD2345)\n");
        put(&e.root, "f.md", "---\nitems: [WXYZ6789]\n---\n本文\n");
        let ex = |v: &Value, path: &str| {
            v["pages"].as_array().unwrap().iter().find(|p| p["path"] == path).unwrap_or_else(|| panic!("{path}: {v}"))["excerpt"].clone()
        };
        let b = backlinks(&e.ctx, &json!({"path": "a.md"})).unwrap();
        assert_eq!(b["total"], 3, "{b}");
        assert_eq!(ex(&b, "w.md"), "本文 [[a|エー]] へ");
        assert_eq!(ex(&b, "m.md"), "見る: [x](a.md)");
        assert_eq!(ex(&b, "h.html"), "<a href=\"a.md\">A</a>");
        let k = backlinks(&e.ctx, &json!({"key": "ABCD2345"})).unwrap();
        assert_eq!(ex(&k, "k.md"), "[文献](openref://open?key=ABCD2345)");
        // 形が無い（frontmatter の items だけ）= 従来どおり鍵を含む最初の行
        let f = backlinks(&e.ctx, &json!({"key": "WXYZ6789"})).unwrap();
        assert_eq!(ex(&f, "f.md"), "items: [WXYZ6789]");
        // 形（JSON）は不変
        let keys: Vec<&String> = b["pages"][0].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["excerpt", "kind", "link_kinds", "path", "title"]);
    }

    /// I2: read は stat（索引の行）の後に上限を超えて増えたファイルを丸ごと読まない = 上限 + 1 バイトまで読んで、超えていれば
    /// stat で超えていたときと同じエラー。上限以下で増えた分は読む（従来と同じ）
    #[test]
    fn read_caps_file_grown_after_stat() {
        let e = env("grown");
        put(&e.root, "g.md", "small");
        assert_eq!(read(&e.ctx, &json!({"path": "g.md"})).unwrap()["content"], "small");
        let grow = |n: usize| {
            BEFORE_BODY.with(|h| {
                *h.borrow_mut() = Some(Box::new(move |p: &Path| {
                    use std::io::Write;
                    let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
                    f.write_all(&vec![b'x'; n]).unwrap();
                }))
            })
        };
        grow(3);
        let r = read(&e.ctx, &json!({"path": "g.md"})).unwrap();
        assert_eq!(r["content"], "smallxxx", "上限以下で増えた分は読む");
        grow(index::BODY_MAX as usize);
        READS.with(|c| c.set((0, 0)));
        let err = read(&e.ctx, &json!({"path": "g.md"})).unwrap_err().to_string();
        BEFORE_BODY.with(|h| *h.borrow_mut() = None);
        let size = index::BODY_MAX + 8;
        assert_eq!(err, bad(format!("g.md は大きすぎて読めません（{size} バイト・上限 {} バイト）", index::BODY_MAX)).to_string());
        assert_eq!(READS.with(|c| c.get()), (0, 1), "本文の read は 1 回・ロックの外");
        // 次の read は索引の行（stat）で上限超えを見る = 同じエラー
        assert!(read(&e.ctx, &json!({"path": "g.md"})).unwrap_err().to_string().contains("大きすぎて読めません"));
    }

    /// 監視中（watched）に索引が壊れて退避・作り直された = 空の索引（last_scan なし）は省かずに走査する（QA M2）
    #[test]
    fn watched_scans_index_recreated_after_corruption() {
        let e = env("watched-corrupt");
        put(&e.root, "a.md", "x");
        put(&e.root, "b.md", "y");
        let w = Ctx { watched: true, ..e.ctx.clone() };
        let total = |ctx: &Ctx| list(ctx, &json!({})).unwrap()["total"].as_u64().unwrap();
        assert_eq!(total(&e.ctx), 2);
        assert_eq!(total(&w), 2);
        let (ip, root) = e.ctx.ready().unwrap();
        // プロセスの接続を外す（別の索引を開く = 握っている接続が閉じる）→ 索引にゴミを書く
        let other = e.base.join("home/other-app/wiki-index.sqlite");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        with_index(&other, &root, |_| Ok(())).unwrap();
        for s in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{s}", ip.display()));
        }
        std::fs::write(&ip, vec![0x5Au8; 8192]).unwrap();
        assert_eq!(total(&w), 2, "退避して作り直した空の索引を走査する");
        assert_eq!(total(&w), 2);
        assert!(index::recovered().is_some());
        // 走査の後（last_scan あり）は省く
        put(&e.root, "c.md", "z");
        with_index(&ip, &root, |ix| Ok(ix.conn_for_test().execute("UPDATE meta SET value = '0' WHERE key = 'last_scan'", [])?)).unwrap();
        assert_eq!(total(&w), 2);
        assert_eq!(total(&e.ctx), 3);
    }

    #[test]
    fn foreign_rebuild_flag_and_root_change() {
        let e = env("flag");
        put(&e.root, "a.md", "x");
        let (ip, root) = e.ctx.ready().unwrap();
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        // 他の生きたプロセスの旗（pid 1 = launchd・kill(1, 0) は EPERM = 生きている）→ 待たずに作り直し中を返す
        let set_flag = |ix: &Index, v: String| {
            ix.conn_for_test()
                .execute_batch(&format!(
                    "INSERT OR REPLACE INTO meta VALUES ('rebuilding', '{v}'); INSERT OR REPLACE INTO meta VALUES ('progress', '42'); DELETE FROM meta WHERE key = 'last_scan';"
                ))
                .unwrap()
        };
        set_flag(&ix, format!("1:{now}"));
        assert_eq!(ix.refresh_if_stale().unwrap(), Fresh::Rebuilding(42));
        assert_eq!(ix.rebuild().unwrap(), Fresh::Rebuilding(42));
        // heartbeat から 10 分を過ぎた旗は引き継ぐ
        set_flag(&ix, format!("1:{}", now - 601));
        assert_eq!(ix.refresh_if_stale().unwrap(), Fresh::Ok);
        // 死んだプロセスの旗（新しい heartbeat でも）は即引き継ぐ。作り直しの後は旗が消える
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        set_flag(&ix, format!("{dead}:{now}"));
        assert_eq!(ix.refresh_if_stale().unwrap(), Fresh::Ok);
        set_flag(&ix, format!("{dead}:{now}"));
        assert_eq!(ix.rebuild().unwrap(), Fresh::Ok);
        assert_eq!(ix.meta("rebuilding").unwrap(), None);
        assert_eq!(ix.stats().unwrap()["pages"], 1);
        // root が変われば捨てる
        let other = e.base.join("home/Documents/other");
        std::fs::create_dir_all(&other).unwrap();
        let ix2 = Index::open(&ip, &std::fs::canonicalize(&other).unwrap()).unwrap();
        assert_eq!(ix2.stats().unwrap()["pages"], 0);
        assert_eq!(ix2.meta("last_scan").unwrap(), None);
    }

    #[test]
    fn schema_version_change_rebuilds_tables() {
        let e = env("schema");
        put(&e.root, "a.md", "x");
        let (ip, root) = e.ctx.ready().unwrap();
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        ix.conn_for_test().execute("UPDATE meta SET value = '0' WHERE key = 'schema'", []).unwrap();
        drop(ix);
        let ix = Index::open(&ip, &root).unwrap();
        assert_eq!(ix.stats().unwrap()["pages"], 0, "版が違えば作り直す");
        assert_eq!(ix.meta("schema").unwrap().as_deref(), Some(index::SCHEMA_VERSION));
    }

    /// root が NFD で実在し、ワークスペースの中の絶対パスを NFC で渡しても読める（m3）・backlinks の path も read と同じ解決
    #[test]
    fn nfd_root_and_backlinks_path_resolution() {
        let mut e = env("nfd");
        let nfd: String = "マイノート".nfd().collect();
        let root = e.base.join(format!("home/Documents/{nfd}"));
        std::fs::create_dir_all(&root).unwrap();
        e.ctx.root = Some(root.to_string_lossy().into());
        put(&root, "Sub/Page.md", "x");
        put(&root, "b.md", "[[page]]");
        let nfc_abs: String = root.join("Sub/Page.md").to_string_lossy().nfc().collect();
        assert_eq!(read(&e.ctx, &json!({"path": nfc_abs})).unwrap()["path"], "Sub/Page.md");
        let nfd_abs = root.join("Sub/Page.md");
        assert_eq!(read(&e.ctx, &json!({"path": nfd_abs.to_string_lossy()})).unwrap()["path"], "Sub/Page.md");
        // 大小だけ違う指定も索引の表記に揃う（APFS は大小無視）
        assert_eq!(read(&e.ctx, &json!({"path": "sub/page.md"})).unwrap()["path"], "Sub/Page.md");
        assert_eq!(backlinks(&e.ctx, &json!({"path": "sub/page.md"})).unwrap()["pages"][0]["path"], "b.md");
        assert_eq!(backlinks(&e.ctx, &json!({"path": nfc_abs})).unwrap()["count"], 1);
        // 解けない path は 0 件ではなくエラー
        assert!(backlinks(&e.ctx, &json!({"path": "nope.md"})).unwrap_err().to_string().contains("not found"));
        assert!(backlinks(&e.ctx, &json!({"path": "../x.md"})).is_err());
    }

    /// root が読めない（readdir も find も失敗）とき、既知のページを消さずに失敗を返す（m4）
    #[test]
    fn unreadable_root_keeps_index() {
        use std::os::unix::fs::PermissionsExt;
        let e = env("locked");
        put(&e.root, "a.md", "x");
        let (ip, root) = e.ctx.ready().unwrap();
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        std::fs::set_permissions(&e.root, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = ix.refresh();
        std::fs::set_permissions(&e.root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err());
        assert_eq!(ix.stats().unwrap()["pages"], 1, "消さない");
    }

    /// 検索: 和文 2 字語の本文一致・英語（記号入り・前方一致）・題名一致が上・tag の AND・抜粋の印・更新と削除の追随
    #[test]
    fn search_tags_and_ranking() {
        let e = env("search");
        put(&e.root, "notes/biliary.md", "---\ntags: [abdomen, peds]\naliases: [胆管拡張]\n---\n# 小児の胆道疾患\n\n胆道閉鎖症は新生児の黄疸で疑う。\n");
        put(&e.root, "notes/prostate.md", "---\ntitle: PI-RADS v2.1\ntags: [GU]\n---\nProstate MRI scoring.\n");
        put(&e.root, "notes/glioma.md", "---\ntags: [neuro, Peds]\n---\n# Pediatric glioma\n\nDiffuse midline glioma vs glioblastoma.\n");
        put(&e.root, "notes/gbm.md", "---\ntags: [neuro]\n---\n# Glioblastoma\n\nIDH wild-type.\n");
        put(&e.root, "notes/misc.md", "# Misc\n\nThe PI-RADS score and 胆道 imaging.\n");
        put(&e.root, "page.html", "<html><head><title>HTML ページ</title><meta name=\"wiki:tags\" content=\"neuro\"><style>.glioblastoma{}</style></head><body><p>膵臓<b>癌</b>の画像</p><script>glioblastoma()</script></body></html>");
        put(&e.root, "img/glioblastoma.png", "\u{89}PNG");
        let s = |a: Value| search(&e.ctx, &a).unwrap();
        let paths = |v: &Value| v["pages"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        // 和文 2 字語: 本文一致（cjk 1 字 phrase）。題名の部分一致（小児の胆道疾患）が上
        let r = s(json!({"query": "胆道"}));
        assert_eq!(paths(&r), ["notes/biliary.md", "notes/misc.md"], "{r}");
        assert_eq!(r["pages"][0]["match"], "title+body");
        assert_eq!(r["pages"][1]["match"], "body");
        assert_eq!(r["pages"][1]["snippet"], "The PI-RADS score and \u{e000}胆道\u{e001} imaging.", "一致の行の頭から");
        // frontmatter の値（aliases）も本文として引ける・title は frontmatter の値から外す
        assert_eq!(paths(&s(json!({"query": "胆管拡張"}))), ["notes/biliary.md"]);
        // 英語の記号入りの語: 題名の完全一致が先頭
        let r = s(json!({"query": "PI-RADS"}));
        assert_eq!(paths(&r), ["notes/prostate.md", "notes/misc.md"], "{r}");
        assert!(r["pages"][1]["snippet"].as_str().unwrap().contains("\u{e000}PI-RADS\u{e001}"));
        // 大小無視・題名の完全一致 → 前方一致 → 本文だけ。media は題名（ファイル名）で引け、本文は持たない（snippet null）
        let r = s(json!({"query": "glioblastoma"}));
        assert_eq!(paths(&r), ["notes/gbm.md", "img/glioblastoma.png", "notes/glioma.md"], "{r}");
        assert_eq!(r["pages"][1]["snippet"], Value::Null);
        // 最後の語は前方一致（入力途中）・語は AND
        assert_eq!(paths(&s(json!({"query": "gliobl"})))[0], "notes/gbm.md");
        assert_eq!(paths(&s(json!({"query": "midline glio"}))), ["notes/glioma.md"]);
        assert!(paths(&s(json!({"query": "midline 胆道"}))).is_empty());
        // html: タグを除いた本文だけ（style / script の中は入らない）・和文は要素の境をまたいでも続く
        assert_eq!(paths(&s(json!({"query": "膵臓癌"}))), ["page.html"]);
        assert!(!paths(&s(json!({"query": "glioblastoma"}))).contains(&"page.html".to_string()));
        // tag の AND（大小無視）・query との併用・query なし
        assert_eq!(paths(&s(json!({"tags": ["peds", "NEURO"]}))), ["notes/glioma.md"]);
        assert_eq!(paths(&s(json!({"tag": "neuro"}))), ["notes/gbm.md", "page.html", "notes/glioma.md"]);
        assert_eq!(paths(&s(json!({"query": "glioblastoma", "tag": "peds"}))), ["notes/glioma.md"]);
        assert_eq!(s(json!({"query": "glioblastoma", "limit": 1}))["total"], 3);
        assert!(search(&e.ctx, &json!({})).unwrap_err().to_string().contains("query"));
        assert!(search(&e.ctx, &json!({"query": " - \""})).is_err(), "英数字も和文も無い語だけ");
        assert!(search(&e.ctx, &json!({"query": "x", "limit": 0})).is_err());
        assert!(search(&e.ctx, &json!({"tags": "neuro"})).is_err(), "tags は配列");
        // FTS の構文になる文字を入れてもエラーにしない
        for q in ["AND", "a OR b", "NEAR(x", "x*", "(胆道)", "col:val", "^x"] {
            assert!(search(&e.ctx, &json!({"query": q})).is_ok(), "{q}");
        }
        // wiki_list の tag も AND
        assert_eq!(list(&e.ctx, &json!({"tag": "neuro", "tags": ["peds"]})).unwrap()["count"], 1);
        // tags: 件数の多い順・大小を束ねる
        let t = tags(&e.ctx, &json!({})).unwrap();
        assert_eq!(t["tags"][0], json!({"tag": "neuro", "count": 3}), "{t}");
        assert_eq!(t["tags"][1]["count"], 2, "peds と Peds は 1 つ: {t}");
        assert_eq!(t["total"], 4);
        assert_eq!(tags(&e.ctx, &json!({"limit": 1})).unwrap()["count"], 1);
        // 更新と削除: refresh_paths で全文検索も追随する（古い語は引けず・新しい語は引ける）
        let (ip, root) = e.ctx.ready().unwrap();
        put(&e.root, "notes/gbm.md", "# GBM\n\nastrocytoma だけ\n");
        std::fs::remove_file(e.root.join("notes/misc.md")).unwrap();
        with_index(&ip, &root, |ix| ix.refresh_paths(&["notes/gbm.md", "notes/misc.md"])).unwrap();
        assert_eq!(paths(&s(json!({"query": "glioblastoma"}))), ["img/glioblastoma.png", "notes/glioma.md"]);
        assert_eq!(paths(&s(json!({"query": "astrocytoma"}))), ["notes/gbm.md"]);
        assert_eq!(paths(&s(json!({"query": "胆道"}))), ["notes/biliary.md"]);
        // 作り直しても同じ結果
        with_index(&ip, &root, |ix| ix.rebuild()).unwrap();
        assert_eq!(paths(&s(json!({"query": "astrocytoma"}))), ["notes/gbm.md"]);
    }

    /// 題名の一致は両側から記号を落として比べる（「胆道。」でも題名「胆道」が完全一致で先頭）
    #[test]
    fn title_match_ignores_symbols() {
        let e = env("title");
        put(&e.root, "a.md", "# 胆道\n\n本文\n");
        put(&e.root, "b.md", "# 総論\n\n胆道の話。胆道。\n");
        put(&e.root, "c.md", "---\ntitle: \"PI-RADS: v2.1\"\n---\nx\n");
        let s = |q: &str| search(&e.ctx, &json!({"query": q})).unwrap();
        let r = s("胆道。");
        assert_eq!(r["pages"][0]["path"], "a.md", "{r}");
        assert_eq!(r["pages"][0]["match"], "title+body", "h1 は本文にも入る");
        assert_eq!(r["pages"][1]["match"], "body");
        assert_eq!(s("「胆道」")["pages"][0]["path"], "a.md");
        assert!(s("pi rads")["pages"][0]["match"].as_str().unwrap().starts_with("title"), "記号を落とした題名に語が含まれる");
        assert_eq!(s("PI-RADS:")["pages"][0]["path"], "c.md");
        assert_eq!(index::title_key("  PI-RADS:  v2.1 "), "pirads v21");
    }

    #[test]
    fn snippet_marks_and_window() {
        let t = |text: &str, q: &[&str]| snippet(text, &q.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(t("abc Glioma def", &["glioma"]), "abc \u{e000}Glioma\u{e001} def");
        // 前後を切って … を付ける・複数の語・空白を 1 つに畳む・wikilink の括弧と強調は落とす
        let long = format!("{}  target [[x]] **b** {}", "a".repeat(100), "z".repeat(300));
        let r = t(&long, &["target", "b"]);
        assert!(r.starts_with('…') && r.ends_with('…'), "{r}");
        assert!(r.contains("\u{e000}target\u{e001} x \u{e000}b\u{e001}"), "{r}");
        // 語がそのまま無ければ英数字の部分で探す（FTS は "PI RADS" も "PI-RADS" で引く）
        assert_eq!(t("PI RADS 3", &["pi-rads"]), "\u{e000}PI\u{e001} \u{e000}RADS\u{e001} 3");
        // 見つからなければ先頭（印なし）
        assert_eq!(t("hello world", &["zzz"]), "hello world");
        assert_eq!(t("", &["x"]), "");
        // 一致の行の頭が窓の中なら行の頭から（… を付けない）・HTML のタグとコメントは落とす
        assert_eq!(t("前の行\n<details><summary>所属</summary> 胆道 <!-- c --> 末尾", &["胆道"]), "所属 \u{e000}胆道\u{e001} 末尾");
        assert_eq!(t("a < b and c<d", &["b"]), "a < \u{e000}b\u{e001} and c<d", "タグでない < は残す");
        // md の記法は印を付ける前に平文へ（md_plain）
        let m = |text: &str, q: &[&str]| snippet(&md_plain(text), &q.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(m("# 胆道", &["胆道"]), "\u{e000}胆道\u{e001}", "見出し記号");
        assert_eq!(m("> [!info] このノート 胆道", &["胆道"]), "このノート \u{e000}胆道\u{e001}", "引用と callout");
        assert_eq!(m("> > [!note]- 折畳 胆道", &["胆道"]), "折畳 \u{e000}胆道\u{e001}");
        assert_eq!(m("📄 [原本 PDF を開く](file:///Users/x/a.pdf) 胆道", &["胆道"]), "📄 原本 PDF を開く \u{e000}胆道\u{e001}", "リンクは text だけ");
        assert_eq!(m("![図](attachments/a.png) 胆道", &["胆道"]), "図 \u{e000}胆道\u{e001}");
        assert_eq!(m("[[topics/胆道|胆道の画像]] と [[肝]] と ![[img.png|300]]", &["胆道"]), "\u{e000}胆道\u{e001}の画像 と 肝 と img.png", "wikilink は別名・埋め込みはファイル名");
        assert_eq!(m("| 部位 | [[a\\|胆道]] |\n|---|:--:|\n| x | y |", &["胆道"]), "部位 \u{e000}胆道\u{e001} x y", "表の | と区切り行");
        assert_eq!(m("- 胆道\n2. 肝\n- [x] 済み", &["胆道"]), "\u{e000}胆道\u{e001} 肝 済み", "リスト記号とタスク");
        assert_eq!(m("#tag は見出しではない 胆道", &["胆道"]), "#tag は見出しではない \u{e000}胆道\u{e001}");
        assert_eq!(m("**太字** __下線__ ~~取消~~ `code` 胆道", &["胆道"]), "太字 下線 取消 code \u{e000}胆道\u{e001}", "強調とインラインコード");
        assert_eq!(m("```python\n胆道 = 1\n```", &["胆道"]), "\u{e000}胆道\u{e001} = 1", "フェンスの行は捨て中身は残す");
        assert_eq!(m("**胆道**の", &["胆道"]), "\u{e000}胆道\u{e001}の", "強調の中の一致も印がずれない");
    }

    /// 本文・抜粋の read は索引のロック（with_index）の外（A-13・A-14）。中では行とリンクだけ取る
    #[test]
    fn body_reads_are_outside_index_lock() {
        let e = env("lockfree");
        put(&e.root, "a.md", "---\ntags: [x]\n---\n# A\n胆道 [[b]]\n");
        put(&e.root, "b.md", "# B\n戻り [[a]] と [論文](openref://open?key=KEY44444)\n");
        put(&e.root, "c.html", "<html><head><title>C</title></head><body><a href=\"a.md\">a</a> 胆道</body></html>");
        status(&e.ctx, &json!({})).unwrap();
        // 呼び出しごとに (with_index の中で読んだ回数, read_file の回数) = (0, 本文・抜粋の数)
        let reads = |what: &str, r: Result<Value>, want: u32| -> Value {
            let (inside, all) = READS.with(|c| c.take());
            let v = r.unwrap();
            assert_eq!((inside, all), (0, want), "{what}: with_index の中で本文を読んだ / read_file を通らない: {v}");
            v
        };
        for p in ["a.md", "b.md", "c.html"] {
            reads(p, read(&e.ctx, &json!({"path": p})), 1);
        }
        // 索引の行と違うファイル（refresh_paths を通る）・索引にまだ無いページ
        put(&e.root, "b.md", "# B2\n戻り [[a]] [論文](openref://open?key=KEY44444) 書換え\n");
        put(&e.root, "d.md", "[[a]] 新しい\n");
        reads("b.md 書換え", read(&e.ctx, &json!({"path": "b.md"})), 1);
        reads("d.md 新規", read(&e.ctx, &json!({"path": "d.md"})), 1);
        reads("backlinks a.md", backlinks(&e.ctx, &json!({"path": "a.md"})), 3);
        // total は limit で切る前の件数・抜粋は切った後の分だけ読む・limit 0 は 1 に丸める
        for limit in [1, 0] {
            let v = reads("backlinks a.md limit", backlinks(&e.ctx, &json!({"path": "a.md", "limit": limit})), 1);
            assert_eq!((v["total"].as_i64(), v["count"].as_i64(), v["pages"].as_array().map(|a| a.len())), (Some(3), Some(1), Some(1)), "limit {limit}: {v}");
        }
        reads("backlinks key", backlinks(&e.ctx, &json!({"key": "KEY44444"})), 1);
        reads("search", search(&e.ctx, &json!({"query": "胆道"})), 2);
        // 本文の無い種類は読まない
        put(&e.root, "img.png", "\u{89}PNG");
        reads("img.png", read(&e.ctx, &json!({"path": "img.png"})), 0);
    }

    /// 索引に行はあるがファイルが読めない（ロックを離した後に消えた・権限が落ちた）= read は従来と同じ io のエラー・
    /// backlinks は抜粋 null で行は返す
    #[test]
    fn unreadable_after_index_lookup() {
        use std::os::unix::fs::PermissionsExt;
        let e = env("gone");
        put(&e.root, "a.md", "# A\n");
        put(&e.root, "b.md", "[[a]] から\n");
        put(&e.root, "c.md", "[[a]] も\n");
        assert_eq!(backlinks(&e.ctx, &json!({"path": "a.md"})).unwrap()["count"], 2);
        read(&e.ctx, &json!({"path": "b.md"})).unwrap();
        let b = e.root.join("b.md");
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = read(&e.ctx, &json!({"path": "b.md"}));
        let bl = backlinks(&e.ctx, &json!({"path": "a.md"}));
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(r, Err(CoreError::Io(ref io)) if io.kind() == std::io::ErrorKind::PermissionDenied), "{r:?}");
        let bl = bl.unwrap();
        assert_eq!(bl["count"], 2, "{bl}");
        assert_eq!(bl["pages"][0]["path"], "b.md");
        assert!(bl["pages"][0]["excerpt"].is_null(), "{bl}");
        assert_eq!(bl["pages"][1]["excerpt"], "[[a]] も");
        // 抜粋の元が消えた（索引にはまだある = 最初の走査から 5 秒以内で、a.md への backlinks は c.md を取り込み直さない）= 行は返し抜粋 null
        std::fs::remove_file(e.root.join("c.md")).unwrap();
        let bl = backlinks(&e.ctx, &json!({"path": "a.md"})).unwrap();
        assert_eq!(bl["count"], 2, "{bl}");
        assert!(bl["pages"].as_array().unwrap().iter().any(|p| p["path"] == "c.md" && p["excerpt"].is_null()), "{bl}");
    }

    #[test]
    fn inbox_lists_ingest_files() {
        let e = env("inbox");
        assert_eq!(inbox(&e.ctx, &json!({})).unwrap()["count"], 0, "ingest/ が無ければ空");
        put(&e.root, "ingest/a.png", "\u{89}PNG");
        put(&e.root, "ingest/sub/note.md", "# n");
        put(&e.root, "ingest/.DS_Store", "x");
        put(&e.root, "ingest/.hidden/x.pdf", "%PDF");
        put(&e.root, "notes/ingest/b.md", "# not inbox");
        let (ip, root) = e.ctx.ready().unwrap();
        with_index(&ip, &root, |ix| ix.refresh()).unwrap();
        let r = inbox(&e.ctx, &json!({})).unwrap();
        assert_eq!(r["count"], 2, "{r}");
        assert_eq!(r["files"][0], json!({"path": "ingest/a.png", "name": "a.png", "kind": "media", "media": "image", "size": 5, "mtime": r["files"][0]["mtime"]}));
        assert_eq!(r["files"][1]["name"], "note.md");
        assert_eq!(r["files"][1]["media"], Value::Null);
        assert_eq!(inbox(&e.ctx, &json!({"limit": 1})).unwrap()["total"], 2);
        assert_eq!(status(&e.ctx, &json!({})).unwrap()["inbox"], 2);
        // dir = chat-log（チャットの記録）は別の箱。ingest/ は数えない。それ以外の dir は拒否
        put(&e.root, "chat-log/2026-11-01 1200 x.md", "# log");
        with_index(&ip, &root, |ix| ix.refresh()).unwrap();
        put(&e.root, "chat-log/2026-11-02 0900 y.md", "# log 2");
        with_index(&ip, &root, |ix| ix.refresh()).unwrap();
        let r = inbox(&e.ctx, &json!({"dir": "chat-log", "limit": 1})).unwrap();
        assert_eq!((r["dir"].as_str(), r["total"].as_u64(), r["count"].as_u64()), (Some("chat-log/"), Some(2), Some(1)), "{r}");
        assert_eq!(r["files"][0]["path"], "chat-log/2026-11-02 0900 y.md", "新しい順・limit で古い方が落ちる");
        assert_eq!(inbox(&e.ctx, &json!({})).unwrap()["count"], 2);
        assert!(inbox(&e.ctx, &json!({"dir": "notes"})).unwrap_err().to_string().contains("dir は"));
        assert!(inbox(&e.ctx, &json!({"dir": 5})).unwrap_err().to_string().contains("文字列"));
    }

    /// 実 vault での計測（`OPENREF_WIKI_BENCH_ROOT` = vault・`OPENREF_WIKI_BENCH_APP` = 索引を置くフォルダ。無ければ何もしない）。
    /// cargo test -p openref-core --release wiki::commands::tests::bench_real_vault -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_real_vault() {
        let (Ok(root), Ok(app)) = (std::env::var("OPENREF_WIKI_BENCH_ROOT"), std::env::var("OPENREF_WIKI_BENCH_APP")) else {
            return;
        };
        let ctx = Ctx { enabled: true, root: Some(root), app_dir: Some(PathBuf::from(&app)), home: std::env::var_os("HOME").map(PathBuf::from), watched: false };
        let (ip, root) = ctx.ready().unwrap();
        let size = |ip: &Path| -> u64 {
            ["", "-wal", "-shm"].iter().filter_map(|s| std::fs::metadata(format!("{}{s}", ip.display())).ok()).map(|m| m.len()).sum()
        };
        index::remove_index(&ip).unwrap();
        let t = std::time::Instant::now();
        with_index(&ip, &root, |ix| ix.refresh()).unwrap();
        let first = t.elapsed();
        let t = std::time::Instant::now();
        with_index(&ip, &root, |ix| ix.rebuild()).unwrap();
        let rebuild = t.elapsed();
        with_index(&ip, &root, |ix| Ok(ix.conn_for_test().execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?)).unwrap();
        let main_size = std::fs::metadata(&ip).unwrap().len();
        let fts_pages: i64 = with_index(&ip, &root, |ix| Ok(ix.conn_for_test().query_row("SELECT count(*) FROM pages", [], |r| r.get(0))?)).unwrap();
        eprintln!("BENCH pages={fts_pages} first_scan={first:?} rebuild={rebuild:?} index_file={}MB (with wal/shm {}MB)", main_size / 1_048_576, size(&ip) / 1_048_576);
        let qs = ["胆道", "PI-RADS", "glioblastoma", "肝細胞癌", "MRI", "拡散強調像", "meningioma", "造影 CT", "dural tail", "小児"];
        let mut all = Vec::new();
        for q in qs {
            let mut ts = Vec::new();
            let mut last = Value::Null;
            for _ in 0..5 {
                let t = std::time::Instant::now();
                last = search(&ctx, &json!({"query": q, "limit": 200})).unwrap();
                ts.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            all.extend(ts.iter().copied());
            let top: Vec<String> = last["pages"].as_array().unwrap().iter().take(3).map(|p| format!("{} [{}]", p["title"].as_str().unwrap_or(""), p["match"].as_str().unwrap_or(""))).collect();
            eprintln!("BENCH q={q:?} total={} median={:.1}ms max={:.1}ms top={top:?}", last["total"], ts[2], ts[4]);
            eprintln!("BENCH   snippet0={:?}", last["pages"][0]["snippet"]);
        }
        all.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!("BENCH all n={} p50={:.1}ms max={:.1}ms", all.len(), all[all.len() / 2], all[all.len() - 1]);
        let t = std::time::Instant::now();
        let tg = tags(&ctx, &json!({"limit": 20})).unwrap();
        eprintln!("BENCH tags total={} in {:?} top={}", tg["total"], t.elapsed(), tg["tags"]);
        let first_tags: Vec<String> = tg["tags"].as_array().unwrap().iter().take(2).map(|t| t["tag"].as_str().unwrap().to_string()).collect();
        let t = std::time::Instant::now();
        let r = search(&ctx, &json!({"tags": first_tags})).unwrap();
        eprintln!("BENCH tag AND {first_tags:?} total={} in {:?}", r["total"], t.elapsed());
        let t = std::time::Instant::now();
        let r = inbox(&ctx, &json!({})).unwrap();
        eprintln!("BENCH inbox total={} in {:?}", r["total"], t.elapsed());
    }

    /// 子プロセスの本体（親の multi_process_* が環境変数を付けて呼ぶ。直に走らせると何もしない）
    #[test]
    #[ignore]
    fn child_proc() {
        let (Ok(ip), Ok(root), Ok(mode)) =
            (std::env::var("OPENREF_WIKI_CHILD_IX"), std::env::var("OPENREF_WIKI_CHILD_ROOT"), std::env::var("OPENREF_WIKI_CHILD_MODE"))
        else {
            return;
        };
        let r = (|| -> Result<String> {
            let mut ix = Index::open(Path::new(&ip), Path::new(&root))?;
            let f = if mode == "rebuild" { ix.rebuild()? } else { ix.refresh_if_stale()? };
            let pages = ix.stats()?["pages"].clone();
            Ok(format!("{f:?} pages={pages}"))
        })();
        match r {
            Ok(s) => println!("RESULT ok {s}"),
            Err(e) => println!("RESULT err {e}"),
        }
    }

    /// 複数プロセスが同時に開く・作り直し中に開く（M3）: どれも「database is locked」にならず、Ok か作り直し中を返す
    #[test]
    fn multi_process_open_and_rebuild() {
        let e = env("multi");
        for i in 0..3000 {
            put(&e.root, &format!("d{}/p{i}.md", i % 30), &format!("# P{i}\n[[p{}]] [[missing{i}]] #t{}\n", i + 1, i % 7));
        }
        let (ip, root) = e.ctx.ready().unwrap();
        let exe = std::env::current_exe().unwrap();
        let spawn = |mode: &str| {
            std::process::Command::new(&exe)
                .args(["--exact", "wiki::commands::tests::child_proc", "--ignored", "--nocapture", "--test-threads=1"])
                .env("OPENREF_WIKI_CHILD_IX", &ip)
                .env("OPENREF_WIKI_CHILD_ROOT", &root)
                .env("OPENREF_WIKI_CHILD_MODE", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        };
        let collect = |kids: Vec<std::process::Child>| -> Vec<String> {
            kids.into_iter()
                .map(|k| {
                    let out = String::from_utf8_lossy(&k.wait_with_output().unwrap().stdout).to_string();
                    out.find("RESULT ").map(|i| out[i..].lines().next().unwrap_or("").to_string()).unwrap_or_else(|| format!("RESULT 出力なし: {out}"))
                })
                .collect()
        };
        // 1) 空の索引に 6 プロセスが同時に開いて鮮度規則（1 つが作り、他は作り直し中か Ok）
        let r1 = collect((0..6).map(|_| spawn("open")).collect());
        assert!(r1.iter().all(|l| l.starts_with("RESULT ok")), "{r1:#?}");
        assert!(r1.iter().any(|l| l.contains("Ok pages=3000")), "{r1:#?}");
        // 2) 1 つが作り直している最中に 6 プロセスが開く
        let mut kids = vec![spawn("rebuild")];
        std::thread::sleep(std::time::Duration::from_millis(30));
        kids.extend((0..6).map(|_| spawn("open")));
        let r2 = collect(kids);
        assert!(r2.iter().all(|l| l.starts_with("RESULT ok")), "{r2:#?}");
        assert!(r2[0].contains("Ok pages=3000"), "{r2:#?}");
        eprintln!("multi_process: {r1:?} / {r2:?}");
        let ix = Index::open(&ip, &root).unwrap();
        assert_eq!(ix.stats().unwrap()["pages"], 3000);
        assert_eq!(ix.meta("rebuilding").unwrap(), None, "旗は残らない");
    }
}
