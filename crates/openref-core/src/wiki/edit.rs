//! 編集の command: `wiki_write`（`write_agent`）`wiki_move`（登録表にもある）と GUI 専用の `wiki_save`（`write`）
//! `wiki_create` `wiki_mkdir` `wiki_rename_preview` `wiki_rename` `wiki_trash`（ゴミ箱へ移す処理は呼び手 = app crate が渡す）
//! `wiki_save_blob` `wiki_import_file`。形は読みの command と同じ `fn(&Ctx, &Value) -> Result<Value>`（Db を要しない）。
//!
//! - ファイルの書込は `with_index` の f の外で行い、f の中は `refresh_paths`（冪等 = 壊れた索引のやり直しで 2 回走っても同じ）だけ。
//!   GUI の監視が同じ書込を拾ってもう一度 refresh_paths しても結果は同じ（一時ファイルは `.` 始まり = 監視・走査が無視する）
//! - 書込は同じフォルダの一時ファイル → rename（原子的・既存の mode を保つ）。新しいファイルを置くのは `renamex_np(RENAME_EXCL)`
//!   （同名があれば上書きせずに次の名前）
//! - 退避: `<app_dir>/wiki-history/<vault の root と相対パスの sha256 先頭 16>/<unix 秒>.<ext>`・ファイルごとに新しい 10 世代。
//!   同じフォルダの `path.txt` に元のファイルの絶対パス（復元の手掛かり）。退避するのは「照合なしの上書き（if_sha256 なし）・force・
//!   改名のリンク書換え・登録表（MCP / CLI）経由の上書き全部（`write_agent`）」。if_sha256 が一致した GUI の通常の保存（自動保存）は退避しない
//! - 拒否: ワークスペースの外・`..`・`.` 始まりの区画・symlink（そのものへの書込・ワークスペースの外へ出る親）・write は md / html 以外

use super::commands::{bad, index_path_for, opt_str, rebuilding, resolve_path, Ctx};
use super::index::{sha256_hex, with_index, Fresh, BODY_MAX};
use super::links::{normalize_rel, LinkKind, Resolver};
use super::parse_md;
use super::scan::{self, Kind};
use crate::{CoreError, Result};
use percent_encoding::percent_decode_str;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// 上書き前の退避の置き場（app_dir の直下）
pub const HISTORY_DIR: &str = "wiki-history";
/// 退避で残す世代（ファイルごと）
pub const HISTORY_KEEP: usize = 10;
/// wiki_save_blob の上限（デコード後）
pub const BLOB_MAX: usize = 50 * 1024 * 1024;
/// 貼付・取込の置き場（vault 相対）
pub const ATTACH_DIR: &str = "attachments";

fn req_str(a: &Value, k: &str, cmd: &str) -> Result<String> {
    opt_str(a, k)?.ok_or_else(|| bad(format!("{cmd}: 引数 {k} は必須です")))
}

fn opt_bool(a: &Value, k: &str) -> Result<Option<bool>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(format!("{k} は true / false で渡してください"))),
    }
}

/// 書込・取込の後に索引へ取り込む（失敗してもファイル操作は済んでいる = エラー文を返して結果に載せる）
fn reindex(ip: &Path, root: &Path, rels: &[&str]) -> Option<String> {
    with_index(ip, root, |ix| ix.refresh_paths(rels)).err().map(|e| e.to_string())
}

fn with_index_error(mut v: Value, e: Option<String>) -> Value {
    if let Some(e) = e {
        v["index_error"] = json!(e);
    }
    v
}

fn app_dir(ctx: &Ctx) -> Result<&Path> {
    ctx.app_dir.as_deref().ok_or_else(|| CoreError::wiki(super::codes::UNAVAILABLE, "in-memory の DB では OpenNote を使えません"))
}

// ---------------- パスの検査 ----------------

/// 書く（作る・移す）先
#[derive(Debug, Clone)]
pub struct Target {
    /// 渡された表記の vault 相対（親は実在するフォルダの表記に揃える・ファイル名は渡されたまま）
    pub rel: String,
    pub abs: PathBuf,
    /// 親フォルダ（無ければ書く直前に作る）
    pub parent: PathBuf,
    /// 既にある（通常ファイル）なら、その実体の (vault 相対, 絶対)（大小を区別しないボリュームで表記が違うことがある）
    pub existing: Option<(String, PathBuf)>,
    /// 書く直前に作るフォルダの名前（親の区画のうち無いもの）
    pub missing: Vec<String>,
    /// ファイル名（渡されたまま）
    pub name: String,
}

/// 新しく作るファイル・フォルダの名前に使えない字（Obsidian がリンクに使えない `[ ] | # ^` と、区切りになる `\` `:`）
pub const NAME_FORBIDDEN: &[char] = &['[', ']', '|', '#', '^', '\\', ':'];

/// 新しく作る名前の検査（既存のファイルを読む・上書きするときは呼ばない）
pub fn check_new_name(name: &str) -> Result<()> {
    if let Some(c) = name.chars().find(|c| NAME_FORBIDDEN.contains(c)) {
        return Err(bad(format!("名前に `{c}` は使えません（`[ ] | # ^ \\ :` はリンクが壊れる）: {name}")));
    }
    Ok(())
}

impl Target {
    /// 作るフォルダとファイル名（name_is_new = ファイル名も新しく作る）の名前を検査する
    pub fn check_new(&self, name_is_new: bool) -> Result<()> {
        for m in &self.missing {
            check_new_name(m)?;
        }
        if name_is_new {
            check_new_name(&self.name)?;
        }
        Ok(())
    }
}

/// 新しく作る / 上書きする / 移す先のパス（vault 相対かワークスペースの中の絶対パス）を検査する。
/// 拒否: `..`・`.` 始まりの区画・制御文字・ワークスペースの外（実在する親を canonicalize して判定 = symlink で外へ出ない）・
/// symlink そのもの・フォルダ
pub fn target_path(root: &Path, p: &str) -> Result<Target> {
    let p: String = p.trim().nfc().collect();
    let given = Path::new(&p);
    let rel: String = if given.is_absolute() {
        super::strip_root(root, given).ok_or_else(|| bad(format!("ワークスペースの外には書けません: {p}")))?
    } else {
        p.clone()
    };
    let mut parts: Vec<String> = Vec::new();
    for c in Path::new(&rel).components() {
        match c {
            Component::Normal(s) => {
                let s = s.to_string_lossy();
                if s.starts_with('.') || s.chars().any(char::is_control) {
                    return Err(bad(format!("パスに `.` 始まりの区画・制御文字は使えません: {p}")));
                }
                parts.push(s.into_owned());
            }
            Component::CurDir => {}
            _ => return Err(bad(format!("パスに `..`・root は使えません: {p}"))),
        }
    }
    let name = parts.pop().ok_or_else(|| bad(format!("ファイル名がありません: {p}")))?;
    // 実在する最も深い親を canonicalize してワークスペースの中か確かめる（無い区画は後で作る = symlink ではありえない）
    let mut anc = root.to_path_buf();
    let mut missing: Vec<String> = Vec::new();
    for seg in &parts {
        if missing.is_empty() {
            let next = anc.join(seg);
            match std::fs::symlink_metadata(&next) {
                Ok(_) => anc = next,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing.push(seg.clone()),
                Err(e) => return Err(e.into()),
            }
        } else {
            missing.push(seg.clone());
        }
    }
    let canon = std::fs::canonicalize(&anc).map_err(|_| bad(format!("フォルダを辿れません（壊れた symlink 等）: {p}")))?;
    let rel_anc = super::strip_root(root, &canon).ok_or_else(|| bad(format!("ワークスペースの外を指しています（symlink）: {p}")))?;
    if scan::hidden(&rel_anc) {
        return Err(bad(format!("このフォルダには書けません: {p}")));
    }
    if !canon.is_dir() {
        return Err(bad(format!("途中の区画がフォルダではありません: {p}")));
    }
    let mut parent = canon.clone();
    let mut rel_parent = rel_anc;
    for seg in &missing {
        parent.push(seg);
        rel_parent = if rel_parent.is_empty() { seg.clone() } else { format!("{rel_parent}/{seg}") };
    }
    let abs = parent.join(&name);
    let rel = if rel_parent.is_empty() { name.clone() } else { format!("{rel_parent}/{name}") };
    let existing = match std::fs::symlink_metadata(&abs) {
        Ok(m) if m.file_type().is_symlink() => return Err(bad(format!("symlink には書けません: {p}"))),
        Ok(m) if m.is_dir() => return Err(bad(format!("同じ名前のフォルダがあります: {p}"))),
        Ok(_) => {
            let c = std::fs::canonicalize(&abs)?;
            let r = super::strip_root(root, &c).ok_or_else(|| bad(format!("ワークスペースの外を指しています: {p}")))?;
            Some((r, c))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    Ok(Target { rel, abs, parent, existing, missing, name })
}

/// 渡されたパスそのもの（vault 相対の表記）が symlink なら拒否（resolve_path は symlink の先を返す = 先を消す・移すことになる）
pub(super) fn reject_symlink(root: &Path, given: &str) -> Result<()> {
    let g: String = given.trim().nfc().collect();
    let rel = if Path::new(&g).is_absolute() { super::strip_root(root, Path::new(&g)).unwrap_or_default() } else { g };
    // 末尾の `/` は落として lstat する（`alias/` だと lstat が symlink を辿り、先の本物のフォルダを返す）
    let rel = rel.trim_start_matches('/').trim_end_matches('/');
    if std::fs::symlink_metadata(root.join(rel)).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(bad(format!("symlink は扱えません: {given}")));
    }
    Ok(())
}

// ---------------- ファイル操作 ----------------

fn tmp_name(dir: &Path) -> PathBuf {
    dir.join(format!(".openref-{}-{:016x}.tmp", std::process::id(), fastrand::u64(..)))
}

/// プロセスの中の編集（write / move / rename / create / mkdir / trash / save_blob / import_file）を直列にする
static EDIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn edit_lock() -> std::sync::MutexGuard<'static, ()> {
    EDIT.lock().unwrap_or_else(|e| e.into_inner())
}

/// 書く前に見たファイルの状態（rename の直前に照合し直す）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    ino: u64,
    mtime_ns: i64,
    size: u64,
}

/// 今のファイルの状態（通常ファイルでなければ None）
pub fn seen(abs: &Path) -> Option<Seen> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(abs).ok()?;
    m.file_type().is_file().then(|| Seen { ino: m.ino(), mtime_ns: m.mtime() * 1_000_000_000 + m.mtime_nsec(), size: m.size() })
}

/// write_atomic の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrote {
    Ok,
    /// 書く間に外部が変えた（expect と違う・新規のはずが既にある）= 書いていない
    Changed,
}

/// 一時ファイルに書いて rename（原子的）。expect = 書く前に見た既存のファイル（None = 新規）。
/// 既存: 書込権が無ければ拒否・mode と拡張属性 / ACL を引き継ぐ・rename の直前に expect と照合し直し、違えば Changed。
/// 新規: RENAME_EXCL で置く（非対応の fs は O_EXCL で写す = 原子的でない。間に外部が同名を作っていれば Changed = 上書きしない）
pub fn write_atomic(abs: &Path, bytes: &[u8], expect: Option<Seen>) -> Result<Wrote> {
    let dir = abs.parent().ok_or_else(|| bad("親フォルダがありません"))?;
    if expect.is_some() && !writable(abs) {
        return Err(bad(format!("書込権がありません（読み取り専用）: {}", abs.display())));
    }
    let tmp = tmp_name(dir);
    let r = (|| -> std::io::Result<Wrote> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        if expect.is_some() {
            f.set_permissions(std::fs::metadata(abs)?.permissions())?;
            copy_xattr_acl(abs, &tmp)?;
        }
        f.sync_all()?;
        match expect {
            Some(want) => {
                if seen(abs) != Some(want) {
                    return Ok(Wrote::Changed);
                }
                std::fs::rename(&tmp, abs)?;
                Ok(Wrote::Ok)
            }
            None => match rename_excl(&tmp, abs) {
                Ok(()) => Ok(Wrote::Ok),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(Wrote::Changed),
                Err(e) => Err(e),
            },
        }
    })();
    if !matches!(r, Ok(Wrote::Ok)) {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(r?)
}

fn writable(abs: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(abs.as_os_str().as_bytes()) else { return false };
    // SAFETY: NUL 終端の C 文字列を渡すだけ
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// 拡張属性（Finder のタグ等）と ACL を写す（mode は呼び手・時刻は写さない = 索引が mtime で変化を見る）
fn copy_xattr_acl(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let f = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        let t = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        // SAFETY: NUL 終端の C 文字列と null の state を渡すだけ
        if unsafe { libc::copyfile(f.as_ptr(), t.as_ptr(), std::ptr::null_mut(), libc::COPYFILE_XATTR | libc::COPYFILE_ACL) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (from, to);
    Ok(())
}

/// from を to へ（to が既にあれば失敗 = 上書きしない）
fn rename_excl(from: &Path, to: &Path) -> std::io::Result<()> {
    rename_excl_with(from, to, renamex_excl, |f, t| std::fs::hard_link(f, t))
}

/// renamex_np(RENAME_EXCL)。macOS 以外は「非対応」= rename_excl_with が hard link へ進む
fn renamex_excl(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let f = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        let t = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        // SAFETY: NUL 終端の C 文字列を渡すだけ
        if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to);
        Err(std::io::Error::from_raw_os_error(libc::ENOTSUP))
    }
}

/// rename_excl の本体: RENAME_EXCL → 非対応なら hard link + remove → それも非対応なら O_EXCL で作って中身を写す。
/// どの段も to が既にあれば AlreadyExists（上書きしない）。excl / link は注入点（cfg(test) の旗は並列のテストへ漏れるので引数）
fn rename_excl_with(
    from: &Path,
    to: &Path,
    excl: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    link: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let code = |e: &std::io::Error, ok: &[i32]| e.raw_os_error().is_some_and(|c| ok.contains(&c));
    // exFAT は RENAME_EXCL が ENOTSUP（実測）
    match excl(from, to) {
        Err(e) if code(&e, &[libc::ENOTSUP, libc::EOPNOTSUPP, libc::EINVAL]) => {}
        r => return r,
    }
    // exFAT / FAT32 は hard link が ENOTSUP（実測）
    match link(from, to) {
        Ok(()) => {
            let _ = std::fs::remove_file(from); // 置けた = 済み（一時ファイルの掃除は最善努力）
            return Ok(());
        }
        Err(e) if code(&e, &[libc::ENOTSUP, libc::EOPNOTSUPP, libc::EPERM]) => {}
        Err(e) => return Err(e),
    }
    // 原子的でない（書き終えるまで途中の中身が見えうる）。作れなければ（同名がある等）何も置かない。mode と拡張属性 / ACL は最善努力
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    let r = std::fs::File::open(from)
        .and_then(|mut f| std::io::copy(&mut f, &mut out))
        .and_then(|_| {
            let _ = std::fs::metadata(from).and_then(|m| out.set_permissions(m.permissions()));
            let _ = copy_xattr_acl(from, to);
            out.sync_all()
        });
    drop(out);
    if let Err(e) = r {
        let _ = std::fs::remove_file(to); // 自分が作った途中のファイル
        return Err(e);
    }
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// dir に name（同名があれば "name 2.ext" "name 3.ext" …）で新しいファイルを置く。中身は fill が一時ファイルに書く。戻り値 = 置いた名前
fn place_new(dir: &Path, name: &str, fill: impl FnOnce(&Path) -> std::io::Result<()>) -> Result<String> {
    std::fs::create_dir_all(dir)?;
    let tmp = tmp_name(dir);
    if let Err(e) = fill(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    let _g = edit_lock(); // 中身を書き終えてから（大きなコピーの間に他の編集を止めない）
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    for n in 1..=999 {
        let cand = if n == 1 { name.to_string() } else { format!("{stem} {n}{ext}") };
        match rename_excl(&tmp, &dir.join(&cand)) {
            Ok(()) => return Ok(cand),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(CoreError::Conflict(format!("同じ名前のファイルが多すぎます: {name}")))
}

// ---------------- 退避 ----------------

/// このファイル（vault の root + vault 相対）の退避の置き場。鍵は `<root>\0<rel>` の sha256 の先頭 16（vault を替えても世代が混ざらない）
pub fn history_dir(app_dir: &Path, root: &Path, rel: &str) -> PathBuf {
    let key = format!("{}\0{rel}", root.to_string_lossy().nfc().collect::<String>());
    app_dir.join(HISTORY_DIR).join(&sha256_hex(key.as_bytes())[..16])
}

/// 上書き・書換えの前の内容を退避して、新しい HISTORY_KEEP 世代だけ残す（今作った退避は消さない）。戻り値 = 退避したファイル。
/// 同じフォルダの path.txt = 元のファイルの絶対パス
pub fn backup(app_dir: &Path, root: &Path, rel: &str, bytes: &[u8]) -> Result<PathBuf> {
    let path = stage_backup(app_dir, root, rel, bytes)?;
    prune(&history_dir(app_dir, root, rel), &path)?;
    Ok(path)
}

/// 退避を置くだけ（古い世代は刈り込まない）。上書きが成功してから `prune` する呼び手用（書けなかったら置いた退避を消せば
/// 世代は元のまま = 最古の世代を失わない）
fn stage_backup(app_dir: &Path, root: &Path, rel: &str, bytes: &[u8]) -> Result<PathBuf> {
    let dir = history_dir(app_dir, root, rel);
    std::fs::create_dir_all(&dir)?;
    let note = dir.join("path.txt");
    let abs = format!("{}\n", root.join(rel).to_string_lossy());
    if std::fs::read_to_string(&note).ok().as_deref() != Some(abs.as_str()) {
        std::fs::write(&note, &abs)?;
    }
    let ext = rel.rsplit('/').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_else(|| "bak".into());
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // 同じ秒の世代は番号を続ける（消した古い番号を使い直すと新しいのに古い順に並ぶ）
    let mut n = history_files(&dir)
        .iter()
        .filter_map(|p| hist_key(p))
        .filter(|(s, _)| *s == secs)
        .map(|(_, n)| n + 1)
        .max()
        .unwrap_or(1);
    let path = loop {
        let name = if n == 1 { format!("{secs}.{ext}") } else { format!("{secs}-{n}.{ext}") };
        let p = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
                    drop(f);
                    let _ = std::fs::remove_file(&p); // 書けなかった退避は残さない（世代に数えない）
                    return Err(e.into());
                }
                break p;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 100_000 => n += 1,
            Err(e) => return Err(e.into()),
        }
    };
    Ok(path)
}

/// 退避のファイル名 `<秒>.<ext>` / `<秒>-<n>.<ext>` → (秒, n)
fn hist_key(p: &Path) -> Option<(u64, u32)> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    let stem = name.split('.').next()?;
    let (s, n) = stem.split_once('-').unwrap_or((stem, "1"));
    Some((s.parse().ok()?, n.parse().ok()?))
}

/// 退避の世代（新しい順）
pub fn history_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<((u64, u32), PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((hist_key(&e.path())?, e.path())))
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.0));
    v.into_iter().map(|(_, p)| p).collect()
}

/// 新しい HISTORY_KEEP 世代を残す。keep（今作った退避）は時計が戻って古い順に並んでも消さない
fn prune(dir: &Path, keep: &Path) -> Result<()> {
    let others: Vec<PathBuf> = history_files(dir).into_iter().filter(|p| p != keep).collect();
    for p in others.into_iter().skip(HISTORY_KEEP - 1) {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

// ---------------- command ----------------

fn sha_arg(a: &Value) -> Result<Option<String>> {
    let s = opt_str(a, "if_sha256")?.map(|s| s.trim().to_ascii_lowercase());
    if let Some(h) = &s {
        if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad("if_sha256 は sha256 の 16 進 64 文字で渡してください"));
        }
    }
    Ok(s)
}

/// GUI の `wiki_save {path, content, if_sha256?, force?}`（エディタの保存）: md / html を書く（新規はフォルダも作る）。
/// if_sha256 が今のファイル（無ければ null）と違えば書かずに `{ok:false, conflict:true, path, sha256}`。
/// force = 照合を飛ばす。既存を照合なし（if_sha256 なし）か force で上書きするときは上書き前を退避する
/// （if_sha256 が一致した通常の保存 = 自動保存は退避しない = 世代の押し出しを防ぐ）。中身が同じなら書かない
pub fn write(ctx: &Ctx, a: &Value) -> Result<Value> {
    write_with(ctx, a, false)
}

/// 登録表（MCP / CLI）の `wiki_write {path, content, if_sha256?}`: GUI の write と同じだが、既存を上書きするときは
/// if_sha256 の有無によらず必ず上書き前を退避して `backup` を返す（agent の書込は全部戻せる = チャットの [元に戻す]）
pub fn write_agent(ctx: &Ctx, a: &Value) -> Result<Value> {
    write_with(ctx, a, true)
}

fn write_with(ctx: &Ctx, a: &Value, always_backup: bool) -> Result<Value> {
    let path = req_str(a, "path", "wiki_write")?;
    let content = match a.get("content") {
        Some(Value::String(s)) => s.clone(),
        None | Some(Value::Null) => return Err(bad("wiki_write: 引数 content は必須です")),
        Some(_) => return Err(bad("content は文字列で渡してください")),
    };
    let if_sha = sha_arg(a)?;
    let force = opt_bool(a, "force")?.unwrap_or(false);
    let (ip, root) = ctx.ready()?;
    let app = app_dir(ctx)?;
    let _g = edit_lock();
    let t = target_path(&root, &path)?;
    t.check_new(t.existing.is_none())?;
    let (rel, abs) = t.existing.clone().unwrap_or_else(|| (t.rel.clone(), t.abs.clone()));
    if !Kind::of(&rel).has_body() {
        return Err(bad(format!("wiki_write は md / html だけ書けます: {rel}")));
    }
    if content.len() as i64 > BODY_MAX {
        return Err(bad(format!("大きすぎます（{} バイト・上限 {BODY_MAX} バイト）", content.len())));
    }
    // 読む前に状態を見ておき、rename の直前に照合し直す（読んでから書くまでの間の外部の書換えも conflict にする）
    let before = match &t.existing {
        Some(_) => Some(seen(&abs).ok_or_else(|| bad(format!("通常ファイルではありません: {rel}")))?),
        None => None,
    };
    let current = match before {
        Some(_) => Some(std::fs::read(&abs)?),
        None => None,
    };
    let cur_sha = current.as_deref().map(sha256_hex);
    let conflict = |rel: &str| -> Value {
        let now = std::fs::read(&abs).ok().map(|b| sha256_hex(&b));
        json!({"ok": false, "conflict": true, "path": rel, "sha256": now})
    };
    if !force {
        if let Some(want) = &if_sha {
            if cur_sha.as_deref() != Some(want.as_str()) {
                return Ok(json!({"ok": false, "conflict": true, "path": rel, "sha256": cur_sha}));
            }
        }
    }
    let mut backed_up = Value::Null;
    let mut history_error = None;
    if current.as_deref() != Some(content.as_bytes()) {
        // 書けないファイルは退避より先に拒否する（退避だけ増えて本物の世代を押し出さない）
        if current.is_some() && !writable(&abs) {
            return Err(bad(format!("書込権がありません（読み取り専用）: {rel}")));
        }
        if let Some(cur) = &current {
            if always_backup || force || if_sha.is_none() {
                // 刈り込みは書けてから（書込に失敗して退避を消したとき、最古の世代を失わない）
                backed_up = json!(stage_backup(app, &root, &rel, cur)?.to_string_lossy());
            }
        }
        // 書けなかった（遅い競合 = 照合し直しで Changed・I/O の失敗）なら上書きしていない = 作った退避を消す（世代を使わない）
        let drop_backup = || {
            if let Some(b) = backed_up.as_str() {
                let _ = std::fs::remove_file(b);
            }
        };
        let wrote = std::fs::create_dir_all(&t.parent).map_err(CoreError::from).and_then(|()| write_atomic(&abs, content.as_bytes(), before));
        match wrote {
            Ok(Wrote::Ok) => {
                if let Some(b) = backed_up.as_str() {
                    if let Err(e) = prune(&history_dir(app, &root, &rel), Path::new(b)) {
                        history_error = Some(format!("古い退避を刈り込めません: {e}"));
                    }
                }
            }
            Ok(Wrote::Changed) => {
                drop_backup();
                return Ok(conflict(&rel));
            }
            Err(e) => {
                drop_backup();
                return Err(e);
            }
        }
    }
    let ie = reindex(&ip, &root, &[rel.as_str()]);
    let (mtime_ns, size) = scan::stat(&abs).unwrap_or((0, content.len() as i64));
    let mut out = json!({
        "ok": true, "path": rel, "sha256": sha256_hex(content.as_bytes()), "size": size,
        "mtime": mtime_ns / 1_000_000_000, "created": current.is_none(),
    });
    if !backed_up.is_null() {
        out["backup"] = backed_up;
    }
    if let Some(e) = history_error {
        out["history_error"] = json!(e);
    }
    Ok(with_index_error(out, ie))
}

/// `wiki_create {dir?, name?}`: 空のページを作る（既定 "Untitled.md"・同名があれば "Untitled 2.md" …・md / html 以外の名前は .md を足す）
pub fn create(ctx: &Ctx, a: &Value) -> Result<Value> {
    let dir = opt_str(a, "dir")?.unwrap_or_default();
    let name: String = opt_str(a, "name")?.unwrap_or_else(|| "Untitled.md".into()).trim().nfc().collect();
    if name.contains('/') || name.starts_with('.') || name.chars().any(char::is_control) {
        return Err(bad(format!("wiki_create: name に `/`・`.` 始まり・制御文字は使えません: {name}")));
    }
    let kind = Kind::of(&name);
    let name = if kind.has_body() { name } else { format!("{name}.md") };
    let (ip, root) = ctx.ready()?;
    let dir = dir.trim().trim_end_matches('/').to_string();
    let joined = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
    let t = target_path(&root, &joined)?;
    t.check_new(true)?;
    // md は空・html は最小の骨格（<title> は書かない = 題名はファイル名から・GUI は作った後に改名することがある）
    let body: &[u8] = if kind == Kind::Html { HTML_SKELETON.as_bytes() } else { b"" };
    let placed = place_new(&t.parent, &name, |tmp| std::fs::write(tmp, body))?; // edit_lock は place_new が置くときに取る
    let rel = match t.rel.rsplit_once('/') {
        Some((d, _)) => format!("{d}/{placed}"),
        None => placed,
    };
    let ie = reindex(&ip, &root, &[rel.as_str()]);
    Ok(with_index_error(json!({"path": rel}), ie))
}

/// `wiki_create` が html のページに書く骨格
pub const HTML_SKELETON: &str = "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n</head>\n<body>\n\n</body>\n</html>\n";

/// `wiki_mkdir {dir?, name}`: 空のフォルダを作る（GUI の [新規フォルダ]）。同じ名前のフォルダ / ファイルがあれば拒否・
/// 途中の無いフォルダは作る。索引には何も入らない（ページが無い）
pub fn mkdir(ctx: &Ctx, a: &Value) -> Result<Value> {
    let dir = opt_str(a, "dir")?.unwrap_or_default();
    let name: String = req_str(a, "name", "wiki_mkdir")?.trim().nfc().collect();
    if name.is_empty() {
        return Err(bad("wiki_mkdir: name が空です"));
    }
    if name.contains('/') || name.starts_with('.') || name.chars().any(char::is_control) {
        return Err(bad(format!("wiki_mkdir: name に `/`・`.` 始まり・制御文字は使えません: {name}")));
    }
    let (_ip, root) = ctx.ready()?;
    let dir = dir.trim().trim_end_matches('/').to_string();
    let joined = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
    let t = target_path(&root, &joined)?; // 同じ名前のフォルダ・symlink・ワークスペースの外はここで拒否
    if let Some((r, _)) = &t.existing {
        return Err(CoreError::Conflict(format!("同じ名前のファイルがあります: {r}")));
    }
    t.check_new(true)?;
    let _g = edit_lock();
    std::fs::create_dir_all(&t.parent)?;
    // 最後の 1 段は create_dir（create_dir_all は検査の後に他のプロセスが同名のフォルダ / フォルダへの symlink を置いても Ok を返す）
    std::fs::create_dir(&t.abs).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => CoreError::Conflict(format!("同じ名前のフォルダかファイルがあります: {}", t.rel)),
        _ => e.into(),
    })?;
    Ok(json!({"path": t.rel}))
}

/// from（実在・resolve_path 済み）を t へ移す。t が既にあれば拒否（大小だけ違う同じ実体 = 大小の改名は可）。
/// 戻り値 = 大小だけの改名だった（旧い表記も stat が通る = refresh_paths では索引から外れない → 呼び手が stat 走査にする）
fn do_move(from_abs: &Path, t: &Target) -> Result<bool> {
    if let Some((_, canon)) = &t.existing {
        if canon.as_path() != from_abs {
            return Err(CoreError::Conflict(format!("移動先が既にあります: {}", t.rel)));
        }
        if t.abs.file_name() == from_abs.file_name() {
            return Err(bad("移動元と移動先が同じです"));
        }
        std::fs::rename(from_abs, &t.abs)?;
        return Ok(true);
    }
    std::fs::create_dir_all(&t.parent)?;
    rename_excl(from_abs, &t.abs).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => CoreError::Conflict(format!("移動先が既にあります: {}", t.rel)),
        _ => e.into(),
    })?;
    Ok(false)
}

/// 移動の後の索引: 大小だけの改名は stat 走査（旧い表記の行を外す）+ from 以外の触ったパス
/// （旧い表記も大小無視の stat が通る = refresh_paths に渡すと旧い表記の行を入れ直してしまう）。それ以外は from を含む触ったパスだけ
fn reindex_move(ip: &Path, root: &Path, case_only: bool, from: &str, rels: &[&str]) -> Option<String> {
    if !case_only {
        let mut all = vec![from];
        all.extend_from_slice(rels);
        return reindex(ip, root, &all);
    }
    let rest: Vec<&str> = rels.iter().copied().filter(|r| *r != from).collect();
    with_index(ip, root, |ix| ix.refresh().map(|_| ()).and_then(|()| ix.refresh_paths(&rest).map(|_| ()))).err().map(|e| e.to_string())
}

/// `wiki_move {from, to}`: ワークスペースの中の移動 / 改名（移動先が既にあれば拒否・リンクは書き換えない = skill の責任）
pub fn move_(ctx: &Ctx, a: &Value) -> Result<Value> {
    let from = req_str(a, "from", "wiki_move")?;
    let to = req_str(a, "to", "wiki_move")?;
    let (ip, root) = ctx.ready()?;
    let _g = edit_lock();
    reject_symlink(&root, &from)?;
    let (from_rel, from_abs) = resolve_path(&root, &from)?;
    let t = target_path(&root, &to)?;
    // 拡張子（大小無視）を変える移動は拒否（md ↔ html・ページ ↔ 画像等 = 種類が変わる。GUI の改名 wiki_rename は別の経路）
    if ext_of(&from_rel) != ext_of(&t.rel) {
        return Err(bad(format!("wiki_move では拡張子を変えられません: {from_rel} → {}", t.rel)));
    }
    // 名前を変えない移動（inbox → sources/ 等）は既存の名前を検査しない（作るフォルダだけ）
    let same_name = from_abs.file_name().map(|n| n.to_string_lossy().nfc().collect::<String>()) == Some(t.name.clone());
    t.check_new(!same_name)?;
    let case_only = do_move(&from_abs, &t)?;
    let ie = reindex_move(&ip, &root, case_only, &from_rel, &[t.rel.as_str()]);
    // path = 移動後（GUI の形）・to = 同じ値（from と対で移動の記録 = 逆の move で戻せる）
    Ok(with_index_error(json!({"from": from_rel, "to": t.rel, "path": t.rel}), ie))
}

// ---------------- 改名とリンクの書換え ----------------

/// 1 ファイルの書換え
#[derive(Debug)]
struct Rewrite {
    /// 改名の前のパス（索引の表記）
    src: String,
    /// 改名の後のパス（改名するファイル自身なら to）
    src_new: String,
    before_sha: String,
    after: Vec<u8>,
    links: usize,
}

#[derive(Debug)]
struct RenamePlan {
    from_rel: String,
    from_abs: PathBuf,
    to: Target,
    rewrites: Vec<Rewrite>,
    /// 書き換えない参照（html のページ・file:// のリンク・読めないページ）: (src, kind)
    other: Vec<(String, String)>,
}

fn ext_of(p: &str) -> String {
    p.rsplit('/').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn dir_of(p: &str) -> &str {
    p.rfind('/').map(|i| &p[..i]).unwrap_or("")
}

/// 書いた形が拡張子を持っていたか（`name` の basename が old の拡張子で終わる）。md 以外は常に拡張子付きで書く（Obsidian の規則）
fn keep_ext(written: &str, old: &str, new: &str) -> bool {
    let e = ext_of(old);
    let had = !e.is_empty() && file_name(written).to_lowercase().ends_with(&format!(".{e}"));
    had || !matches!(ext_of(new).as_str(), "md")
}

fn shaped(p: &str, with_ext: bool) -> String {
    if with_ext {
        return p.to_string();
    }
    match p.rsplit_once('.') {
        Some((s, e)) if e.eq_ignore_ascii_case("md") && !file_name(s).is_empty() => s.to_string(),
        _ => p.to_string(),
    }
}

/// from_dir（vault 相対のフォルダ）から to（vault 相対のファイル）への相対パス
fn relpath(from_dir: &str, to: &str) -> String {
    let a: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let b: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut out: Vec<&str> = vec![".."; a.len() - common];
    out.extend(&b[common..]);
    out.join("/")
}

/// md リンクの dest に書く（`<…>` の中ならそのまま。それ以外は空白・括弧・`<>#?%` を % に。元が非 ASCII を % にしていたら非 ASCII も）
fn encode_dest(p: &str, angle: bool, full: bool) -> String {
    if angle {
        return p.to_string();
    }
    let mut out = String::with_capacity(p.len());
    for c in p.chars() {
        let enc = matches!(c, ' ' | '(' | ')' | '<' | '>' | '#' | '?' | '%' | '\\') || c.is_control() || full && !c.is_ascii();
        if enc {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// md リンクの dest（`#` `?` の前・% を解いたもの）が src のフォルダからの相対で path に当たるか（拡張子省略も）
fn md_rel_hit(src: &str, decoded: &str, path: &str) -> bool {
    !decoded.starts_with('/')
        && normalize_rel(&format!("{}/{}", dir_of(src), decoded)).is_some_and(|j| j.eq_ignore_ascii_case(path) || format!("{j}.md").eq_ignore_ascii_case(path))
}

fn md_decoded(raw: &str) -> String {
    let cut = raw.find(['#', '?']).unwrap_or(raw.len());
    percent_decode_str(&raw[..cut]).decode_utf8_lossy().nfc().collect()
}

/// 1 本のリンクの新しい表記（置き換える範囲 = Span::range の中身）。形（basename / 拡張子の有無 / パス指定 / 相対 / `/` 始まり）を保ち、
/// 改名後の解決器で want に一意に解決できる最初の候補を採る（basename が同名で曖昧になるならパス指定へ）
fn new_link_text(new: &Resolver, src_new: &str, span: &parse_md::Span, text: &str, src_old: &str, old: &str, want: &str) -> String {
    let raw = &text[span.range.clone()];
    let ok = |cand: &str, suffix: &str| {
        let r = new.resolve(src_new, span.kind, &format!("{cand}{suffix}"));
        r.path.as_deref() == Some(want) && !r.ambiguous
    };
    match span.kind {
        LinkKind::Wiki | LinkKind::Embed => {
            let suffix = span.target.split_once('#').map(|(_, h)| format!("#{h}")).unwrap_or_default();
            let form = shaped(want, keep_ext(raw, old, want));
            let mut cands = Vec::new();
            if !raw.contains('/') {
                cands.push(file_name(&form).to_string());
            }
            cands.push(form.clone());
            cands.push(want.to_string());
            cands.into_iter().find(|c| ok(c, &suffix)).unwrap_or(form)
        }
        _ => {
            let angle = span.range.start > 0 && text.as_bytes()[span.range.start - 1] == b'<';
            let cut = raw.find(['#', '?']).unwrap_or(raw.len());
            let (path_part, suffix) = raw.split_at(cut);
            let decoded: String = percent_decode_str(path_part).decode_utf8_lossy().nfc().collect();
            let full = decoded != path_part && !decoded.is_ascii();
            let form = shaped(want, keep_ext(&decoded, old, want));
            let rel_hit = md_rel_hit(src_old, &decoded, old);
            let rel = relpath(dir_of(src_new), &form);
            let cands: Vec<String> = if decoded.starts_with('/') {
                vec![format!("/{form}")]
            } else if rel_hit {
                vec![rel.clone()]
            } else {
                vec![file_name(&form).to_string(), rel.clone()]
            };
            let pick = cands.iter().map(|c| encode_dest(c, angle, full)).find(|c| ok(c, suffix)).unwrap_or_else(|| encode_dest(&rel, angle, full));
            format!("{pick}{suffix}")
        }
    }
}

fn plan_rename(ctx: &Ctx, a: &Value, cmd: &str) -> Result<(PathBuf, PathBuf, RenamePlan)> {
    let from = req_str(a, "from", cmd)?;
    let to = req_str(a, "to", cmd)?;
    let (ip, root) = ctx.ready()?;
    reject_symlink(&root, &from)?;
    let (from_rel, from_abs) = resolve_path(&root, &from)?;
    let t = target_path(&root, &to)?;
    t.check_new(true)?;
    if let Some((_, canon)) = &t.existing {
        if canon != &from_abs {
            return Err(CoreError::Conflict(format!("移動先が既にあります: {}", t.rel)));
        }
    }
    if t.rel == from_rel {
        return Err(bad("移動元と移動先が同じです"));
    }
    let (from_ix, paths, links) = with_index(&ip, &root, |ix| {
        // 鮮度規則（5 秒）を待たずに必ず stat 走査 = 直前に外部が書いたリンクも数える
        if let Fresh::Rebuilding(p) = ix.refresh()? {
            return Err(rebuilding(p));
        }
        if index_path_for(ix, &from_rel, &from_abs)?.is_none() {
            ix.refresh_paths(&[from_rel.as_str()])?;
        }
        let from_ix = index_path_for(ix, &from_rel, &from_abs)?.unwrap_or_else(|| from_rel.clone());
        Ok((from_ix.clone(), ix.all_paths()?, ix.links_to(&from_ix)?))
    })?;
    let old = Resolver::new(&root, paths.iter().map(String::as_str));
    let new_paths: Vec<String> = paths.iter().map(|p| if *p == from_ix { t.rel.clone() } else { p.clone() }).collect();
    let new = Resolver::new(&root, new_paths.iter().map(String::as_str));
    // 書き換える候補 = from を指すリンクを持つ md のページ + 改名する md 自身（フォルダが変われば自分の相対リンクが外れる）。
    // チャットのログ（chat-log/）は追記だけ・上書きしない約束なので書き換えず other に出す（リンクは索引に残る = 改名後は未解決 = 履歴として可）
    let mut srcs: Vec<String> = Vec::new();
    let mut other: Vec<(String, String)> = Vec::new();
    for l in &links {
        let kind = if is_chat_log(&l.src) { "chat-log" } else { l.kind.as_str() };
        if l.kind == "file" || Kind::of(&l.src) != Kind::Md || is_chat_log(&l.src) {
            if !other.iter().any(|(s, k)| *s == l.src && k == kind) {
                other.push((l.src.clone(), kind.to_string()));
            }
        } else if !srcs.contains(&l.src) {
            srcs.push(l.src.clone());
        }
    }
    if Kind::of(&from_ix) == Kind::Md && !is_chat_log(&from_ix) && !srcs.contains(&from_ix) {
        srcs.push(from_ix.clone());
    }
    let mut rewrites = Vec::new();
    for src in srcs {
        let abs = root.join(&src);
        let bytes = match std::fs::metadata(&abs) {
            Ok(m) if (m.len() as i64) <= BODY_MAX => std::fs::read(&abs).ok(),
            _ => None,
        };
        let Some(text) = bytes.as_deref().and_then(|b| std::str::from_utf8(b).ok()) else {
            other.push((src.clone(), "unreadable".into()));
            continue;
        };
        let src_new = if src == from_ix { t.rel.clone() } else { src.clone() };
        let body_at = super::frontmatter::split(text).1;
        let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        for span in parse_md::link_spans(text) {
            if !matches!(span.kind, LinkKind::Wiki | LinkKind::Embed | LinkKind::Md) || span.range.is_empty() {
                continue;
            }
            let Some(op) = old.resolve(&src, span.kind, &span.target).path else { continue };
            let want = if op == from_ix { t.rel.clone() } else { op.clone() };
            if op != from_ix && src == from_ix && new.resolve(&src_new, span.kind, &span.target).path.as_deref() == Some(want.as_str()) {
                // 改名するページ自身の他へのリンクで、改名後もそのまま解ける（相対で当たっていた md リンクは相対のまま当たるときだけ）
                let d = md_decoded(&text[span.range.clone()]);
                if span.kind != LinkKind::Md || !md_rel_hit(&src, &d, &op) || md_rel_hit(&src_new, &d, &want) {
                    continue;
                }
            }
            if op != from_ix && src != from_ix {
                continue;
            }
            let rep = new_link_text(&new, &src_new, &span, text, &src, &op, &want);
            if span.range.end <= body_at && rep.contains(['"', '\'']) {
                // frontmatter の行で引用符を含む名前 = YAML の引用を壊しうるので書き換えず other に出す
                if !other.iter().any(|(s, k)| *s == src && k == "frontmatter") {
                    other.push((src.clone(), "frontmatter".into()));
                }
                continue;
            }
            if rep != text[span.range.clone()] {
                edits.push((span.range.clone(), rep));
            }
        }
        if edits.is_empty() {
            continue;
        }
        let mut after = text.to_string();
        for (r, rep) in edits.iter().rev() {
            after.replace_range(r.clone(), rep);
        }
        rewrites.push(Rewrite { src: src.clone(), src_new, before_sha: sha256_hex(text.as_bytes()), after: after.into_bytes(), links: edits.len() });
    }
    Ok((ip, root, RenamePlan { from_rel, from_abs, to: t, rewrites, other }))
}

/// チャットのログ（app crate の wiki_chat_log が `chat-log/` に追記だけする）。改名のリンク書換えの対象にしない
fn is_chat_log(rel: &str) -> bool {
    rel.starts_with("chat-log/")
}

fn other_json(other: &[(String, String)]) -> Value {
    Value::Array(other.iter().map(|(p, k)| json!({"path": p, "kind": k})).collect())
}

/// `wiki_rename_preview {from, to}`: 書き換える対象（リンクを持つページ）と件数。links = 書き換えるリンクの本数
/// （from を指す `[[…]]` `![[…]]` 相対リンク + フォルダが変わるときの改名するページ自身の相対リンク）。
/// other = 書き換えない参照（html のページ・file:// の絶対リンク・読めないページ・チャットのログ = kind "chat-log"）
pub fn rename_preview(ctx: &Ctx, a: &Value) -> Result<Value> {
    let (_, _, plan) = plan_rename(ctx, a, "wiki_rename_preview")?;
    Ok(json!({
        "from": plan.from_rel,
        "to": plan.to.rel,
        "links": plan.rewrites.iter().map(|r| r.links).sum::<usize>(),
        "files": plan.rewrites.iter().map(|r| r.src.clone()).collect::<Vec<_>>(),
        "other": other_json(&plan.other),
    }))
}

/// `wiki_rename {from, to, rewrite}`: 改名（移動先が既にあれば拒否）。rewrite なら各ファイルを退避してからリンクを書き換える
/// （計画の後にそのファイルが書き換わっていたら触らず skipped に載せる）
pub fn rename(ctx: &Ctx, a: &Value) -> Result<Value> {
    let rewrite = opt_bool(a, "rewrite")?.unwrap_or(false);
    let app = app_dir(ctx)?.to_path_buf();
    let _g = edit_lock();
    let (ip, root, plan) = plan_rename(ctx, a, "wiki_rename")?;
    let case_only = do_move(&plan.from_abs, &plan.to)?;
    let mut rewritten: Vec<String> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut links = 0;
    if rewrite {
        for r in &plan.rewrites {
            let abs = root.join(&r.src_new);
            let res = (|| -> Result<bool> {
                let Some(before) = seen(&abs) else { return Ok(false) };
                let cur = std::fs::read(&abs)?;
                if sha256_hex(&cur) != r.before_sha {
                    return Ok(false);
                }
                if !writable(&abs) {
                    return Err(bad("書込権がありません（読み取り専用）"));
                }
                backup(&app, &root, &r.src_new, &cur)?;
                Ok(write_atomic(&abs, &r.after, Some(before))? == Wrote::Ok)
            })();
            match res {
                Ok(true) => {
                    rewritten.push(r.src_new.clone());
                    links += r.links;
                }
                Ok(false) => skipped.push(json!({"path": r.src_new, "reason": "計画の後に書き換わった"})),
                Err(e) => skipped.push(json!({"path": r.src_new, "reason": e.to_string()})),
            }
        }
    }
    let mut touched: Vec<&str> = vec![plan.to.rel.as_str()];
    touched.extend(rewritten.iter().map(String::as_str));
    let ie = reindex_move(&ip, &root, case_only, &plan.from_rel, &touched);
    let out = json!({"path": plan.to.rel, "from": plan.from_rel, "rewritten": rewritten, "links": links, "skipped": skipped, "other": other_json(&plan.other)});
    Ok(with_index_error(out, ie))
}

/// `wiki_trash {path, dir?}`: ワークスペースの中のファイルかフォルダ（中身ごと）を mover（app crate = macOS のゴミ箱 = ワークスペースの外）へ渡し、
/// 索引から外す。`dir`（省略可）= 呼び手の期待（true = フォルダ・false = ファイル）と実体が違えば移さない。
/// mover の戻り値（ゴミ箱へ移した方式の名前）は結果の `method` に、フォルダなら `dir: true` と中のファイル数 `files`
/// （`.` 始まりと symlink を除く）も載せる。vault の root 自体・`.` 始まり・symlink は拒否。
/// 索引からは、歩いて見つけたものに加え、索引に残る `<rel>/` 配下の行も外す（外部で消された・読めない区画の残骸を残さない）
pub fn trash_with(ctx: &Ctx, a: &Value, mover: impl FnOnce(&Path) -> std::result::Result<String, String>) -> Result<Value> {
    let path = req_str(a, "path", "wiki_trash")?;
    let expect_dir = opt_bool(a, "dir")?;
    let (ip, root) = ctx.ready()?;
    let _g = edit_lock();
    reject_symlink(&root, &path)?;
    let (rel, abs) = super::commands::resolve_entry(&root, &path, true)?;
    let is_dir = std::fs::symlink_metadata(&abs)?.is_dir();
    if expect_dir.is_some_and(|d| d != is_dir) {
        return Err(CoreError::Conflict(format!("{}: {rel}", if is_dir { "フォルダです" } else { "フォルダではありません" })));
    }
    // 移す前に中のファイルを控える（移した後は歩けない。索引から外すのに要る）
    let mut inside: Vec<String> = Vec::new();
    if is_dir {
        files_under(&abs, &rel, &mut inside)?;
    }
    let method = mover(&abs).map_err(|m| CoreError::wiki(super::codes::FS, m))?;
    let prefix = format!("{rel}/");
    let ie = with_index(&ip, &root, |ix| {
        let mut rels: Vec<String> = inside.clone();
        rels.push(rel.clone());
        if is_dir {
            rels.extend(ix.all_paths()?.into_iter().filter(|p| p.starts_with(&prefix)));
        }
        ix.refresh_paths(&rels)
    })
    .err()
    .map(|e| e.to_string());
    let mut out = json!({"path": rel, "method": method});
    if is_dir {
        out["dir"] = json!(true);
        out["files"] = json!(inside.len());
    }
    Ok(with_index_error(out, ie))
}

/// dir の下の全ファイルの vault 相対パス（`.` 始まりの名前と symlink は入れない・辿らない）。読めない区画があれば場所を添えて Err
fn files_under(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
    let at = |e: std::io::Error| CoreError::wiki(super::codes::FS, format!("{}: {e}", dir.display()));
    for e in std::fs::read_dir(dir).map_err(at)? {
        let e = e.map_err(at)?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let ft = e.file_type().map_err(at)?;
        if ft.is_dir() {
            files_under(&e.path(), &child_rel, out)?;
        } else if !ft.is_symlink() {
            out.push(child_rel);
        }
    }
    Ok(())
}

/// 貼付・取込のファイル名の消毒: 区切り（`/` `\` `:`）・Obsidian のリンクに使えない字（`[ ] | # ^`）・`* ? " < >`・制御文字を `-` に、
/// 先頭の `.` と空白・末尾の `.` と空白を落とす。見えない書式文字（Cf / Zl / Zp = 双方向の制御・ゼロ幅・行 / 段落区切り）は落とす。
/// 長さは 120 文字まで（拡張子は残す）。空なら None
pub fn sanitize_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base
        .nfc()
        .filter(|c| !invisible(*c))
        .map(|c| if c.is_control() || "/\\:*?\"<>|#^[]".contains(c) { '-' } else { c })
        .collect();
    let s = s.trim_start_matches(['.', ' ']).trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() || s.chars().all(|c| c == '-') {
        return None;
    }
    if s.chars().count() <= 120 {
        return Some(s);
    }
    let (stem, ext) = match s.rsplit_once('.') {
        Some((st, e)) if e.chars().count() <= 10 && !st.is_empty() => (st.to_string(), format!(".{e}")),
        _ => (s.clone(), String::new()),
    };
    Some(format!("{}{ext}", stem.chars().take(120 - ext.chars().count()).collect::<String>().trim_end()))
}

/// Unicode の Cf（書式）・Zl・Zp に当たる文字（unicode の表を持たないので主なものを列挙）。
/// 貼付・取込のファイル名（sanitize_name）とチャットのログのファイル名（app の wiki_chat_log::log_title）が落とす
pub fn invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x061C | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// 中身の先頭から画像 / PDF の拡張子を推す（名前が既知の拡張子で終わらないとき）
pub(crate) fn sniff_ext(b: &[u8]) -> Option<&'static str> {
    Some(match b {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xFF, 0xD8, 0xFF, ..] => "jpg",
        [b'G', b'I', b'F', b'8', ..] => "gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        [b'%', b'P', b'D', b'F', ..] => "pdf",
        _ => return None,
    })
}

/// 今月（ローカル時刻）の `YYYY-MM`
fn year_month() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    if let Some(t) = local_time(secs) {
        return format!("{:04}-{:02}", t.year, t.month);
    }
    let d = time::OffsetDateTime::from_unix_timestamp(secs).unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!("{:04}-{:02}", d.year(), u8::from(d.month()))
}

/// ローカル時刻の年・月（1〜12）・日・時・分
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// unix 秒 → ローカル時刻（libc の localtime_r = TZ に従う）。変換できなければ None（unix 以外も None）
pub fn local_time(secs: i64) -> Option<LocalTime> {
    #[cfg(unix)]
    {
        let t: libc::time_t = secs as libc::time_t;
        // SAFETY: localtime_r（再入可）は渡した tm を埋めるだけ
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return Some(LocalTime {
                year: tm.tm_year + 1900,
                month: (tm.tm_mon + 1) as u32,
                day: tm.tm_mday as u32,
                hour: tm.tm_hour as u32,
                minute: tm.tm_min as u32,
            });
        }
    }
    #[cfg(not(unix))]
    let _ = secs;
    None
}

/// 取り込んだファイル（vault 相対）を本文に入れる文字列: media = `![[…]]`・それ以外 = `[[…]]`
fn embed_of(rel: &str) -> String {
    if Kind::of(rel) == Kind::Media {
        format!("![[{rel}]]")
    } else {
        format!("[[{rel}]]")
    }
}

/// attachments/<YYYY-MM>/ に name で置く（同名は連番）。fill（中身を一時ファイルに書く = 大きなコピー）は edit_lock の外、
/// 置く（RENAME_EXCL）ところだけ lock を取る。戻り値 = vault 相対
fn put_attachment(root: &Path, name: &str, fill: impl FnOnce(&Path) -> std::io::Result<()>) -> Result<String> {
    let t = target_path(root, &format!("{ATTACH_DIR}/{}/{name}", year_month()))?;
    let placed = place_new(&t.parent, name, fill)?;
    Ok(match t.rel.rsplit_once('/') {
        Some((d, _)) => format!("{d}/{placed}"),
        None => placed,
    })
}

/// data_base64（`data:…;base64,` 付きでも可）→ バイト列（上限 max バイト）。上限の判定は空白（76 字折返しの改行等）を除いた後の長さで
/// （除く前の長さで判定すると、改行入りの base64 が上限近くで正当でも落ちる）。除く前の判定は空白の分の余裕（2 倍）を持たせた粗い足切り
fn blob_bytes(data: &str, max: usize) -> Result<Vec<u8>> {
    use base64::Engine;
    let too_big = || bad(format!("大きすぎます（上限 {} MB）", max / 1024 / 1024));
    let data = match data.split_once(";base64,") {
        Some((head, rest)) if head.starts_with("data:") => rest,
        _ => data,
    };
    let b64_max = max / 3 * 4 + 8;
    if data.len() > b64_max.saturating_mul(2) {
        return Err(too_big());
    }
    let clean: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if clean.len() > b64_max {
        return Err(too_big());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(clean.as_bytes()).map_err(|e| bad(format!("data_base64 を読めません: {e}")))?;
    if bytes.len() > max {
        return Err(too_big());
    }
    Ok(bytes)
}

/// `wiki_save_blob {name, data_base64, page?}`: 貼付の画像等（画像・PDF・動画・音声）を attachments/<YYYY-MM>/<name> に置く（同名は連番）。
/// data_base64 は `data:…;base64,` 付きでも可・上限 50MB。page は受け取るだけ（置き場は page に依らない）
pub fn save_blob(ctx: &Ctx, a: &Value) -> Result<Value> {
    let name = req_str(a, "name", "wiki_save_blob")?;
    let data = req_str(a, "data_base64", "wiki_save_blob")?;
    let _page = opt_str(a, "page")?;
    let (ip, root) = ctx.ready()?;
    let bytes = blob_bytes(&data, BLOB_MAX)?;
    let mut name = sanitize_name(&name).unwrap_or_else(|| "pasted".into());
    if Kind::of(&name) != Kind::Media {
        if let Some(e) = sniff_ext(&bytes) {
            name = format!("{name}.{e}");
        }
    }
    if Kind::of(&name) != Kind::Media {
        return Err(bad(format!("wiki_save_blob は画像・PDF・動画・音声だけ置けます: {name}")));
    }
    let rel = put_attachment(&root, &name, |tmp| std::fs::write(tmp, &bytes))?;
    let ie = reindex(&ip, &root, &[rel.as_str()]);
    Ok(with_index_error(json!({"path": rel, "embed": embed_of(&rel)}), ie))
}

/// `wiki_import_file {abs, page?}`: OS からドロップしたファイル（絶対パス）。ワークスペースの中（`.` 始まりの区画の外）ならコピーせずそのまま、
/// 外なら画像・PDF・動画・音声だけ attachments/<YYYY-MM>/ へコピー（同名は連番・大きさの上限なし = 動画）
pub fn import_file(ctx: &Ctx, a: &Value) -> Result<Value> {
    let abs = req_str(a, "abs", "wiki_import_file")?;
    let _page = opt_str(a, "page")?;
    let (ip, root) = ctx.ready()?;
    let p = Path::new(abs.trim());
    if !p.is_absolute() {
        return Err(bad(format!("wiki_import_file: abs は絶対パスで渡してください: {abs}")));
    }
    let canon = std::fs::canonicalize(p).map_err(|_| CoreError::NotFound(format!("ファイル {abs}")))?;
    if !std::fs::metadata(&canon)?.is_file() {
        return Err(bad(format!("ファイルではありません（フォルダは取り込めません）: {abs}")));
    }
    if let Some(rel) = super::paths::rel_inside(&root, &canon) {
        return Ok(json!({"path": rel, "embed": embed_of(&rel), "copied": false}));
    }
    let name = canon.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = sanitize_name(&name).ok_or_else(|| bad(format!("ファイル名を使えません: {abs}")))?;
    if Kind::of(&name) != Kind::Media {
        return Err(bad(format!("ワークスペースの外から取り込めるのは画像・PDF・動画・音声だけです: {name}")));
    }
    let rel = put_attachment(&root, &name, |tmp| std::fs::copy(&canon, tmp).map(|_| ()))?;
    let ie = reindex(&ip, &root, &[rel.as_str()]);
    Ok(with_index_error(json!({"path": rel, "embed": embed_of(&rel), "copied": true}), ie))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wiki::commands;

    /// local_time は旧の year_month の libc の写しと同じ年・月（境界の秒 + 今）・year_month の形は `YYYY-MM`
    #[test]
    fn local_time_matches_old_libc() {
        let old = |secs: i64| {
            let t: libc::time_t = secs as libc::time_t;
            // SAFETY: localtime_r は渡した tm を埋めるだけ
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            assert!(!unsafe { libc::localtime_r(&t, &mut tm) }.is_null());
            (tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        for s in [0, 1, 86_399, 951_782_400, 1_700_000_000, 2_147_483_647, 4_102_444_800, now] {
            let t = local_time(s).unwrap();
            assert_eq!((t.year, t.month as i32, t.day as i32, t.hour as i32, t.minute as i32), old(s), "{s}");
            assert_eq!(format!("{:04}-{:02}", t.year, t.month), { let o = old(s); format!("{:04}-{:02}", o.0, o.1) });
        }
        let ym = year_month();
        assert!(ym.len() == 7 && ym.as_bytes()[4] == b'-', "{ym}");
    }

    struct Env {
        base: PathBuf,
        ctx: Ctx,
        root: PathBuf,
        /// 索引（プロセス共有）を使うテストの直列化（index::test_serial・drop まで持つ）
        _serial: std::sync::MutexGuard<'static, ()>,
        app: PathBuf,
    }
    impl Drop for Env {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }
    fn env(name: &str) -> Env {
        let _serial = crate::wiki::index::test_serial();
        let base = std::env::temp_dir().join(format!("openref-wikiedit-{}-{}-{name}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(&base).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let home = base.join("home");
        let root = home.join("Documents/vault");
        let app = home.join("scratch-app");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        let ctx = Ctx { enabled: true, root: Some(root.to_string_lossy().into()), app_dir: Some(app.clone()), home: Some(home), watched: false };
        Env { base, ctx, root, app, _serial }
    }
    fn put(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    fn read(root: &Path, rel: &str) -> String {
        std::fs::read_to_string(root.join(rel)).unwrap()
    }
    fn hist(e: &Env, rel: &str) -> Vec<PathBuf> {
        history_files(&history_dir(&e.app, &e.root, rel))
    }

    #[test]
    fn write_new_conflict_force_and_history() {
        let e = env("w");
        // 新規（フォルダも作る）
        let r = write(&e.ctx, &json!({"path": "新しい/ページ.md", "content": "# 題\n[[b]]"})).unwrap();
        assert_eq!((r["ok"].as_bool(), r["created"].as_bool(), r["path"].as_str()), (Some(true), Some(true), Some("新しい/ページ.md")));
        let sha1 = r["sha256"].as_str().unwrap().to_string();
        assert_eq!(sha1, sha256_hex("# 題\n[[b]]".as_bytes()));
        // 索引に同期で入っている（5 秒の鮮度を待たない）
        let rd = commands::read(&e.ctx, &json!({"path": "新しい/ページ.md"})).unwrap();
        assert_eq!(rd["title"], "題");
        // 一致する if_sha256 = 退避しない
        let r = write(&e.ctx, &json!({"path": "新しい/ページ.md", "content": "v2", "if_sha256": sha1})).unwrap();
        assert_eq!(r["created"], false);
        let sha2 = r["sha256"].as_str().unwrap().to_string();
        assert!(hist(&e, "新しい/ページ.md").is_empty());
        // 外部が書き換えた → 古い sha256 では conflict（書かない）
        std::fs::write(e.root.join("新しい/ページ.md"), "external").unwrap();
        let c = write(&e.ctx, &json!({"path": "新しい/ページ.md", "content": "mine", "if_sha256": sha2})).unwrap();
        assert_eq!(c, json!({"ok": false, "conflict": true, "path": "新しい/ページ.md", "sha256": sha256_hex(b"external")}));
        assert_eq!(read(&e.root, "新しい/ページ.md"), "external");
        // force = 上書き前（外部の内容）を退避
        let f = write(&e.ctx, &json!({"path": "新しい/ページ.md", "content": "mine", "if_sha256": sha2, "force": true})).unwrap();
        assert_eq!(f["ok"], true);
        assert_eq!(read(&e.root, "新しい/ページ.md"), "mine");
        let h = hist(&e, "新しい/ページ.md");
        assert_eq!(h.len(), 1);
        assert_eq!(std::fs::read_to_string(&h[0]).unwrap(), "external");
        assert!(h[0].extension().unwrap() == "md");
        assert_eq!(
            std::fs::read_to_string(history_dir(&e.app, &e.root, "新しい/ページ.md").join("path.txt")).unwrap(),
            format!("{}\n", e.root.join("新しい/ページ.md").display())
        );
        // 消えたファイルに if_sha256 = conflict（sha256 null）
        std::fs::remove_file(e.root.join("新しい/ページ.md")).unwrap();
        let g = write(&e.ctx, &json!({"path": "新しい/ページ.md", "content": "x", "if_sha256": sha2})).unwrap();
        assert_eq!(g["conflict"], true);
        assert!(g["sha256"].is_null());
        assert!(!e.root.join("新しい/ページ.md").exists());
        // 監視が同じ書込を拾って取り込み直しても索引は同じ（refresh_paths は冪等）
        let (ip, root) = e.ctx.ready().unwrap();
        let snap = || with_index(&ip, &root, |ix| Ok((ix.stats()?, ix.page("新しい/ページ.md")?.map(|p| p.to_json())))).unwrap();
        write(&e.ctx, &json!({"path": "新しい/ページ.md", "content": "# 題 2\n[[b]] [[c]]"})).unwrap();
        let s1 = snap();
        for _ in 0..2 {
            with_index(&ip, &root, |ix| ix.refresh_paths(&[e.root.join("新しい/ページ.md")])).unwrap();
        }
        assert_eq!(snap(), s1);
        assert_eq!(s1.0["unresolved"], 2);
        std::fs::remove_file(e.root.join("新しい/ページ.md")).unwrap();
        // 一時ファイルが残っていない
        let left: Vec<_> = std::fs::read_dir(e.root.join("新しい")).unwrap().flatten().map(|x| x.file_name()).collect();
        assert!(left.is_empty(), "{left:?}");
    }

    /// 登録表の write_agent: 一致する if_sha256 でも・無しでも上書き前を退避して backup を返す。新規 = created・同じ中身は書かない
    #[test]
    fn write_agent_always_backs_up() {
        let e = env("wa");
        let r = write_agent(&e.ctx, &json!({"path": "p.md", "content": "v1"})).unwrap();
        assert_eq!((r["created"].as_bool(), r.get("backup")), (Some(true), None));
        let r = write_agent(&e.ctx, &json!({"path": "p.md", "content": "v2", "if_sha256": sha256_hex(b"v1")})).unwrap();
        assert_eq!((r["ok"].as_bool(), r["created"].as_bool()), (Some(true), Some(false)));
        assert_eq!(r["sha256"], sha256_hex(b"v2"));
        assert_eq!(std::fs::read_to_string(r["backup"].as_str().unwrap()).unwrap(), "v1");
        let r = write_agent(&e.ctx, &json!({"path": "p.md", "content": "v3"})).unwrap();
        assert_eq!(std::fs::read_to_string(r["backup"].as_str().unwrap()).unwrap(), "v2");
        assert_eq!(hist(&e, "p.md").len(), 2);
        // 中身が同じ = 上書きしない = 退避もしない
        let r = write_agent(&e.ctx, &json!({"path": "p.md", "content": "v3"})).unwrap();
        assert!(r.get("backup").is_none() && r["ok"] == true);
        // 不一致は今どおり conflict（書かない・退避しない）
        let c = write_agent(&e.ctx, &json!({"path": "p.md", "content": "x", "if_sha256": sha256_hex(b"v1")})).unwrap();
        assert_eq!(c["conflict"], true);
        assert_eq!(hist(&e, "p.md").len(), 2);
        // GUI の write は一致する if_sha256 なら退避しない（今どおり）
        write(&e.ctx, &json!({"path": "p.md", "content": "v4", "if_sha256": sha256_hex(b"v3")})).unwrap();
        assert_eq!(hist(&e, "p.md").len(), 2);
    }

    #[test]
    fn write_keeps_mode_and_history_keeps_ten() {
        use std::os::unix::fs::PermissionsExt;
        let e = env("mode");
        put(&e.root, "a.md", "0");
        std::fs::set_permissions(e.root.join("a.md"), std::fs::Permissions::from_mode(0o600)).unwrap();
        for i in 1..=13 {
            write(&e.ctx, &json!({"path": "a.md", "content": format!("{i}")})).unwrap(); // if_sha256 なし = 照合なしの上書き → 退避
        }
        assert_eq!(std::fs::metadata(e.root.join("a.md")).unwrap().permissions().mode() & 0o777, 0o600);
        let h = hist(&e, "a.md");
        assert_eq!(h.len(), HISTORY_KEEP);
        // 新しい順 = 直前の内容（12）が先頭・最古は 3
        let names: Vec<String> = h.iter().map(|p| format!("{}={}", p.file_name().unwrap().to_string_lossy(), std::fs::read_to_string(p).unwrap())).collect();
        assert_eq!(std::fs::read_to_string(&h[0]).unwrap(), "12", "{names:?}");
        assert_eq!(std::fs::read_to_string(&h[9]).unwrap(), "3");
        // 同じ中身は書かない・退避しない
        write(&e.ctx, &json!({"path": "a.md", "content": "13"})).unwrap();
        assert_eq!(std::fs::read_to_string(&hist(&e, "a.md")[0]).unwrap(), "12");
    }

    /// 書込に失敗したら（フォルダが書込不可 = 一時ファイルを作れない）置いた退避を消し、世代は刈り込まない（最古も残る）。GUI の
    /// 照合なしの保存・登録表の write_agent の両方
    #[test]
    fn failed_write_keeps_all_generations() {
        use std::os::unix::fs::PermissionsExt;
        let e = env("wfail");
        put(&e.root, "sub/a.md", "v0");
        for i in 1..=12 {
            write(&e.ctx, &json!({"path": "sub/a.md", "content": format!("v{i}")})).unwrap();
        }
        let before: Vec<String> = hist(&e, "sub/a.md").iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
        assert_eq!(before.len(), HISTORY_KEEP);
        assert_eq!(before.last().map(String::as_str), Some("v2"), "最古 = v2");
        /// テストが落ちても書込権を戻す（tempdir を消せるように）
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        let sub = e.root.join("sub");
        let _r = Restore(sub.clone());
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(write(&e.ctx, &json!({"path": "sub/a.md", "content": "gui"})).is_err());
        assert!(write_agent(&e.ctx, &json!({"path": "sub/a.md", "content": "agent", "if_sha256": sha256_hex(b"v12")})).is_err());
        let after: Vec<String> = hist(&e, "sub/a.md").iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
        assert_eq!(after, before, "退避の世代が変わった");
        assert_eq!(read(&e.root, "sub/a.md"), "v12");
    }

    #[test]
    fn write_rejects() {
        let e = env("deny");
        put(&e.root, "img.png", "x");
        let outside = e.base.join("home/out");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, e.root.join("ext")).unwrap();
        std::fs::write(outside.join("t.md"), "o").unwrap();
        std::os::unix::fs::symlink(outside.join("t.md"), e.root.join("link.md")).unwrap();
        let err = |p: &str| write(&e.ctx, &json!({"path": p, "content": "x"})).unwrap_err().to_string();
        assert!(err("img.png").contains("md / html"));
        assert!(err("a.txt").contains("md / html"));
        assert!(err("../x.md").contains(".."));
        assert!(err(".obsidian/x.md").contains("`.`"));
        assert!(err("a/.hidden.md").contains("`.`"));
        assert!(err("ext/x.md").contains("ワークスペースの外"), "外を指す symlink のフォルダ");
        assert!(err("ext/new/x.md").contains("ワークスペースの外"));
        assert!(err("link.md").contains("symlink"));
        assert!(err(&outside.join("t.md").to_string_lossy()).contains("ワークスペースの外"));
        assert!(err("bad\u{7}.md").contains("制御文字"));
        assert_eq!(std::fs::read_to_string(outside.join("t.md")).unwrap(), "o");
        assert!(!outside.join("x.md").exists());
        assert!(write(&e.ctx, &json!({"path": "a.md"})).unwrap_err().to_string().contains("content"));
        assert!(write(&e.ctx, &json!({"path": "a.md", "content": "x", "if_sha256": "zz"})).unwrap_err().to_string().contains("64"));
        // 空の中身は書ける
        assert_eq!(write(&e.ctx, &json!({"path": "empty.md", "content": ""})).unwrap()["size"], 0);
    }

    #[test]
    fn mkdir_makes_empty_folder() {
        let e = env("mk");
        assert_eq!(mkdir(&e.ctx, &json!({"name": "新しいフォルダ"})).unwrap()["path"], "新しいフォルダ");
        assert!(e.root.join("新しいフォルダ").is_dir());
        assert_eq!(mkdir(&e.ctx, &json!({"dir": "新しいフォルダ/", "name": " sub "})).unwrap()["path"], "新しいフォルダ/sub");
        assert!(e.root.join("新しいフォルダ/sub").is_dir());
        // 途中の無いフォルダも作る・NFD は NFC に
        assert_eq!(mkdir(&e.ctx, &json!({"dir": "a/b", "name": "\u{30cf}\u{309a}"})).unwrap()["path"], "a/b/\u{30d1}");
        assert!(e.root.join("a/b/\u{30d1}").is_dir());
        // 同じ名前のフォルダ・ファイルは拒否
        assert!(mkdir(&e.ctx, &json!({"name": "新しいフォルダ"})).unwrap_err().to_string().contains("同じ名前のフォルダがあります"));
        put(&e.root, "memo.md", "m");
        assert!(mkdir(&e.ctx, &json!({"name": "memo.md"})).unwrap_err().to_string().contains("同じ名前のファイルがあります"));
        // dir が実在のファイル・name が既存と大小だけ違う（APFS）は拒否。ワークスペースの外を指す symlink のフォルダも
        assert!(mkdir(&e.ctx, &json!({"dir": "memo.md", "name": "x"})).unwrap_err().to_string().contains("フォルダではありません"));
        if std::fs::symlink_metadata(e.root.join("新しいフォルダ").with_file_name("新しいふぉるだ")).is_err() {
            // 大小の違いは英字で確かめる
            std::fs::create_dir(e.root.join("Case")).unwrap();
            if std::fs::symlink_metadata(e.root.join("case")).is_ok() {
                assert!(mkdir(&e.ctx, &json!({"name": "case"})).is_err(), "大小だけ違うフォルダ");
            }
        }
        let outside = e.base.join("home/outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, e.root.join("ext")).unwrap();
        assert!(mkdir(&e.ctx, &json!({"dir": "ext", "name": "x"})).unwrap_err().to_string().contains("ワークスペースの外"));
        assert!(!outside.join("x").exists());
        // 名前の検査（空・/・. 始まり・制御文字・リンクを壊す字・ワークスペースの外）
        for bad_name in ["", " ", "a/b", ".hidden", "x\u{7}", "a[b", "a:b"] {
            assert!(mkdir(&e.ctx, &json!({"name": bad_name})).is_err(), "{bad_name:?}");
        }
        assert!(mkdir(&e.ctx, &json!({})).is_err(), "name は必須");
        assert!(mkdir(&e.ctx, &json!({"dir": "..", "name": "x"})).is_err());
        assert!(mkdir(&e.ctx, &json!({"dir": ".obsidian", "name": "x"})).is_err());
        assert!(!e.base.join("home/Documents/x").exists());
    }

    #[test]
    fn create_and_move() {
        let e = env("cm");
        assert_eq!(create(&e.ctx, &json!({})).unwrap()["path"], "Untitled.md");
        assert_eq!(create(&e.ctx, &json!({})).unwrap()["path"], "Untitled 2.md");
        assert_eq!(create(&e.ctx, &json!({"dir": "sub/深い", "name": "メモ"})).unwrap()["path"], "sub/深い/メモ.md");
        assert!(create(&e.ctx, &json!({"name": "a/b"})).is_err());
        assert!(create(&e.ctx, &json!({"dir": "..", "name": "x"})).is_err());
        // html は骨格を書く（md は空）・.htm も html
        assert_eq!(create(&e.ctx, &json!({"name": "Untitled.html"})).unwrap()["path"], "Untitled.html");
        assert_eq!(read(&e.root, "Untitled.html"), HTML_SKELETON);
        assert_eq!(read(&e.root, "Untitled.md"), "");
        assert_eq!(create(&e.ctx, &json!({"name": "p.htm"})).unwrap()["path"], "p.htm");
        assert_eq!(read(&e.root, "p.htm"), HTML_SKELETON);
        assert_eq!(create(&e.ctx, &json!({"name": "Untitled.html"})).unwrap()["path"], "Untitled 2.html");
        put(&e.root, "ingest/素材.pdf", "%PDF");
        let m = move_(&e.ctx, &json!({"from": "ingest/素材.pdf", "to": "sources/2026/素材.pdf"})).unwrap();
        assert_eq!(m["path"], "sources/2026/素材.pdf");
        assert_eq!((m["from"].as_str(), m["to"].as_str()), (Some("ingest/素材.pdf"), Some("sources/2026/素材.pdf")));
        // 拡張子を変える移動は拒否（md ↔ html・pdf → md）・大小だけの違いは可
        put(&e.root, "k.md", "k");
        for to in ["k.html", "k.txt", "k"] {
            assert!(move_(&e.ctx, &json!({"from": "k.md", "to": to})).unwrap_err().to_string().contains("拡張子"), "{to}");
        }
        assert!(move_(&e.ctx, &json!({"from": "sources/2026/素材.pdf", "to": "素材.md"})).is_err());
        assert_eq!(move_(&e.ctx, &json!({"from": "k.md", "to": "sub/k2.MD"})).unwrap()["to"], "sub/k2.MD");
        assert!(e.root.join("sources/2026/素材.pdf").is_file());
        assert!(!e.root.join("ingest/素材.pdf").exists());
        // 移動先が既にある = 拒否（上書きしない）
        put(&e.root, "x.md", "x");
        put(&e.root, "y.md", "y");
        assert!(move_(&e.ctx, &json!({"from": "x.md", "to": "y.md"})).unwrap_err().to_string().contains("既に"));
        assert_eq!(read(&e.root, "y.md"), "y");
        // 大小だけの改名は可（大小を区別しないボリューム）
        let r = move_(&e.ctx, &json!({"from": "x.md", "to": "X.md"})).unwrap();
        assert_eq!(r["path"], "X.md");
        let names: Vec<String> = std::fs::read_dir(&e.root).unwrap().flatten().map(|d| d.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"X.md".to_string()), "{names:?}");
        let st = commands::status(&e.ctx, &json!({})).unwrap();
        assert!(commands::read(&e.ctx, &json!({"path": "sources/2026/素材.pdf"})).is_ok(), "{st}");
    }

    #[test]
    fn rename_rewrites_links_keeping_form() {
        let e = env("rn");
        put(&e.root, "notes/Target.md", "# T\n## 見出し\n自分へ [[#見出し]] と [隣](other.md)\n");
        put(&e.root, "notes/other.md", "o");
        put(
            &e.root,
            "a.md",
            "---\nrelated: \"[[Target]]\"\n---\n[[Target]] [[target|別名]] [[Target#見出し]] ![[Target]] [[notes/Target]] [[Target.md]]\n\
             [相対](notes/Target.md) [角](<notes/Target.md#見出し>) [根](/notes/Target.md)\n\
             `[[Target]]` はコード\n```\n[[Target]]\n```\n[[Targetx]] [[other]]\n",
        );
        put(&e.root, "notes/sub/b.md", "[上](../Target.md) [[Target|表 \\| 内]]\n");
        put(&e.root, "c.html", "<a href=\"notes/Target.md\">x</a>");
        let p = rename_preview(&e.ctx, &json!({"from": "notes/Target.md", "to": "notes/新しい名前.md"})).unwrap();
        // a.md: frontmatter 1 + 本文 wiki 6 + md 3 = 10 / b.md: 2
        assert_eq!(p["files"], json!(["a.md", "notes/sub/b.md"]), "{p}");
        assert_eq!(p["links"], 12, "{p}");
        assert_eq!(p["other"], json!([{"path": "c.html", "kind": "html"}]));
        let r = rename(&e.ctx, &json!({"from": "notes/Target.md", "to": "notes/新しい名前.md", "rewrite": true})).unwrap();
        assert_eq!(r["rewritten"], json!(["a.md", "notes/sub/b.md"]), "{r}");
        assert_eq!(r["links"], p["links"]);
        let a = read(&e.root, "a.md");
        assert!(a.contains("related: \"[[新しい名前]]\""), "{a}");
        assert!(a.contains("[[新しい名前]] [[新しい名前|別名]] [[新しい名前#見出し]] ![[新しい名前]] [[notes/新しい名前]] [[新しい名前.md]]"), "{a}");
        assert!(a.contains("[相対](notes/%E6%96%B0%E3%81%97%E3%81%84%E5%90%8D%E5%89%8D.md)") || a.contains("[相対](notes/新しい名前.md)"), "{a}");
        assert!(a.contains("[角](<notes/新しい名前.md#見出し>)"), "{a}");
        assert!(a.contains("[根](/notes/新しい名前.md)"), "{a}");
        assert!(a.contains("`[[Target]]` はコード\n```\n[[Target]]\n```\n[[Targetx]] [[other]]"), "{a}");
        let b = read(&e.root, "notes/sub/b.md");
        assert_eq!(b, "[上](../新しい名前.md) [[新しい名前|表 \\| 内]]\n");
        // 自分の中の同じページ・同じフォルダへのリンクは触らない
        assert_eq!(read(&e.root, "notes/新しい名前.md"), "# T\n## 見出し\n自分へ [[#見出し]] と [隣](other.md)\n");
        // 書き換えた各ファイルが退避されている（中身は書換え前）
        assert!(std::fs::read_to_string(&hist(&e, "a.md")[0]).unwrap().contains("[[Target]] [[target|別名]]"));
        assert_eq!(hist(&e, "notes/sub/b.md").len(), 1);
        // 未解決: a.md の [[Targetx]] と、書き換えない html の href（other に出したもの）
        let st = commands::status(&e.ctx, &json!({})).unwrap();
        assert_eq!(st["unresolved"], 2, "{st}");
        // 移動先が既にある = 拒否
        put(&e.root, "z.md", "z");
        assert!(rename(&e.ctx, &json!({"from": "z.md", "to": "a.md", "rewrite": true})).unwrap_err().to_string().contains("既に"));
    }

    #[test]
    fn rename_to_other_folder_fixes_own_links_and_ambiguity() {
        let e = env("rn2");
        put(&e.root, "x/P.md", "[兄弟](Q.md) [[Q]] ![[img.png]]\n");
        put(&e.root, "x/Q.md", "[[P]]");
        put(&e.root, "x/img.png", "png");
        put(&e.root, "y/Dup.md", "既にある同名");
        put(&e.root, "r.md", "[[P]] [p](x/P.md)");
        let r = rename(&e.ctx, &json!({"from": "x/P.md", "to": "z/Dup.md", "rewrite": true})).unwrap();
        assert_eq!(r["path"], "z/Dup.md");
        // basename の Dup は y/Dup.md と曖昧 → パス指定（拡張子なし）に
        assert_eq!(read(&e.root, "x/Q.md"), "[[z/Dup]]");
        assert_eq!(read(&e.root, "r.md"), "[[z/Dup]] [p](z/Dup.md)");
        // 改名したページ自身の相対リンクは新しいフォルダから辿り直す・wikilink の basename はそのまま解ける
        assert_eq!(read(&e.root, "z/Dup.md"), "[兄弟](../x/Q.md) [[Q]] ![[img.png]]\n");
        let st = commands::status(&e.ctx, &json!({})).unwrap();
        assert_eq!(st["unresolved"], 0, "{st}");
    }

    #[test]
    fn rename_without_rewrite_and_changed_during() {
        let e = env("rn3");
        put(&e.root, "a.md", "x");
        put(&e.root, "b.md", "[[a]]");
        let r = rename(&e.ctx, &json!({"from": "a.md", "to": "c.md"})).unwrap();
        assert_eq!(r["rewritten"], json!([]));
        assert_eq!(read(&e.root, "b.md"), "[[a]]");
        assert!(e.root.join("c.md").exists());
    }

    /// save_blob の上限（A-11・縮尺 = 上限 3000 バイト）: 76 字で折り返した（CRLF）base64 は、空白を除いた長さで判定する =
    /// ちょうど上限のデータは通る（除く前の長さでは上限の base64 長を超える）。上限 + 1 バイト・空白を除いても長いもの・空白だけで
    /// 2 倍を超えるものは「大きすぎ」。`data:…;base64,` 付きも同じ
    #[test]
    fn blob_bytes_limit_after_whitespace() {
        use base64::Engine;
        let max = 3000;
        let wrap = |b: &[u8]| -> String {
            let s = base64::engine::general_purpose::STANDARD.encode(b);
            s.as_bytes().chunks(76).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("\r\n")
        };
        let at_max: Vec<u8> = (0..max).map(|i| (i * 7 % 251) as u8).collect();
        let w = wrap(&at_max);
        assert!(w.len() > max / 3 * 4 + 8, "折返しで除く前の長さが上限の base64 長を超える前提: {}", w.len());
        assert_eq!(blob_bytes(&w, max).unwrap(), at_max);
        assert_eq!(blob_bytes(&format!("data:image/png;base64,{w}"), max).unwrap(), at_max);
        let big = |e: Result<Vec<u8>>| e.unwrap_err().to_string().contains("大きすぎ");
        assert!(big(blob_bytes(&wrap(&vec![1u8; max + 1]), max)), "上限 + 1 バイト");
        assert!(big(blob_bytes(&wrap(&vec![1u8; max + 30]), max)), "空白を除いても base64 が長い");
        let padded = format!("{}{}", " ".repeat((max / 3 * 4 + 8) * 2), base64::engine::general_purpose::STANDARD.encode(b"x"));
        assert!(big(blob_bytes(&padded, max)), "空白込みで 2 倍を超える = 除く前に足切り");
        assert!(!big(blob_bytes("!!!", max)), "読めない base64 は大きさでなく読めない");
    }

    #[test]
    fn save_blob_and_import() {
        use base64::Engine;
        let e = env("blob");
        let png = b"\x89PNG\r\n\x1a\nxxxx".to_vec();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let ym = year_month();
        let r = save_blob(&e.ctx, &json!({"name": "貼り付け 画像.png", "data_base64": b64})).unwrap();
        assert_eq!(r["path"], format!("attachments/{ym}/貼り付け 画像.png"));
        assert_eq!(r["embed"], format!("![[attachments/{ym}/貼り付け 画像.png]]"));
        let r2 = save_blob(&e.ctx, &json!({"name": "貼り付け 画像.png", "data_base64": format!("data:image/png;base64,{b64}")})).unwrap();
        assert_eq!(r2["path"], format!("attachments/{ym}/貼り付け 画像 2.png"));
        assert_eq!(std::fs::read(e.root.join(r2["path"].as_str().unwrap())).unwrap(), png);
        // 名前の消毒（区切り・..・制御文字・リンクに使えない字）と拡張子の推定
        let r3 = save_blob(&e.ctx, &json!({"name": "../../etc/a\u{1}b|c#d.png", "data_base64": b64})).unwrap();
        assert_eq!(r3["path"], format!("attachments/{ym}/a-b-c-d.png"));
        let r4 = save_blob(&e.ctx, &json!({"name": "..", "data_base64": b64})).unwrap();
        assert_eq!(r4["path"], format!("attachments/{ym}/pasted.png"));
        let txt = base64::engine::general_purpose::STANDARD.encode(b"hello");
        assert!(save_blob(&e.ctx, &json!({"name": "a.md", "data_base64": txt})).unwrap_err().to_string().contains("画像"));
        // 既知の拡張子で終わらない名前は中身から推す（"a.md" の png = a.md.png・"shot 1.0" = shot 1.0.png）
        assert_eq!(save_blob(&e.ctx, &json!({"name": "shot 1.0", "data_base64": b64})).unwrap()["path"], format!("attachments/{ym}/shot 1.0.png"));
        // 見えない書式文字（U+202E・U+200B・U+2028）は落とす
        let r5 = save_blob(&e.ctx, &json!({"name": "ab\u{202E}c\u{200B}d\u{2028}.png", "data_base64": b64})).unwrap();
        assert_eq!(r5["path"], format!("attachments/{ym}/abcd.png"));
        assert!(save_blob(&e.ctx, &json!({"name": "a.png", "data_base64": "!!!"})).is_err());
        let big = "A".repeat(BLOB_MAX / 3 * 4 + 100);
        assert!(save_blob(&e.ctx, &json!({"name": "a.png", "data_base64": big})).unwrap_err().to_string().contains("大きすぎ"));
        // 索引に入った
        assert!(commands::read(&e.ctx, &json!({"path": r["path"]})).is_ok());
        // ワークスペースの外からのコピー（和文名・同名は連番）
        let out = e.base.join("home/Desktop");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("スキャン 画像.pdf"), "%PDF-1.4").unwrap();
        let i1 = import_file(&e.ctx, &json!({"abs": out.join("スキャン 画像.pdf")})).unwrap();
        assert_eq!(i1, json!({"path": format!("attachments/{ym}/スキャン 画像.pdf"), "embed": format!("![[attachments/{ym}/スキャン 画像.pdf]]"), "copied": true}));
        let i2 = import_file(&e.ctx, &json!({"abs": out.join("スキャン 画像.pdf")})).unwrap();
        assert_eq!(i2["path"], format!("attachments/{ym}/スキャン 画像 2.pdf"));
        assert!(out.join("スキャン 画像.pdf").exists(), "元は残す");
        // ワークスペースの中 = コピーしない
        put(&e.root, "media/v.mp4", "mp4");
        put(&e.root, "page.md", "p");
        assert_eq!(import_file(&e.ctx, &json!({"abs": e.root.join("media/v.mp4")})).unwrap(), json!({"path": "media/v.mp4", "embed": "![[media/v.mp4]]", "copied": false}));
        assert_eq!(import_file(&e.ctx, &json!({"abs": e.root.join("page.md")})).unwrap()["embed"], "[[page.md]]");
        // 外の md・相対パス・フォルダは拒否
        std::fs::write(out.join("n.md"), "x").unwrap();
        assert!(import_file(&e.ctx, &json!({"abs": out.join("n.md")})).is_err());
        assert!(import_file(&e.ctx, &json!({"abs": "x.png"})).is_err());
        assert!(import_file(&e.ctx, &json!({"abs": out})).is_err());
    }

    #[test]
    fn trash_with_mover() {
        let e = env("tr");
        put(&e.root, "a.md", "x");
        put(&e.root, "b.md", "[[a]]");
        commands::status(&e.ctx, &json!({})).unwrap();
        let gone = e.base.join("gone");
        let r = trash_with(&e.ctx, &json!({"path": "a.md"}), |p| std::fs::rename(p, &gone).map(|_| "test".into()).map_err(|e| e.to_string())).unwrap();
        assert_eq!(r, json!({"path": "a.md", "method": "test"}));
        assert!(!e.root.join("a.md").exists());
        assert_eq!(commands::status(&e.ctx, &json!({})).unwrap()["unresolved"], 1);
        assert!(trash_with(&e.ctx, &json!({"path": "../x"}), |_| Ok("x".into())).is_err());
        std::os::unix::fs::symlink(e.root.join("b.md"), e.root.join("l.md")).unwrap();
        assert!(trash_with(&e.ctx, &json!({"path": "l.md"}), |_| panic!("symlink は渡さない")).unwrap_err().to_string().contains("symlink"));
        assert!(trash_with(&e.ctx, &json!({"path": "b.md"}), |_| Err("拒否".into())).unwrap_err().to_string().contains("拒否"));
        assert!(e.root.join("b.md").exists());
    }

    /// フォルダは中身ごと（中のページは索引から消え、外からのリンクは未解決に）。root 自体は拒否
    #[test]
    fn trash_dir_with_mover() {
        let e = env("trdir");
        put(&e.root, "d/x.md", "x");
        put(&e.root, "d/sub/y.md", "[[x]]");
        put(&e.root, "d/sub/img.png", "p");
        put(&e.root, "keep.md", "[[y]] [[x]]");
        commands::status(&e.ctx, &json!({})).unwrap();
        assert_eq!(commands::status(&e.ctx, &json!({})).unwrap()["unresolved"], 0);
        put(&e.root, "d/.DS_Store", "junk");
        std::os::unix::fs::symlink(e.root.join("keep.md"), e.root.join("d/link.md")).unwrap();
        // 外部で消えたのに索引に残っているページ（鮮度規則を待たず、フォルダの trash で索引から外れる）
        std::fs::remove_file(e.root.join("d/sub/y.md")).unwrap();
        // 期待とちがう形は移さない
        assert!(trash_with(&e.ctx, &json!({"path": "d", "dir": false}), |_| panic!("渡さない")).unwrap_err().to_string().contains("フォルダです"));
        assert!(trash_with(&e.ctx, &json!({"path": "keep.md", "dir": true}), |_| panic!("渡さない")).unwrap_err().to_string().contains("フォルダではありません"));
        let gone = e.base.join("gone-dir");
        let r = trash_with(&e.ctx, &json!({"path": "d", "dir": true}), |p| std::fs::rename(p, &gone).map(|_| "test".into()).map_err(|e| e.to_string())).unwrap();
        assert_eq!(r, json!({"path": "d", "method": "test", "dir": true, "files": 2}), "x.md と img.png（. 始まり・symlink・消えた y.md は数えない）: {r}");
        assert!(!e.root.join("d").exists() && gone.join("sub/img.png").is_file() && e.root.join("keep.md").is_file());
        assert_eq!(commands::status(&e.ctx, &json!({})).unwrap()["unresolved"], 2, "keep.md の 2 本が未解決に");
        let pages = commands::list(&e.ctx, &json!({})).unwrap();
        assert!(pages["pages"].as_array().unwrap().iter().all(|p| !p["path"].as_str().unwrap().starts_with("d/")), "索引に d/ の残骸: {pages}");
        for bad in ["", ".", "/", ".obsidian"] {
            assert!(trash_with(&e.ctx, &json!({"path": bad}), |_| panic!("渡さない: {bad}")).is_err(), "{bad}");
        }
        // symlink のフォルダは末尾 `/` を付けても拒否（lstat が辿らないように）
        std::fs::create_dir(e.root.join("real")).unwrap();
        put(&e.root, "real/r.md", "r");
        std::os::unix::fs::symlink(e.root.join("real"), e.root.join("alias")).unwrap();
        for p in ["alias", "alias/", "alias//"] {
            assert!(trash_with(&e.ctx, &json!({"path": p}), |_| panic!("渡さない: {p}")).unwrap_err().to_string().contains("symlink"), "{p}");
        }
        assert!(e.root.join("real/r.md").is_file());
    }

    /// M1: 5 秒以内に外部が書いたリンクも改名で数える（鮮度規則を待たず stat 走査）
    #[test]
    fn rename_sees_links_written_just_before() {
        let e = env("fresh");
        put(&e.root, "A.md", "a");
        commands::status(&e.ctx, &json!({})).unwrap(); // 索引を作る（最終走査 = 今）
        put(&e.root, "new.md", "[[A]]"); // 直後に外部が書く
        let p = rename_preview(&e.ctx, &json!({"from": "A.md", "to": "B.md"})).unwrap();
        assert_eq!((p["links"].as_u64(), p["files"].clone()), (Some(1), json!(["new.md"])), "{p}");
        rename(&e.ctx, &json!({"from": "A.md", "to": "B.md", "rewrite": true})).unwrap();
        assert_eq!(read(&e.root, "new.md"), "[[B]]");
        assert_eq!(commands::status(&e.ctx, &json!({})).unwrap()["unresolved"], 0);
    }

    /// M2: 作る名前に `[ ] | # ^ \ :` は使えない（既存の名前の上書きは可）。frontmatter の引用を壊す名前は書き換えず other に
    #[test]
    fn new_names_are_checked_and_frontmatter_quotes_kept() {
        let e = env("names");
        put(&e.root, "A.md", "a");
        put(&e.root, "r.md", "---\nrel: \"[[A]]\"\n---\n[[A]]\n");
        for bad_name in ["x[1].md", "a|b.md", "Case #12.md", "a^b.md", "a:b.md", "a\\b.md"] {
            let err = rename(&e.ctx, &json!({"from": "A.md", "to": bad_name, "rewrite": true})).unwrap_err().to_string();
            assert!(err.contains("使えません"), "{bad_name}: {err}");
            assert!(move_(&e.ctx, &json!({"from": "A.md", "to": bad_name})).is_err(), "{bad_name}");
            assert!(write(&e.ctx, &json!({"path": bad_name, "content": "x"})).is_err(), "{bad_name}");
            assert!(create(&e.ctx, &json!({"name": bad_name})).is_err(), "{bad_name}");
        }
        assert!(write(&e.ctx, &json!({"path": "d#1/x.md", "content": "x"})).unwrap_err().to_string().contains("使えません"), "作るフォルダも");
        // 名前を変えない移動は既存の名前を検査しない（inbox → sources/）。作るフォルダは検査する
        put(&e.root, "ingest/Case #1.md", "c");
        assert_eq!(move_(&e.ctx, &json!({"from": "ingest/Case #1.md", "to": "sources/Case #1.md"})).unwrap()["path"], "sources/Case #1.md");
        assert!(move_(&e.ctx, &json!({"from": "sources/Case #1.md", "to": "s#2/Case #1.md"})).is_err());
        assert!(move_(&e.ctx, &json!({"from": "sources/Case #1.md", "to": "sources/Case #2.md"})).is_err(), "名前を変えるなら検査する");
        assert!(e.root.join("A.md").exists());
        // 既にある名前（他のツールが作った）は上書きできる
        put(&e.root, "old #1.md", "o");
        assert_eq!(write(&e.ctx, &json!({"path": "old #1.md", "content": "n"})).unwrap()["ok"], true);
        // 引用符を含む名前: 本文は書き換え・frontmatter の行は other
        let p = rename_preview(&e.ctx, &json!({"from": "A.md", "to": "say \"hi\".md"})).unwrap();
        assert_eq!(p["links"], 1, "{p}");
        assert_eq!(p["other"], json!([{"path": "r.md", "kind": "frontmatter"}]));
        rename(&e.ctx, &json!({"from": "A.md", "to": "say \"hi\".md", "rewrite": true})).unwrap();
        assert_eq!(read(&e.root, "r.md"), "---\nrel: \"[[A]]\"\n---\n[[say \"hi\"]]\n");
    }

    /// チャットのログ（chat-log/）は改名の書換え対象にせず other に出す（追記だけの約束）。名前が前方一致だけのページは普通に書き換える。
    /// リンクは索引に残る = 改名後のログの [[旧名]] は未解決（履歴なので可）。ログ自身の改名でも中身を書き換えない
    #[test]
    fn rename_leaves_chat_log_alone() {
        let e = env("chatlog");
        put(&e.root, "a.md", "a");
        put(&e.root, "r.md", "[[a.md]]");
        let log = "# log\n\n文脈: [[a.md]] [r](../r.md)\n";
        put(&e.root, "chat-log/2026-11-02 0900 x.md", log);
        put(&e.root, "chat-log.md", "[[a.md]]");
        put(&e.root, "chat-logs/y.md", "[[a.md]]");
        let p = rename_preview(&e.ctx, &json!({"from": "a.md", "to": "b.md"})).unwrap();
        assert_eq!((p["links"].as_u64(), p["files"].clone()), (Some(3), json!(["chat-log.md", "chat-logs/y.md", "r.md"])), "{p}");
        assert_eq!(p["other"], json!([{"path": "chat-log/2026-11-02 0900 x.md", "kind": "chat-log"}]), "{p}");
        let r = rename(&e.ctx, &json!({"from": "a.md", "to": "b.md", "rewrite": true})).unwrap();
        assert_eq!(r["rewritten"], json!(["chat-log.md", "chat-logs/y.md", "r.md"]), "{r}");
        assert_eq!(read(&e.root, "chat-log/2026-11-02 0900 x.md"), log, "ログは書き換えない");
        assert_eq!(read(&e.root, "r.md"), "[[b.md]]");
        assert_eq!((read(&e.root, "chat-log.md"), read(&e.root, "chat-logs/y.md")), ("[[b.md]]".into(), "[[b.md]]".into()));
        // 索引にはログのリンクが残る（未解決 1 = ログの [[a.md]]）
        assert_eq!(commands::status(&e.ctx, &json!({})).unwrap()["unresolved"], 1);
        // ログ自身を改名（フォルダが変わる）: 自分のリンクも書き換えない
        let p = rename_preview(&e.ctx, &json!({"from": "chat-log/2026-11-02 0900 x.md", "to": "old/deep/x.md"})).unwrap();
        assert_eq!((p["links"].as_u64(), p["files"].clone()), (Some(0), json!([])), "{p}");
    }

    /// m1: 大小だけの改名で旧い表記の行が索引に残らない
    #[test]
    fn case_only_rename_leaves_no_stale_row() {
        let e = env("case");
        put(&e.root, "note.md", "n");
        put(&e.root, "r.md", "[[note]]");
        commands::status(&e.ctx, &json!({})).unwrap();
        rename(&e.ctx, &json!({"from": "note.md", "to": "Note.md", "rewrite": true})).unwrap();
        let paths: Vec<String> = commands::list(&e.ctx, &json!({})).unwrap()["pages"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap().to_string()).collect();
        assert_eq!(paths, vec!["Note.md", "r.md"]);
        assert_eq!(read(&e.root, "r.md"), "[[Note]]");
        move_(&e.ctx, &json!({"from": "Note.md", "to": "NOTE.md"})).unwrap();
        let paths: Vec<String> = commands::list(&e.ctx, &json!({})).unwrap()["pages"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap().to_string()).collect();
        assert_eq!(paths, vec!["NOTE.md", "r.md"]);
    }

    /// m2: 書く直前の照合（読んだ後に外部が書き換えた・新規のはずが外部に作られた）= 書かずに Changed
    #[test]
    fn write_atomic_rechecks_before_rename() {
        let e = env("race");
        let f = e.root.join("a.md");
        std::fs::write(&f, "v1").unwrap();
        let before = seen(&f).unwrap();
        std::fs::write(&f, "external!").unwrap(); // 大きさも変える
        assert_eq!(write_atomic(&f, b"mine", Some(before)).unwrap(), Wrote::Changed);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "external!");
        let g = e.root.join("new.md");
        std::fs::write(&g, "外部が先に作った").unwrap();
        assert_eq!(write_atomic(&g, b"mine", None).unwrap(), Wrote::Changed);
        assert_eq!(std::fs::read_to_string(&g).unwrap(), "外部が先に作った");
        let left: Vec<_> = std::fs::read_dir(&e.root).unwrap().flatten().filter(|d| d.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(left.is_empty());
    }

    /// m3: 拡張属性を引き継ぐ・読み取り専用は拒否
    #[test]
    fn write_keeps_xattr_and_rejects_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let e = env("xattr");
        put(&e.root, "a.md", "v1");
        let f = e.root.join("a.md");
        let st = std::process::Command::new("/usr/bin/xattr").args(["-w", "com.openref.test", "keep"]).arg(&f).status().unwrap();
        assert!(st.success());
        write(&e.ctx, &json!({"path": "a.md", "content": "v2"})).unwrap();
        let out = std::process::Command::new("/usr/bin/xattr").args(["-p", "com.openref.test"]).arg(&f).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "keep");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
        let n = hist(&e, "a.md").len();
        assert!(write(&e.ctx, &json!({"path": "a.md", "content": "v3"})).unwrap_err().to_string().contains("読み取り専用"));
        assert_eq!(read(&e.root, "a.md"), "v2");
        assert_eq!(hist(&e, "a.md").len(), n, "拒否した書込で退避を増やさない");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// m4: 退避の鍵は vault ごと・今作った退避は時計が戻っても消さない
    #[test]
    fn history_key_per_vault_and_keeps_newest() {
        let e = env("hist");
        let other_root = e.base.join("home/Documents/vault2");
        assert_ne!(history_dir(&e.app, &e.root, "a.md"), history_dir(&e.app, &other_root, "a.md"));
        let dir = history_dir(&e.app, &e.root, "a.md");
        std::fs::create_dir_all(&dir).unwrap();
        // 未来の時刻の世代が 10 ある（時計が戻った）
        let future = 4_000_000_000u64;
        for i in 0..HISTORY_KEEP {
            std::fs::write(dir.join(format!("{}.md", future + i as u64)), "old").unwrap();
        }
        let p = backup(&e.app, &e.root, "a.md", b"now").unwrap();
        assert!(p.exists(), "今作った退避は消さない");
        assert_eq!(history_files(&dir).len(), HISTORY_KEEP);
    }

    /// import のコピー（中身を一時名に書く）は edit_lock の外。置くときだけ lock を取る
    #[test]
    fn place_new_fills_outside_lock() {
        let e = env("lock");
        let dir = e.root.join("attachments/x");
        let name = place_new(&dir, "a.png", |tmp| {
            // 別スレッドが lock を取れる = fill の間は lock を持っていない
            // （他のテストが一瞬 lock を持つことがあるので何度か試す）
            let ok = std::thread::spawn(|| (0..200).any(|_| EDIT.try_lock().is_ok() || { std::thread::sleep(std::time::Duration::from_millis(5)); false })).join().unwrap();
            assert!(ok, "fill の間に edit_lock を持っている");
            std::fs::write(tmp, b"png")
        })
        .unwrap();
        assert_eq!(name, "a.png");
        assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), b"png");
    }

    #[test]
    fn rename_excl_falls_back_when_unsupported() {
        let e = env("excl");
        let d = e.root.clone();
        let os = |c: i32| {
            move |_: &Path, _: &Path| -> std::io::Result<()> {
                Err(std::io::Error::from_raw_os_error(c))
            }
        };
        let never = |_: &Path, _: &Path| -> std::io::Result<()> {
            panic!("この段へ進んではいけない")
        };
        let fresh = |n: &str| {
            let p = d.join(n);
            std::fs::write(&p, n).unwrap();
            p
        };
        let link = |f: &Path, t: &Path| std::fs::hard_link(f, t);
        // RENAME_EXCL が ENOTSUP / EOPNOTSUPP / EINVAL → hard link + remove
        for c in [libc::ENOTSUP, libc::EOPNOTSUPP, libc::EINVAL] {
            let (from, to) = (fresh(&format!(".l{c}.tmp")), d.join(format!("l{c}.md")));
            rename_excl_with(&from, &to, os(c), link).unwrap();
            assert_eq!(std::fs::read_to_string(&to).unwrap(), format!(".l{c}.tmp"));
            assert!(!from.exists(), "一時ファイルが残った");
        }
        // hard link も ENOTSUP / EOPNOTSUPP / EPERM → O_EXCL で作って写す（mode と拡張属性も写す）
        for c in [libc::ENOTSUP, libc::EOPNOTSUPP, libc::EPERM] {
            use std::os::unix::fs::PermissionsExt;
            let (from, to) = (fresh(&format!(".c{c}.tmp")), d.join(format!("c{c}.md")));
            std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o640)).unwrap();
            #[cfg(target_os = "macos")]
            xattr_set(&from, "dev.openref.test", b"tag");
            rename_excl_with(&from, &to, os(libc::ENOTSUP), os(c)).unwrap();
            assert_eq!(std::fs::read_to_string(&to).unwrap(), format!(".c{c}.tmp"));
            assert_eq!(
                std::fs::metadata(&to).unwrap().permissions().mode() & 0o777,
                0o640
            );
            #[cfg(target_os = "macos")]
            assert_eq!(
                xattr_get(&to, "dev.openref.test").as_deref(),
                Some(&b"tag"[..])
            );
            assert!(!from.exists(), "一時ファイルが残った");
        }
        // 写す途中で失敗（読めない from）→ 自分が作った to を消す
        let to = d.join("half.md");
        let r = rename_excl_with(
            &d.join(".missing.tmp"),
            &to,
            os(libc::ENOTSUP),
            os(libc::ENOTSUP),
        );
        assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::NotFound);
        assert!(!to.exists(), "途中のファイルが残った");
        // 許可リストの外（EIO）は fallback へ進まない: RENAME_EXCL の EIO は hard link へ・hard link の EIO は写す段へ進まない
        let (from, to) = (fresh(".io.tmp"), d.join("io.md"));
        let r = rename_excl_with(&from, &to, os(libc::EIO), never);
        assert_eq!(r.unwrap_err().raw_os_error(), Some(libc::EIO));
        assert!(!to.exists());
        let r = rename_excl_with(&from, &to, os(libc::ENOTSUP), os(libc::EIO));
        assert_eq!(r.unwrap_err().raw_os_error(), Some(libc::EIO));
        assert!(!to.exists(), "写す段へ進んだ");
        std::fs::remove_file(&from).unwrap();
        // 写す段で同名が既にある → AlreadyExists・上書きしない・一時ファイルは呼び手のために残す（place_new が次の名前で使う）
        let (from, to) = (fresh(".x.tmp"), fresh("x.md"));
        let r = rename_excl_with(&from, &to, os(libc::ENOTSUP), os(libc::ENOTSUP));
        assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "x.md");
        assert!(from.exists());
        // hard link の段で同名が既にある → AlreadyExists（写す段へ進まない）
        let r = rename_excl_with(&from, &to, os(libc::ENOTSUP), link);
        assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "x.md");
        // RENAME_EXCL の EEXIST・他の失敗（EACCES / hard link の EXDEV）はそのまま返す（fallback へ進まない）
        assert_eq!(
            rename_excl_with(&from, &to, os(libc::EEXIST), never)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            rename_excl_with(&from, &to, os(libc::EACCES), never)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EACCES)
        );
        let r = rename_excl_with(&from, &d.join("y.md"), os(libc::ENOTSUP), os(libc::EXDEV));
        assert_eq!(r.unwrap_err().raw_os_error(), Some(libc::EXDEV));
        assert!(!d.join("y.md").exists());
        // 本物（この tempdir = APFS）は RENAME_EXCL で置く
        let to = d.join("real.md");
        rename_excl(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), ".x.tmp");
        assert!(!from.exists());
    }

    #[cfg(target_os = "macos")]
    fn xattr_set(p: &Path, name: &str, v: &[u8]) {
        use std::os::unix::ffi::OsStrExt;
        let (c, n) = (
            std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap(),
            std::ffi::CString::new(name).unwrap(),
        );
        // SAFETY: NUL 終端の C 文字列と長さ付きのバッファを渡すだけ
        assert_eq!(
            unsafe { libc::setxattr(c.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0, 0) },
            0
        );
    }

    #[cfg(target_os = "macos")]
    fn xattr_get(p: &Path, name: &str) -> Option<Vec<u8>> {
        use std::os::unix::ffi::OsStrExt;
        let (c, n) = (
            std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap(),
            std::ffi::CString::new(name).unwrap(),
        );
        let mut buf = [0u8; 64];
        // SAFETY: NUL 終端の C 文字列と長さ付きのバッファを渡すだけ
        let r = unsafe {
            libc::getxattr(
                c.as_ptr(),
                n.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
                0,
            )
        };
        (r >= 0).then(|| buf[..r as usize].to_vec())
    }

    /// RENAME_EXCL や hard link の無いボリューム（exFAT / FAT32）での実弾。場所 = ディスクイメージの中（env で受ける）:
    /// `hdiutil create -size 64m -fs ExFAT -volname X <dir>/x.dmg` → `hdiutil attach -nobrowse -mountpoint <mnt> <dir>/x.dmg` →
    /// `OPENREF_WIKI_EXFAT_ROOT=<mnt> cargo test -p openref-core fallback_on_exfat -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn fallback_on_exfat_volume() {
        let vol = std::fs::canonicalize(
            std::env::var("OPENREF_WIKI_EXFAT_ROOT").expect("OPENREF_WIKI_EXFAT_ROOT"),
        )
        .unwrap();
        let _serial = crate::wiki::index::test_serial();
        let base = vol.join(format!(
            "openref-wikiedit-{}-{:x}",
            std::process::id(),
            fastrand::u64(..)
        ));
        let root = base.join("vault");
        let app = std::env::temp_dir().join(format!(
            "openref-wikiedit-exfat-app-{}-{:x}",
            std::process::id(),
            fastrand::u64(..)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        let ctx = Ctx {
            enabled: true,
            root: Some(root.to_string_lossy().into()),
            app_dir: Some(app.clone()),
            home: Some(vol.clone()),
            watched: false,
        };
        // 素の errno（記録用）
        let probe = root.join(".probe.tmp");
        std::fs::write(&probe, b"p").unwrap();
        let excl = |f: &Path, t: &Path| {
            use std::os::unix::ffi::OsStrExt;
            let f = std::ffi::CString::new(f.as_os_str().as_bytes()).unwrap();
            let t = std::ffi::CString::new(t.as_os_str().as_bytes()).unwrap();
            // SAFETY: NUL 終端の C 文字列を渡すだけ
            if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        };
        let r = excl(&probe, &root.join("probe.md"));
        eprintln!("renamex_np(RENAME_EXCL) = {r:?}");
        if r.is_ok() {
            std::fs::rename(root.join("probe.md"), &probe).unwrap();
        }
        let r = std::fs::hard_link(&probe, root.join("probe.md"));
        eprintln!("hard_link = {r:?}");
        let _ = std::fs::remove_file(root.join("probe.md"));
        std::fs::remove_file(&probe).unwrap();
        // 新規の write（write_atomic の None）
        let r = write(&ctx, &json!({"path": "新規/ページ.md", "content": "# 題"}));
        eprintln!("write 新規 = {r:?}");
        let r = r.unwrap();
        assert_eq!(r["created"], true);
        assert_eq!(read(&root, "新規/ページ.md"), "# 題");
        // 新規のはずが既にある = Changed（上書きしない）
        let abs = root.join("新規/ページ.md");
        assert_eq!(write_atomic(&abs, b"x", None).unwrap(), Wrote::Changed);
        assert_eq!(read(&root, "新規/ページ.md"), "# 題");
        // 既存の上書き（std::fs::rename・この単位では変えない経路）
        let r = write_atomic(&abs, b"# 2", seen(&abs));
        eprintln!("write_atomic 上書き = {r:?}");
        assert_eq!(r.unwrap(), Wrote::Ok);
        assert_eq!(read(&root, "新規/ページ.md"), "# 2");
        // create（place_new・同名は連番）
        let r = create(&ctx, &json!({}));
        eprintln!("create = {r:?}");
        assert_eq!(r.unwrap()["path"], "Untitled.md");
        assert_eq!(create(&ctx, &json!({})).unwrap()["path"], "Untitled 2.md");
        // save_blob / import_file（attachments/<YYYY-MM>/）
        let b64 = "iVBORw0KGgo=";
        let r = save_blob(&ctx, &json!({"name": "a.png", "data_base64": b64}));
        eprintln!("save_blob = {r:?}");
        let p1 = r.unwrap()["path"].as_str().unwrap().to_string();
        let p2 = save_blob(&ctx, &json!({"name": "a.png", "data_base64": b64})).unwrap()["path"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            p1.ends_with("/a.png") && p2.ends_with("/a 2.png"),
            "{p1} {p2}"
        );
        let src = app.join("取込.png");
        std::fs::write(&src, b"\x89PNG\r\n\x1a\n").unwrap();
        let r = import_file(&ctx, &json!({"abs": src.to_string_lossy()}));
        eprintln!("import_file = {r:?}");
        let p3 = r.unwrap()["path"].as_str().unwrap().to_string();
        assert_eq!(std::fs::read(root.join(&p3)).unwrap(), b"\x89PNG\r\n\x1a\n");
        // 一時ファイル（と exFAT が作る `._` の相棒）が残っていない
        let mut left = vec![];
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for en in std::fs::read_dir(&d).unwrap().flatten() {
                let n = en.file_name().to_string_lossy().into_owned();
                if en.file_type().unwrap().is_dir() {
                    stack.push(en.path());
                } else if n.ends_with(".tmp") {
                    left.push(n);
                }
            }
        }
        assert!(left.is_empty(), "一時ファイルが残った: {left:?}");
        // exFAT は rename で上書きした後のファイルを readdir の名前（NFD）で unlink できない（ENOENT・NFC なら消える = 実測）→ remove_dir_all が止まる
        fn rm_tree(d: &Path) {
            for en in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = en.path();
                if en.file_type().is_ok_and(|t| t.is_dir()) {
                    rm_tree(&p);
                } else if std::fs::remove_file(&p).is_err() {
                    let _ = std::fs::remove_file(
                        d.join(en.file_name().to_string_lossy().nfc().collect::<String>()),
                    );
                }
            }
            let _ = std::fs::remove_dir(d);
        }
        rm_tree(&base);
        assert!(!base.exists(), "後片付けできない: {}", base.display());
        let _ = std::fs::remove_dir_all(&app);
    }

    #[test]
    fn helpers() {
        assert_eq!(relpath("a/b", "a/c/d.md"), "../c/d.md");
        assert_eq!(relpath("", "x.md"), "x.md");
        assert_eq!(relpath("a", "x.md"), "../x.md");
        assert_eq!(encode_dest("a b(1).md", false, false), "a%20b%281%29.md");
        assert_eq!(encode_dest("和 文.md", false, true), "%E5%92%8C%20%E6%96%87.md");
        assert_eq!(encode_dest("a b.md", true, false), "a b.md");
        assert_eq!(sanitize_name("  .x.png").as_deref(), Some("x.png"));
        assert_eq!(sanitize_name("---"), None);
        let long = format!("{}.png", "あ".repeat(300));
        let s = sanitize_name(&long).unwrap();
        assert_eq!(s.chars().count(), 120);
        assert!(s.ends_with(".png"));
        assert!(year_month().len() == 7);
    }
}
