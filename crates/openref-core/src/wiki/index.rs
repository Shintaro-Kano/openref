//! 索引 `<app_dir>/wiki-index/<root の sha256 先頭 16>.sqlite`（ワークスペースごと = `index_path`。以前は
//! `<app_dir>/wiki-index.sqlite` の 1 本 = `INDEX_FILE`）。`openref.sqlite` とは別ファイル・消しても再構築で戻るキャッシュ。
//! 所有者は core: GUI・CLI・MCP のどの入口も同じ鮮度規則（`refresh_if_stale`）で更新する。
//!
//! - 接続は WAL・busy_timeout 5s。プロセスに 1 つ（`with_index` = `OnceLock<Mutex<Option<Index>>>`・索引パスか root が変われば開き直す）
//! - 鮮度: meta の最終走査（プロセス間で共有）から 5 秒以上なら stat 走査 → mtime / size の変わったものだけ再解析
//! - 大きな走査（初回・作り直し・変更 500 件超）は `meta.rebuilding = <pid>:<開始 unix 秒>` を `BEGIN IMMEDIATE` で立ててから行う。
//!   他のプロセスは待たずに「作り直し中（n%）」を返す。10 分を過ぎた旗（落ちたプロセスの物）と自分の pid の旗は引き継ぐ
//! - 索引が壊れている（SQLITE_CORRUPT / SQLITE_NOTADB = malformed・not a database）と分かったら、ファイルを
//!   `<名前>.sqlite.corrupt-<unix 秒>`（1 世代だけ）へ退避して作り直す（`with_index`。索引は再生成できるキャッシュ）。
//!   別の接続で開き直して壊れていなければ退避しない（他のプロセスが先に作り直した索引を捨てない）。作り直しは通常の鮮度規則
//!   （大きな走査は rebuilding の旗）で行う
//! - `meta.root` が違えば中身を捨てる。`meta.schema` が違えば表を作り直す（版 2 = 全文検索 pages_fts を足した）
//! - 全文検索 `pages_fts(title, body)` は FTS5 の **contentless**（`content=''`・`contentless_delete=1`）。両列とも
//!   `model::cjk_spaced`（和文は 1 字 1 トークン・英数はそのまま）で入れ、問い合わせ側も同じ変換の phrase にする（蔵書の
//!   items_fts / fulltext_fts と同じ方式）。本文の写しを索引に持たない（93MB の wiki で 311MB → 54MB）ので、抜粋は
//!   結果のファイルを読み直して作る（`snippet`）

use super::links::{LinkKind, Resolver};
use super::parse_md::Parsed;
use super::scan::{self, Entry, Kind};
use super::{parse_html, parse_md};
use crate::{CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use unicode_normalization::UnicodeNormalization;

/// 表の版。表の形を変えたら上げる（開いたときに作り直す）
pub const SCHEMA_VERSION: &str = "2";
/// 最終走査からこれ未満なら走査しない
const STALE_MS: i64 = 5_000;
/// rebuilding の旗の寿命
const FLAG_TTL_S: i64 = 600;
/// 旗を立てる走査の大きさ（再解析する件数）
const BIG_SCAN: usize = 500;
/// 1 トランザクションで書くページ数
const BATCH: usize = 300;
/// 本文を読む上限（これより大きい md / html は本文を解析しない = リンク・tags なし）
pub const BODY_MAX: i64 = 16 * 1024 * 1024;
/// 旧の索引のファイル名（app_dir の直下）。以前の置き場 = GUI の起動時の掃除が消す（今の置き場は `index_path`。他は quarantine の名前の控えだけ）
pub const INDEX_FILE: &str = "wiki-index.sqlite";
/// 索引の置き場（app_dir の下のフォルダ・ワークスペースごとに 1 ファイル = `index_path`）
pub const INDEX_DIR: &str = "wiki-index";

/// このワークスペースの索引 = `<app_dir>/wiki-index/<sha256_hex(root の文字列) の先頭 16 桁>.sqlite`。root は `check_root` の戻り
/// （canonical）。文字列は NFC にしてから hash する（退避 wiki-history/ の鍵 `edit::history_dir` と同じ型 = 同じ root が NFC / NFD の
/// どちらの綴りで来ても同じファイル）。GUI / MCP / CLI が `Ctx::ready()` 経由で同じ規則を使う
pub fn index_path(app_dir: &Path, root: &Path) -> PathBuf {
    let s: String = root.to_string_lossy().nfc().collect();
    app_dir.join(INDEX_DIR).join(format!("{}.sqlite", &sha256_hex(s.as_bytes())[..16]))
}
/// 本文を読めなかったページの mtime の印。走査が出す mtime（0 以上。epoch 前・取れない = 0）と重ならない値にして、
/// 次の走査で必ず mtime が合わず再試行される
pub const READ_ERROR_MTIME: i64 = -1;

/// `refresh_paths` の結果（vault 相対パス・届いた順・重複なし）。added = 索引に無かった / modified = あった（中身を読み直した）/ removed = 消えたので外した。
/// stat が拒否されたパス（据え置き）・隠し・vault の外はどれにも入らない
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub removed: Vec<String>,
}

impl Refreshed {
    /// 取り込み直した / 外した件数
    pub fn len(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 鮮度の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fresh {
    Ok,
    /// 他のプロセスが作り直し中（進捗 %）
    Rebuilding(u32),
}

pub struct Index {
    conn: Connection,
    path: PathBuf,
    root: PathBuf,
    /// 自分が立てた rebuilding の旗の開始時刻（立てていなければ None）
    flag_start: Option<i64>,
}

/// pages の 1 行
#[derive(Debug, Clone, PartialEq)]
pub struct PageRow {
    pub path: String,
    pub kind: String,
    pub title: String,
    pub tags: Vec<String>,
    pub items: Vec<String>,
    pub mtime_ns: i64,
    pub size: i64,
    pub sha256: Option<String>,
}

impl PageRow {
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path, "kind": self.kind, "title": self.title, "tags": self.tags, "items": self.items,
            "mtime": self.mtime_ns / 1_000_000_000, "size": self.size,
        })
    }
}

/// links の 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRow {
    pub src: String,
    pub kind: String,
    pub target: String,
    pub path: Option<String>,
    pub ambiguous: bool,
}

impl LinkRow {
    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind, "target": self.target, "path": self.path, "ambiguous": self.ambiguous})
    }
}

/// list の条件
#[derive(Debug, Clone, Default)]
pub struct ListQuery {
    /// vault 相対のパスの前方一致（大小区別）
    pub prefix: Option<String>,
    pub kind: Option<String>,
    /// tag（大小無視・複数は AND）
    pub tags: Vec<String>,
    pub limit: usize,
}

/// search の条件（query も tags も空なら呼び手が弾く）
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// 空白区切りの語（AND）。題名の部分一致 + 全文（FTS）
    pub query: String,
    /// tag の AND（大小無視）
    pub tags: Vec<String>,
    pub limit: usize,
}

/// search の 1 件
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub row: PageRow,
    /// 題名の一致: 0 = 完全一致・1 = 前方一致・2 = 全語を含む・None = 題名は一致しない
    pub title_match: Option<u8>,
    /// 全文（FTS）で一致したか
    pub body_match: bool,
    /// FTS の bm25（小さいほど良い・題名だけの一致は 0）
    pub score: f64,
}

/// 検索語 → 語の列（NFC・`"` を除く・英数字も和文も含まない語は捨てる）
pub fn query_terms(q: &str) -> Vec<String> {
    let q: String = q.nfc().collect();
    q.split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        .collect()
}

/// FTS5 の MATCH 式: 語ごとに `model::cjk_spaced` の phrase（AND）。最後の語は前方一致（入力途中の語を引く）
fn fts_match(terms: &[String]) -> String {
    let n = terms.len();
    terms
        .iter()
        .enumerate()
        .map(|(i, t)| format!("\"{}\"{}", crate::model::cjk_spaced(t), if i + 1 == n { "*" } else { "" }))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 題名の照合用: NFC・小文字・英数字と和文（`char::is_alphanumeric`）以外を落とし、空白は 1 つに（両側に同じ変換）
pub fn title_key(s: &str) -> String {
    let low: String = s.nfc().collect::<String>().to_lowercase();
    let mut out = String::with_capacity(low.len());
    for c in low.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if c.is_whitespace() && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim_end().to_string()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn sqlerr(e: rusqlite::Error) -> CoreError {
    CoreError::from(e)
}

/// 自プロセスの作り直しの進捗（%・-1 = していない）。`wiki_status` がロックを待たずに読む
static PROGRESS: AtomicI32 = AtomicI32::new(-1);

pub fn progress() -> Option<u32> {
    let p = PROGRESS.load(Ordering::Relaxed);
    (p >= 0).then_some(p as u32)
}

static GLOBAL: OnceLock<Mutex<Option<Index>>> = OnceLock::new();

/// このプロセスで最後に壊れた索引を作り直した記録（unix 秒, 退避先）。wiki_status が `recovered` に出す
static RECOVERED: Mutex<Option<(i64, String)>> = Mutex::new(None);

/// このプロセスで壊れた索引を退避して作り直したか（unix 秒・退避先のパス）
pub fn recovered() -> Option<(i64, String)> {
    RECOVERED.lock().ok().and_then(|g| g.clone())
}

/// SQLite が「壊れている / データベースではない」と言ったエラーか
pub fn is_corrupt(e: &CoreError) -> bool {
    matches!(e, CoreError::Sqlite(rusqlite::Error::SqliteFailure(f, _))
        if matches!(f.code, rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase))
}

/// プロセスに 1 つの索引で f を走らせる（索引パスか root が変われば開き直す）。
/// 開く・f の途中で索引が壊れていると分かったら、退避して（`quarantine`）新しい索引で f を 1 回だけやり直す。
/// 注意: f は 2 回走りうる = f の中に vault へのファイル書込などの副作用を入れるなら、やり直しで二重に効かないようにする
pub fn with_index<T>(index_path: &Path, root: &Path, mut f: impl FnMut(&mut Index) -> Result<T>) -> Result<T> {
    #[cfg(test)]
    let _held = InIndex::enter();
    let m = GLOBAL.get_or_init(|| Mutex::new(None));
    let mut g = m.lock().unwrap_or_else(|e| {
        // 前の呼び出しが panic した = 接続の状態が分からないので開き直す
        let mut g = e.into_inner();
        *g = None;
        g
    });
    let mut retried = false;
    loop {
        let r = (|| {
            if g.as_ref().is_none_or(|ix| ix.path != index_path || ix.root != root) {
                *g = None;
                *g = Some(Index::open(index_path, root)?);
            }
            f(g.as_mut().expect("直前に入れた"))
        })();
        match r {
            Err(e) if !retried && is_corrupt(&e) => {
                retried = true;
                *g = None; // 接続を閉じてから退避する
                quarantine(index_path, &e, Duration::from_secs(5))?;
            }
            r => return r,
        }
    }
}

/// 索引ファイルが今も壊れているか（別の接続 = 読み専用・作らない・busy_timeout 付きで `quick_check(1)`）。
/// Some(true) = CORRUPT / NOTADB が返った・quick_check が ok 以外を返した。Some(false) = 健全。
/// None = 判定できない（BUSY・開けない・無い等の一時的なもの）= 退避しない
fn corrupt_verdict(index_path: &Path, busy: Duration) -> Option<bool> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let c = match Connection::open_with_flags(index_path, flags) {
        Ok(c) => c,
        Err(e) => return is_corrupt(&CoreError::from(e)).then_some(true),
    };
    if c.busy_timeout(busy).is_err() {
        return None;
    }
    match c.query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0)) {
        Ok(v) => Some(v != "ok"),
        Err(e) => is_corrupt(&CoreError::from(e)).then_some(true),
    }
}

/// 壊れた索引を退避する: 別の接続で確かめ（`corrupt_verdict`）、壊れていると確定したときだけ前の退避を消し（1 世代だけ残す）、
/// -wal → 本体の順に `<名前>.corrupt-<unix 秒>[-wal]` へ移し（古い WAL が新しい空の索引に当たる窓を作らない）、-shm を消す。
/// 健全（他のプロセスが既に作り直した）・判定できない（BUSY 等）なら何もしない。stderr と RECOVERED（wiki_status の recovered）に残す
fn quarantine(index_path: &Path, why: &CoreError, busy: Duration) -> Result<()> {
    if corrupt_verdict(index_path, busy) != Some(true) {
        return Ok(());
    }
    let name = index_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| INDEX_FILE.into());
    let dir = index_path.parent().unwrap_or(Path::new("."));
    let old_prefix = format!("{name}.corrupt-");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&old_prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let now = now_ms() / 1000;
    let moved = dir.join(format!("{old_prefix}{now}"));
    let sib = |suffix: &str| PathBuf::from(format!("{}{suffix}", index_path.display()));
    let _ = std::fs::rename(sib("-wal"), PathBuf::from(format!("{}-wal", moved.display())));
    match std::fs::rename(index_path, &moved) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let _ = std::fs::remove_file(sib("-shm"));
    let moved_s = moved.to_string_lossy().to_string();
    eprintln!("openref: OpenNote の索引が壊れていたので作り直しました（{why}・退避: {moved_s}）");
    if let Ok(mut g) = RECOVERED.lock() {
        *g = Some((now, moved_s));
    }
    Ok(())
}

/// 索引のファイル（と -wal / -shm）を消す（GUI の Settings › OpenNote › 索引を削除）。プロセスの接続がこの索引なら
/// 閉じてから消す（作り直し中なら終わるまで待つ）。ロックを握ったまま消すので、消す間に他のスレッドが開き直すことはない。
/// 無いファイルは無視。他のプロセスが開いている接続は消したファイルを持ち続ける（次に開くとき作り直す）
pub fn remove_index(index_path: &Path) -> Result<()> {
    let m = GLOBAL.get_or_init(|| Mutex::new(None));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_some_and(|ix| ix.path == index_path) {
        *g = None;
    }
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{suffix}", index_path.display()));
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// テスト用: プロセス共有の索引（GLOBAL・PROGRESS・busy）に触るテストを直列にする。並列に走ると、他のテストの作り直し中に
/// `wiki_status` が `{rebuilding}` だけを返し・`refresh_if_stale` が Rebuilding を返して落ちる（製品は 1 プロセス 1 vault なので起きない）。
/// 索引を使うテストの環境（commands / edit の env・index の tmp）が取って drop まで持つ
#[cfg(test)]
pub(crate) fn test_serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
thread_local! {
    /// テスト用: このスレッドが with_index の中にいるか（commands の本文・抜粋の read がロックの中で呼ばれていないことを数える）
    static IN_INDEX: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
struct InIndex;
#[cfg(test)]
impl InIndex {
    fn enter() -> InIndex {
        IN_INDEX.with(|c| c.set(c.get() + 1));
        InIndex
    }
}
#[cfg(test)]
impl Drop for InIndex {
    fn drop(&mut self) {
        IN_INDEX.with(|c| c.set(c.get() - 1));
    }
}
#[cfg(test)]
pub(crate) fn in_index() -> bool {
    IN_INDEX.with(|c| c.get() > 0)
}

/// 索引（GLOBAL のロック）が作り直し中などで握られていれば true（待たずに確かめる）
pub fn busy() -> bool {
    GLOBAL.get().is_some_and(|m| matches!(m.try_lock(), Err(std::sync::TryLockError::WouldBlock)))
}

impl Index {
    /// 索引を開く（無ければ作る）。root は canonicalize 済みの vault（`wiki::check_root` の戻り値）。
    /// 表・版・root が合っていれば**読むだけ**（他のプロセスが作り直し中でもロックを取らない）。作る・捨てる必要があるときだけ
    /// `BEGIN IMMEDIATE`（busy_timeout が効く）で書く。読んでから書きに上げる deferred は同時に開くと即 BUSY になるので使わない
    pub fn open(index_path: &Path, root: &Path) -> Result<Index> {
        if let Some(d) = index_path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let conn = Connection::open(index_path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // WAL への切替（新しいファイルのときだけ）は排他ロックを要し、他のプロセスが同時に開いていると busy_timeout を待たずに
        // BUSY を返すことがある（SQLite の仕様）→ 5 秒まで小刻みに取り直す
        retry_busy(|| {
            if !conn.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))?.eq_ignore_ascii_case("wal") {
                conn.pragma_update(None, "journal_mode", "WAL")?;
            }
            Ok(())
        })?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let ix = Index { conn, path: index_path.to_path_buf(), root: root.to_path_buf(), flag_start: None };
        let root_s = root.to_string_lossy().to_string();
        if ix.needs_setup(&root_s)? {
            let tx = wtx(&ix.conn)?;
            // ロックを取ってから読み直す（同時に開いた他のプロセスが先に整えていれば何もしない）
            if ix.needs_setup(&root_s)? {
                tx.execute_batch("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")?;
                if ix.meta("schema")?.as_deref() != Some(SCHEMA_VERSION) {
                    tx.execute_batch(
                        "DROP TABLE IF EXISTS pages; DROP TABLE IF EXISTS links; DROP TABLE IF EXISTS tags; DROP TABLE IF EXISTS pages_fts;
                         DELETE FROM meta;",
                    )?;
                }
                tx.execute_batch(TABLES)?;
                set_meta(&tx, "schema", SCHEMA_VERSION)?;
                if ix.meta("root")?.as_deref() != Some(root_s.as_str()) {
                    // vault が変わった = 中身を捨てる
                    tx.execute_batch(CLEAR_ALL)?;
                    tx.execute("DELETE FROM meta WHERE key NOT IN ('schema')", [])?;
                    set_meta(&tx, "root", &root_s)?;
                }
            }
            tx.commit()?;
        }
        Ok(ix)
    }

    /// 表が揃い・版と root が合っているかを読むだけで確かめる（true = 書く必要がある）
    fn needs_setup(&self, root_s: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('meta', 'pages', 'links', 'tags', 'pages_fts')",
            [],
            |r| r.get(0),
        )?;
        if n < 5 {
            return Ok(true);
        }
        Ok(self.meta("schema")?.as_deref() != Some(SCHEMA_VERSION) || self.meta("root")?.as_deref() != Some(root_s))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    #[cfg(test)]
    pub(crate) fn conn_for_test(&self) -> &Connection {
        &self.conn
    }

    pub fn meta(&self, k: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [k], |r| r.get(0)).optional()?)
    }

    /// 他のプロセスの生きている旗なら Some(進捗 %)
    fn foreign_flag(&self) -> Result<Option<u32>> {
        match self.meta("rebuilding")? {
            Some(f) if flag_alive(&f) => Ok(Some(self.meta("progress")?.and_then(|p| p.parse().ok()).unwrap_or(0))),
            _ => Ok(None),
        }
    }

    /// 鮮度規則: 最終走査から 5 秒以上なら stat 走査して変わったものだけ再解析
    pub fn refresh_if_stale(&mut self) -> Result<Fresh> {
        if let Some(p) = self.foreign_flag()? {
            return Ok(Fresh::Rebuilding(p));
        }
        let last: i64 = self.meta("last_scan")?.and_then(|s| s.parse().ok()).unwrap_or(0);
        if now_ms() - last < STALE_MS {
            return Ok(Fresh::Ok);
        }
        self.refresh()
    }

    /// 今すぐ stat 走査（変わったものだけ再解析）
    pub fn refresh(&mut self) -> Result<Fresh> {
        self.scan(false)
    }

    /// held = 旗を既に立てている（rebuild）
    fn scan(&mut self, held: bool) -> Result<Fresh> {
        let r = self.scan_inner(held);
        // 旗を立てていれば、どの経路で終わっても（途中のエラー・last_scan の書込や commit の失敗も）必ず外す
        if self.flag_start.is_some() {
            let d = self.drop_flag();
            let v = r?;
            d?;
            return Ok(v);
        }
        r
    }

    fn scan_inner(&mut self, held: bool) -> Result<Fresh> {
        let walk = scan::walk(&self.root)?;
        let known = self.known()?;
        let seen: HashSet<&str> = walk.entries.iter().map(|e| e.rel.as_str()).collect();
        let changed: Vec<&Entry> =
            walk.entries.iter().filter(|e| known.get(&e.rel).is_none_or(|&(m, s)| m != e.mtime_ns || s != e.size)).collect();
        // 走査に現れなかった既知のページ = 消えた。ただし読めなかった区画（Drive の TCC 等）の配下は前の行を保持する
        let removed: Vec<String> = known.keys().filter(|k| !seen.contains(k.as_str()) && !walk.unreadable(k)).cloned().collect();
        let set_changed = !removed.is_empty() || changed.iter().any(|e| !known.contains_key(&e.rel));
        // 大きな走査の判定から、読めなかったページの再試行分を除く（読めないページが何百件も続く = Drive がオフライン等の間、
        // 毎回の走査が旗を取りに行って他のプロセスの wiki_* が「作り直し中」になるのを防ぐ）
        let retries = changed.iter().filter(|e| known.get(&e.rel).is_some_and(|&(m, _)| m == READ_ERROR_MTIME)).count();
        let big = held || changed.len() - retries > BIG_SCAN;
        if big && !held && !self.take_flag()? {
            return Ok(Fresh::Rebuilding(self.foreign_flag()?.unwrap_or(0)));
        }
        let read_errors = self.apply(&changed, &removed, set_changed, big)?;
        let tx = wtx(&self.conn)?;
        set_meta(&tx, "last_scan", &now_ms().to_string())?;
        set_meta(&tx, "used_find", if walk.used_find { "1" } else { "0" })?;
        set_meta(&tx, "walk_errors", &walk.errors.to_string())?;
        set_meta(&tx, "read_errors", &read_errors.to_string())?;
        tx.commit()?;
        Ok(Fresh::Ok)
    }

    /// 中身を捨てて全走査（GUI の「索引を作り直す」）。旗を立ててから捨てる（他のプロセスに空の索引を見せない）
    pub fn rebuild(&mut self) -> Result<Fresh> {
        if !self.take_flag()? {
            return Ok(Fresh::Rebuilding(self.foreign_flag()?.unwrap_or(0)));
        }
        let cleared = (|| -> Result<()> {
            let tx = wtx(&self.conn)?;
            tx.execute_batch(&format!("{CLEAR_ALL} DELETE FROM meta WHERE key = 'last_scan';"))?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(e) = cleared {
            self.drop_flag()?;
            return Err(e);
        }
        self.scan(true)
    }

    /// 旗を立てる（他の生きた旗があれば false）。旗 = `<pid>:<最後の heartbeat の unix 秒>`
    fn take_flag(&mut self) -> Result<bool> {
        let tx = wtx(&self.conn)?;
        let flag: Option<String> = tx.query_row("SELECT value FROM meta WHERE key = 'rebuilding'", [], |r| r.get(0)).optional()?;
        if flag.as_deref().is_some_and(flag_alive) {
            return Ok(false);
        }
        let start = now_ms() / 1000;
        set_meta(&tx, "rebuilding", &flag_value(start))?;
        set_meta(&tx, "progress", "0")?;
        tx.commit()?;
        self.flag_start = Some(start);
        PROGRESS.store(0, Ordering::Relaxed);
        Ok(true)
    }

    /// 自分の旗（pid と開始時刻が一致）だけを外す。他のプロセスが引き継いだ旗は消さない
    fn drop_flag(&mut self) -> Result<()> {
        PROGRESS.store(-1, Ordering::Relaxed);
        let Some(start) = self.flag_start.take() else { return Ok(()) };
        let tx = wtx(&self.conn)?;
        let mine = format!("{}:{start}:", std::process::id());
        let n = tx.execute("DELETE FROM meta WHERE key = 'rebuilding' AND substr(value, 1, length(?1)) = ?1", [&mine])?;
        if n > 0 {
            tx.execute("DELETE FROM meta WHERE key = 'progress'", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 索引にあるパス → (mtime, size)
    fn known(&self) -> Result<HashMap<String, (i64, i64)>> {
        let mut st = self.conn.prepare("SELECT path, mtime, size FROM pages")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 戻り値 = 本文を読めなかったページの数（mtime を READ_ERROR_MTIME で書いた = 次の走査で再試行する）
    fn apply(&mut self, changed: &[&Entry], removed: &[String], set_changed: bool, report: bool) -> Result<usize> {
        if changed.is_empty() && removed.is_empty() {
            return Ok(0);
        }
        let only: Vec<String> = changed.iter().map(|e| e.rel.clone()).collect();
        if !report && changed.len() <= BATCH {
            // 小さな適用は書込と解決を 1 つの tx で行う = 差分解決の前提（前の links 表が全解決と同じ）を崩さない:
            // 途中で止まっても（kill・解決の BUSY）pages と links が割れず、別のプロセスの書込とも混ざらない（同じスナップショットで読む）。
            // 増減の有無は tx の中で実際の行から決める（呼び手の known は別のプロセスの書込の前の物でありうる）
            let parsed: Vec<(&Entry, PageData)> = changed.iter().map(|e| (*e, read_page(&self.root, e))).collect();
            let read_errors = parsed.iter().filter(|(_, d)| d.read_error).count();
            let tx = wtx(&self.conn)?;
            let mut grew = set_changed;
            for (e, d) in &parsed {
                grew |= !write_page(&tx, e, d)?;
            }
            for p in removed {
                grew |= delete_page(&tx, p)?;
            }
            if grew {
                // ページの増減は他のページから出るリンクの解決も変えうる。読み直したページも名前の比べに入れる（取り過ぎは手間だけ）
                let touched: Vec<String> = only.iter().chain(removed).cloned().collect();
                resolve_delta(&tx, &self.root, &only, &touched)?;
            } else {
                resolve_links(&tx, &self.root, Some(&only))?;
            }
            tx.commit()?;
            return Ok(read_errors);
        }
        // 大きな適用（大きな走査・作り直し・BATCH 件を超える refresh_paths）は区切って書き（書込のロックを短く）、最後に全部を解決し直す
        // （解析に比べて解決の手間は小さい・区切りの途中で止まった回の残りもここで直る）
        let total = changed.len().max(1);
        let mut read_errors = 0;
        for (n, chunk) in changed.chunks(BATCH).enumerate() {
            // 読み・解析はトランザクションの外（書込のロックを短く）
            let parsed: Vec<(&Entry, PageData)> = chunk.iter().map(|e| (*e, read_page(&self.root, e))).collect();
            read_errors += parsed.iter().filter(|(_, d)| d.read_error).count();
            let tx = wtx(&self.conn)?;
            for (e, d) in &parsed {
                write_page(&tx, e, d)?;
            }
            if report {
                let pct = (((n * BATCH + chunk.len()) * 100) / total).min(99) as u32;
                set_meta(&tx, "progress", &pct.to_string())?;
                if let Some(start) = self.flag_start {
                    set_meta(&tx, "rebuilding", &flag_value(start))?; // heartbeat
                }
                PROGRESS.store(pct as i32, Ordering::Relaxed);
            }
            tx.commit()?;
        }
        let tx = wtx(&self.conn)?;
        for p in removed {
            delete_page(&tx, p)?;
        }
        resolve_links(&tx, &self.root, None)?;
        tx.commit()?;
        Ok(read_errors)
    }

    /// 指定のパスだけ取り込み直す（GUI のファイル監視・書込の後）。絶対パス（vault の中）か vault 相対。
    /// 消えていれば索引から外す。`.` 始まりの区画・vault の外は無視。戻り値 = 取り込み直した / 外したパス（種別ごと = GUI が "wiki-changed" に載せる）
    pub fn refresh_paths<P: AsRef<Path>>(&mut self, paths: &[P]) -> Result<Refreshed> {
        let known = self.known()?;
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        let mut added = Vec::new();
        let mut set_changed = false;
        let mut seen = HashSet::new();
        for p in paths {
            let Some(rel) = self.rel_of(p.as_ref()) else { continue };
            if scan::hidden(&rel) || !seen.insert(rel.clone()) {
                continue;
            }
            match scan::stat_io(&self.root.join(&rel)) {
                Ok(Some((mtime_ns, size))) => {
                    if !known.contains_key(&rel) {
                        set_changed = true;
                        added.push(rel.clone());
                    }
                    changed.push(Entry { rel, mtime_ns, size });
                }
                // 無い（親が無い・親がファイルになった）か通常ファイルでなくなった = 索引から外す
                Ok(None) | Err(_) if !known.contains_key(&rel) => {}
                Ok(None) => removed_push(&mut removed, &mut set_changed, rel),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {
                    removed_push(&mut removed, &mut set_changed, rel)
                }
                // stat 自体が拒否された（Drive の TCC・権限）= 消えたとは言えない → 据え置き（走査と同じく前の行を保持）
                Err(_) => {}
            }
        }
        let refs: Vec<&Entry> = changed.iter().collect();
        self.apply(&refs, &removed, set_changed, false)?;
        let modified = changed.iter().map(|e| e.rel.clone()).filter(|r| !added.contains(r)).collect();
        Ok(Refreshed { added, modified, removed })
    }

    /// 絶対パス / vault 相対 → vault 相対（NFC）。vault の外・`..` で外へ出るものは None
    pub(super) fn rel_of(&self, p: &Path) -> Option<String> {
        let s: String = if p.is_absolute() {
            super::strip_root(&self.root, p).or_else(|| super::strip_root(&self.root, &std::fs::canonicalize(p).ok()?))?
        } else {
            p.to_str()?.nfc().collect()
        };
        super::links::normalize_rel(&s).filter(|r| !r.is_empty())
    }

    /// リンクの解決（only = このページから出るリンクだけ・None = 全部）。試験が差分解決の結果と比べる
    #[cfg(test)]
    fn resolve(&mut self, only: Option<&[String]>) -> Result<()> {
        let tx = wtx(&self.conn)?;
        resolve_links(&tx, &self.root, only)?;
        tx.commit()?;
        Ok(())
    }

    // ---------------- 読み ----------------

    /// 索引にあるパスの表記（完全一致 → 大小無視の一致）。無ければ None
    pub fn lookup_path(&self, rel: &str) -> Result<Option<String>> {
        let q = |sql: &str| self.conn.query_row(sql, [rel], |r| r.get::<_, String>(0)).optional();
        Ok(match q("SELECT path FROM pages WHERE path = ?1")? {
            Some(p) => Some(p),
            None => q("SELECT path FROM pages WHERE path = ?1 COLLATE NOCASE ORDER BY path LIMIT 1")?,
        })
    }

    /// 索引にある全パス（リンクの解決器を組む = GUI の改名の書換え）
    pub fn all_paths(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT path FROM pages ORDER BY path")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn page(&self, path: &str) -> Result<Option<PageRow>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {PAGE_COLS} FROM pages WHERE path = ?1"), [path], page_row)
            .optional()?)
    }

    /// 戻り値 = (絞込後の総数, パス順の先頭 limit 件)。総数は count(*)・行は LIMIT で取る（全行を読んでから切らない）。
    /// 2 つは 1 つの読みトランザクション = 同じスナップショット（間に他のプロセスの走査が入っても総数と行がずれない）
    pub fn list(&self, q: &ListQuery) -> Result<(usize, Vec<PageRow>)> {
        let mut cond = String::from("1");
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(pre) = q.prefix.as_deref().filter(|s| !s.is_empty()) {
            args.push(pre.to_string().into());
            cond.push_str(&format!(" AND substr(path, 1, length(?{n})) = ?{n}", n = args.len()));
        }
        if let Some(k) = q.kind.as_deref().filter(|s| !s.is_empty()) {
            args.push(k.to_string().into());
            cond.push_str(&format!(" AND kind = ?{}", args.len()));
        }
        for t in q.tags.iter().map(|t| t.trim().trim_start_matches('#')).filter(|t| !t.is_empty()) {
            args.push(t.to_string().into());
            cond.push_str(&format!(" AND EXISTS (SELECT 1 FROM tags t WHERE t.path = p.path AND t.tag = ?{} COLLATE NOCASE)", args.len()));
        }
        let tx = self.conn.unchecked_transaction()?;
        let total: i64 =
            tx.query_row(&format!("SELECT count(*) FROM pages p WHERE {cond}"), rusqlite::params_from_iter(args.iter()), |r| r.get(0))?;
        // usize::MAX（inbox の chat-log = 全件）は i64 に収まらない → i64::MAX
        args.push(i64::try_from(q.limit).unwrap_or(i64::MAX).into());
        let sql = format!("SELECT {PAGE_COLS} FROM pages p WHERE {cond} ORDER BY path LIMIT ?{}", args.len());
        let rows: Vec<PageRow> = {
            let mut st = tx.prepare(&sql)?;
            let rows = st.query_map(rusqlite::params_from_iter(args.iter()), page_row)?.collect::<rusqlite::Result<_>>()?;
            rows
        };
        tx.commit()?;
        Ok((total as usize, rows))
    }

    /// 検索: 題名の部分一致（完全一致 → 前方一致 → 全語を含む）を上に、続いて全文（FTS・bm25 順）。tags は AND の絞込。
    /// query が空なら tags を全部持つページを題名順。戻り値 = (絞込後の総数, 先頭 limit 件)
    pub fn search(&self, q: &SearchQuery) -> Result<(usize, Vec<SearchHit>)> {
        let terms = query_terms(&q.query);
        let tags: Vec<String> =
            q.tags.iter().map(|t| t.trim().trim_start_matches('#').to_string()).filter(|t| !t.is_empty()).collect();
        // tag の AND（path の集合）
        let tag_set: Option<HashSet<String>> = if tags.is_empty() {
            None
        } else {
            let mut st = self.conn.prepare_cached("SELECT DISTINCT path FROM tags WHERE tag = ?1 COLLATE NOCASE")?;
            let mut acc: Option<HashSet<String>> = None;
            for t in &tags {
                let set: HashSet<String> = st.query_map([t], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                acc = Some(match acc {
                    None => set,
                    Some(a) => a.intersection(&set).cloned().collect(),
                });
            }
            acc
        };
        let keep = |p: &str| tag_set.as_ref().is_none_or(|s| s.contains(p));
        let mut hits: HashMap<i64, SearchHit> = HashMap::new();
        if terms.is_empty() {
            let Some(set) = &tag_set else { return Ok((0, Vec::new())) };
            let mut st = self.conn.prepare_cached(&format!("SELECT rowid, {PAGE_COLS} FROM pages WHERE path = ?1"))?;
            for p in set {
                if let Some((id, row)) = st.query_row([p], |r| Ok((r.get::<_, i64>(0)?, page_row_at(r, 1)?))).optional()? {
                    hits.insert(id, SearchHit { row, title_match: None, body_match: false, score: 0.0 });
                }
            }
        } else {
            // 題名: 両側から記号を落として（`title_key`）全語を含むか。「胆道。」でも題名「胆道」が完全一致・「PI-RADS」は「pi rads」も
            let keys: Vec<String> = terms.iter().map(|t| title_key(t)).filter(|k| !k.is_empty()).collect();
            let whole = keys.join(" ");
            if !keys.is_empty() {
                let mut st = self.conn.prepare_cached("SELECT rowid, title_norm FROM pages")?;
                let cands: Vec<(i64, String)> = st
                    .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .filter_map(|r| r.ok())
                    .filter_map(|(id, norm)| {
                        let k = title_key(&norm);
                        let compact = k.replace(' ', "");
                        keys.iter().all(|t| k.contains(t.as_str()) || compact.contains(t.as_str())).then_some((id, k))
                    })
                    .collect();
                let mut row_st = self.conn.prepare_cached(&format!("SELECT {PAGE_COLS} FROM pages WHERE rowid = ?1"))?;
                for (id, k) in cands {
                    let Some(row) = row_st.query_row([id], page_row).optional()? else { continue };
                    if !keep(&row.path) {
                        continue;
                    }
                    let tier = if k == whole || k.replace(' ', "") == whole.replace(' ', "") {
                        0
                    } else if k.starts_with(&whole) {
                        1
                    } else {
                        2
                    };
                    hits.insert(id, SearchHit { row, title_match: Some(tier), body_match: false, score: 0.0 });
                }
            }
            // 全文（題名の列は重み 5）
            let mut st = self.conn.prepare(&format!(
                "SELECT p.rowid, {cols}, bm25(pages_fts, 5.0, 1.0) FROM pages_fts JOIN pages p ON p.rowid = pages_fts.rowid
                 WHERE pages_fts MATCH ?1",
                cols = PAGE_COLS.split(", ").map(|c| format!("p.{c}")).collect::<Vec<_>>().join(", ")
            ))?;
            let rows = st.query_map([fts_match(&terms)], |r| Ok((r.get::<_, i64>(0)?, page_row_at(r, 1)?, r.get::<_, f64>(9)?)))?;
            for r in rows {
                let (id, row, score) = r?;
                if !keep(&row.path) {
                    continue;
                }
                let h = hits.entry(id).or_insert(SearchHit { row, title_match: None, body_match: false, score: 0.0 });
                h.body_match = true;
                h.score = score;
            }
        }
        let mut v: Vec<SearchHit> = hits.into_values().collect();
        v.sort_by(|a, b| {
            let tier = |h: &SearchHit| h.title_match.unwrap_or(3);
            tier(a)
                .cmp(&tier(b))
                .then(a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.row.title.to_lowercase().cmp(&b.row.title.to_lowercase()))
                .then_with(|| a.row.path.cmp(&b.row.path))
        });
        let total = v.len();
        v.truncate(q.limit);
        Ok((total, v))
    }

    /// tag と件数（ページ数の多い順 → 名前順）。大小だけ違う表記は 1 つに束ねる（ASCII の大小・list の tag 絞込と同じ）。
    /// 戻り値 = (tag の総数, 先頭 limit 件)
    pub fn tags(&self, limit: usize) -> Result<(usize, Vec<(String, i64)>)> {
        let mut st = self.conn.prepare(
            "SELECT min(tag), count(DISTINCT path) FROM tags GROUP BY tag COLLATE NOCASE ORDER BY 2 DESC, 1 COLLATE NOCASE",
        )?;
        let out: Vec<(String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let total = out.len();
        Ok((total, out.into_iter().take(limit).collect()))
    }

    pub fn links_from(&self, path: &str) -> Result<Vec<LinkRow>> {
        let mut st = self.conn.prepare("SELECT src_path, kind, target_raw, target_path, ambiguous FROM links WHERE src_path = ?1 ORDER BY rowid")?;
        let rows = st.query_map([path], link_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// このパスを指すリンク（解決済みのもの）
    pub fn links_to(&self, path: &str) -> Result<Vec<LinkRow>> {
        let mut st = self.conn.prepare(
            "SELECT src_path, kind, target_raw, target_path, ambiguous FROM links WHERE target_path = ?1 ORDER BY src_path, rowid",
        )?;
        let rows = st.query_map([path], link_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// この item key を items に持つページ
    pub fn pages_with_item(&self, key: &str) -> Result<Vec<PageRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {PAGE_COLS} FROM pages p WHERE EXISTS (SELECT 1 FROM json_each(p.items_json) j WHERE j.value = ?1) ORDER BY path"
        ))?;
        let rows = st.query_map([key], page_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 未解決の内部リンク（解決できなかった wiki / embed / md / html）
    pub fn unresolved(&self, limit: usize) -> Result<Vec<LinkRow>> {
        self.link_query("target_path IS NULL AND kind IN ('wiki','embed','md','html')", limit)
    }

    /// 同名が複数あったリンク
    pub fn ambiguous(&self, limit: usize) -> Result<Vec<LinkRow>> {
        self.link_query("ambiguous = 1", limit)
    }

    fn link_query(&self, cond: &str, limit: usize) -> Result<Vec<LinkRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT src_path, kind, target_raw, target_path, ambiguous FROM links WHERE {cond} ORDER BY src_path, rowid LIMIT ?1"
        ))?;
        let rows = st.query_map([limit as i64], link_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 件数の集計（wiki_status）
    pub fn stats(&self) -> Result<Value> {
        let q = |sql: &str| -> Result<i64> { Ok(self.conn.query_row(sql, [], |r| r.get(0))?) };
        let mut kinds = serde_json::Map::new();
        for k in ["md", "html", "media", "other"] {
            kinds.insert(k.into(), json!(q(&format!("SELECT count(*) FROM pages WHERE kind = '{k}'"))?));
        }
        let mut link_kinds = serde_json::Map::new();
        {
            let mut st = self.conn.prepare("SELECT kind, count(*) FROM links GROUP BY kind ORDER BY kind")?;
            for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (k, n) = r?;
                link_kinds.insert(k, json!(n));
            }
        }
        let last: Option<i64> = self.meta("last_scan")?.and_then(|s| s.parse().ok());
        Ok(json!({
            "pages": q("SELECT count(*) FROM pages")?,
            "kinds": kinds,
            "links": q("SELECT count(*) FROM links")?,
            "link_kinds": link_kinds,
            "unresolved": q("SELECT count(*) FROM links WHERE target_path IS NULL AND kind IN ('wiki','embed','md','html')")?,
            "ambiguous": q("SELECT count(*) FROM links WHERE ambiguous = 1")?,
            "inbox": q("SELECT count(*) FROM pages WHERE substr(path, 1, 7) = 'ingest/'")?,
            "frontmatter": {
                "pages": q("SELECT count(*) FROM pages WHERE has_fm = 1")?,
                "raw_pages": q("SELECT count(*) FROM pages WHERE fm_raw > 0")?,
                "raw_fields": q("SELECT coalesce(sum(fm_raw), 0) FROM pages")?,
            },
            "last_scan": last.map(|ms| ms / 1000),
            "used_find": self.meta("used_find")?.as_deref() == Some("1"),
            "walk_errors": self.meta("walk_errors")?.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0),
            "read_errors": self.meta("read_errors")?.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0),
        }))
    }
}

const PAGE_COLS: &str = "path, kind, title, tags_json, items_json, mtime, size, sha256";

fn page_row(r: &rusqlite::Row) -> rusqlite::Result<PageRow> {
    page_row_at(r, 0)
}

/// PAGE_COLS が列 i から並ぶ行
fn page_row_at(r: &rusqlite::Row, i: usize) -> rusqlite::Result<PageRow> {
    let list = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
    Ok(PageRow {
        path: r.get(i)?,
        kind: r.get(i + 1)?,
        title: r.get(i + 2)?,
        tags: list(r.get(i + 3)?),
        items: list(r.get(i + 4)?),
        mtime_ns: r.get(i + 5)?,
        size: r.get(i + 6)?,
        sha256: r.get(i + 7)?,
    })
}

fn link_row(r: &rusqlite::Row) -> rusqlite::Result<LinkRow> {
    Ok(LinkRow { src: r.get(0)?, kind: r.get(1)?, target: r.get(2)?, path: r.get(3)?, ambiguous: r.get::<_, i64>(4)? != 0 })
}

/// 書込のトランザクション（`BEGIN IMMEDIATE` = 書きのロックを先に取る・busy_timeout が効く）
fn wtx(c: &Connection) -> Result<Transaction<'_>> {
    Ok(Transaction::new_unchecked(c, TransactionBehavior::Immediate)?)
}

/// pages.id は INTEGER PRIMARY KEY（= rowid の別名）。pages_fts の rowid と対応させるので、VACUUM で番号が振り直されないよう明示する
const TABLES: &str = "CREATE TABLE IF NOT EXISTS pages (
       id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, kind TEXT NOT NULL, title TEXT NOT NULL, title_norm TEXT NOT NULL,
       tags_json TEXT NOT NULL, items_json TEXT NOT NULL, mtime INTEGER NOT NULL, size INTEGER NOT NULL,
       sha256 TEXT, has_fm INTEGER NOT NULL DEFAULT 0, fm_raw INTEGER NOT NULL DEFAULT 0);
     CREATE TABLE IF NOT EXISTS links (
       src_path TEXT NOT NULL, target_raw TEXT NOT NULL, target_path TEXT, kind TEXT NOT NULL,
       ambiguous INTEGER NOT NULL DEFAULT 0);
     CREATE INDEX IF NOT EXISTS idx_links_src ON links(src_path);
     CREATE INDEX IF NOT EXISTS idx_links_target ON links(target_path);
     CREATE TABLE IF NOT EXISTS tags (tag TEXT NOT NULL, path TEXT NOT NULL);
     CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag COLLATE NOCASE);
     CREATE INDEX IF NOT EXISTS idx_tags_path ON tags(path);
     CREATE VIRTUAL TABLE IF NOT EXISTS pages_fts USING fts5(
       title, body, content='', contentless_delete=1, tokenize=\"unicode61 remove_diacritics 2\");";

/// 中身を全部捨てる（vault の切替・作り直し）
const CLEAR_ALL: &str =
    "DELETE FROM pages; DELETE FROM links; DELETE FROM tags; INSERT INTO pages_fts(pages_fts) VALUES('delete-all');";

/// SQLITE_BUSY / LOCKED のときだけ 5 秒まで取り直す（busy handler が呼ばれない経路 = journal_mode の切替用）
fn retry_busy<T>(mut f: impl FnMut() -> rusqlite::Result<T>) -> Result<T> {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match f() {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if matches!(e.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                    && std::time::Instant::now() < until =>
            {
                std::thread::sleep(Duration::from_millis(10 + fastrand::u64(..20)));
            }
            r => return Ok(r?),
        }
    }
}

/// 旗の値 = `<pid>:<開始の unix 秒>:<最後の heartbeat の unix 秒>`
fn flag_value(start: i64) -> String {
    format!("{}:{start}:{}", std::process::id(), now_ms() / 1000)
}

/// 旗の持ち主が生きていて（自プロセスではなく）heartbeat から 10 分以内なら true。死んだプロセスの旗は即引き継ぐ
fn flag_alive(flag: &str) -> bool {
    // `<pid>:<開始>:<heartbeat>`（旧形 `<pid>:<時刻>` も読む）
    let parts: Vec<&str> = flag.split(':').collect();
    let pid = parts.first().copied().unwrap_or("");
    let beat: i64 = parts.last().and_then(|b| b.parse().ok()).unwrap_or(0);
    let Ok(pid) = pid.parse::<i32>() else { return false };
    if pid <= 0 || pid as u32 == std::process::id() || now_ms() / 1000 - beat > FLAG_TTL_S {
        return false;
    }
    pid_alive(pid)
}

#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    // kill(pid, 0): 0 = 生きている・EPERM = 生きている（他人のプロセス）・ESRCH = いない
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_alive(_pid: i32) -> bool {
    true
}

fn removed_push(removed: &mut Vec<String>, set_changed: &mut bool, rel: String) {
    removed.push(rel);
    *set_changed = true;
}

fn set_meta(c: &Connection, k: &str, v: &str) -> Result<()> {
    c.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [k, v])
        .map_err(sqlerr)?;
    Ok(())
}

/// 1 ページの解析結果（書込前）
pub struct PageData {
    pub kind: Kind,
    pub parsed: Parsed,
    pub sha256: Option<String>,
    /// 本文を読めなかった（権限・File Provider の一時的な失敗等）。mtime を READ_ERROR_MTIME で書いて次の走査で再試行する
    pub read_error: bool,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// md / html を解析する（本文が無い種類・読めない・大きすぎるものは空の Parsed）
pub fn parse_bytes(kind: Kind, bytes: &[u8]) -> Parsed {
    let text = String::from_utf8_lossy(bytes);
    match kind {
        Kind::Md => parse_md::parse(&text),
        Kind::Html => parse_html::parse(&text),
        _ => Parsed::default(),
    }
}

fn read_page(root: &Path, e: &Entry) -> PageData {
    let kind = Kind::of(&e.rel);
    if !kind.has_body() || e.size > BODY_MAX {
        return PageData { kind, parsed: Parsed::default(), sha256: None, read_error: false };
    }
    match std::fs::read(root.join(&e.rel)) {
        Ok(bytes) => PageData { kind, sha256: Some(sha256_hex(&bytes)), parsed: parse_bytes(kind, &bytes), read_error: false },
        Err(_) => PageData { kind, parsed: Parsed::default(), sha256: None, read_error: true },
    }
}

/// 題名の既定: md / html は拡張子を除いたファイル名・それ以外はファイル名
pub fn default_title(rel: &str, kind: Kind) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if kind.has_body() {
        name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name).to_string()
    } else {
        name.to_string()
    }
}

/// 今の pages から解決器を組む（戻り値の 2 つめ = pages の全パス）
fn resolver(tx: &Connection, root: &Path) -> Result<(Resolver, Vec<String>)> {
    let paths: Vec<String> = {
        let mut st = tx.prepare("SELECT path FROM pages")?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    Ok((Resolver::new(root, paths.iter().map(String::as_str)), paths))
}

/// リンクの解決（only = このページから出るリンクだけ・None = 全部）。tx = 呼び手の書込 tx
fn resolve_links(tx: &Connection, root: &Path, only: Option<&[String]>) -> Result<()> {
    let (r, _) = resolver(tx, root)?;
    let rows: Vec<(i64, String, String, String)> = {
        let sql = "SELECT rowid, src_path, target_raw, kind FROM links WHERE kind IN ('wiki','embed','md','html','file')";
        let mut st = tx.prepare(&format!("{sql}{}", if only.is_some() { " AND src_path = ?1" } else { "" }))?;
        let map = |r: &rusqlite::Row| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?));
        match only {
            None => st.query_map([], map)?.collect::<rusqlite::Result<_>>()?,
            Some(srcs) => {
                let mut all = Vec::new();
                for s in srcs {
                    all.extend(st.query_map([s], map)?.collect::<rusqlite::Result<Vec<_>>>()?);
                }
                all
            }
        }
    };
    let mut up = tx.prepare_cached("UPDATE links SET target_path = ?1, ambiguous = ?2 WHERE rowid = ?3")?;
    for (id, src, target, kind) in &rows {
        let Some(k) = LinkKind::parse(kind) else { continue };
        let res = r.resolve(src, k, target);
        up.execute(params![res.path, res.ambiguous as i64, id])?;
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// 試験用: 次の resolve_delta を失敗させる（解決の BUSY・中断の代わり）
    static FAIL_DELTA: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// ページの増減の後の解決（tx = 呼び手の書込 tx・ページの書込と同じ tx）。全部を解決し直したのと同じ結果になるリンクだけを解決し直す:
/// srcs から出る・touched（足した / 外した / 読み直したページ）を指していた・指す先が今の pages に無い・未解決・曖昧・
/// 引く名前（`Resolver::probe_names`）が touched の名前（`links::touched_names`）に当たる。名前が当たらないリンクは
/// 解決器の中で引く所が変わっていない。「指す先が無い」は前の回の links 表が全解決とずれていた場合（前の版の中断・
/// 大きな適用の途中の中断）に、消えたページを指したままの行を直す（足したページで曖昧化・付け替えが起きるはずだった行は直せない）
fn resolve_delta(tx: &Connection, root: &Path, srcs: &[String], touched: &[String]) -> Result<()> {
    #[cfg(test)]
    if FAIL_DELTA.with(|f| f.replace(false)) {
        return Err(CoreError::BadInput("試験: resolve_delta の失敗".into()));
    }
    let (r, live) = resolver(tx, root)?;
    let live: HashSet<&str> = live.iter().map(String::as_str).collect();
    let srcs: HashSet<&str> = srcs.iter().map(String::as_str).collect();
    let paths: HashSet<&str> = touched.iter().map(String::as_str).collect();
    let names: HashSet<String> = touched.iter().flat_map(|p| super::links::touched_names(p)).collect();
    let rows: Vec<(i64, String, String, String, Option<String>, bool)> = {
        let mut st = tx.prepare(
            "SELECT rowid, src_path, target_raw, kind, target_path, ambiguous FROM links WHERE kind IN ('wiki','embed','md','html','file')",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, i64>(5)? != 0)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut up = tx.prepare_cached("UPDATE links SET target_path = ?1, ambiguous = ?2 WHERE rowid = ?3")?;
    for (id, src, target, kind, path, ambiguous) in &rows {
        let Some(k) = LinkKind::parse(kind) else { continue };
        let hit = srcs.contains(src.as_str())
            || path.as_deref().is_none_or(|p| paths.contains(p) || !live.contains(p))
            || *ambiguous
            || r.probe_names(src, k, target).iter().any(|n| names.contains(n));
        if !hit {
            continue;
        }
        let res = r.resolve(src, k, target);
        // 結果が今の行と同じなら書かない（未解決のまま・同じ先のまま）
        if res.path != *path || res.ambiguous != *ambiguous {
            up.execute(params![res.path, res.ambiguous as i64, id])?;
        }
    }
    Ok(())
}

/// 戻り値 = 書く前に同じパスのページがあったか
fn write_page(tx: &Connection, e: &Entry, d: &PageData) -> Result<bool> {
    let existed = delete_page(tx, &e.rel)?;
    let title = d.parsed.title.clone().unwrap_or_else(|| default_title(&e.rel, d.kind));
    let norm: String = title.nfc().collect::<String>().to_lowercase();
    let (has_fm, fm_raw) = match &d.parsed.frontmatter {
        Some(f) => (1, f.raw_count() as i64),
        None => (0, 0),
    };
    tx.prepare_cached(
        "INSERT INTO pages (path, kind, title, title_norm, tags_json, items_json, mtime, size, sha256, has_fm, fm_raw)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        e.rel,
        d.kind.as_str(),
        title,
        norm,
        serde_json::to_string(&d.parsed.tags).unwrap_or_else(|_| "[]".into()),
        serde_json::to_string(&d.parsed.items).unwrap_or_else(|_| "[]".into()),
        // 読めなかったページは mtime を印で書く = 次の走査で mtime が合わず再解析される（同じ mtime / size のまま「空」で固定しない）
        if d.read_error { READ_ERROR_MTIME } else { e.mtime_ns },
        e.size,
        d.sha256,
        has_fm,
        fm_raw
    ])?;
    // 全文検索（本文のある md / html だけ。rowid = pages の rowid）
    if d.kind.has_body() {
        let rowid = tx.last_insert_rowid();
        tx.prepare_cached("INSERT INTO pages_fts (rowid, title, body) VALUES (?1, ?2, ?3)")?.execute(params![
            rowid,
            crate::model::cjk_spaced(&title),
            crate::model::cjk_spaced(&d.parsed.text)
        ])?;
    }
    {
        let mut ins = tx.prepare_cached("INSERT INTO links (src_path, target_raw, kind) VALUES (?1, ?2, ?3)")?;
        for l in &d.parsed.links {
            ins.execute(params![e.rel, l.target, l.kind.as_str()])?;
        }
    }
    let mut ins = tx.prepare_cached("INSERT INTO tags (tag, path) VALUES (?1, ?2)")?;
    for t in &d.parsed.tags {
        ins.execute(params![t, e.rel])?;
    }
    Ok(existed)
}

/// 戻り値 = ページがあった（消した）か
fn delete_page(tx: &Connection, rel: &str) -> Result<bool> {
    let rowid: Option<i64> = tx.prepare_cached("SELECT rowid FROM pages WHERE path = ?1")?.query_row([rel], |r| r.get(0)).optional()?;
    if let Some(id) = rowid {
        tx.prepare_cached("DELETE FROM pages_fts WHERE rowid = ?1")?.execute([id])?;
    }
    let n = tx.prepare_cached("DELETE FROM pages WHERE path = ?1")?.execute([rel])?;
    tx.prepare_cached("DELETE FROM links WHERE src_path = ?1")?.execute([rel])?;
    tx.prepare_cached("DELETE FROM tags WHERE path = ?1")?.execute([rel])?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tempdir の vault と索引（drop で消す）
    /// 1 つめ = 消すフォルダ・2 つめ = 索引を使うテストの直列化（test_serial）
    struct Tmp(PathBuf, #[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tmp(name: &str) -> (Tmp, PathBuf, PathBuf) {
        let serial = test_serial();
        let base = std::env::temp_dir().join(format!("openref-wiki-ix-{}-{}-{name}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(base.join("vault")).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let root = base.join("vault");
        let ip = base.join("ix/wiki-index.sqlite");
        (Tmp(base, serial), root, ip)
    }

    /// remove_index: プロセスの接続を閉じてファイル一式を消す・無いファイルは無視・次の with_index は空の索引から作り直す
    #[test]
    fn remove_index_closes_and_deletes() {
        let (_t, root, ip) = tmp("rm");
        std::fs::write(root.join("a.md"), "# A\n").unwrap();
        let n = with_index(&ip, &root, |ix| {
            ix.refresh()?;
            Ok(ix.list(&ListQuery { limit: 10, ..Default::default() })?.0)
        })
        .unwrap();
        assert_eq!(n, 1);
        assert!(ip.exists());
        remove_index(&ip).unwrap();
        for s in ["", "-wal", "-shm"] {
            assert!(!PathBuf::from(format!("{}{s}", ip.display())).exists(), "{s}");
        }
        remove_index(&ip).unwrap(); // 2 回目（ファイル無し）もエラーにしない
        let n = with_index(&ip, &root, |ix| Ok(ix.list(&ListQuery { limit: 10, ..Default::default() })?.0)).unwrap();
        assert_eq!(n, 0, "消した後は空の索引から");
    }

    /// index_path: 決定的・別の root は別名・名前 = 16 桁の hex + `.sqlite`・置き場 = app_dir/wiki-index/・NFC / NFD の綴りは同じ名
    #[test]
    fn index_path_per_workspace() {
        let app = Path::new("/app");
        let a = index_path(app, Path::new("/Users/u/Documents/ref"));
        assert_eq!(a, index_path(app, Path::new("/Users/u/Documents/ref")), "決定的");
        assert_eq!(a.parent().unwrap(), Path::new("/app/wiki-index"));
        let name = a.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name.len(), 16 + ".sqlite".len(), "{name}");
        assert!(name.ends_with(".sqlite") && name[..16].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{name}");
        assert_eq!(name[..16], sha256_hex(b"/Users/u/Documents/ref")[..16]);
        let b = index_path(app, Path::new("/Users/u/Documents/blog"));
        assert_ne!(a, b, "別の root は別のファイル");
        assert_ne!(a, index_path(app, Path::new("/Users/u/Documents/ref/")), "文字列が違えば別（root は canonical で来る）");
        let nfc = "/Users/u/マイドライブ/wiki";
        let nfd: String = nfc.nfd().collect();
        assert_ne!(nfd, nfc);
        assert_eq!(index_path(app, Path::new(nfc)), index_path(app, Path::new(&nfd)), "NFC / NFD は同じ名");
        assert_ne!(a, Path::new("/app").join(INDEX_FILE));
    }

    thread_local! {
        static LIST_SQL: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// list（A-29）: 総数 = count(*)・行 = LIMIT。全行を取ってから切る旧の形（SQL を写した参照）と、条件 × limit（0 / 1 / 全件 / 全件超 /
    /// usize::MAX）の全組合せで総数と行（順序も）が同じ・件数 = min(limit, 総数)。発行する SQL は count 1 本と LIMIT 付きの 1 本だけ
    #[test]
    fn list_counts_and_limits_in_sql() {
        let (_t, root, ip) = tmp("list");
        for (rel, body) in [
            ("a.md", "---\ntags: [x]\n---\n# A\n"),
            ("b/c.md", "---\ntags: [x, y]\n---\n"),
            ("b/d.html", "<meta name=\"wiki:tags\" content=\"x\"><h1>D</h1>"),
            ("b/e.png", ""),
            ("bb.md", "#y\n"),
            ("z/f.md", "# F\n"),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        // 旧の list（全行を取ってから limit で切る）
        let old = |ix: &Index, q: &ListQuery| -> (usize, Vec<String>) {
            let mut sql = format!("SELECT {PAGE_COLS} FROM pages p WHERE 1");
            let mut args: Vec<String> = Vec::new();
            if let Some(pre) = q.prefix.as_deref().filter(|s| !s.is_empty()) {
                args.push(pre.to_string());
                sql.push_str(&format!(" AND substr(path, 1, length(?{n})) = ?{n}", n = args.len()));
            }
            if let Some(k) = q.kind.as_deref().filter(|s| !s.is_empty()) {
                args.push(k.to_string());
                sql.push_str(&format!(" AND kind = ?{}", args.len()));
            }
            for t in q.tags.iter().map(|t| t.trim().trim_start_matches('#')).filter(|t| !t.is_empty()) {
                args.push(t.to_string());
                sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM tags t WHERE t.path = p.path AND t.tag = ?{} COLLATE NOCASE)", args.len()));
            }
            sql.push_str(" ORDER BY path");
            let mut st = ix.conn_for_test().prepare(&sql).unwrap();
            let rows: Vec<String> =
                st.query_map(rusqlite::params_from_iter(args.iter()), page_row).unwrap().map(|r| r.unwrap().path).collect();
            (rows.len(), rows.into_iter().take(q.limit).collect())
        };
        let s = |v: &str| Some(v.to_string());
        let conds = [
            ListQuery::default(),
            ListQuery { prefix: s("b/"), ..Default::default() },
            ListQuery { prefix: s("b"), ..Default::default() },
            ListQuery { prefix: s(""), kind: s(""), ..Default::default() },
            ListQuery { kind: s("md"), ..Default::default() },
            ListQuery { tags: vec!["X".into()], ..Default::default() },
            ListQuery { tags: vec!["x".into(), "#y".into()], ..Default::default() },
            ListQuery { prefix: s("b/"), kind: s("html"), tags: vec!["x".into()], ..Default::default() },
            ListQuery { prefix: s("nope/"), ..Default::default() },
        ];
        let mut checked = 0;
        for c in &conds {
            let all = old(&ix, &ListQuery { limit: usize::MAX, ..c.clone() }).0;
            for limit in [0, 1, 2, all, all + 5, usize::MAX] {
                let q = ListQuery { limit, ..c.clone() };
                let (total, rows) = ix.list(&q).unwrap();
                let paths: Vec<String> = rows.into_iter().map(|r| r.path).collect();
                assert_eq!((total, paths.clone()), old(&ix, &q), "{q:?}");
                assert_eq!(total, all, "{q:?}");
                assert_eq!(paths.len(), limit.min(all), "{q:?}");
                checked += 1;
            }
        }
        assert_eq!(checked, conds.len() * 6);
        assert_eq!(ix.list(&ListQuery { limit: usize::MAX, ..Default::default() }).unwrap().0, 6);
        // 発行する SQL: 1 つの読みトランザクションの中で count(*) と LIMIT 付きの SELECT の 2 本（全行を読む SELECT が無い）
        fn tick(sql: &str, _: Duration) {
            LIST_SQL.with(|v| v.borrow_mut().push(sql.to_string()));
        }
        ix.conn.profile(Some(tick));
        LIST_SQL.with(|v| v.borrow_mut().clear());
        let (total, rows) = ix.list(&ListQuery { limit: 2, ..Default::default() }).unwrap();
        ix.conn.profile(None);
        assert_eq!((total, rows.len()), (6, 2));
        let sqls = LIST_SQL.with(|v| v.borrow().clone());
        assert!(sqls.first().is_some_and(|q| q.starts_with("BEGIN")) && sqls.last().is_some_and(|q| q == "COMMIT"), "{sqls:?}");
        let sqls: Vec<String> = sqls[1..sqls.len() - 1].to_vec();
        assert_eq!(sqls.len(), 2, "{sqls:?}");
        assert!(sqls.iter().any(|q| q.contains("count(*)")), "{sqls:?}");
        assert!(sqls.iter().filter(|q| !q.contains("count(*)")).all(|q| q.contains(" LIMIT ")), "{sqls:?}");
    }

    /// 壊れた索引（ゴミを書いたファイル・WAL の無い写しで malformed）は退避して作り直し、検索が通る。退避は 1 世代だけ
    #[test]
    fn corrupt_index_is_quarantined_and_rebuilt() {
        let (_t, root, ip) = tmp("corrupt");
        std::fs::write(root.join("a.md"), "# 胆道\n\n本文 glioma\n").unwrap();
        let dir = ip.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let corrupt_files = || -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.contains(".corrupt-") && !n.ends_with("-wal"))
                .collect();
            v.sort();
            v
        };
        let search = |q: &str| {
            with_index(&ip, &root, |ix| {
                ix.refresh()?;
                Ok(ix.search(&SearchQuery { query: q.into(), limit: 10, ..Default::default() })?.1.len())
            })
        };
        // 1) not a database（ゴミ）
        std::fs::write(&ip, vec![0x5Au8; 8192]).unwrap();
        remove_index_handle(&ip);
        assert_eq!(search("胆道").unwrap(), 1);
        assert_eq!(corrupt_files().len(), 1, "{:?}", corrupt_files());
        assert!(recovered().is_some());
        let first = corrupt_files();
        // 2) malformed（本体の途中を壊す = 開けるが読むと SQLITE_CORRUPT）
        remove_index_handle(&ip);
        with_index(&ip, &root, |ix| Ok(ix.conn_for_test().execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?)).unwrap();
        remove_index_handle(&ip);
        let mut bytes = std::fs::read(&ip).unwrap();
        let len = bytes.len();
        for b in &mut bytes[4096..len] {
            *b = 0xA5;
        }
        std::fs::write(&ip, &bytes).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100)); // 退避名の秒を変える
        assert_eq!(search("glioma").unwrap(), 1);
        assert_eq!(corrupt_files().len(), 1, "1 世代だけ: {:?}", corrupt_files());
        assert_ne!(corrupt_files(), first, "malformed も退避した（前の退避は消えた）");
        // 3) 健全な索引は退避しない
        let before = corrupt_files();
        remove_index_handle(&ip);
        assert_eq!(search("glioma").unwrap(), 1);
        assert_eq!(corrupt_files(), before);
    }

    /// 判定できない（BUSY = 他の接続が排他で握っている）ときは退避しない（健全な索引を捨てない）。CORRUPT は退避する
    #[test]
    fn quarantine_skips_busy_and_healthy() {
        let (_t, root, ip) = tmp("busy");
        std::fs::write(root.join("a.md"), "# A\n").unwrap();
        {
            let mut ix = Index::open(&ip, &root).unwrap();
            ix.refresh().unwrap();
        }
        let why = CoreError::wiki(crate::wiki::codes::FS, "test");
        let moved = |ip: &Path| {
            std::fs::read_dir(ip.parent().unwrap()).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
        };
        // 健全
        assert_eq!(corrupt_verdict(&ip, Duration::from_millis(100)), Some(false));
        quarantine(&ip, &why, Duration::from_millis(100)).unwrap();
        assert!(!moved(&ip) && ip.exists());
        // 他の接続が排他ロック（WAL でも locking_mode = EXCLUSIVE の書込は読み手を BUSY にする）
        let lock = Connection::open(&ip).unwrap();
        lock.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE; UPDATE meta SET value = value WHERE key = 'schema';").unwrap();
        assert_eq!(corrupt_verdict(&ip, Duration::from_millis(100)), None, "BUSY は判定できない");
        quarantine(&ip, &why, Duration::from_millis(100)).unwrap();
        assert!(!moved(&ip) && ip.exists(), "BUSY では退避しない");
        lock.execute_batch("COMMIT").unwrap();
        drop(lock);
        // 無いファイルは判定できない（作らない）
        let none = ip.with_file_name("nope.sqlite");
        assert_eq!(corrupt_verdict(&none, Duration::from_millis(100)), None);
        assert!(!none.exists(), "読み専用で開く = 作らない");
        // ゴミは退避する
        std::fs::write(&ip, vec![0x5Au8; 8192]).unwrap();
        let _ = std::fs::remove_file(format!("{}-wal", ip.display()));
        let _ = std::fs::remove_file(format!("{}-shm", ip.display()));
        assert_eq!(corrupt_verdict(&ip, Duration::from_millis(100)), Some(true));
        quarantine(&ip, &why, Duration::from_millis(100)).unwrap();
        assert!(moved(&ip) && !ip.exists());
    }

    /// プロセスの接続を閉じる（ファイルは残す）
    fn remove_index_handle(ip: &Path) {
        let m = GLOBAL.get_or_init(|| Mutex::new(None));
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_some_and(|ix| ix.path == ip) {
            *g = None;
        }
    }

    /// drop_flag は自分の旗（pid と開始時刻が一致）だけを外す（n4）
    #[test]
    fn drop_flag_only_removes_own_flag() {
        let (_t, root, ip) = tmp("own");
        let mut ix = Index::open(&ip, &root).unwrap();
        assert!(ix.take_flag().unwrap());
        // 他のプロセスが引き継いだ（旗の値が変わった）
        ix.conn.execute("UPDATE meta SET value = '1:5:5' WHERE key = 'rebuilding'", []).unwrap();
        ix.drop_flag().unwrap();
        assert_eq!(ix.meta("rebuilding").unwrap().as_deref(), Some("1:5:5"), "他人の旗は消さない");
        // 自分の旗（heartbeat の時刻だけ変わったもの）は消す
        ix.conn.execute("DELETE FROM meta WHERE key = 'rebuilding'", []).unwrap();
        assert!(ix.take_flag().unwrap());
        let start = ix.flag_start.unwrap();
        ix.conn
            .execute("UPDATE meta SET value = ?1 WHERE key = 'rebuilding'", [format!("{}:{start}:{}", std::process::id(), start + 100)])
            .unwrap();
        ix.drop_flag().unwrap();
        assert_eq!(ix.meta("rebuilding").unwrap(), None);
        assert_eq!(ix.meta("progress").unwrap(), None);
    }

    /// 走査が途中で失敗しても旗は必ず外れる（n2）
    #[test]
    fn flag_is_dropped_when_rebuild_fails() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, root, ip) = tmp("fail");
        std::fs::write(root.join("a.md"), "x").unwrap();
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = ix.rebuild();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err());
        assert_eq!(ix.meta("rebuilding").unwrap(), None);
        assert_eq!(ix.flag_start, None);
        // 次の作り直しは通る
        assert_eq!(ix.rebuild().unwrap(), Fresh::Ok);
        assert_eq!(ix.stats().unwrap()["pages"], 1);
    }

    /// 大小だけ違う候補は同じ実体のときだけ採る（n1・大小を区別するボリュームの別ファイルを混ぜない）
    #[test]
    fn case_correction_only_for_same_file() {
        let (_t, root, ip) = tmp("case");
        std::fs::write(root.join("Page.md"), "x").unwrap();
        std::fs::write(root.join("other.md"), "y").unwrap();
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        assert_eq!(ix.lookup_path("page.md").unwrap().as_deref(), Some("Page.md"));
        let same = std::fs::canonicalize(root.join("page.md")).unwrap();
        assert_eq!(crate::wiki::commands::index_path_for(&ix, "page.md", &same).unwrap().as_deref(), Some("Page.md"));
        // 別の実体（大小を区別するボリュームで page.md が別ファイルだった場合に相当）は採らない
        let other = root.join("other.md");
        assert_eq!(crate::wiki::commands::index_path_for(&ix, "page.md", &other).unwrap(), None);
    }

    /// 走査が終わったら権限を戻す（assert で落ちても tempdir を消せるように）
    struct Unlock(PathBuf);
    impl Drop for Unlock {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    /// 一部のフォルダが読めない走査では、その配下の既知のページ（リンク・tags・FTS）を消さない。
    /// 読める所で消えたページはいつも通り外す。読めるようになれば普通に更新される
    #[test]
    fn unreadable_subdir_keeps_its_pages() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, root, ip) = tmp("a2");
        std::fs::create_dir_all(root.join("locked/deep")).unwrap();
        std::fs::write(root.join("locked/x.md"), "#neuro\n[[top]] と [[locked/deep/y]]").unwrap();
        std::fs::write(root.join("locked/deep/y.md"), "zebrafish").unwrap();
        std::fs::write(root.join("top.md"), "[[locked/x]]").unwrap();
        std::fs::write(root.join("gone.md"), "g").unwrap();
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        assert_eq!(ix.stats().unwrap()["pages"], 4);
        let count = |ix: &Index, sql: &str| ix.conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count(&ix, "SELECT count(*) FROM links WHERE src_path = 'locked/x.md' AND target_path IS NOT NULL"), 2);
        // 読めなくなった区画 + 読める所で消えたページ
        std::fs::remove_file(root.join("gone.md")).unwrap();
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let _unlock = Unlock(root.join("locked"));
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok);
        let st = ix.stats().unwrap();
        assert_eq!(st["walk_errors"], 1, "{st}");
        assert_eq!(st["pages"], 3, "locked/ の 2 ページは残り gone.md だけ消える: {st}");
        assert!(ix.page("locked/x.md").unwrap().is_some());
        assert!(ix.page("locked/deep/y.md").unwrap().is_some());
        assert!(ix.page("gone.md").unwrap().is_none());
        assert_eq!(count(&ix, "SELECT count(*) FROM links WHERE src_path = 'locked/x.md' AND target_path IS NOT NULL"), 2, "リンクも保持");
        assert_eq!(count(&ix, "SELECT count(*) FROM tags WHERE path = 'locked/x.md'"), 1, "tags も保持");
        assert_eq!(count(&ix, "SELECT count(*) FROM pages_fts WHERE pages_fts MATCH 'zebrafish'"), 1, "FTS も保持");
        assert_eq!(count(&ix, "SELECT count(*) FROM links WHERE src_path = 'top.md' AND target_path = 'locked/x.md'"), 1, "バックリンクも保持");
        // refresh_paths（wiki_read・GUI の監視）でも、stat が拒否されたページは外さない。無いページはいつも通り外す
        std::fs::write(root.join("tmp.md"), "t").unwrap();
        ix.refresh_paths(&[root.join("tmp.md")]).unwrap();
        std::fs::remove_file(root.join("tmp.md")).unwrap();
        assert_eq!(ix.refresh_paths(&[root.join("locked/x.md"), root.join("tmp.md"), root.join("never.md")]).unwrap().len(), 1);
        assert!(ix.page("locked/x.md").unwrap().is_some(), "EACCES は据え置き");
        assert!(ix.page("tmp.md").unwrap().is_none(), "NotFound は外す");
        assert_eq!(ix.stats().unwrap()["pages"], 3);
        // 読めるようになれば普通に更新（消したページは外れ・書き換えは取り込む）
        drop(_unlock);
        std::fs::remove_file(root.join("locked/deep/y.md")).unwrap();
        std::fs::write(root.join("locked/x.md"), "#onco\n[[top]]").unwrap();
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok);
        let st = ix.stats().unwrap();
        assert_eq!(st["walk_errors"], 0, "{st}");
        assert_eq!(st["pages"], 2, "{st}");
        assert!(ix.page("locked/deep/y.md").unwrap().is_none());
        assert_eq!(count(&ix, "SELECT count(*) FROM tags WHERE path = 'locked/x.md' AND tag = 'onco'"), 1);
    }

    /// 本文を読めなかったページは mtime の印で書かれ（read_errors に数える）、次の走査で再試行してリンク・tags が入る。
    /// 読めるページと媒体（本文を読まない）は数えない
    #[test]
    fn unreadable_page_is_retried_next_scan() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, root, ip) = tmp("a3");
        std::fs::write(root.join("r.md"), "#neuro\n[[ok]]").unwrap();
        std::fs::write(root.join("ok.md"), "fine").unwrap();
        std::fs::write(root.join("img.png"), [0u8; 3]).unwrap();
        let real_mtime = scan::stat(&root.join("r.md")).unwrap().0;
        assert_ne!(real_mtime, 0);
        std::fs::set_permissions(root.join("r.md"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let _unlock = Unlock(root.join("r.md"));
        let mut ix = Index::open(&ip, &root).unwrap();
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok);
        let st = ix.stats().unwrap();
        assert_eq!(st["pages"], 3, "{st}");
        assert_eq!(st["read_errors"], 1, "{st}");
        let p = ix.page("r.md").unwrap().unwrap();
        assert_eq!(p.mtime_ns, READ_ERROR_MTIME, "読めなかったページは mtime の印（次の走査で再試行）");
        assert_eq!(p.sha256, None);
        assert!(ix.page("ok.md").unwrap().unwrap().sha256.is_some());
        let count = |ix: &Index, sql: &str| ix.conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count(&ix, "SELECT count(*) FROM links WHERE src_path = 'r.md'"), 0);
        // まだ読めない = また試みて、また 1 件（固定されない・増えもしない）
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok);
        assert_eq!(ix.stats().unwrap()["read_errors"], 1);
        assert_eq!(ix.page("r.md").unwrap().unwrap().mtime_ns, READ_ERROR_MTIME);
        // 読めるようになった（mtime / size は変わっていない）→ 次の走査で取り込まれる
        drop(_unlock);
        assert_eq!(scan::stat(&root.join("r.md")).unwrap().0, real_mtime);
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok);
        let st = ix.stats().unwrap();
        assert_eq!(st["read_errors"], 0, "{st}");
        let p = ix.page("r.md").unwrap().unwrap();
        assert_eq!(p.mtime_ns, real_mtime);
        assert!(p.sha256.is_some());
        assert_eq!(p.tags, ["neuro"]);
        assert_eq!(count(&ix, "SELECT count(*) FROM links WHERE src_path = 'r.md' AND target_path = 'ok.md'"), 1);
        // refresh_paths（GUI のファイル監視）の経路でも同じ印が付く
        std::fs::set_permissions(root.join("ok.md"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let _unlock2 = Unlock(root.join("ok.md"));
        std::fs::write(root.join("new.md"), "n").unwrap();
        ix.refresh_paths(&[root.join("ok.md"), root.join("new.md")]).unwrap();
        assert_eq!(ix.page("ok.md").unwrap().unwrap().mtime_ns, READ_ERROR_MTIME);
        assert!(ix.page("new.md").unwrap().unwrap().sha256.is_some());
        drop(_unlock2);
        ix.refresh().unwrap();
        assert!(ix.page("ok.md").unwrap().unwrap().sha256.is_some());
        assert_eq!(ix.stats().unwrap()["read_errors"], 0);
    }

    /// 読めないページが BIG_SCAN 件を超えて続いても、再試行分は大きな走査（旗）に数えない。走査が出さない mtime の印なので
    /// epoch 前（mtime 0）のファイルと混ざらない
    #[test]
    fn retries_do_not_count_as_big_scan() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, root, ip) = tmp("big");
        std::fs::create_dir_all(root.join("u")).unwrap();
        let n = BIG_SCAN + 1;
        for i in 0..n {
            let f = root.join(format!("u/p{i}.md"));
            std::fs::write(&f, "x").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        let _unlock = Unlock(root.join("u"));
        let mut ix = Index::open(&ip, &root).unwrap();
        // 初回は新規 n 件 = 大きな走査（自分で旗を立てて外す）
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok);
        assert_eq!(ix.stats().unwrap()["read_errors"], n as i64);
        assert_eq!(ix.conn.query_row("SELECT count(*) FROM pages WHERE mtime = ?1", [READ_ERROR_MTIME], |r| r.get::<_, i64>(0)).unwrap(), n as i64);
        // 他のプロセスの生きた旗がある状態で 2 回目: 再試行だけなら旗を取りに行かず Ok（取りに行くと Rebuilding になる）
        let s = now_ms() / 1000;
        ix.conn.execute("INSERT OR REPLACE INTO meta VALUES ('rebuilding', ?1)", [format!("1:{s}:{s}")]).unwrap();
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok, "再試行 {n} 件は大きな走査ではない");
        assert_eq!(ix.stats().unwrap()["read_errors"], n as i64);
        ix.conn.execute("DELETE FROM meta WHERE key = 'rebuilding'", []).unwrap();
        // epoch 前の mtime（0）のファイルは再試行の印と混ざらない = 読めれば普通に取り込まれ、再試行にも数えない
        let old = root.join("old.md");
        std::fs::write(&old, "#epoch").unwrap();
        std::fs::File::options().write(true).open(&old).unwrap().set_times(filetime_epoch()).unwrap();
        assert_eq!(scan::stat(&old).unwrap().0, 0);
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok);
        let p = ix.page("old.md").unwrap().unwrap();
        assert_eq!((p.mtime_ns, p.tags.clone()), (0, vec!["epoch".to_string()]));
        assert_eq!(ix.stats().unwrap()["read_errors"], n as i64, "old.md は読めたので数えない");
        // mtime 0 のファイルが読めなくなる → 印で書かれ、読めるようになった次の走査で再試行される（印が 0 だと mtime が合って固まる）
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _unlock_old = Unlock(old.clone());
        ix.refresh_paths(std::slice::from_ref(&old)).unwrap();
        assert_eq!(ix.page("old.md").unwrap().unwrap().mtime_ns, READ_ERROR_MTIME);
        drop(_unlock_old);
        assert_eq!(scan::stat(&old).unwrap().0, 0, "権限を戻しても mtime は 0 のまま");
        assert_eq!(ix.refresh().unwrap(), Fresh::Ok);
        let p = ix.page("old.md").unwrap().unwrap();
        assert_eq!((p.mtime_ns, p.sha256.is_some(), p.tags.clone()), (0, true, vec!["epoch".to_string()]), "再試行で戻る");
    }

    /// mtime = epoch 前（1969-12-31）の FileTimes
    fn filetime_epoch() -> std::fs::FileTimes {
        std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH - std::time::Duration::from_secs(86_400))
    }

    type LinkDump = Vec<(i64, String, String, String, Option<String>, i64)>;

    fn dump_links(ix: &Index) -> LinkDump {
        let mut st =
            ix.conn.prepare("SELECT rowid, src_path, target_raw, kind, target_path, ambiguous FROM links ORDER BY rowid").unwrap();
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    }

    /// 差分解決の後の links 表と、同じ状態で全部を解決し直した links 表が全行一致すること（戻り値 = その表）
    fn same_as_full(ix: &mut Index, scene: &str) -> LinkDump {
        let delta = dump_links(ix);
        ix.resolve(None).unwrap();
        let full = dump_links(ix);
        assert_eq!(delta.len(), full.len(), "{scene}: 件数");
        for (d, f) in delta.iter().zip(&full) {
            assert_eq!(d, f, "{scene}: 差分解決と全解決が違う");
        }
        full
    }

    fn link<'a>(rows: &'a LinkDump, src: &str, target: &str) -> (Option<&'a str>, i64) {
        let r = rows.iter().find(|r| r.1 == src && r.2 == target).unwrap_or_else(|| panic!("{src} → {target} が無い"));
        (r.4.as_deref(), r.5)
    }

    /// ページの増減の後は影響のあるリンクだけ解決し直す。7 つの場面で全解決と全行一致し、各場面で解決が実際に変わる
    #[test]
    fn delta_resolve_matches_full() {
        let (_t, root, ip) = tmp("delta");
        let w = |rel: &str, body: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            p
        };
        w(
            "index.md",
            &format!(
                "[[alpha]] [[beta]] [[missing]] [[gone]] [[dup]] [[target-later]] [r](notes/sub/../alpha.md) [u](../x.md) ![[pic.jpg]] \
                 [f](file://{}/notes/alpha.md) [[#h]]",
                root.display()
            ),
        );
        w("notes/alpha.md", "[[beta]] [up](../index.md) [[sub/deep]] [[Target-Later.md]] [d](sub/..)");
        w("notes/sub/deep.md", "[[alpha]] [[dup]] [s](./)");
        w("other/beta.md", "[[gone]]");
        w("gone.md", "x");
        w("deep/a/dup.md", "");
        w("misc/dup.md", "");
        w("page.html", "<a href=\"notes/alpha.md\">a</a><img src=\"img/pic.jpg\"><a href=\"gone.md\">g</a>");
        w("img/pic.jpg", "jpg");
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        let r0 = same_as_full(&mut ix, "初回");
        assert_eq!(link(&r0, "index.md", "alpha"), (Some("notes/alpha.md"), 0));
        assert_eq!(link(&r0, "index.md", "dup"), (Some("misc/dup.md"), 1));

        // 1 ページを 1 件追加（出るリンクも解決される）
        let p = w("new.md", "[[alpha]] [[nothing]] [n](notes)");
        ix.refresh_paths(&[&p]).unwrap();
        let r = same_as_full(&mut ix, "1 追加");
        assert_eq!(link(&r, "new.md", "alpha"), (Some("notes/alpha.md"), 0));
        assert_eq!(link(&r, "new.md", "notes"), (None, 0), "notes はフォルダ（notes.md は無い）");
        // 2 ページを 1 件削除（出るリンクも消える）
        std::fs::remove_file(&p).unwrap();
        ix.refresh_paths(&[&p]).unwrap();
        let r = same_as_full(&mut ix, "2 削除");
        assert!(r.iter().all(|l| l.1 != "new.md"));
        // 3 同じ basename を別フォルダに追加 = 曖昧になる（未解決でも曖昧でもなく、先も変わらないリンクが名前で引かれる）
        let p = w("other/Alpha.md", "");
        ix.refresh_paths(&[&p]).unwrap();
        let r = same_as_full(&mut ix, "3 曖昧化");
        assert_eq!(link(&r, "index.md", "alpha"), (Some("notes/alpha.md"), 1));
        assert_eq!(link(&r, "notes/sub/deep.md", "alpha"), (Some("notes/alpha.md"), 1));
        assert_eq!(link(&r, "index.md", "notes/sub/../alpha.md"), (Some("notes/alpha.md"), 0), "md の相対は exact で当たる");
        // 4 曖昧の片方を削除 = 解消
        std::fs::remove_file(root.join("misc/dup.md")).unwrap();
        ix.refresh_paths(&[root.join("misc/dup.md")]).unwrap();
        let r = same_as_full(&mut ix, "4 曖昧の解消");
        assert_eq!(link(&r, "index.md", "dup"), (Some("deep/a/dup.md"), 0));
        // 5 未解決リンクの先を作る（大小・拡張子の有無の違う 2 つの書き方）
        let p = w("later/target-later.md", "");
        ix.refresh_paths(&[&p]).unwrap();
        let r = same_as_full(&mut ix, "5 未解決の先を作る");
        assert_eq!(link(&r, "index.md", "target-later"), (Some("later/target-later.md"), 0));
        assert_eq!(link(&r, "notes/alpha.md", "Target-Later.md"), (Some("later/target-later.md"), 0));
        // 6 リンク先を消す（wiki と html の相対の両方）
        std::fs::remove_file(root.join("gone.md")).unwrap();
        ix.refresh_paths(&[root.join("gone.md")]).unwrap();
        let r = same_as_full(&mut ix, "6 リンク先を消す");
        assert_eq!(link(&r, "other/beta.md", "gone"), (None, 0));
        assert_eq!(link(&r, "page.html", "gone.md"), (None, 0));
        // 7 変更のみ（増減なし = 出るリンクだけ解決し直す従来の経路）
        let p = w("notes/alpha.md", "[[beta]] [[index]] [[sub/deep]]");
        ix.refresh_paths(&[&p]).unwrap();
        let r = same_as_full(&mut ix, "7 変更のみ");
        assert_eq!(link(&r, "notes/alpha.md", "index"), (Some("index.md"), 0));
        // 増減と変更が同じ回に来る（監視の束）+ 走査の経路
        std::fs::remove_file(root.join("other/Alpha.md")).unwrap();
        w("misc/dup.md", "[[gone]]");
        w("notes/sub/deep.md", "[[alpha]] [[dup]]");
        ix.refresh_paths(&[root.join("other/Alpha.md"), root.join("misc/dup.md"), root.join("notes/sub/deep.md")]).unwrap();
        let r = same_as_full(&mut ix, "束");
        assert_eq!(link(&r, "index.md", "alpha"), (Some("notes/alpha.md"), 0));
        assert_eq!(link(&r, "index.md", "dup"), (Some("misc/dup.md"), 1));
        w("gone.md", "back");
        ix.refresh().unwrap();
        let r = same_as_full(&mut ix, "走査");
        assert_eq!(link(&r, "other/beta.md", "gone"), (Some("gone.md"), 0));
    }

    /// 前の回の links 表が全解決とずれている（前の版の apply が削除・追加の commit の後、解決の前に止まった）状態から、
    /// 無関係なページの増減の差分解決で、消えたページを指したままの行が直る（未解決の行は (b) で直る）
    #[test]
    fn delta_resolve_heals_stale_rows() {
        let (_t, root, ip) = tmp("heal");
        let w = |rel: &str, body: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            p
        };
        w("index.md", "[[gone]] [[late]]");
        w("gone.md", "");
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        // 中断を模す: gone.md の削除と late.md の追加は commit されたが解決は走らなかった
        std::fs::remove_file(root.join("gone.md")).unwrap();
        let late = w("late.md", "");
        {
            let tx = wtx(&ix.conn).unwrap();
            delete_page(&tx, "gone.md").unwrap();
            let (m, sz) = scan::stat_io(&late).unwrap().unwrap();
            let e = Entry { rel: "late.md".into(), mtime_ns: m, size: sz };
            write_page(&tx, &e, &read_page(&root, &e)).unwrap();
            tx.commit().unwrap();
        }
        let stale = dump_links(&ix);
        assert_eq!(link(&stale, "index.md", "gone"), (Some("gone.md"), 0));
        // 次の回: 無関係なページの追加
        let p = w("other.md", "");
        ix.refresh_paths(&[&p]).unwrap();
        let r = same_as_full(&mut ix, "中断の後");
        assert_eq!(link(&r, "index.md", "gone"), (None, 0));
        assert_eq!(link(&r, "index.md", "late"), (Some("late.md"), 0));
    }

    /// 小さな適用はページの書込と差分解決が 1 つの tx: 解決が失敗したら（BUSY・中断の代わり）ページの書込も残らず、
    /// links 表は全解決とずれない。次の走査で同じ増減が改めて取り込まれる
    #[test]
    fn failed_delta_leaves_no_half_apply() {
        let (_t, root, ip) = tmp("halfapply");
        let w = |rel: &str, body: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            p
        };
        w("index.md", "[[alpha]]");
        w("notes/alpha.md", "");
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        let p = w("other/alpha.md", "");
        FAIL_DELTA.with(|f| f.set(true));
        assert!(ix.refresh_paths(&[&p]).is_err());
        assert!(ix.page("other/alpha.md").unwrap().is_none(), "失敗した回のページの書込は残らない");
        // 名前の当たらない別の増減の後も全解決と一致する
        let q = w("unrelated.md", "");
        ix.refresh_paths(&[&q]).unwrap();
        let r = same_as_full(&mut ix, "失敗の後");
        assert_eq!(link(&r, "index.md", "alpha"), (Some("notes/alpha.md"), 0));
        ix.refresh().unwrap();
        let r = same_as_full(&mut ix, "取り込み直し");
        assert_eq!(link(&r, "index.md", "alpha"), (Some("notes/alpha.md"), 1));
    }

    /// 実データ: ワークスペースの写しで 7 つの場面の差分解決と全解決を全行比較し、新規 1 件の refresh_paths を測る。写しの中に
    /// `zz-delta-resolve/` を作り、読んだページを消す（写しは使い捨て）。索引は写しの外の一時フォルダ
    /// OPENREF_WIKI_REAL_ROOT=<ワークスペースの写し> OPENREF_WIKI_PROD_ROOTS=<触らないワークスペースを : 区切り> \
    ///   cargo test -p openref-core --release --lib real_delta_resolve -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_delta_resolve() {
        let prod_roots = std::env::var("OPENREF_WIKI_PROD_ROOTS").expect("OPENREF_WIKI_PROD_ROOTS（触ってはいけないワークスペースを : 区切りで）");
        let root = std::fs::canonicalize(std::env::var("OPENREF_WIKI_REAL_ROOT").expect("OPENREF_WIKI_REAL_ROOT")).unwrap();
        for prod in prod_roots.split(':').filter(|s| !s.is_empty()) {
            let prod = std::fs::canonicalize(prod).unwrap_or_else(|_| PathBuf::from(prod));
            assert!(!root.starts_with(&prod) && !prod.starts_with(&root), "本番のワークスペース: {}", root.display());
        }
        // 前の回の残り（自分の作業フォルダ）を消してから索引を作る
        let dir = root.join("zz-delta-resolve");
        let _ = std::fs::remove_dir_all(&dir);
        let (_t, _, ip) = tmp("real-delta");
        let mut ix = Index::open(&ip, &root).unwrap();
        ix.refresh().unwrap();
        let r0 = same_as_full(&mut ix, "初回");
        let q = |sql: &str| -> Vec<(String, String, Option<String>)> {
            let mut st = ix.conn.prepare(sql).unwrap();
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        let plain = |t: &str| !t.is_empty() && !t.contains(['/', '#', '|', '\\', ':']) && !t.starts_with('.');
        // 一意に解決している wikilink の先（曖昧化に使う）・未解決の wikilink・外からリンクされているページ・出るリンクのあるページ
        let resolved = q("SELECT src_path, target_raw, target_path FROM links WHERE kind = 'wiki' AND target_path LIKE '%.md' AND ambiguous = 0 ORDER BY rowid");
        let (_, amb_raw, amb_path) = resolved.iter().find(|(s, t, p)| plain(t) && s != p.as_ref().unwrap()).cloned().unwrap();
        assert!(!amb_path.as_ref().unwrap().starts_with("zz-delta-resolve/"));
        let unres = q("SELECT src_path, target_raw, target_path FROM links WHERE kind = 'wiki' AND target_path IS NULL ORDER BY rowid");
        let (_, unres_raw, _) = unres.iter().find(|(_, t, _)| plain(t) && !t.contains('.')).cloned().unwrap();
        let linked = q("SELECT src_path, target_raw, target_path FROM links WHERE kind = 'wiki' AND ambiguous = 0 AND target_path LIKE '%.md'
                        AND target_path NOT IN (SELECT src_path FROM links) ORDER BY rowid");
        let gone = linked.iter().find(|(s, _, p)| s != p.as_ref().unwrap()).unwrap().2.clone().unwrap();
        let amb_path = amb_path.unwrap();
        let src_page = r0.iter().rev().map(|l| l.1.clone()).find(|s| s.ends_with(".md") && *s != gone && *s != amb_path).unwrap();
        println!("pages {} / links {} / 曖昧化 = {amb_raw}（{amb_path}）/ 未解決 = {unres_raw} / 消す先 = {gone} / 変更 = {src_page}",
            ix.stats().unwrap()["pages"], r0.len());
        std::fs::create_dir_all(&dir).unwrap();
        let ambiguous = |rows: &LinkDump| rows.iter().filter(|l| l.5 != 0).count();
        let unresolved = |rows: &LinkDump| rows.iter().filter(|l| l.4.is_none() && LinkKind::parse(&l.3).is_some_and(LinkKind::internal)).count();
        let report = |scene: &str, rows: &LinkDump, ms: f64| {
            println!("{scene}: 全行一致 {} 行・未解決 {}・曖昧 {}・refresh_paths {ms:.1}ms", rows.len(), unresolved(rows), ambiguous(rows))
        };
        let timed = |ix: &mut Index, p: &Path| {
            let t = std::time::Instant::now();
            ix.refresh_paths(&[p]).unwrap();
            t.elapsed().as_secs_f64() * 1000.0
        };
        // 1 追加 / 2 削除（3 回ずつ = 計測）
        for i in 0..3 {
            let p = dir.join(format!("new-{i}.md"));
            std::fs::write(&p, format!("[[{amb_raw}]] [[{unres_raw}]] [x](../{gone})")).unwrap();
            let ms = timed(&mut ix, &p);
            report(&format!("1 追加 #{i}"), &same_as_full(&mut ix, "1 追加"), ms);
            std::fs::remove_file(&p).unwrap();
            let ms = timed(&mut ix, &p);
            report(&format!("2 削除 #{i}"), &same_as_full(&mut ix, "2 削除"), ms);
        }
        // 3 曖昧化 / 4 解消
        let name = amb_path.rsplit('/').next().unwrap();
        let p = dir.join(name);
        std::fs::write(&p, "").unwrap();
        let ms = timed(&mut ix, &p);
        let r = same_as_full(&mut ix, "3 曖昧化");
        assert!(ambiguous(&r) > ambiguous(&r0), "曖昧が増える");
        report("3 曖昧化", &r, ms);
        std::fs::remove_file(&p).unwrap();
        let ms = timed(&mut ix, &p);
        let r = same_as_full(&mut ix, "4 曖昧の解消");
        assert_eq!(ambiguous(&r), ambiguous(&r0));
        report("4 曖昧の解消", &r, ms);
        // 5 未解決の先を作る
        let p = dir.join(format!("{unres_raw}.md"));
        std::fs::write(&p, "").unwrap();
        let ms = timed(&mut ix, &p);
        let r = same_as_full(&mut ix, "5 未解決の先を作る");
        assert!(unresolved(&r) < unresolved(&r0), "未解決が減る");
        report("5 未解決の先を作る", &r, ms);
        // 6 リンク先を消す
        let before = unresolved(&r);
        let p = root.join(&gone);
        std::fs::remove_file(&p).unwrap();
        let ms = timed(&mut ix, &p);
        let r = same_as_full(&mut ix, "6 リンク先を消す");
        assert!(unresolved(&r) > before, "未解決が増える");
        report("6 リンク先を消す", &r, ms);
        // 7 変更のみ
        let p = root.join(&src_page);
        let body = std::fs::read_to_string(&p).unwrap_or_default();
        std::fs::write(&p, format!("{body}\n[[{amb_raw}]]\n")).unwrap();
        let ms = timed(&mut ix, &p);
        report("7 変更のみ", &same_as_full(&mut ix, "7 変更のみ"), ms);
    }
}

