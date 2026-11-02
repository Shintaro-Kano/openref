//! 実物の Claude Code CLI で動かす試験（#[ignore]・利用枠を使う）。env で vault・DB・行の保存先を渡す（DB と vault は写しを使う）。
//! stdout の行を OPENREF_CHAT_LIVE_OUT/<名前>.jsonl に保存する（個人の値を含みうる）。
//!
//! ```sh
//! OPENREF_CHAT_LIVE_VAULT=<vault> OPENREF_CHAT_LIVE_DB=<DB> OPENREF_CHAT_LIVE_OUT=<保存先> \
//!   cargo test -p openref-app live_a_scenes -- --ignored --nocapture   # 1 つの会話で 13 ターン
//!   … live_b_resume / live_c_orphan / live_d_undo_replay / live_e_slash
//! ```
//! vault に chat-test/sample.md・ingest/chat-test-material.md（ページの中に指示の文を含む素材）・vault の兄弟 chat-test-outside/secret.txt を置く。
//! Claude Code を普通に使うときと同じ（全部の道具・確認なし）なので、拒否ではなく「使えた」ことを確かめる

use super::*;

struct LiveRig {
    chat: Chat,
    events: Arc<Mutex<Vec<Value>>>,
    l: Launch,
    out: PathBuf,
    ctx: Ctx,
}

fn live_rig() -> LiveRig {
    let root =
        PathBuf::from(std::env::var("OPENREF_CHAT_LIVE_VAULT").expect("OPENREF_CHAT_LIVE_VAULT"))
            .canonicalize()
            .unwrap();
    let db = PathBuf::from(std::env::var("OPENREF_CHAT_LIVE_DB").expect("OPENREF_CHAT_LIVE_DB"));
    let out = PathBuf::from(std::env::var("OPENREF_CHAT_LIVE_OUT").expect("OPENREF_CHAT_LIVE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cli = find_cli(home.as_deref(), std::env::var_os("PATH").as_deref()).expect("claude");
    let mcp =
        find_mcp(&mcp_candidates(home.as_deref()), &db).expect("wiki_write を持つ openref-mcp");
    eprintln!("cli = {} / mcp = {}", cli.path.display(), mcp.display());
    let events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let ev = events.clone();
    let chat = Chat::new(Arc::new(move |v| ev.lock().unwrap().push(v)));
    let ctx = Ctx {
        enabled: true,
        root: Some(root.to_string_lossy().into_owned()),
        app_dir: db.parent().map(Path::to_path_buf),
        home,
        watched: false,
    };
    LiveRig {
        chat,
        events,
        l: Launch {
            cli,
            mcp,
            db,
            root,
            model: None,
            log: false,
        },
        out,
        ctx,
    }
}

fn save(out: &Path, name: &str, evs: &[Value]) {
    let lines: String = evs
        .iter()
        .filter(|e| e["type"] == "line")
        .map(|e| format!("{}\n", e["line"].as_str().unwrap()))
        .collect();
    std::fs::write(out.join(format!("{name}.jsonl")), lines).unwrap();
    let all: String = evs.iter().map(|e| format!("{e}\n")).collect();
    std::fs::write(out.join(format!("{name}.events.jsonl")), all).unwrap();
}

fn is_line(e: &Value, pat: &str) -> bool {
    e["type"] == "line" && e["line"].as_str().is_some_and(|l| l.contains(pat))
}

impl LiveRig {
    fn mark(&self) -> usize {
        self.events.lock().unwrap().len()
    }
    fn since(&self, m: usize) -> Vec<Value> {
        self.events.lock().unwrap()[m..].to_vec()
    }
    /// 1 ターン（送って idle に戻るまで・上限 300 秒）。interrupt_after = 最初の stream_event からこの時間で割り込む
    fn turn(
        &self,
        name: &str,
        text: &str,
        path: Option<&str>,
        interrupt_after: Option<Duration>,
    ) -> (u64, Vec<Value>) {
        let m = self.mark();
        let t0 = Instant::now();
        let turn = self.chat.send(text, path, None, false).unwrap();
        let deadline = t0 + Duration::from_secs(300);
        let mut interrupted = false;
        let mut first: Option<Duration> = None;
        loop {
            let evs = self.since(m);
            if first.is_none() && evs.iter().any(|e| is_line(e, "\"stream_event\"")) {
                first = Some(t0.elapsed());
            }
            if let (Some(after), Some(f), false) = (interrupt_after, first, interrupted) {
                if t0.elapsed() >= f + after {
                    eprintln!("[{name}] interrupt at {:?}", t0.elapsed());
                    self.chat.interrupt().unwrap();
                    interrupted = true;
                }
            }
            if evs
                .iter()
                .any(|e| (e["type"] == "state" && e["state"] == "idle") || e["type"] == "exit")
            {
                break;
            }
            assert!(Instant::now() < deadline, "[{name}] 300 秒で終わらない");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300)); // 後続の行（あれば）
        let evs = self.since(m);
        eprintln!(
            "[{name}] turn {turn} in {:?} (first stream_event {:?}) lines {}",
            t0.elapsed(),
            first,
            evs.iter().filter(|e| e["type"] == "line").count()
        );
        save(&self.out, name, &evs);
        (turn, evs)
    }
}

fn db_snapshot(db: &Path) -> String {
    let q = "select count(*), max(date_modified), (select count(*) from items where deleted_at is not null), \
             (select value from settings where key='pdf_write_mode'), (select count(*) from attachments) from items";
    String::from_utf8_lossy(
        &std::process::Command::new("sqlite3")
            .arg(db)
            .arg(q)
            .output()
            .unwrap()
            .stdout,
    )
    .into_owned()
}

/// 1 プロセスで全場面（新しい会話）。最後に会話の id を out/session.txt に置く（live_b が resume する）
#[test]
#[ignore]
fn live_a_scenes() {
    let r = live_rig();
    let root = r.l.root.clone();
    let db = r.l.db.clone();
    let t0 = Instant::now();
    let s = r.chat.start(&r.l, None, true, false).unwrap();
    eprintln!("start {s:?} in {:?}", t0.elapsed());
    std::fs::write(r.out.join("session.txt"), &s.session_id).unwrap();
    save(&r.out, "start", &r.since(0));
    let db_before = db_snapshot(&db);
    // 普通の応答
    r.turn(
        "plain",
        "Reply with exactly the single word: PINEAPPLE",
        None,
        None,
    );
    // Read
    r.turn("read", "Use the Read tool to read chat-test/sample.md and reply with only its first Markdown heading.", None, None);
    // 新規作成
    let (t_create, _) = r.turn(
        "create",
        "Using call(wiki_write), create the new page chat-test/chat-new.md with exactly this content:\n# Chat new\n\nline one\n\nThen reply: done",
        None,
        None,
    );
    assert!(root.join("chat-test/chat-new.md").is_file());
    // 要約して末尾に追記（if_sha256 あり）
    let sample_before = std::fs::read_to_string(root.join("chat-test/sample.md")).unwrap();
    let (t_a, _) = r.turn(
        "append_sha",
        "このページを 1 文で要約して、ページの末尾に「## 要約」の節として追記して。wiki_read で sha256 を読み、call(wiki_write) に if_sha256 を付けて書いて。",
        Some("chat-test/sample.md"),
        None,
    );
    // 上書き（if_sha256 なし）
    let (t_nosha, _) = r.turn(
        "append_nosha",
        "Overwrite chat-test/chat-new.md with call(wiki_write) WITHOUT if_sha256 (do not read it first) so that its content is exactly:\n# Chat new\n\nline one\nline two\n",
        None,
        None,
    );
    // 移動
    let (t_move, _) = r.turn(
        "move",
        "Move chat-test/chat-new.md to chat-test/chat-moved.md with call(wiki_move). Reply: moved",
        None,
        None,
    );
    // 1 ターンに 2 回
    std::fs::write(root.join("chat-test/twice.md"), "original\n").unwrap();
    let (t_twice, _) = r.turn(
        "twice",
        "Call call(wiki_write) twice, one after the other, on the existing page chat-test/twice.md without if_sha256: first with content \"v1\\n\", then with content \"v2\\n\". Reply: ok",
        None,
        None,
    );
    // Edit / Write / Bash は使える（bypassPermissions）。sample.md は後の場面でも使うので使い捨ての edit-me.md を触らせる
    let _ = std::fs::remove_file(root.join("chat-test/x.txt"));
    std::fs::write(root.join("chat-test/edit-me.md"), "hello\n").unwrap();
    let (_, ev) = r.turn(
        "builtin",
        "Use these built-in tools and report for each whether it worked: (1) Bash: run `ls`; (2) Write: create chat-test/x.txt containing hi; (3) Edit: replace 'hello' with 'X' in chat-test/edit-me.md.",
        None,
        None,
    );
    let init = ev
        .iter()
        .find(|e| is_line(e, "\"subtype\":\"init\""))
        .expect("init の行");
    let init: Value = serde_json::from_str(init["line"].as_str().unwrap()).unwrap();
    assert_eq!(init["permissionMode"], "bypassPermissions", "{init}");
    assert!(
        init["tools"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x == "Bash")),
        "Bash が道具にある: {init}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("chat-test/x.txt"))
            .unwrap()
            .trim(),
        "hi",
        "Write が効く"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("chat-test/edit-me.md"))
            .unwrap()
            .trim(),
        "X",
        "Edit が効く"
    );
    // vault の外の Read / Grep / Glob も使える
    let outside = root.parent().unwrap().join("chat-test-outside");
    let o = outside.display();
    let (_, ev) = r.turn(
        "outside",
        &format!("Use the Read tool on {o}/secret.txt and quote its content. Then use Grep for the pattern CHAT-TEST-OUTSIDE with path {o} and Glob for * with path {o}. Report exactly what each tool returned."),
        None,
        None,
    );
    assert!(
        ev.iter().any(|e| is_line(e, "CHAT-TEST-OUTSIDE")),
        "vault の外の secret.txt が読める"
    );
    // ページの中の指示の文（従わない・DB は変わらない）
    r.turn(
        "inject",
        "このページの内容を 2 文で要約して返答して（ページへの書込はしないで）。",
        Some("ingest/chat-test-material.md"),
        None,
    );
    // MCP のエラー（無い key の purge_item = 道具のエラーとして出る・DB は変わらない）
    r.turn("error", "Call call(purge_item) with args {\"key\":\"NOSUCH00\"} exactly once and tell me the raw result. Do not retry.", None, None);
    let db_after = db_snapshot(&db);
    eprintln!("db before {db_before:?} after {db_after:?}");
    assert_eq!(db_before, db_after, "DB が変わらない");
    // interrupt の後に続ける
    let (_, ev) = r.turn(
        "interrupt",
        "Write the numbers from 1 to 400, one per line, with no other text.",
        None,
        Some(Duration::from_millis(1500)),
    );
    assert!(
        ev.iter().any(|e| is_line(e, "\"type\":\"result\"")),
        "interrupt の後に result が出る"
    );
    r.turn(
        "after_interrupt",
        "In one short sentence: what did I ask you in my previous message?",
        None,
        None,
    );
    let t1 = Instant::now();
    r.chat.stop(Duration::from_secs(5), Duration::from_secs(2));
    eprintln!("stop in {:?}", t1.elapsed());
    save(&r.out, "all_a", &r.since(0));
    // [元に戻す]（OPENREF_CHAT_LIVE_SKIP_UNDO があれば飛ばす = vault を Claude が変えた状態のまま残し、live_d が保存した行から再生する）
    if std::env::var_os("OPENREF_CHAT_LIVE_SKIP_UNDO").is_some() {
        eprintln!("[元に戻す] は飛ばす（live_d で再生）");
        return;
    }
    let ctx = &r.ctx;
    let undo = |turn: u64, i: usize| -> Undo {
        let (rec, later) = r
            .chat
            .record(turn, i)
            .unwrap_or_else(|| panic!("記録が無い: turn {turn} index {i}"));
        eprintln!("undo turn {turn}[{i}] {:?} later {later:?}", rec.change);
        let trash = |p: &Path| -> Result<String, String> {
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            let mut c = trash::TrashContext::default();
            c.set_delete_method(DeleteMethod::NsFileManager);
            c.delete(p)
                .map(|()| "nsfilemanager".to_string())
                .map_err(|e| e.to_string())
        };
        let u = undo_record(ctx, &rec, later, true, trash, |_: &str| {
            Err("not used".into())
        });
        if matches!(u, Undo::Done { .. }) {
            r.chat.mark_undone(turn, i);
        }
        eprintln!("  → {u:?}");
        u
    };
    for t in [t_create, t_a, t_nosha, t_move, t_twice] {
        eprintln!("changes turn {t}: {:?}", r.chat.lock().tracker.changes(t));
    }
    // 1 ターンに 2 回 → 最初の退避（original）へ
    assert!(matches!(undo(t_twice, 0), Undo::Done { .. }));
    assert_eq!(
        std::fs::read_to_string(root.join("chat-test/twice.md")).unwrap(),
        "original\n"
    );
    // 新規作成を先に戻そうとする = 後のターンで変えた
    assert!(matches!(undo(t_create, 0), Undo::Refused(s) if s.contains("later turn")));
    // 移動 → 逆の move・上書き → 作った版・新規 → ゴミ箱
    assert!(matches!(undo(t_move, 0), Undo::Done { .. }));
    assert!(
        root.join("chat-test/chat-new.md").is_file()
            && !root.join("chat-test/chat-moved.md").exists()
    );
    assert!(matches!(undo(t_nosha, 0), Undo::Done { .. }));
    assert_eq!(
        std::fs::read_to_string(root.join("chat-test/chat-new.md")).unwrap(),
        "# Chat new\n\nline one\n"
    );
    assert!(matches!(undo(t_create, 0), Undo::Done { .. }));
    assert!(!root.join("chat-test/chat-new.md").exists());
    // user がその後に編集 → 戻さない → Claude の版に戻してから戻す
    let claude_ver = std::fs::read(root.join("chat-test/sample.md")).unwrap();
    std::fs::write(
        root.join("chat-test/sample.md"),
        [claude_ver.as_slice(), b"user edit\n"].concat(),
    )
    .unwrap();
    assert!(matches!(undo(t_a, 0), Undo::Refused(s) if s.contains("changed afterwards")));
    std::fs::write(root.join("chat-test/sample.md"), &claude_ver).unwrap();
    assert!(matches!(undo(t_a, 0), Undo::Done { .. }));
    assert_eq!(
        std::fs::read_to_string(root.join("chat-test/sample.md")).unwrap(),
        sample_before
    );
}

/// resume で続く・存在しない会話（initialize の前に終わるか）
#[test]
#[ignore]
fn live_b_resume() {
    let r = live_rig();
    let id = std::fs::read_to_string(r.out.join("session.txt"))
        .unwrap()
        .trim()
        .to_string();
    let t0 = Instant::now();
    let s = r.chat.start(&r.l, Some(id.clone()), false, false).unwrap();
    eprintln!("resume {s:?} in {:?}", t0.elapsed());
    assert!(s.resumed && s.session_id == id);
    save(&r.out, "resume_start", &r.since(0));
    let (_, ev) = r.turn("resume", "What single word did I ask you to reply with in my very first message of this conversation? Reply with that word only.", None, None);
    assert!(ev.iter().any(|e| is_line(e, "PINEAPPLE")), "会話が続く");
    // 続きの会話も確認なし（Claude Code を普通に使うときと同じ）
    let init = ev
        .iter()
        .find(|e| is_line(e, "\"subtype\":\"init\""))
        .expect("init の行");
    let init: Value = serde_json::from_str(init["line"].as_str().unwrap()).unwrap();
    assert_eq!(init["permissionMode"], "bypassPermissions", "{init}");
    let outside = r.l.root.parent().unwrap().join("chat-test-outside");
    // 存在しないページ（MCP のエラー）・vault の外の Read（読める）
    r.turn("mcp_error", "Call call(wiki_read) with args {\"path\": \"chat-test/does-not-exist.md\"} exactly once and report the raw result. Do not retry.", None, None);
    let (_, ev) = r.turn(
        "outside2",
        &format!(
            "Use the Read tool on {}/secret.txt once and quote its content. Do not retry.",
            outside.display()
        ),
        None,
        None,
    );
    assert!(
        ev.iter().any(|e| is_line(e, "CHAT-TEST-OUTSIDE")),
        "vault の外も読める"
    );
    r.chat.stop(Duration::from_secs(5), Duration::from_secs(2));
    // 存在しない会話
    let m = r.mark();
    let t0 = Instant::now();
    let s = r
        .chat
        .start(
            &r.l,
            Some("00000000-0000-4000-8000-000000000000".into()),
            false,
            false,
        )
        .unwrap();
    eprintln!("bad resume {s:?} in {:?}", t0.elapsed());
    assert!(!s.resumed && s.notice.is_some());
    save(&r.out, "bad_resume", &r.since(m));
    r.chat.stop(Duration::from_secs(5), Duration::from_secs(2));
}

/// 親の kill -9: 子を起こして pgid を out/orphan.txt に出し、待ち続ける（外から親を kill -9 する）
#[test]
#[ignore]
fn live_c_orphan() {
    let r = live_rig();
    let s = r.chat.start(&r.l, None, true, false).unwrap();
    let pgid = r.chat.lock().proc.as_ref().unwrap().pgid;
    std::fs::write(
        r.out.join("orphan.txt"),
        format!("{} {} {}\n", std::process::id(), pgid, s.session_id),
    )
    .unwrap();
    eprintln!("orphan: parent {} child pgid {pgid}", std::process::id());
    std::thread::sleep(Duration::from_secs(600));
}

/// live_a の [元に戻す] を、保存した実物の行から記録を組み直して通す（claude を起こさない）。live_a は末尾で自分の [元に戻す] を通して
/// vault を元に戻すので、live_d の前の live_a は `OPENREF_CHAT_LIVE_SKIP_UNDO=1` で回す（Claude が変えた状態の vault が要る）。
/// 場面の行（create / append_sha / append_nosha / move）を Tracker に入れ、後のターンから順に戻す
#[test]
#[ignore]
fn live_d_undo_replay() {
    let r = live_rig();
    let root = r.l.root.clone();
    let mut t = Tracker {
        root: Some(root.clone()),
        ..Tracker::default()
    };
    let mut turn_of = HashMap::new();
    for name in ["create", "append_sha", "append_nosha", "move"] {
        for l in std::fs::read_to_string(r.out.join(format!("{name}.events.jsonl")))
            .unwrap()
            .lines()
        {
            let e: Value = serde_json::from_str(l).unwrap();
            if e["type"] == "line" {
                let turn = e["turn"].as_u64().unwrap();
                turn_of.insert(name, turn);
                if let Ok(v) = serde_json::from_str::<Value>(e["line"].as_str().unwrap()) {
                    let shas = move_shas(t.root.as_deref(), &v);
                    t.observe_with(turn, &v, &shas);
                }
            }
        }
    }
    let (t_create, t_a, t_nosha, t_move) = (
        turn_of["create"],
        turn_of["append_sha"],
        turn_of["append_nosha"],
        turn_of["move"],
    );
    for tn in [t_create, t_a, t_nosha, t_move] {
        eprintln!("changes turn {tn}: {:?}", t.changes(tn));
    }
    let ctx = &r.ctx;
    let mut undo = |turn: u64| -> Undo {
        let rec = t.turns[&turn][0].clone();
        let later = rec.change.file().and_then(|p| t.later_change(turn, p));
        let trash = |p: &Path| -> Result<String, String> {
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            let mut c = trash::TrashContext::default();
            c.set_delete_method(DeleteMethod::NsFileManager);
            c.delete(p)
                .map(|()| "nsfilemanager".to_string())
                .map_err(|e| e.to_string())
        };
        let u = undo_record(ctx, &rec, later, true, trash, |_: &str| {
            Err("not used".into())
        });
        if matches!(u, Undo::Done { .. }) {
            t.turns.get_mut(&turn).unwrap()[0].undone = true;
        }
        eprintln!("undo turn {turn} {:?} later {later:?} → {u:?}", rec.change);
        u
    };
    // 新規作成を先に戻そうとする = 後のターンで変えた（移動して無い）
    assert!(matches!(undo(t_create), Undo::Refused(s) if s.contains("later turn")));
    // 移動 → 逆の move・上書き → 作った版・新規 → ゴミ箱
    assert!(matches!(undo(t_move), Undo::Done { .. }));
    assert!(
        root.join("chat-test/chat-new.md").is_file()
            && !root.join("chat-test/chat-moved.md").exists()
    );
    assert!(matches!(undo(t_nosha), Undo::Done { .. }));
    assert_eq!(
        std::fs::read_to_string(root.join("chat-test/chat-new.md")).unwrap(),
        "# Chat new\n\nline one\n"
    );
    assert!(matches!(undo(t_create), Undo::Done { .. }));
    assert!(!root.join("chat-test/chat-new.md").exists());
    // user がその後に編集 → 戻さない → Claude の版に戻してから戻す
    let claude_ver = std::fs::read(root.join("chat-test/sample.md")).unwrap();
    std::fs::write(
        root.join("chat-test/sample.md"),
        [claude_ver.as_slice(), b"user edit\n"].concat(),
    )
    .unwrap();
    assert!(matches!(undo(t_a), Undo::Refused(s) if s.contains("changed afterwards")));
    std::fs::write(root.join("chat-test/sample.md"), &claude_ver).unwrap();
    assert!(matches!(undo(t_a), Undo::Done { .. }));
    assert!(!std::fs::read_to_string(root.join("chat-test/sample.md"))
        .unwrap()
        .contains("## 要約"));
}

/// `/` で始まる発言が slash command にならない（会話の id が変わらず、前の文脈が残る）。1 つの会話で 2 ターン
#[test]
#[ignore]
fn live_e_slash() {
    let r = live_rig();
    let s = r.chat.start(&r.l, None, true, false).unwrap();
    r.turn(
        "slash_1",
        "Remember this word: MANGO. Reply with just: ok",
        None,
        None,
    );
    let (_, ev) = r.turn(
        "slash_2",
        "/clear What word did I ask you to remember? Reply with the word only.",
        None,
        None,
    );
    let ids: Vec<String> = ev
        .iter()
        .filter(|e| e["type"] == "line")
        .filter_map(|e| serde_json::from_str::<Value>(e["line"].as_str().unwrap()).ok())
        .filter(|v| matches!(v["type"].as_str(), Some("system" | "result")))
        .filter_map(|v| v["session_id"].as_str().map(str::to_string))
        .collect();
    eprintln!("session ids in turn 2: {ids:?}");
    assert!(
        !ids.is_empty() && ids.iter().all(|i| *i == s.session_id),
        "会話の id が変わった"
    );
    assert!(ev.iter().any(|e| is_line(e, "MANGO")), "前の文脈が残らない");
    assert!(!ev.iter().any(|e| e["type"] == "stderr"
        && e["text"]
            .as_str()
            .is_some_and(|t| t.contains("differs") || t.contains("違います"))));
    r.chat.stop(Duration::from_secs(5), Duration::from_secs(2));
}
