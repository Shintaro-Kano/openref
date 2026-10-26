//! 新版の確認（app の起動時・前回から 24 時間以上経っていれば 1 回）。配布サイトの `dl/VERSION`（版番号 1 行のテキスト =
//! deploy-site.sh が tar と一緒に置く。**GitHub Releases には無い** = 知らせの基準は site だけ）を GET して、動いている版と比べる。
//! 送るのはその GET 1 回だけ（識別子・利用情報は無し。User-Agent に動いている版 `openref/X.Y.Z` が載る = 他の経路の固定の
//! `openref/0.1 (…)` と違い版が分かる）。結果（確認した日時・取れた版番号・失敗の理由）は settings に書き、取れても取れなくても
//! 次の 24 時間は取りに行かない（= 起動のたびに GET しない）。
//! Settings › 起動時に新版を確認（`update_check`・"0" で止める = opt-out。既定は確認する。初回起動の 1 回は止める前に走る）。
//! **入れ替えはしない**（画面に知らせるだけ。更新は install.sh の再実行 = SECURITY.md §1・§3）。CLI / MCP / connectd は呼ばない。
//! 開発時は環境変数 `OPENREF_UPDATE_URL` で確認先を差し替える（app の check_update が読む）。

use crate::{CoreError, Db, Result};
use std::io::Read;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// 配布サイトの版番号ファイル。開発時は `OPENREF_UPDATE_URL` で差し替える（app の check_update）
pub const CHECK_URL: &str = "https://openref.unitedradiology.jp/dl/VERSION";
/// 取りに行く間隔（秒）= 1 日
pub const INTERVAL_SECS: i64 = 24 * 60 * 60;

/// settings の鍵: 確認する / しない（"0" = しない。それ以外・未設定 = する）
pub const KEY_ENABLED: &str = "update_check";
/// settings の鍵: 最後に取りに行った日時（unix 秒）
pub const KEY_CHECKED_AT: &str = "update_checked_at";
/// settings の鍵: 最後に取れた版番号
pub const KEY_LATEST: &str = "update_latest";
/// settings の鍵: 最後に取りに行って失敗した理由（`<unix 秒>: <理由>`。取れたら消す。GUI の stderr は捨てられるので sql / MCP から読める場所に残す）
pub const KEY_ERROR: &str = "update_error";

/// settings に残している前回の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    pub enabled: bool,
    pub checked_at: Option<i64>,
    pub latest: Option<String>,
}

/// [`check`] の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// 動いている版より新しい版番号（今回の取得か、前回の結果）。無ければ None
    pub newer: Option<String>,
    /// 今回取りに行って・結果を書いて失敗した理由（前回の結果で続けている。settings の update_error にも残し、呼び手がログに出す）
    pub error: Option<String>,
}

pub fn cached(db: &Db) -> Cached {
    Cached {
        enabled: db.setting(KEY_ENABLED).as_deref() != Some("0"),
        checked_at: db.setting(KEY_CHECKED_AT).and_then(|v| v.trim().parse().ok()),
        latest: db.setting(KEY_LATEST).filter(|v| parse_version(v).is_some()),
    }
}

/// 取りに行く番か。止めていれば行かない。前回から 1 日経った・前回が無い・時計が戻っている（前回が 5 分より先の未来。NTP の
/// 数秒の戻りでは行かない）なら行く。壊れた値で桁あふれしないよう saturating
pub fn due(c: &Cached, now: i64) -> bool {
    c.enabled && c.checked_at.is_none_or(|t| t.saturating_sub(now) > 300 || now.saturating_sub(t) >= INTERVAL_SECS)
}

/// `X.Y.Z`（前後の空白・先頭の `v` は許す。pre-release 等の付加は不正 = VERSION はリリース tag と同じ素の 3 つ組）
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut it = s.split('.').map(|p| if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) { None } else { p.parse::<u64>().ok() });
    let v = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() {
        return None;
    }
    Some(v)
}

/// `latest` が `current` より新しいか（数値で比べる: 0.10.0 > 0.9.0）。どちらかが読めなければ false（知らせない側に倒す）
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// 本文の上限（版番号 1 行 = 10 バイト足らず。これより長い応答は VERSION ではない）
const MAX_BODY: u64 = 64;

/// 版番号ファイルを GET する（接続 5 秒・読み書き 10 秒。https の URL なら http へのリダイレクトには付いていかない）。
/// 本文が 64 バイトを超える・`X.Y.Z` でなければ Parse エラー（captive portal や保守ページの HTML を版と誤認しない）
pub fn fetch_latest(url: &str, current: &str) -> Result<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .https_only(url.starts_with("https://"))
        .user_agent(&format!("openref/{current}"))
        .build();
    let resp = agent.get(url).call().map_err(|e| {
        CoreError::Http(match e {
            ureq::Error::Status(code, _) => format!("HTTP {code}"),
            e => e.to_string(),
        })
    })?;
    let mut body = Vec::new();
    resp.into_reader().take(MAX_BODY + 1).read_to_end(&mut body)?;
    if body.len() as u64 > MAX_BODY {
        return Err(CoreError::Parse(format!("VERSION の形式が不正: 本文が {MAX_BODY} バイトを超える")));
    }
    let v = String::from_utf8_lossy(&body);
    let v = v.trim();
    if parse_version(v).is_none() {
        return Err(CoreError::Parse(format!("VERSION の形式が不正: {:?}", v.chars().take(32).collect::<String>())));
    }
    Ok(v.to_string())
}

/// settings の 2 鍵を 1 文で書く（まとめて入るか入らないか）
const UPSERT_TWO: &str =
    "INSERT INTO settings (key, value) VALUES (?1, ?2), (?3, ?4) ON CONFLICT(key) DO UPDATE SET value = excluded.value";

/// 取れた結果を settings に書く（1 文 = まとめて入るか入らないか。前回の失敗の理由は消す）
pub fn record(db: &Db, now: i64, latest: &str) -> Result<()> {
    db.conn.execute(
        UPSERT_TWO,
        rusqlite::params![KEY_CHECKED_AT, now.to_string(), KEY_LATEST, latest],
    )?;
    db.conn.execute("DELETE FROM settings WHERE key = ?1", [KEY_ERROR])?;
    Ok(())
}

/// 取れなかったことを settings に書く（確認した日時は進める = 起動のたびに GET しない。前回の版番号はそのまま）
pub fn record_failure(db: &Db, now: i64, why: &str) -> Result<()> {
    db.conn.execute(
        UPSERT_TWO,
        rusqlite::params![KEY_CHECKED_AT, now.to_string(), KEY_ERROR, format!("{now}: {why}")],
    )?;
    Ok(())
}

fn lock(db: &Mutex<Db>) -> Result<MutexGuard<'_, Db>> {
    db.lock().map_err(|e| CoreError::Conflict(format!("DB のロックに失敗: {e}")))
}

/// 起動時の 1 回: settings を読み、番なら取りに行って書き戻し、動いている版より新しい版番号を返す。
/// 取りに行く間は DB のロックを離す（握ったままだと起動直後の一覧の読込を最大 10 秒止める）
pub fn check(db: &Mutex<Db>, current: &str, url: &str, now: i64) -> Result<Outcome> {
    let c = {
        let g = lock(db)?;
        cached(&g)
    };
    let newer =|latest: Option<&str>| latest.filter(|l| is_newer(l, current)).map(str::to_string);
    if !c.enabled {
        return Ok(Outcome { newer: None, error: None });
    }
    if !due(&c, now) {
        return Ok(Outcome { newer: newer(c.latest.as_deref()), error: None });
    }
    match fetch_latest(url, current) {
        Ok(latest) => {
            // 書けなくても取れた新版は返す（帯を出す）。書けなかったことは理由として返す
            let g = lock(db)?;
            let error = record(&g, now, &latest).err().map(|e| format!("結果を保存できません: {e}"));
            Ok(Outcome { newer: newer(Some(&latest)), error })
        }
        Err(e) => {
            let mut why = e.to_string();
            let g = lock(db)?;
            if let Err(e2) = record_failure(&g, now, &why) {
                why = format!("{why}（失敗を保存できません: {e2}）");
            }
            Ok(Outcome { newer: newer(c.latest.as_deref()), error: Some(why) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn parses_plain_three_part_versions_only() {
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version(" v0.2.1\n"), Some((0, 2, 1)));
        assert_eq!(parse_version("10.0.3"), Some((10, 0, 3)));
        for bad in ["0.1", "0.1.0.1", "a.b.c", "0.1.0-beta", "", "0..1", "1.2.3 extra"] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn newer_compares_numerically() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"), "数値で比べる（文字列なら 0.9 > 0.10）");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.2.0", "garbage"));
    }

    #[test]
    fn due_once_a_day_unless_disabled() {
        let on = |t: Option<i64>| Cached { enabled: true, checked_at: t, latest: None };
        assert!(due(&on(None), 1_000_000));
        assert!(!due(&on(Some(1_000_000 - 3600)), 1_000_000), "1 時間前 = まだ");
        assert!(!due(&on(Some(1_000_000 - INTERVAL_SECS + 1)), 1_000_000));
        assert!(due(&on(Some(1_000_000 - INTERVAL_SECS)), 1_000_000), "ちょうど 1 日");
        assert!(!due(&on(Some(1_000_000 + 10)), 1_000_000), "前回が 10 秒先 = NTP の戻り程度 → 行かない");
        assert!(due(&on(Some(1_000_000 + 301)), 1_000_000), "前回が 5 分より先の未来 = 時計が戻った → 行く");
        assert!(due(&on(Some(i64::MIN)), 1_000_000), "壊れた値で桁あふれしない");
        assert!(due(&on(Some(i64::MAX)), 1_000_000));
        assert!(!due(&Cached { enabled: false, checked_at: None, latest: None }, 1_000_000));
    }

    #[test]
    fn cached_ignores_corrupt_settings_values() {
        let db = Db::open_in_memory().unwrap();
        db.set_setting(KEY_CHECKED_AT, "abc").unwrap();
        db.set_setting(KEY_LATEST, "<html>").unwrap();
        db.set_setting(KEY_ENABLED, " 0").unwrap();
        let c = cached(&db);
        assert_eq!(c, Cached { enabled: true, checked_at: None, latest: None }, "読めない値は無いものとして扱う（\"0\" ちょうどだけが opt-out）");
    }

    /// 2 秒の間、来た接続に `status` + `body` を返す HTTP サーバ（実際に ureq で GET させる）。受けた数を数える
    fn serve(status: &'static str, body: &'static str) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/VERSION", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        listener.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            // テストが終わるまで待ち続けず、2 秒で諦める（接続が来ない = 期待どおり、のケースがある）
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        s.set_nonblocking(false).unwrap();
                        let mut buf = [0u8; 2048];
                        let mut head = Vec::new();
                        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                            let n = s.read(&mut buf).unwrap();
                            if n == 0 {
                                break;
                            }
                            head.extend_from_slice(&buf[..n]);
                        }
                        let req = String::from_utf8_lossy(&head);
                        assert!(req.starts_with("GET /VERSION HTTP/1.1"), "{req}");
                        assert!(req.contains("User-Agent: openref/0.1.0\r\n"), "版だけの UA: {req}");
                        h.fetch_add(1, Ordering::SeqCst);
                        let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        let _ = s.flush();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(e) => panic!("{e}"),
                }
            }
        });
        (url, hits)
    }

    fn mem_db() -> Mutex<Db> {
        Mutex::new(Db::open_in_memory().unwrap())
    }

    #[test]
    fn first_check_fetches_records_and_reports_newer() {
        let (url, hits) = serve("200 OK", "9.9.9\n");
        let db = mem_db();
        {
            let g = db.lock().unwrap();
            g.set_setting(KEY_ERROR, "1: 前回の失敗").unwrap();
        }
        let out = check(&db, "0.1.0", &url, 1_000).unwrap();
        assert_eq!(out, Outcome { newer: Some("9.9.9".into()), error: None });
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let g = db.lock().unwrap();
        assert_eq!(g.setting(KEY_CHECKED_AT).as_deref(), Some("1000"));
        assert_eq!(g.setting(KEY_LATEST).as_deref(), Some("9.9.9"));
        assert_eq!(g.setting(KEY_ERROR), None, "取れたら前回の失敗の理由は消す");
    }

    #[test]
    fn within_a_day_uses_cache_without_fetching() {
        let (url, hits) = serve("200 OK", "9.9.9\n");
        let db = mem_db();
        {
            let g = db.lock().unwrap();
            record(&g, 1_000, "0.5.0").unwrap();
        }
        let out = check(&db, "0.1.0", &url, 1_000 + 3600).unwrap();
        assert_eq!(out, Outcome { newer: Some("0.5.0".into()), error: None }, "前回の結果を使う");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(hits.load(Ordering::SeqCst), 0, "取りに行かない");
        // 1 日経てば取りに行き、新しい結果で上書き
        let out = check(&db, "0.1.0", &url, 1_000 + INTERVAL_SECS).unwrap();
        assert_eq!(out.newer.as_deref(), Some("9.9.9"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(db.lock().unwrap().setting(KEY_CHECKED_AT).as_deref(), Some(&(1_000 + INTERVAL_SECS).to_string()[..]));
    }

    #[test]
    fn disabled_never_fetches_even_with_cached_newer() {
        let (url, hits) = serve("200 OK", "9.9.9\n");
        let db = mem_db();
        {
            let g = db.lock().unwrap();
            g.set_setting(KEY_ENABLED, "0").unwrap();
            record(&g, 1, "0.5.0").unwrap();
        }
        let out = check(&db, "0.1.0", &url, 1_000_000).unwrap();
        assert_eq!(out, Outcome { newer: None, error: None }, "止めていれば前回の結果も出さない");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn same_or_older_version_is_not_newer() {
        let (url, _) = serve("200 OK", "0.1.0");
        let db = mem_db();
        assert_eq!(check(&db, "0.1.0", &url, 1_000).unwrap(), Outcome { newer: None, error: None });
        assert_eq!(db.lock().unwrap().setting(KEY_LATEST).as_deref(), Some("0.1.0"), "同じ版でも結果は書く（次の 1 日は読み直さない）");
        let (url, _) = serve("200 OK", "0.0.1");
        let older = mem_db();
        assert_eq!(check(&older, "0.1.0", &url, 1_000).unwrap(), Outcome { newer: None, error: None });
        assert_eq!(older.lock().unwrap().setting(KEY_LATEST).as_deref(), Some("0.0.1"));
    }

    #[test]
    fn http_error_keeps_cache_and_records_the_failure() {
        let (url, hits) = serve("404 Not Found", "");
        let db = mem_db();
        {
            let g = db.lock().unwrap();
            record(&g, 1, "0.5.0").unwrap();
        }
        let out = check(&db, "0.1.0", &url, 1_000_000).unwrap();
        assert_eq!(out.newer.as_deref(), Some("0.5.0"), "前回の結果で続ける");
        assert_eq!(out.error.as_deref(), Some("HTTP error: HTTP 404"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let g = db.lock().unwrap();
        assert_eq!(g.setting(KEY_CHECKED_AT).as_deref(), Some("1000000"), "失敗でも確認した日時は進める = 起動のたびに GET しない");
        assert_eq!(g.setting(KEY_LATEST).as_deref(), Some("0.5.0"), "前回の版番号はそのまま");
        assert_eq!(g.setting(KEY_ERROR).as_deref(), Some("1000000: HTTP error: HTTP 404"));
        drop(g);
        // 同じ日のうちは取りに行かない（失敗の直後でも）
        let out = check(&db, "0.1.0", &url, 1_000_000 + 60).unwrap();
        assert_eq!(out, Outcome { newer: Some("0.5.0".into()), error: None });
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn garbage_body_is_a_parse_error_and_version_is_not_recorded() {
        let (url, _) = serve("200 OK", "<html>maintenance</html>");
        let db = mem_db();
        let out = check(&db, "0.1.0", &url, 1_000).unwrap();
        assert_eq!(out.newer, None);
        assert!(out.error.as_deref().unwrap_or("").contains("VERSION の形式が不正"), "{out:?}");
        let g = db.lock().unwrap();
        assert_eq!(g.setting(KEY_LATEST), None);
        assert_eq!(g.setting(KEY_CHECKED_AT).as_deref(), Some("1000"));
        assert!(g.setting(KEY_ERROR).unwrap().starts_with("1000: parse error: VERSION の形式が不正"));
    }

    #[test]
    fn oversize_body_is_rejected_even_if_it_starts_with_a_version() {
        // 版番号の後ろに HTML が続く 65 バイト超の本文（保守ページ・プロキシ）を版と誤認しない
        let body: &'static str = Box::leak(format!("0.9.0{}<html>maintenance</html>", " ".repeat(60)).into_boxed_str());
        let (url, _) = serve("200 OK", body);
        let db = mem_db();
        let out = check(&db, "0.1.0", &url, 1_000).unwrap();
        assert_eq!(out.newer, None);
        assert!(out.error.as_deref().unwrap_or("").contains("64 バイトを超える"), "{out:?}");
        assert_eq!(db.lock().unwrap().setting(KEY_LATEST), None);
    }

    #[test]
    fn unreachable_host_is_reported_not_fatal() {
        // 誰も聴いていないポート（bind して即閉じる）
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let db = mem_db();
        let out = check(&db, "0.1.0", &format!("http://127.0.0.1:{port}/VERSION"), 1_000).unwrap();
        assert_eq!(out.newer, None);
        assert!(out.error.is_some());
    }
}
