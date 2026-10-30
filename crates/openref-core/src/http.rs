//! 外部 HTTP の定型: 経路ごとの agent・UA・応答本文の文字列化・429 の再試行。
//!
//! agent は経路ごとに 1 つを**プロセスで共有**する（OnceLock）が、**接続は使い回さない**（`max_idle_connections(0)` = 要求ごとに
//! 新しい TCP / TLS）。ureq 2 の接続プールには idle の期限が無く、再利用の前の生存確認は 1 回の peek だけ = 常駐プロセス
//! （connectd・app・MCP）でスリープやネットワークの切替の後に死んだ接続を掴むと、POST は送り直されず GET も RST で失敗しうる
//! （書誌の解決では esearch の失敗が doi.org の書誌への黙った格下げになる）。ureq の `cookies` feature は有効でない
//! （Cargo.lock の ureq の依存に cookie_store が無い）ので、共有しても要求ごとに送るヘッダは変わらない。
//!
//! 送り先ごとの門（E-utilities の `EUTILS_THROTTLE`・Crossref の `CROSSREF_GATE`）は resolver に置いたまま。ここは門を知らず、
//! 呼び手が渡す `pace` を各送信の直前に呼ぶだけ。新版の確認（update.rs）はここを使わない（版を名乗る UA・接続 5 秒・
//! URL ごとの https_only = 別の agent）。

use crate::{CoreError, Result};
use std::sync::OnceLock;
use std::time::Duration;

/// 書誌（resolver = E-utilities・doi.org・api.crossref.org の検索・医書.jp）と引用スタイル（citestyle）の UA。
/// 版を出さない（update.rs が「固定の `openref/0.1 (…)` = 版が分からない」を前提に書いている）
const UA: &str = "openref/0.1 (personal reference manager)";
/// PDF 取得（pdffetch = PMC の S3・Unpaywall・Crossref link[]・出版社ページ）の UA。連絡先 URL つき
const UA_FETCH: &str = "openref/0.1 (personal reference manager; +https://openref.unitedradiology.jp)";

/// 書誌・引用スタイルの agent（timeout 30 s・リダイレクトは ureq 既定の追従）
pub(crate) fn agent() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| no_pool().timeout(Duration::from_secs(30)).user_agent(UA).build())
}

/// PDF 取得の agent（timeout 60 s・ureq 既定の追従）。URL を自前で組む固定ホスト（PMC の S3・Unpaywall・api.crossref.org）用
pub(crate) fn fetch_agent() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| no_pool().timeout(Duration::from_secs(60)).user_agent(UA_FETCH).build())
}

/// PDF 取得の agent・リダイレクトを追わない（timeout 60 s）。外部 URL 用 = pdffetch の `http_get` が各ホップを SSRF 検査して手で追う
pub(crate) fn fetch_agent_no_redirect() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| {
        no_pool()
            .timeout(Duration::from_secs(60))
            .redirects(0)
            .user_agent(UA_FETCH)
            .build()
    })
}

/// 接続を使い回さない builder（モジュール冒頭の理由）
fn no_pool() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new().max_idle_connections(0).max_idle_connections_per_host(0)
}

/// 送った結果（`call()` / `send_form()` の戻り）を本文の文字列にする。送信の失敗（4xx / 5xx を含む）は `on_err` が文言にし、
/// 本文の読み出しの失敗はその `to_string()`
pub(crate) fn text(
    sent: std::result::Result<ureq::Response, ureq::Error>,
    on_err: impl FnOnce(ureq::Error) -> String,
) -> std::result::Result<String, String> {
    sent.map_err(on_err)?.into_string().map_err(|e| e.to_string())
}

/// 429（E-utilities は 3 req/s 超・Crossref も混雑時）なら 1s, 2s 待って最大 2 回再試行。成功したら本文を文字列で返す。
/// PDF 一括取込では 1 件あたり esearch+efetch の 2 連打になり、素のままでは実測で 429 が出た（今は EUTILS_THROTTLE が要求ごとに空ける）。
/// `pace` = 各送信（再試行を含む）の直前に呼ぶ待ち（E-utilities は `EUTILS_THROTTLE`・Crossref は `CROSSREF_GATE` が渡す）。
pub(crate) fn call_with_retry_paced(req: ureq::Request, what: &str, pace: impl Fn()) -> Result<String> {
    let mut attempt: u64 = 0;
    loop {
        pace();
        match req.clone().call() {
            Ok(r) => return r.into_string().map_err(|e| CoreError::Http(e.to_string())),
            Err(ureq::Error::Status(429, _)) if attempt < 2 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(1000 * attempt));
            }
            Err(e) => return Err(CoreError::Http(format!("{what}: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 127.0.0.1 の使い捨てサーバ。`responses` を順に 1 つずつ返し、受けた要求（頭 + 本文）のバイト列と、受けた接続の数を返す
    fn serve(responses: Vec<&'static str>) -> (String, std::thread::JoinHandle<(Vec<String>, usize)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let mut reqs = Vec::new();
            let mut conns = 0;
            let mut left = responses.into_iter().peekable();
            while left.peek().is_some() {
                let (stream, _) = listener.accept().unwrap();
                conns += 1;
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut w = stream;
                // 1 本の接続で続けて来る要求（keep-alive）も受ける。相手が閉じたら次の接続へ
                while let Some(resp) = left.peek() {
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).unwrap() == 0 {
                            break;
                        }
                        head.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    if head.is_empty() {
                        break;
                    }
                    let len = head
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().to_string()))
                        .map(|v| v.parse::<usize>().unwrap())
                        .unwrap_or(0);
                    let mut body = vec![0; len];
                    r.read_exact(&mut body).unwrap();
                    reqs.push(head + &String::from_utf8(body).unwrap());
                    w.write_all(resp.as_bytes()).unwrap();
                    w.flush().unwrap();
                    let close = resp.contains("Connection: close");
                    left.next();
                    if close {
                        break;
                    }
                }
            }
            (reqs, conns)
        });
        (base, h)
    }

    const OK_CLOSE: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

    /// 以前の各モジュールが要求ごとに組んでいた agent（pubmed.rs・citestyle.rs・pdffetch.rs の写し）と、共有の agent が
    /// 同じ要求のバイト列（要求行・Host・UA・Accept・Accept-Encoding・本文）を送ること
    #[test]
    fn shared_agents_send_the_same_bytes_as_the_old_per_request_builders() {
        let old_resolver = || {
            ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("openref/0.1 (personal reference manager)")
                .build()
        };
        let old_fetch = || {
            ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(60))
                .user_agent("openref/0.1 (personal reference manager; +https://openref.unitedradiology.jp)")
                .build()
        };
        let old_no_redirect = || {
            ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(60))
                .redirects(0)
                .user_agent("openref/0.1 (personal reference manager; +https://openref.unitedradiology.jp)")
                .build()
        };
        let (base, h) = serve(vec![OK_CLOSE; 8]);
        let get = |a: &ureq::Agent| {
            a.get(&format!("{base}/works"))
                .query("q", "a b")
                .set("Accept", "application/vnd.citationstyles.csl+json")
                .call()
                .unwrap()
                .into_string()
                .unwrap()
        };
        let post = |a: &ureq::Agent| {
            a.post(&format!("{base}/bib/download"))
                .send_form(&[("doi", "10.18885/CI.0000000000"), ("format", "0")])
                .unwrap()
                .into_string()
                .unwrap()
        };
        get(&old_resolver());
        get(agent());
        post(&old_resolver());
        post(agent());
        get(&old_fetch());
        get(fetch_agent());
        get(&old_no_redirect());
        get(fetch_agent_no_redirect());
        let (reqs, _) = h.join().unwrap();
        assert_eq!(reqs.len(), 8);
        for pair in reqs.chunks(2) {
            assert_eq!(pair[0], pair[1], "旧の agent と共有の agent で要求が違う");
        }
        assert!(reqs[0].contains("\r\nUser-Agent: openref/0.1 (personal reference manager)\r\n"), "{}", reqs[0]);
        assert!(reqs[2].ends_with("\r\n\r\ndoi=10.18885%2FCI.0000000000&format=0"), "{}", reqs[2]);
        for r in &reqs[4..] {
            assert!(
                r.contains("\r\nUser-Agent: openref/0.1 (personal reference manager; +https://openref.unitedradiology.jp)\r\n"),
                "{r}"
            );
        }
    }

    /// リダイレクトの扱いは経路ごとに旧と同じ: no_redirect は 3xx をそのまま返し（pdffetch が Location を検査して手で追う）・他は追う
    #[test]
    fn only_the_no_redirect_agent_stops_at_3xx() {
        let (base, h) = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            OK_CLOSE,
            "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            OK_CLOSE,
        ]);
        let r = fetch_agent_no_redirect().get(&format!("{base}/start")).call().unwrap();
        assert_eq!((r.status(), r.header("Location")), (302, Some("/next")));
        assert_eq!(fetch_agent().get(&format!("{base}/start")).call().unwrap().into_string().unwrap(), "ok");
        assert_eq!(agent().get(&format!("{base}/start")).call().unwrap().into_string().unwrap(), "ok");
        let (reqs, _) = h.join().unwrap();
        let lines: Vec<&str> = reqs.iter().map(|r| r.lines().next().unwrap()).collect();
        assert_eq!(
            lines,
            ["GET /start HTTP/1.1", "GET /start HTTP/1.1", "GET /next HTTP/1.1", "GET /start HTTP/1.1", "GET /next HTTP/1.1"]
        );
    }

    /// 共有の agent でも接続は使い回さない（旧の要求ごとの agent と同じ = 要求ごとに新しい接続）。サーバが keep-alive を許しても
    #[test]
    fn shared_agents_do_not_reuse_connections() {
        for a in [agent(), fetch_agent(), fetch_agent_no_redirect()] {
            let (base, h) = serve(vec![
                "HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\na",
                "HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nb",
            ]);
            let x = text(a.get(&format!("{base}/1")).call(), |e| e.to_string()).unwrap();
            let y = text(a.get(&format!("{base}/2")).call(), |e| e.to_string()).unwrap();
            assert_eq!((x.as_str(), y.as_str()), ("a", "b"));
            assert_eq!(h.join().unwrap().1, 2, "2 要求が 2 本の接続に分かれる");
        }
    }

    /// `text` の文言: 送信の失敗は `on_err`（Status も渡る）
    #[test]
    fn text_maps_status_through_on_err() {
        let (base, h) = serve(vec!["HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"]);
        let e = text(agent().get(&format!("{base}/x")).call(), |e| match e {
            ureq::Error::Status(code, _) => format!("HTTP {code}"),
            e => e.to_string(),
        });
        assert_eq!(e, Err("HTTP 404".to_string()));
        h.join().unwrap();
    }

    /// 再試行: 429 は待って送り直し・`pace` は送信（再試行を含む）ごとに 1 回・429 以外の失敗は `what:` を前に付けて即返す
    #[test]
    fn retry_paces_every_send() {
        let (base, h) = serve(vec![
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            OK_CLOSE,
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ]);
        let paced = AtomicUsize::new(0);
        let pace = || {
            paced.fetch_add(1, Ordering::SeqCst);
        };
        let body = call_with_retry_paced(agent().get(&format!("{base}/a")), "esearch", pace).unwrap();
        assert_eq!((body.as_str(), paced.load(Ordering::SeqCst)), ("ok", 2));
        let e = call_with_retry_paced(agent().get(&format!("{base}/b")), "efetch", pace).unwrap_err();
        assert_eq!(paced.load(Ordering::SeqCst), 3);
        assert!(matches!(&e, CoreError::Http(m) if m.starts_with("efetch: ")), "{e}");
        assert_eq!(h.join().unwrap().0.len(), 3);
    }
}
