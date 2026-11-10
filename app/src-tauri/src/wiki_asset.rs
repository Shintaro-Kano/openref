//! OpenNote の asset:// の自前配信（asset_reply）・WebView の遷移の許可リスト（wiki_nav_allowed）・外部のアプリで開く先の判定（wiki_open_target）。
//! Tauri command の包みは main.rs（parity test が main.rs だけを走査する）

use crate::err;
use crate::wiki_gui::tr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Emitter;

/// 既定のアプリに渡してよい拡張子（小文字・許可一覧）= 文書・画像・動画 / 音声・Office / iWork・テキスト類・html・epub。
/// それ以外は全部 open せず Finder で場所を表示する（vault の中身は LLM / ingest / git 由来。拒否一覧は LaunchServices の判定に
/// 漏れる = `.term` → Terminal・`.ipa` → iOS App Installer・`.wflow` → Shortcuts・`.zip` → 展開 等）。実行ビット付きも同じ
pub(crate) const WIKI_OPEN_EXT: &[&str] = &[
    // 文書
    "pdf", "epub",
    // 画像
    // svg は入れない（既定のブラウザで開くと svg の中の script が走る・Wiki の中では <img> で見られる）
    "png", "jpg", "jpeg", "gif", "webp", "heic", "avif", "tif", "tiff", "bmp",
    // 動画・音声
    "mp4", "m4v", "mov", "mp3", "m4a", "wav", "aac", "flac",
    // Office・iWork
    // iWork は単一ファイル形式だけここで open（パッケージ形式 = ディレクトリは WIKI_PACKAGE_EXT で Finder 表示）
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "key", "pages", "numbers",
    // テキスト類・html
    "txt", "md", "csv", "tsv", "json", "rtf", "html", "htm",
];

/// ディレクトリでも拒否せず Finder で表示する拡張子（iWork のパッケージ形式）。.app 等のバンドル・ふつうのフォルダは今どおり拒否
const WIKI_PACKAGE_EXT: &[&str] = &["key", "pages", "numbers"];

/// ファイルが「このアプリで開く」の指定（拡張属性 com.apple.LaunchServices.OpenWith）を持つか（有無だけ・値は読まない）。
/// libc の getxattr を呼ぶ（size 0 = 長さだけ問う）。-1 のときは errno が ENOATTR の場合だけ「無い」=
/// それ以外の失敗（権限・xattr 非対応のボリューム・I/O 等 = 有無が分からない）は「有る」側 = Finder 表示に倒す
pub(crate) fn has_open_with_xattr(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(p) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    let name = c"com.apple.LaunchServices.OpenWith";
    // SAFETY: どちらも NUL 終端の C 文字列・value は null と size 0（長さを問うだけで書き込まれない）
    let r = unsafe { libc::getxattr(p.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    r >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOATTR)
}

/// 試験用のフック（呼ぶ側のパスを受け取る）
#[cfg(test)]
type PathHook = Box<dyn Fn(&Path)>;

#[cfg(test)]
thread_local! {
    /// 試験用: asset_reply が scope を照合した後・ファイルを開く前に呼ぶ（照合と open の間に symlink を差し替える窓を作る）
    pub(crate) static ASSET_BEFORE_OPEN: std::cell::RefCell<Option<PathHook>> = const { std::cell::RefCell::new(None) };
}

/// asset:// の 1 要求への応答（register_asynchronous_uri_scheme_protocol の中身・ファイル IO はこの関数の中）。
/// label = 要求した webview（"main" 以外 → 403・ファイルに触らない = 窓を足しても外部ページに vault を読ませない）。パスは Tauri の asset と同じ形
/// （`asset://localhost//Users/…` = 先頭の `/` を 1 つ落として %xx を解く）。`.` / `..` の区画は拒否 → scope（allow_directory したフォルダ）→
/// 読む。Range は単一区間だけ（複数は全体を 200 で返す）。CORS = 要求の Origin をそのまま返す（main は OpenRef 自身のページ）
pub(crate) fn asset_reply(
    label: &str,
    allowed: &dyn Fn(&Path) -> bool,
    req: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{header, Response, StatusCode};
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut b = Response::builder();
    if let Some(o) = &origin {
        b = b.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, o.as_str());
    }
    let deny = |b: tauri::http::response::Builder, st: StatusCode| {
        b.status(st).body(Vec::new()).unwrap_or_default()
    };
    if label != "main" {
        eprintln!(
            "openref-app: asset: webview {label} からの要求を拒否: {}",
            req.uri()
        );
        return deny(b, StatusCode::FORBIDDEN);
    }
    let raw = req.uri().path();
    let path = percent_decode(raw.strip_prefix('/').unwrap_or(raw));
    let p = Path::new(&path);
    if !p.is_absolute() || wiki_has_dot_segment(p) {
        return deny(b, StatusCode::FORBIDDEN);
    }
    // canonicalize を 1 回して、その値で scope を照合し、その値を開く（照合の後に symlink を含みうる p を開き直さない = TOCTOU）。
    // 解けない（無い・読めない）ときは p で照合する = scope の外は 403・中で無いものは 404（従来と同じ応答）
    let canon = match std::fs::canonicalize(p) {
        Ok(c) => c,
        Err(e) => {
            if !allowed(p) {
                eprintln!("openref-app: asset: scope の外: {path}");
                return deny(b, StatusCode::FORBIDDEN);
            }
            return deny(
                b,
                if e.kind() == std::io::ErrorKind::NotFound {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::FORBIDDEN
                },
            );
        }
    };
    if !allowed(&canon) {
        eprintln!("openref-app: asset: scope の外: {path}");
        return deny(b, StatusCode::FORBIDDEN);
    }
    #[cfg(test)]
    ASSET_BEFORE_OPEN.with(|h| h.borrow().as_ref().map(|f| f(p)));
    let mut f = match std::fs::File::open(&canon) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return deny(b, StatusCode::NOT_FOUND)
        }
        Err(_) => return deny(b, StatusCode::FORBIDDEN),
    };
    let len = match f.metadata() {
        Ok(m) if m.is_file() => m.len(),
        _ => return deny(b, StatusCode::FORBIDDEN),
    };
    // mime = 先頭 8 KiB の magic + 拡張子（Tauri と同じ）
    let mut head = vec![0u8; len.min(8192) as usize];
    if std::io::Read::read_exact(&mut f, &mut head).is_err() {
        return deny(b, StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mime = tauri::utils::mime_type::MimeType::parse(&head, &path);
    b = b
        .header(header::CONTENT_TYPE, mime.as_str())
        .header(header::ACCEPT_RANGES, "bytes");
    let range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());
    if let Some(r) = range {
        const MAX_LEN: u64 = 1000 * 1024;
        let ranges = match http_range::HttpRange::parse(r, len) {
            Ok(v) => v,
            Err(_) => {
                return b
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                    .body(Vec::new())
                    .unwrap_or_default()
            }
        };
        // 長さ 0 の区間（空ファイルへの `bytes=-N`）は 206 の content-range で表せない → Range を無視して下の 200（空 body）
        if let [one @ http_range::HttpRange { length: 1.., .. }] = ranges[..] {
            let start = one.start;
            let end = start + (one.length - 1).min(MAX_LEN - 1);
            let n = end + 1 - start;
            let mut buf = vec![0u8; n as usize];
            use std::io::{Read, Seek, SeekFrom};
            if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() {
                return deny(b, StatusCode::INTERNAL_SERVER_ERROR);
            }
            return b
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "content-range")
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
                .header(header::CONTENT_LENGTH, n)
                .body(buf)
                .unwrap_or_default();
        }
    }
    let mut body = head;
    if (len as usize) > body.len() && std::io::Read::read_to_end(&mut f, &mut body).is_err() {
        return deny(b, StatusCode::INTERNAL_SERVER_ERROR);
    }
    b.status(StatusCode::OK)
        .header(header::CONTENT_LENGTH, body.len())
        .body(body)
        .unwrap_or_default()
}

/// WebView の遷移（トップと iframe）を許すか。許可リスト = 次の 4 つだけ・それ以外（data: file: blob: 外部の http(s)・未知の scheme）は止める
/// - `tauri://localhost`（アプリ本体）・`about:blank` / `about:srcdoc`（空の iframe）
/// - asset protocol の URL（macOS は `asset://localhost/<絶対パス>`・他は `http(s)://asset.localhost/…`）で中身が PDF のファイル:
///   asset protocol は Content-Type を中身で決める（tauri-utils の MimeType::parse）ため、Wiki の PDF の iframe の中のリンクで vault 内の
///   html（や中身が html の任意のファイル）へ遷移すると text/html で返り asset://localhost の origin で script が走る →
///   asset scope の中（in_scope）で「拡張子 .pdf の通常ファイルかつ先頭 5 バイトが `%PDF-`」だけ（MediaView の iframe の初回の読込）
/// - dev の origin（`tauri dev` の devUrl = 引数 dev）の http(s)。アプリの画面は外部の http(s) へ遷移しない（外部のページは
///   open_external = シェル経由）ので、PDF の iframe の中のリンクで外部のページが WebView に載るのを止める
///
/// img / video / fetch は遷移ではないので掛からない
pub(crate) fn wiki_nav_allowed(
    url: &tauri::Url,
    dev: Option<&tauri::Url>,
    in_scope: &dyn Fn(&Path) -> bool,
) -> bool {
    let http = matches!(url.scheme(), "http" | "https");
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        "asset" => wiki_asset_is_pdf(url, in_scope),
        _ if http && url.host_str() == Some("asset.localhost") => wiki_asset_is_pdf(url, in_scope),
        _ if http => dev.is_some_and(|d| d.origin() == url.origin()),
        _ => false,
    }
}

/// asset の URL が指すファイルが中身の PDF か。tauri の asset protocol と同じく先頭の `/` を 1 つ落として %xx を解く
/// （`asset://localhost//Users/…` → `/Users/…`）
pub(crate) fn wiki_asset_is_pdf(url: &tauri::Url, in_scope: &dyn Fn(&Path) -> bool) -> bool {
    let path = percent_decode(url.path().get(1..).unwrap_or(""));
    // decode 後の `..` / `.`（%2E%2E 等）は scope の照合の前に落とす
    !wiki_has_dot_segment(Path::new(&path)) && wiki_is_pdf_file(Path::new(&path), in_scope)
}

/// p が roots のどれかの中（root そのものは除く）か。文字列だけで判定する（ファイルシステムに触らない = main thread で使う）
pub(crate) fn wiki_under_roots(roots: &[PathBuf], p: &Path) -> bool {
    if wiki_has_dot_segment(p) {
        return false;
    }
    roots
        .iter()
        .any(|r| openref_core::wiki::strip_root(r, p).is_some_and(|rel| !rel.is_empty()))
}

/// パスの区画に `..` か `.` があるか（文字列で見る = Path::components は途中の `.` を黙って落とす）
pub(crate) fn wiki_has_dot_segment(p: &Path) -> bool {
    p.to_string_lossy()
        .split('/')
        .any(|seg| seg == ".." || seg == ".")
}

/// 止めた遷移を画面へ知らせる（event "nav-blocked" → toast）。連打や PDF の OpenAction の自動遷移で嵐にならないよう 2 秒に 1 回まで
pub(crate) fn wiki_nav_blocked_notice(webview: &tauri::Webview) {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let now = std::time::Instant::now();
    let Ok(mut last) = LAST.lock() else { return };
    if last.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_secs(2)) {
        return;
    }
    *last = Some(now);
    let _ = webview.emit("nav-blocked", ());
}

/// `%xx` を解く（UTF-8 として不正なら置換文字・壊れた escape はそのまま = core の links / edit と同じ percent_encoding）
pub(crate) fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// 拡張子が pdf（大小無視）で、先頭 5 バイトが `%PDF-` の通常ファイルか
/// on_navigation（main thread）で呼ぶので、開く前に asset scope の中（in_scope）・通常ファイル（FIFO・デバイス・ディレクトリを
/// 開かない）を確かめ、O_NONBLOCK で開く（確かめた後に FIFO へ差し替わっても open / read で待たない）
pub(crate) fn wiki_is_pdf_file(p: &Path, in_scope: &dyn Fn(&Path) -> bool) -> bool {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let pdf_ext = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !(pdf_ext
        && p.is_absolute()
        && in_scope(p)
        && std::fs::metadata(p).is_ok_and(|m| m.is_file()))
    {
        return false;
    }
    let mut head = [0u8; 5];
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(p)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == b"%PDF-"
}

/// 既定のアプリで開かずに Finder で表示するだけにするか（純関数）: 拡張子が WIKI_OPEN_EXT（大小無視）に無い・拡張子が無い・実行ビット付き
pub(crate) fn wiki_reveal_only(path: &Path, mode: u32) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    mode & 0o111 != 0 || !WIKI_OPEN_EXT.contains(&ext.as_str())
}

/// 「通常のファイルではありません: <パス>」（開く先・参照フォルダの実体の検査で共通）
pub(crate) fn wiki_not_regular_file(en: bool, p: impl std::fmt::Display) -> String {
    format!(
        "{}: {p}",
        tr(en, "通常のファイルではありません", "not a regular file")
    )
}

/// wiki_open_path の開く先（純関数に近い = ファイルシステムを読むだけ）。戻り値 = (実体の絶対パス, open せず Finder で表示する)。
/// - vault 相対 / vault の中の絶対パス: core の resolve_path の検査（`..`・隠し・symlink の外出し・実在・ファイル）
/// - vault の外の絶対パス（本文の `file://`）: canonicalize して実在する通常ファイルだけ（ディレクトリ・.app 等の
///   バンドル・存在しない物は拒否）。`.` 始まりの区画を含むものは Finder で表示に落とす
/// - どちらも許可一覧の外の拡張子・実行ビット付き（wiki_reveal_only）・「このアプリで開く」の拡張属性付きは Finder で表示に落とす
pub(crate) fn wiki_open_target(root: &Path, path: &str, en: bool) -> Result<(PathBuf, bool), String> {
    use std::os::unix::fs::PermissionsExt;
    let given = Path::new(path.trim());
    let outside = given.is_absolute() && openref_core::wiki::strip_root(root, given).is_none();
    let (abs, hidden) = if outside {
        let canon = std::fs::canonicalize(given).map_err(|_| {
            format!(
                "{}: {}",
                tr(en, "見つかりません", "not found"),
                given.display()
            )
        })?;
        let hidden = openref_core::wiki::paths::dot_component(&canon);
        (canon, hidden)
    } else {
        (
            openref_core::wiki::commands::resolve_entry(root, path, true)
                .map_err(err)?
                .1,
            false,
        )
    };
    let meta = std::fs::metadata(&abs).map_err(err)?;
    if meta.is_dir() {
        // iWork のパッケージ形式（ディレクトリ）は Finder で表示するだけ。それ以外のディレクトリ・バンドルは拒否
        let ext = abs
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if WIKI_PACKAGE_EXT.contains(&ext.as_str()) {
            return Ok((abs, true));
        }
        return Err(wiki_not_regular_file(en, abs.display()));
    }
    if !meta.is_file() {
        return Err(wiki_not_regular_file(en, abs.display()));
    }
    Ok((
        abs.clone(),
        hidden || wiki_reveal_only(&abs, meta.permissions().mode()) || has_open_with_xattr(&abs),
    ))
}
