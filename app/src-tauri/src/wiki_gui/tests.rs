use super::*;
use crate::wiki_asset::*;

/// wiki_chat_status の force: {force: true} だけが true（無い・false・文字列・null・args が object でない = 自動の読み直し）
#[test]
fn wiki_chat_force_arg() {
    use serde_json::json;
    assert!(wiki_chat_force(Some(json!({"force": true}))));
    for a in [
        None,
        Some(json!({})),
        Some(json!({"force": false})),
        Some(json!({"force": "true"})),
        Some(json!({"force": null})),
        Some(json!([true])),
    ] {
        assert!(!wiki_chat_force(a.clone()), "{a:?}");
    }
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("openref-refdir-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// [追加…] / [外す]: 現在の root の生の一覧に足す / から外す（検査に通らないものも残す）・すでにあれば書かない・他の root の一覧に触らない
#[test]
fn ref_dirs_for_add_rules() {
    use openref_core::wiki::workspaces;
    let notes = PathBuf::from("/notes");
    let gone = PathBuf::from("/Volumes/gone");
    let mut m = workspaces::parse(Some(
        r#"{"/a": {"last_used": 1, "ref_dirs": ["/Volumes/gone"]}, "/b": {"ref_dirs": ["/other"]}}"#,
    ))
    .unwrap();
    // 見えないディスクの参照フォルダは残したまま足す
    let plan = wiki_ref_dirs_for_add(&m, "/a", &notes);
    assert_eq!(
        plan,
        WikiRefAdd {
            dirs: vec![gone.clone(), notes.clone()],
            changed: true,
            kind: WikiDir::Ref,
        }
    );
    wiki_save_ref_dirs(&mut m, "/a", &plan.dirs);
    // すでにある = 書かない
    let again = wiki_ref_dirs_for_add(&m, "/a", &notes);
    assert!(!again.changed);
    assert_eq!(again.dirs, vec![gone.clone(), notes.clone()]);
    // 項目の無い root = 1 件
    let first = wiki_ref_dirs_for_add(&m, "/c", &notes);
    assert_eq!((first.dirs.clone(), first.changed), (vec![notes.clone()], true));
    // 他の root（/b）と last_used は変わらない
    assert_eq!(wiki_ref_dirs_of(&m, Some("/b")), vec![PathBuf::from("/other")]);
    assert_eq!(m["/a"]["last_used"], serde_json::json!(1));
    // [外す]: 外す・無いものは書かない・root 未設定は空
    assert_eq!(wiki_ref_dirs_for_remove(&mut m, Some("/a"), &gone), vec![notes.clone()]);
    assert_eq!(wiki_ref_dirs_of(&m, Some("/a")), vec![notes.clone()]);
    let before = m.clone();
    assert_eq!(wiki_ref_dirs_for_remove(&mut m, Some("/a"), Path::new("/nope")), vec![notes.clone()]);
    assert_eq!(m, before);
    assert!(wiki_ref_dirs_for_remove(&mut m, Some("/none"), &notes).is_empty());
    assert!(!m.contains_key("/none"), "外すだけで項目を作らない");
    assert!(wiki_ref_dirs_for_remove(&mut m, None, &notes).is_empty());
    assert!(wiki_ref_dirs_of(&m, None).is_empty());
}

/// 壊れた設定の退避先の鍵 = `<鍵>.broken-<UTC の日時>`
#[test]
fn broken_key_is_utc_stamp() {
    assert_eq!(wiki_broken_key("wiki_workspaces", 0), "wiki_workspaces.broken-19700101T000000Z");
    assert_eq!(
        wiki_broken_key("shelf_workspaces", 1_793_491_200),
        "shelf_workspaces.broken-20261101T000000Z"
    );
    assert_eq!(
        wiki_broken_key("wiki_workspaces", 1_793_491_200 + 3 * 3600 + 4 * 60 + 5),
        "wiki_workspaces.broken-20261101T030405Z"
    );
}

/// 設定を読む（テスト）
fn setting(db: &openref_core::Db, k: &str) -> Option<String> {
    db.setting(k).unwrap()
}

/// wiki_ws_tx: 2.0.0 より前の旧キー wiki_ref_dirs の行が DB に残っていても読まない・消さない・退避しない・エラーにしない。
/// 今の形（wiki_workspaces の ref_dirs）だけが参照フォルダ
#[test]
fn ws_tx_leaves_the_legacy_ref_dirs_row_alone() {
    use openref_core::wiki::workspaces;
    let db = openref_core::Db::open_in_memory().unwrap();
    db.set_setting("wiki_root", "/v/a").unwrap();
    for legacy in [r#"["/notes", "/Volumes/gone"]"#, "not json"] {
        db.set_setting("wiki_ref_dirs", legacy).unwrap();
        let (dirs, note) = wiki_ws_tx(&db, 1_793_491_200, false, None, |m, root| Ok(wiki_ref_dirs_of(m, root)));
        assert_eq!(dirs.unwrap(), Vec::<PathBuf>::new(), "{legacy}: 取り込まない");
        assert_eq!(note, WikiWsNote::default(), "{legacy}: 退避しない");
        assert_eq!(setting(&db, "wiki_ref_dirs").as_deref(), Some(legacy), "消さない");
        assert_eq!(setting(&db, "wiki_ref_dirs.broken-20261101T000000Z"), None);
        assert_eq!(setting(&db, workspaces::KEY), None, "wiki_workspaces を書かない");
    }
    // 今の形の参照フォルダはそのまま読める
    db.set_setting(workspaces::KEY, r#"{"/v/a": {"ref_dirs": ["/notes2"]}}"#).unwrap();
    let (dirs, _) = wiki_ws_tx(&db, 0, false, None, |m, root| Ok(wiki_ref_dirs_of(m, root)));
    assert_eq!(dirs.unwrap(), vec![PathBuf::from("/notes2")]);
    assert_eq!(setting(&db, "wiki_ref_dirs").as_deref(), Some("not json"));
}

/// 同じ DB を開いた別の接続（2 重に起動した GUI = 別のプロセス）の書込が、こちらが読んでから書くまでの間に入っても失わない。
/// BEGIN IMMEDIATE = 後から来た方は先の読み書きが終わるまで待ち（busy_timeout）、終わった値を読み直して足す
#[test]
fn ws_tx_does_not_lose_a_write_from_another_connection() {
    use openref_core::wiki::workspaces;
    use std::sync::mpsc;
    let d = scratch("ws-race");
    let path = d.join("openref.sqlite");
    let a = openref_core::Db::open(&path).unwrap();
    let b = openref_core::Db::open(&path).unwrap();
    let (read_tx, read_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let ta = std::thread::spawn(move || {
        wiki_ws_tx(&a, 0, false, None, |m, _| {
            read_tx.send(()).unwrap(); // 読んだ（書く前）
            go_rx.recv().unwrap();
            workspaces::touch(m, "/v/a", 1, workspaces::MAX);
            Ok(())
        })
        .0
    });
    read_rx.recv().unwrap();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let tb = std::thread::spawn(move || {
        let r = wiki_ws_tx(&b, 0, false, None, |m, _| {
            workspaces::touch(m, "/v/b", 2, workspaces::MAX);
            Ok(())
        })
        .0;
        done_tx.send(()).unwrap();
        r
    });
    // トランザクションが無ければ B はすぐ書き終わる（A の読みの後）。あれば A が終わるまで待つ
    let _ = done_rx.recv_timeout(Duration::from_millis(300));
    go_tx.send(()).unwrap();
    ta.join().unwrap().unwrap();
    tb.join().unwrap().unwrap();
    let c = openref_core::Db::open(&path).unwrap();
    let m = workspaces::parse(c.setting(workspaces::KEY).unwrap().as_deref()).unwrap();
    assert!(m.contains_key("/v/a") && m.contains_key("/v/b"), "片方の書込を失った: {m:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// 別の接続が読み書きの途中（BEGIN IMMEDIATE を持ったまま）で busy_timeout を過ぎたら、f を呼ばずに Err（database is locked）・
/// 何も書かない（壊れた値の退避も書かない = rollback）。待つ間に相手が終われば通る（上の試験）
#[test]
fn ws_tx_gives_up_with_busy_and_writes_nothing() {
    use openref_core::wiki::workspaces;
    let d = scratch("ws-busy");
    let path = d.join("openref.sqlite");
    let a = openref_core::Db::open(&path).unwrap();
    let b = openref_core::Db::open(&path).unwrap();
    b.set_setting(workspaces::KEY, "[broken").unwrap();
    b.conn.busy_timeout(Duration::from_millis(100)).unwrap();
    let held = workspaces::begin_immediate(&a).unwrap();
    let mut called = false;
    let (r, note) = wiki_ws_tx(&b, 1_793_491_200, false, None, |m, _| {
        called = true;
        workspaces::touch(m, "/v/b", 1, workspaces::MAX);
        Ok(())
    });
    let e = r.unwrap_err();
    assert!(e.contains("database is locked"), "{e}");
    assert!(!called && note == WikiWsNote::default());
    drop(held);
    assert_eq!(setting(&b, workspaces::KEY).as_deref(), Some("[broken"), "何も書かない");
    assert_eq!(setting(&b, "wiki_workspaces.broken-20261101T000000Z"), None, "退避も書かない");
    let _ = std::fs::remove_dir_all(&d);
}

/// wiki_ws_tx: 壊れた wiki_workspaces は退避して空から（値は退避先に残る）・f が Err なら f の変更は書かない・変わらなければ書かない・
/// 未知の項目は保つ
#[test]
fn ws_tx_sets_aside_broken_and_discards_failed_changes() {
    use openref_core::wiki::workspaces;
    let db = openref_core::Db::open_in_memory().unwrap();
    db.set_setting("wiki_root", "/v/a").unwrap();
    db.set_setting(workspaces::KEY, "[broken").unwrap();
    let (r, note) = wiki_ws_tx(&db, 0, true, None, |m, root| {
        Ok(workspaces::touch(m, root.unwrap(), 100, workspaces::MAX))
    });
    assert!(r.unwrap().is_empty());
    assert_eq!(note.set_aside.len(), 1);
    assert_eq!(note.set_aside[0].1, "wiki_workspaces.broken-19700101T000000Z");
    assert!(note.set_aside[0].2.contains("wiki_workspaces"), "{:?}", note.set_aside);
    assert_eq!(setting(&db, "wiki_workspaces.broken-19700101T000000Z").as_deref(), Some("[broken"));
    let m = workspaces::parse(setting(&db, workspaces::KEY).as_deref()).unwrap();
    assert_eq!(workspaces::order(&m), vec![("/v/a".to_string(), 100)]);
    // f が Err = f の変更は捨てる（書かない）
    let (r, note) = wiki_ws_tx(&db, 0, false, None, |m, _| -> Result<(), String> {
        workspaces::touch(m, "/v/zzz", 999, workspaces::MAX);
        Err("だめ".into())
    });
    assert_eq!(r.unwrap_err(), "だめ");
    assert!(note.set_aside.is_empty());
    assert!(!workspaces::parse(setting(&db, workspaces::KEY).as_deref()).unwrap().contains_key("/v/zzz"));
    // 壊れた値 + f が Err = 退避と空からの書き戻しは書く（commit する）・f の変更は書かない
    db.set_setting(workspaces::KEY, "{oops").unwrap();
    let (r, note) = wiki_ws_tx(&db, 60, false, None, |m, _| -> Result<(), String> {
        workspaces::touch(m, "/v/zzz", 999, workspaces::MAX);
        Err("だめ".into())
    });
    assert_eq!(r.unwrap_err(), "だめ");
    assert_eq!(note.set_aside.len(), 1);
    assert_eq!(setting(&db, "wiki_workspaces.broken-19700101T000100Z").as_deref(), Some("{oops"), "退避は残る");
    assert!(workspaces::parse(setting(&db, workspaces::KEY).as_deref()).unwrap().is_empty(), "空から書き戻す");
    // 変わらない読みは書かない（外から整形して書いた値がそのまま残る = 書いていない）
    db.set_setting(workspaces::KEY, r#"{ "/v/a" : { "last_used" : 100, "future" : [1] } }"#).unwrap();
    let (r, _) = wiki_ws_tx(&db, 0, false, None, |m, _| Ok(m.len()));
    assert_eq!(r.unwrap(), 1);
    assert_eq!(
        setting(&db, workspaces::KEY).as_deref(),
        Some(r#"{ "/v/a" : { "last_used" : 100, "future" : [1] } }"#)
    );
    // 書けば未知の項目は保つ
    wiki_ws_tx(&db, 0, false, None, |m, _| Ok(workspaces::touch(m, "/v/b", 200, workspaces::MAX))).0.unwrap();
    let m = workspaces::parse(setting(&db, workspaces::KEY).as_deref()).unwrap();
    assert_eq!(m["/v/a"]["future"], serde_json::json!([1]));
    // 何も無い DB を読むだけなら書かない
    let empty = openref_core::Db::open_in_memory().unwrap();
    wiki_ws_tx(&empty, 0, false, None, |m, _| Ok(m.len())).0.unwrap();
    assert_eq!(setting(&empty, workspaces::KEY), None);
}

/// 切替 / [外す] の検査: 切替は履歴にある root だけ・[外す] は現在の root を拒否・履歴に無ければ Err・外したら保存される
#[test]
fn switch_and_forget_rules() {
    use openref_core::wiki::workspaces;
    let db = openref_core::Db::open_in_memory().unwrap();
    db.set_setting("wiki_root", "/v/a").unwrap();
    db.set_setting(workspaces::KEY, r#"{"/v/a": {"last_used": 2}, "/v/b": {"last_used": 1, "ref_dirs": ["/notes"]}}"#).unwrap();
    let switch = |root: &str| wiki_ws_tx(&db, 0, false, None, |m, _| wiki_switch_in(m, root, false)).0;
    assert!(switch("/v/b").is_ok());
    assert!(switch("/v/a").is_ok(), "現在への切替は止めない（画面が押させない・押しても選び直しと同じ）");
    assert_eq!(switch("/v/c").unwrap_err(), "履歴に無いワークスペースです");
    assert_eq!(
        wiki_ws_tx(&db, 0, true, None, |m, _| wiki_switch_in(m, "/v/c", true)).0.unwrap_err(),
        "That workspace is not in the history"
    );
    let forget = |root: &str, en: bool| wiki_ws_tx(&db, 0, en, None, |m, cur| wiki_forget_in(m, &[cur], root, en)).0;
    assert_eq!(forget("/v/a", false).unwrap_err(), "現在のワークスペースは履歴から外せません");
    assert_eq!(forget("/v/a", true).unwrap_err(), "The current workspace cannot be removed from the history");
    assert_eq!(forget("/v/c", false).unwrap_err(), "履歴に無いワークスペースです");
    let m = workspaces::parse(setting(&db, workspaces::KEY).as_deref()).unwrap();
    assert_eq!(m.len(), 2, "拒否は何も変えない");
    forget("/v/b", false).unwrap();
    let m = workspaces::parse(setting(&db, workspaces::KEY).as_deref()).unwrap();
    assert_eq!(m.keys().cloned().collect::<Vec<_>>(), vec!["/v/a".to_string()]);
    assert_eq!(forget("/v/b", false).unwrap_err(), "履歴に無いワークスペースです");
    // 現在の root が未設定なら、現在の保護は効かない（履歴にあれば外せる）
    let db2 = openref_core::Db::open_in_memory().unwrap();
    db2.set_setting(workspaces::KEY, r#"{"/v/a": {}}"#).unwrap();
    wiki_ws_tx(&db2, 0, false, None, |m, cur| wiki_forget_in(m, &[cur], "/v/a", false)).0.unwrap();
}

/// 前の root の配信が残る知らせ: stale のときプロセスで最初の 1 回だけ・stale でなければ印を使わない
#[test]
fn stale_note_only_once() {
    let told = AtomicBool::new(false);
    assert!(!wiki_stale_once(&told, false));
    assert!(!wiki_stale_once(&told, false), "stale でない呼び出しは印を使わない");
    assert!(wiki_stale_once(&told, true));
    assert!(!wiki_stale_once(&told, true));
    assert!(!wiki_stale_once(&told, false));
    // toast の detail = 名前 + 一言（1 枠の toast にまとめる）
    assert_eq!(wiki_toast_detail(false, "site".into(), None), "site");
    assert_eq!(wiki_toast_detail(false, "site".into(), Some("前の".into())), "site（前の）");
    assert_eq!(wiki_toast_detail(true, "site".into(), Some("Files".into())), "site (Files)");
    assert_eq!(wiki_workspace_name(Path::new("/Users/u/Documents/site")), "site");
    assert_eq!(wiki_workspace_name(Path::new("/")), "/");
}

/// wiki-index/ の掃除の判定: keep に無い `<16 桁>.sqlite` の一族（-wal / -shm / 退避）だけ・他の名前・keep の一族には触らない
#[test]
fn index_sweep_plan_rules() {
    let cur = "0123456789abcdef.sqlite".to_string();
    let hist = "fedcba9876543210.sqlite".to_string();
    let keep: HashSet<String> = [cur.clone(), hist.clone()].into_iter().collect();
    let orphan = "aaaaaaaaaaaaaaaa";
    let names: Vec<String> = [
        cur.clone(),
        format!("{cur}-wal"),
        format!("{cur}-shm"),
        format!("{cur}.corrupt-1700000000"),
        hist.clone(),
        format!("{hist}-wal"),
        format!("{orphan}.sqlite"),
        format!("{orphan}.sqlite-wal"),
        format!("{orphan}.sqlite-shm"),
        format!("{orphan}.sqlite.corrupt-1700000000"),
        format!("{orphan}.sqlite.corrupt-1700000000-wal"),
        // 触らない: 16 桁でない・大文字・hex でない・知らない接尾辞・他の種類のファイル
        "abc.sqlite".into(),
        "AAAAAAAAAAAAAAAA.sqlite".into(),
        "gggggggggggggggg.sqlite".into(),
        format!("{orphan}.sqlite.bak"),
        format!("{orphan}.sqlite.corrupt-"),
        format!("{orphan}.sqlite.corrupt-x"),
        "notes.txt".into(),
        ".DS_Store".into(),
        "wiki-index.sqlite".into(),
    ]
    .into_iter()
    .collect();
    let mut plan = wiki_index_sweep_plan(&names, &keep);
    plan.sort();
    assert_eq!(
        plan,
        vec![
            format!("{orphan}.sqlite"),
            format!("{orphan}.sqlite-shm"),
            format!("{orphan}.sqlite-wal"),
            format!("{orphan}.sqlite.corrupt-1700000000"),
            format!("{orphan}.sqlite.corrupt-1700000000-wal"),
        ]
    );
    // keep が空 = 16 桁の一族は全部（現在の root が無い = 起動時に無効・未設定）
    assert_eq!(wiki_index_sweep_plan(&names, &HashSet::new()).len(), 4 + 2 + 5);
}

/// 起動時の掃除（実ファイル・偽の app_dir）: wiki-index/ は履歴と現在の root の索引を残して孤児だけ消す・掃除の開始より後に
/// 変更されたファイルは残す・app_dir 直下（DB と 2.0.0 より前の wiki-index.sqlite の一族）には触らない
#[test]
fn startup_sweep_on_real_files() {
    use openref_core::wiki::index::{index_path, INDEX_DIR, INDEX_FILE};
    let app = scratch("sweep");
    let touch = |p: &Path| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    };
    for n in ["openref.sqlite", "openref.sqlite-wal", "openref.sqlite-shm"] {
        touch(&app.join(n));
    }
    for n in [INDEX_FILE, "wiki-index.sqlite-wal", "wiki-index.sqlite-shm", "wiki-index.sqlite.corrupt-1"] {
        touch(&app.join(n));
    }
    let cur = "/Users/u/Documents/ref";
    let hist = "/Users/u/Documents/site";
    let orphan = "/Users/u/Documents/old";
    for r in [cur, hist, orphan] {
        let ip = index_path(&app, Path::new(r));
        touch(&ip);
        touch(&PathBuf::from(format!("{}-wal", ip.display())));
    }
    touch(&app.join(INDEX_DIR).join("README.txt"));
    let started = SystemTime::now() + Duration::from_secs(60); // ここまでに作ったものは「前から有った」
    let keep = wiki_index_keep(&app, [cur, hist]);
    let swept = wiki_index_sweep(&app, &keep, started);
    assert_eq!(swept.len(), 2, "{swept:?}");
    for n in ["openref.sqlite", "openref.sqlite-wal", "openref.sqlite-shm", INDEX_FILE, "wiki-index.sqlite-wal", "wiki-index.sqlite-shm", "wiki-index.sqlite.corrupt-1"] {
        assert!(app.join(n).exists(), "app_dir 直下は残る: {n}");
    }
    for r in [cur, hist] {
        let ip = index_path(&app, Path::new(r));
        assert!(ip.exists() && PathBuf::from(format!("{}-wal", ip.display())).exists(), "{r} は残る");
    }
    assert!(!index_path(&app, Path::new(orphan)).exists());
    assert!(app.join(INDEX_DIR).join("README.txt").exists(), "知らない名前には触らない");
    // 掃除の開始より後に変更された孤児は残す（掃除の途中で作られた索引を消さない）
    let late = index_path(&app, Path::new("/Users/u/Documents/new"));
    touch(&late);
    assert!(wiki_index_sweep(&app, &keep, SystemTime::now() - Duration::from_secs(60)).is_empty());
    assert!(late.exists());
    // wiki-index/ が無くても落ちない
    let empty = scratch("sweep-empty");
    assert!(wiki_index_sweep(&empty, &keep, started).is_empty());
    let _ = std::fs::remove_dir_all(&app);
    let _ = std::fs::remove_dir_all(&empty);
}

/// 前の root の配信が残るか: 同じ root の選び直し = false・別の root = true・参照フォルダは数えない
#[test]
fn root_stale_counts_roots_only() {
    let a = PathBuf::from("/v/a");
    let b = PathBuf::from("/v/b");
    let notes = PathBuf::from("/v/notes");
    let mut al = WikiAllowed::default();
    // まだ何も配信していない（起動直後に選ぶ）
    assert!(!wiki_root_stale(&al, &a));
    // 同じ root を選び直す
    assert!(al.insert(&a, WikiDir::Root), "scope にまだ無い");
    assert!(!wiki_root_stale(&al, &a));
    // 別の root
    assert!(wiki_root_stale(&al, &b));
    // A → B → A（A の配信は残っているが B も残る = 再起動を勧める）
    let mut ab = al.clone();
    ab.insert(&b, WikiDir::Root);
    assert!(wiki_root_stale(&ab, &a));

    // 参照フォルダが配信中でも、root の記録だけで見る（= 同じ root の選び直しで再起動を勧めない）
    assert!(al.insert(&notes, WikiDir::Ref));
    assert!(!wiki_root_stale(&al, &a));
    assert!(wiki_root_stale(&al, &b));
    // 参照フォルダだけが配信中（root はまだ）= 選んだ root が何であれ stale でない
    let mut refs_only = WikiAllowed::default();
    refs_only.insert(&notes, WikiDir::Ref);
    assert!(!wiki_root_stale(&refs_only, &a));
    // 配信そのものは両方を数える
    assert!(al.contains(&notes) && al.contains(&a) && !al.contains(&b));
    assert!(!al.is_empty());
    assert_eq!(al.all(), vec![a.clone(), notes.clone()]);
    // 同じフォルダを 2 度足しても scope には 1 回・記録も 1 つ
    assert!(!al.insert(&a, WikiDir::Root));
    assert_eq!(al.roots, vec![a.clone()]);
    // 参照フォルダだった場所を root に選ぶ: scope には既にある（false）が root として記録される = その root の選び直しは stale でない
    assert!(!al.insert(&notes, WikiDir::Root));
    assert_eq!(al.roots, vec![a.clone(), notes.clone()]);
    assert_eq!(al.refs, vec![notes.clone()]);
    assert!(wiki_root_stale(&al, &notes), "前の root a の配信は残る");
}

/// wiki_apply の段取り: 無効 = Off（ready を見ない）/ root が使えない = Unusable / 配信 = root。
/// 参照フォルダは別（wiki_apply_refs）: 実在するものだけ・参照フォルダとして記録・設定が読めなければ空 + 理由
#[test]
fn apply_plan_rules() {
    let ready = || Ok((PathBuf::from("/app/wiki-index.sqlite"), PathBuf::from("/v")));
    assert_eq!(wiki_apply_plan(false, ready()), WikiApplyPlan::Off);
    assert_eq!(wiki_apply_plan(false, Err("x".into())), WikiApplyPlan::Off);
    assert_eq!(
        wiki_apply_plan(true, Err("root が無い".into())),
        WikiApplyPlan::Unusable("root が無い".into())
    );
    assert_eq!(
        wiki_apply_plan(true, ready()),
        WikiApplyPlan::Serve {
            index: PathBuf::from("/app/wiki-index.sqlite"),
            root: PathBuf::from("/v"),
        }
    );
    let exists = |p: &Path| p != Path::new("/gone");
    assert_eq!(
        wiki_apply_refs(Ok(vec![PathBuf::from("/notes"), PathBuf::from("/gone")]), &exists),
        (vec![(PathBuf::from("/notes"), WikiDir::Ref)], None)
    );
    // 参照フォルダの設定が読めない = 参照フォルダは配信しない・理由を持ち帰る（root の配信は plan 側で済んでいる）
    assert_eq!(
        wiki_apply_refs(Err("壊れている".into()), &exists),
        (Vec::new(), Some("壊れている".to_string()))
    );
}

/// 参照フォルダの中のファイルの見せ方: media / html だけ・probe は種類だけ・html は BODY_MAX まで
#[test]
fn ref_show_rules() {
    use openref_core::wiki::{index, scan::Kind};
    assert_eq!(wiki_ref_show(Kind::Media, false, 1, "a.pdf", true).unwrap(), WikiRefShow::Media);
    assert_eq!(wiki_ref_show(Kind::Html, false, 1, "a.html", true).unwrap(), WikiRefShow::Html);
    assert_eq!(wiki_ref_show(Kind::Media, true, 1, "a.png", true).unwrap(), WikiRefShow::Probe);
    assert_eq!(wiki_ref_show(Kind::Html, true, 1, "a.html", true).unwrap(), WikiRefShow::Probe);
    // md・その他は内蔵で見せない（probe でも）
    for k in [Kind::Md, Kind::Other] {
        for probe in [false, true] {
            let e = wiki_ref_show(k, probe, 1, "x.md", true).unwrap_err();
            assert!(e.contains("not shown") && e.contains("x.md"), "{k:?} probe={probe}: {e}");
        }
    }
    assert!(
        wiki_ref_show(Kind::Md, false, 1, "x.md", false)
            .unwrap_err()
            .contains("表示しません"),
        "日本語"
    );
    // html の大きさ: 上限ちょうどは可・1 バイト超は Err。media と probe は大きさを見ない
    let max = index::BODY_MAX as u64;
    assert_eq!(
        wiki_ref_show(Kind::Html, false, max, "b.html", true).unwrap(),
        WikiRefShow::Html
    );
    let e = wiki_ref_show(Kind::Html, false, max + 1, "b.html", false).unwrap_err();
    assert!(e.contains("大きすぎて") && e.contains("b.html"), "{e}");
    let e = wiki_ref_show(Kind::Html, false, max + 1, "b.html", true).unwrap_err();
    assert!(e.contains("too large") && e.contains("b.html"), "{e}");
    assert_eq!(
        wiki_ref_show(Kind::Media, false, max + 1, "b.mp4", true).unwrap(),
        WikiRefShow::Media
    );
    assert_eq!(
        wiki_ref_show(Kind::Html, true, max + 1, "b.html", true).unwrap(),
        WikiRefShow::Probe
    );
}

/// チャットが使えない理由の優先順: Wiki 無効 › チャット無効 › root が使えない › claude が無い › 未ログイン › mcp が無い › None
#[test]
fn chat_reason_priority() {
    let ok = WikiChatProbe {
        wiki_on: true,
        chat_on: true,
        ready_error: None,
        cli: true,
        logged_in: Some(true),
        mcp: true,
    };
    assert_eq!(wiki_chat_reason(false, &ok), None);
    assert_eq!(wiki_chat_reason(true, &ok), None);
    // ログインの有無が分からなくても止めない
    assert_eq!(wiki_chat_reason(false, &WikiChatProbe { logged_in: None, ..ok.clone() }), None);
    // 全部悪いときは Wiki 無効が勝つ
    let bad = WikiChatProbe {
        wiki_on: false,
        chat_on: false,
        ready_error: Some("root が無い".into()),
        cli: false,
        logged_in: Some(false),
        mcp: false,
    };
    let r = wiki_chat_reason(false, &bad).unwrap();
    assert!(r.contains("OpenNote が無効"), "{r}");
    let r = wiki_chat_reason(true, &bad).unwrap();
    assert!(r.contains("OpenNote is disabled"), "{r}");
    // 1 段ずつ直すと次の理由が出る
    let r = wiki_chat_reason(false, &WikiChatProbe { wiki_on: true, ..bad.clone() }).unwrap();
    assert!(r.contains("チャットは無効"), "{r}");
    let r = wiki_chat_reason(false, &WikiChatProbe { wiki_on: true, chat_on: true, ..bad.clone() }).unwrap();
    assert_eq!(r, "root が無い", "ready の理由はそのまま");
    let step = WikiChatProbe { wiki_on: true, chat_on: true, ready_error: None, ..bad.clone() };
    let r = wiki_chat_reason(false, &step).unwrap();
    assert!(r.contains("claude コマンド"), "{r}");
    let step = WikiChatProbe { cli: true, ..step };
    let r = wiki_chat_reason(false, &step).unwrap();
    assert!(r.contains("ログインしていません"), "{r}");
    let step = WikiChatProbe { logged_in: Some(true), ..step };
    let r = wiki_chat_reason(false, &step).unwrap();
    assert!(r.contains("openref-mcp"), "{r}");
    assert_eq!(wiki_chat_reason(false, &WikiChatProbe { mcp: true, ..step }), None);
    // 無効のときの画面向け（status の早期 return と同じ材料）: Wiki が有効でチャットだけ OFF
    let r = wiki_chat_reason(true, &WikiChatProbe { wiki_on: true, chat_on: false, ..WikiChatProbe::default() }).unwrap();
    assert!(r.contains("chat is disabled"), "{r}");
}

/// asset:// の配信: main 以外の webview は 403・`.`/`..` と scope の外は 403・Range は単一区間・全体は 200・
/// scope の中の symlink で外を指すものは 403（照合も open も canonicalize した 1 つの値）
#[test]
fn asset_reply_rules() {
    use tauri::http::{header, Request, StatusCode};
    // scope は canonical なパスで照合する（Tauri の is_allowed と同じ）= temp_dir（/var → /private/var）も解いておく
    let d = std::fs::canonicalize(scratch("asset")).unwrap();
    let f = d.join("v").join("a.txt");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, b"0123456789").unwrap();
    let allowed = |p: &Path| p.starts_with(d.join("v"));
    // scratch のパスは英数字と `-` `.` `/` だけ = encode なしで URL にできる（`asset://localhost//private/…` の形）
    let url = format!("asset://localhost/{}", f.display());
    let req = |u: &str, range: Option<&str>| {
        let mut b = Request::builder()
            .uri(u)
            .header(header::ORIGIN, "tauri://localhost");
        if let Some(r) = range {
            b = b.header(header::RANGE, r);
        }
        b.body(Vec::new()).unwrap()
    };
    // main 以外の webview からは読めない（ファイルに触らない = allowed も呼ばれない）
    let r = asset_reply("other", &|_| panic!("scope を見た"), &req(&url, None));
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = asset_reply("main", &allowed, &req(&url, None));
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.body(), b"0123456789");
    assert_eq!(
        r.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "tauri://localhost"
    );
    assert!(r
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/plain"));
    let r = asset_reply("main", &allowed, &req(&url, Some("bytes=2-4")));
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.body(), b"234");
    assert_eq!(
        r.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes 2-4/10"
    );
    let r = asset_reply("main", &allowed, &req(&url, Some("bytes=20-30")));
    assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    // 空ファイル × Range: `-100` は長さ 0 の区間（206 で表せない）→ Range を無視して 200 の空 body・`0-` / `0-0` は満たせない → 416
    let e = d.join("v").join("empty.txt");
    std::fs::write(&e, b"").unwrap();
    let eurl = format!("asset://localhost/{}", e.display());
    let r = asset_reply("main", &allowed, &req(&eurl, Some("bytes=-100")));
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.body().is_empty());
    assert_eq!(r.headers().get(header::CONTENT_LENGTH).unwrap(), "0");
    assert!(r.headers().get(header::CONTENT_RANGE).is_none());
    for spec in ["bytes=0-", "bytes=0-0"] {
        let r = asset_reply("main", &allowed, &req(&eurl, Some(spec)));
        assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE, "{spec}");
        assert_eq!(r.headers().get(header::CONTENT_RANGE).unwrap(), "bytes */0", "{spec}");
    }
    // `..`（%2E%2E も）・scope の外・無いファイル
    for dotted in [
        format!("asset://localhost/{}/v/../v/a.txt", d.display()),
        format!("asset://localhost/{}/v/%2E%2E/v/a.txt", d.display()),
    ] {
        assert_eq!(
            asset_reply("main", &allowed, &req(&dotted, None)).status(),
            StatusCode::FORBIDDEN,
            "{dotted}"
        );
    }
    assert_eq!(
        asset_reply("main", &|_| false, &req(&url, None)).status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        asset_reply("main", &allowed, &req(&format!("{url}.nope"), None)).status(),
        StatusCode::NOT_FOUND
    );
    // scope の外の無いファイルは 404 でなく 403（有無を漏らさない）
    assert_eq!(
        asset_reply("main", &allowed, &req(&format!("asset://localhost/{}/nope.txt", d.display()), None)).status(),
        StatusCode::FORBIDDEN
    );
    // scope の中の symlink → 外のファイル: 403（ファイルに触らない）・中を指す symlink は配信する（照合と open は解いた先）
    let outside = d.join("secret.txt");
    std::fs::write(&outside, b"SECRET").unwrap();
    std::os::unix::fs::symlink(&outside, d.join("v").join("out.txt")).unwrap();
    std::os::unix::fs::symlink("a.txt", d.join("v").join("in.txt")).unwrap();
    let seen = std::cell::RefCell::new(Vec::new());
    let spy = |p: &Path| {
        seen.borrow_mut().push(p.to_path_buf());
        allowed(p)
    };
    let r = asset_reply("main", &spy, &req(&format!("asset://localhost/{}/v/out.txt", d.display()), None));
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(r.body().is_empty());
    assert_eq!(*seen.borrow(), vec![outside.clone()], "照合は解いた先で");
    let r = asset_reply("main", &allowed, &req(&format!("asset://localhost/{}/v/in.txt", d.display()), None));
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.body(), b"0123456789");
    // 照合と open の間に symlink を外へ差し替えても、開くのは照合した canonical な実体（中の a.txt）= 外を読まない
    let swap = d.join("v").join("swap.txt");
    std::os::unix::fs::symlink("a.txt", &swap).unwrap();
    let outside2 = outside.clone();
    ASSET_BEFORE_OPEN.with(|h| {
        *h.borrow_mut() = Some(Box::new(move |p: &Path| {
            std::fs::remove_file(p).unwrap();
            std::os::unix::fs::symlink(&outside2, p).unwrap();
        }))
    });
    let r = asset_reply("main", &allowed, &req(&format!("asset://localhost/{}", swap.display()), None));
    ASSET_BEFORE_OPEN.with(|h| *h.borrow_mut() = None);
    assert_eq!(std::fs::read_link(&swap).unwrap(), outside, "差し替えが起きた（試験の前提）");
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.body(), b"0123456789", "照合の後に差し替えた先（外）を読んだ");
    let _ = std::fs::remove_dir_all(&d);
}

/// 監視の印: 張り終えるまで・張った後の stat 走査（prime）の前は省かない。止めた・失敗・root が違う = 今どおり。
/// 取り込みの失敗は次の入口で走査し直す。前の監視（gen が古い）の知らせは今の監視に効かない
#[test]
fn wiki_live_plan() {
    let l = WikiLive::default();
    let r = Some("/v");
    assert_eq!(l.plan(r), WatchPlan::Off, "監視なし");
    let g = l.begin(r.map(String::from));
    assert_eq!(l.plan(r), WatchPlan::Off, "張り終える前");
    assert!(!l.prime(g), "張り終える前の走査は数えない");
    l.armed(g);
    assert_eq!(l.plan(r), WatchPlan::Prime(g));
    assert_eq!(l.plan(Some("/other")), WatchPlan::Off, "root が違う");
    assert_eq!(l.plan(None), WatchPlan::Off);
    assert!(l.prime(g));
    assert_eq!(l.plan(r), WatchPlan::Skip);
    l.unprime(g);
    assert_eq!(l.plan(r), WatchPlan::Prime(g), "取り込みの失敗 = 走査し直す");
    assert!(l.prime(g));
    l.fail(g);
    assert_eq!(l.plan(r), WatchPlan::Off, "監視の失敗");
    assert!(!l.prime(g));
    // 張り直し: 前の gen の知らせ（失敗・prime）は効かない
    let g2 = l.begin(r.map(String::from));
    l.armed(g);
    assert_eq!(l.plan(r), WatchPlan::Off, "前の gen の armed");
    l.armed(g2);
    assert!(!l.prime(g));
    assert_eq!(l.plan(r), WatchPlan::Prime(g2));
    assert!(l.prime(g2));
    l.fail(g);
    l.unprime(g);
    assert_eq!(l.plan(r), WatchPlan::Skip, "前の gen の失敗");
    l.stop();
    assert_eq!(l.plan(r), WatchPlan::Off, "止めた");
    assert!(!l.prime(g2));
}

/// 監視の印: prime の前（Prime）に索引が握られていれば（背景の走査・作り直し）待たずに watched = false
/// （core の入口が busy の経路で進捗を返す）。握られていなければ stat 走査して watched = true・以後は走査しない（Skip）
#[test]
fn wiki_watched_does_not_wait_for_busy_index() {
    use openref_core::wiki::{with_index, Ctx};
    let base = std::fs::canonicalize(scratch("watched")).unwrap();
    let home = base.join("home");
    let root = home.join("Documents/vault");
    let app_dir = home.join("scratch-app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(root.join("a.md"), "# A\n").unwrap();
    let ctx = Ctx {
        enabled: true,
        root: Some(root.to_string_lossy().into()),
        app_dir: Some(app_dir),
        home: Some(home),
        watched: false,
        ..Default::default()
    };
    let at = ctx.ready().unwrap();
    let live = WikiLive::default();
    let g = live.begin(ctx.root.clone());
    live.armed(g);
    // 別のスレッドが索引を 1.5 秒握る（背景の走査の代わり）
    let (tx, rx) = std::sync::mpsc::channel();
    let held = {
        let at = at.clone();
        std::thread::spawn(move || {
            with_index(&at, |_| {
                tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(1500));
                Ok(())
            })
            .unwrap()
        })
    };
    rx.recv().unwrap();
    let t = std::time::Instant::now();
    assert!(!wiki_watched(&live, &ctx), "busy の間は省かない");
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "索引を待った: {:?}", t.elapsed());
    assert_eq!(live.plan(ctx.root.as_deref()), WatchPlan::Prime(g), "prime していない");
    held.join().unwrap();
    // 握られていなければ stat 走査して省く（並行する他の試験が一瞬握っていれば busy = 取り直す）
    assert!((0..100).any(|_| wiki_watched(&live, &ctx) || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        false
    }));
    assert_eq!(live.plan(ctx.root.as_deref()), WatchPlan::Skip);
    assert!(with_index(&at, |ix| ix.page("a.md")).unwrap().is_some(), "prime の走査で索引に入る");
    let _ = std::fs::remove_dir_all(&base);
}

/// 参照フォルダの検査: root と同じ規則 + vault の中は不可
#[test]
fn ref_dir_check_rules() {
    let d = scratch("check");
    let home = d.join("home");
    let notes = home.join("notes");
    let vault = home.join("Documents").join("vault");
    let hidden = home.join(".claude");
    for p in [&notes, &vault, &hidden] {
        std::fs::create_dir_all(p).unwrap();
    }
    let canon_home = std::fs::canonicalize(&home).unwrap();
    let canon_vault = std::fs::canonicalize(&vault).unwrap();
    let ok = wiki_ref_dir_check(&notes, &canon_home, Some(&canon_vault), None, false).unwrap();
    assert_eq!(ok, std::fs::canonicalize(&notes).unwrap());
    // 相対・無い・ファイル・隠し・ホームそのもの・vault そのもの・vault の中
    assert!(wiki_ref_dir_check(Path::new("notes"), &canon_home, None, None, false).is_err());
    assert!(wiki_ref_dir_check(&home.join("nope"), &canon_home, None, None, false).is_err());
    std::fs::write(notes.join("f.txt"), "x").unwrap();
    assert!(wiki_ref_dir_check(&notes.join("f.txt"), &canon_home, None, None, false).is_err());
    assert!(wiki_ref_dir_check(&hidden, &canon_home, None, None, false)
        .unwrap_err()
        .contains("隠し"));
    // 主語は「参照フォルダに」（check_root の規則と理由はそのまま・前置きの「入力を解釈できません」は付けない）
    assert_eq!(
        wiki_ref_dir_check(&hidden, &canon_home, None, None, false).unwrap_err(),
        format!("参照フォルダに {} は使えません（`.` で始まるフォルダ（隠しフォルダ）の中です）", hidden.display())
    );
    assert!(wiki_ref_dir_check(&home, &canon_home, None, None, false)
        .unwrap_err()
        .contains("ホーム"));
    assert!(
        wiki_ref_dir_check(&vault, &canon_home, Some(&canon_vault), None, false)
            .unwrap_err()
            .contains("ワークスペースの中")
    );
    std::fs::create_dir_all(vault.join("sub")).unwrap();
    assert!(
        wiki_ref_dir_check(&vault.join("sub"), &canon_home, Some(&canon_vault), None, false)
            .unwrap_err()
            .contains("ワークスペースの中")
    );
    // vault を含むフォルダは可（root が vault = root/wiki を含む = 主な使い方。vault の中のファイルは vault として先に判定される）
    assert!(wiki_ref_dir_check(&home.join("Documents"), &canon_home, Some(&canon_vault), None, false).is_ok());
    // 文言は「参照フォルダに」
    assert!(wiki_ref_dir_check(&hidden, &canon_home, None, None, false)
        .unwrap_err()
        .starts_with("参照フォルダに"));
    let _ = std::fs::remove_dir_all(&d);
}

/// 参照フォルダの中のファイルの実体: 絶対・`.` `..` の区画なし・隠しの区画なし・symlink で外へ出ない・通常ファイル
#[test]
fn ref_target_rules() {
    let d = scratch("target");
    let notes = std::fs::canonicalize(&d).unwrap().join("notes");
    let outside = std::fs::canonicalize(&d).unwrap().join("outside");
    std::fs::create_dir_all(notes.join("journals")).unwrap();
    std::fs::create_dir_all(notes.join(".git")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(notes.join("journals/a.pdf"), "%PDF").unwrap();
    std::fs::write(notes.join(".git/h.pdf"), "%PDF").unwrap();
    std::fs::write(outside.join("o.pdf"), "%PDF").unwrap();
    std::os::unix::fs::symlink(outside.join("o.pdf"), notes.join("journals/link.pdf")).unwrap();
    std::os::unix::fs::symlink(&outside, notes.join("odir")).unwrap();
    let dirs = vec![notes.clone()];
    let a = notes.join("journals/a.pdf").to_string_lossy().into_owned();
    let (root, canon) = wiki_ref_target(&dirs, &a, true).unwrap();
    assert_eq!((root, canon), (notes.clone(), notes.join("journals/a.pdf")));
    assert!(
        wiki_ref_target(&dirs, "journals/a.pdf", true).is_err(),
        "相対"
    );
    assert!(
        wiki_ref_target(&dirs, "journals/a.pdf", false)
            .unwrap_err()
            .contains("絶対パス"),
        "日本語"
    );
    assert!(
        wiki_ref_target(
            &dirs,
            &format!("{}/journals/../journals/a.pdf", notes.display()),
            true,
        )
        .is_err(),
        "`..` の区画"
    );
    assert!(
        wiki_ref_target(&dirs, &notes.join(".git/h.pdf").to_string_lossy(), true).is_err(),
        "隠しの区画"
    );
    assert!(
        wiki_ref_target(&dirs, &notes.join("journals/link.pdf").to_string_lossy(), true)
            .unwrap_err()
            .contains("symlink"),
        "symlink の先が外"
    );
    assert!(
        wiki_ref_target(&dirs, &notes.join("odir/o.pdf").to_string_lossy(), true)
            .unwrap_err()
            .contains("symlink"),
        "symlink のフォルダ経由"
    );
    assert!(
        wiki_ref_target(&dirs, &notes.join("journals").to_string_lossy(), true).is_err(),
        "フォルダ"
    );
    assert!(
        wiki_ref_target(&dirs, &notes.to_string_lossy(), true).is_err(),
        "参照フォルダそのもの"
    );
    assert!(
        wiki_ref_target(&dirs, &outside.join("o.pdf").to_string_lossy(), true)
            .unwrap_err()
            .contains("not under"),
        "外"
    );
    assert!(
        wiki_ref_target(&dirs, &notes.join("journals/nope.pdf").to_string_lossy(), true)
            .unwrap_err()
            .contains("not found")
    );
    assert!(wiki_ref_target(&[], &a, true).is_err(), "参照フォルダなし");
    let _ = std::fs::remove_dir_all(&d);
}

/// wiki_open_url の検査: http(s) だけ通す（前後の空白は除く）・それ以外と URL でない文字列は拒む（ja / en の文言）
#[test]
fn wiki_url_checked_is_http_only() {
    let ok = |u: &str| wiki_url_checked(u, false).map(|u| u.to_string());
    assert_eq!(ok("https://note.com/x").as_deref(), Ok("https://note.com/x"));
    assert_eq!(ok("  http://example.org/a?b=1#c \n").as_deref(), Ok("http://example.org/a?b=1#c"));
    // 全角空白・NBSP（コピーで付きやすい）は url crate が落とさない = trim が要る
    assert_eq!(ok("\u{3000}https://note.com/x\u{a0}").as_deref(), Ok("https://note.com/x"));
    for bad in [
        "about:blank",
        "asset://localhost//Users/u/v/a.pdf",
        "tauri://localhost/",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "mailto:a@example.org",
    ] {
        assert_eq!(wiki_url_checked(bad, false).unwrap_err(), "http(s) のみ許可", "{bad}");
        assert_eq!(wiki_url_checked(bad, true).unwrap_err(), "Only http(s) is allowed", "{bad}");
    }
    for bad in ["", "note.com/x", "//note.com/x"] {
        assert_eq!(wiki_url_checked(bad, false).unwrap_err(), "URL ではありません", "{bad}");
        assert_eq!(wiki_url_checked(bad, true).unwrap_err(), "Not a URL", "{bad}");
    }
}

/// 実機のゴミ箱（Finder を操作する = 手で走らせる）: OPENREF_TRASH_TEST=<ホームの下の試験用フォルダ>
/// cargo test -p openref-app -- --ignored --nocapture trash_real
#[test]
#[ignore]
fn trash_real() {
    let dir = PathBuf::from(std::env::var("OPENREF_TRASH_TEST").unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join(format!("openref-trash-試験-{}.md", std::process::id()));
    std::fs::write(&f, "trash").unwrap();
    let method = wiki_move_to_trash(&f).unwrap();
    println!("{method}: {}", f.display());
    assert!(!f.exists());
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    assert!(
        std::fs::symlink_metadata(home.join(".Trash").join(f.file_name().unwrap())).is_ok(),
        "~/.Trash に無い"
    );
}

#[test]
fn reveal_only_unless_document_extension() {
    // 許可一覧の外（LaunchServices が起動・展開・導入するもの・拡張子なし）は Finder 表示
    for p in [
        "a.term",
        "b.ipa",
        "c.wflow",
        "d.shortcut",
        "e.zip",
        "f.xip",
        "g.img",
        "h.cdr",
        "i.cer",
        "j.vncloc",
        "k.afploc",
        "a.command",
        "x/B.WEBLOC",
        "s.sh",
        "noext",
        "dot.",
        "i.svg",
        "j.SVG",
    ] {
        assert!(wiki_reveal_only(Path::new(p), 0o644), "{p}");
    }
    // 許可一覧の内（大小無視）は open
    for p in [
        "a.md", "b.PNG", "c.mp4", "d.pdf", "e.html", "f.docx", "g.epub", "h.heic", "i.csv",
    ] {
        assert!(!wiki_reveal_only(Path::new(p), 0o644), "{p}");
    }
    // 実行ビットはどれか 1 つでも（拡張子に依らない）
    for m in [0o755, 0o744, 0o654, 0o645] {
        assert!(wiki_reveal_only(Path::new("a.pdf"), m), "{m:o}");
    }
    // 一覧は小文字・重複なし（大小無視の照合が効く前提）
    let mut seen = std::collections::HashSet::new();
    for e in WIKI_OPEN_EXT {
        assert_eq!(*e, e.to_lowercase());
        assert!(seen.insert(*e), "重複: {e}");
    }
}

/// vault の内外の開く先: 外は実在する通常ファイルだけ・隠しフォルダの中と実行系は Finder 表示・中は resolve_path の検査
#[test]
fn open_target_inside_and_outside_vault() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!(
        "openref-wiki-open-{}-{}",
        std::process::id(),
        fastrand_u64()
    ));
    let base = {
        std::fs::create_dir_all(&base).unwrap();
        std::fs::canonicalize(&base).unwrap()
    };
    let root = base.join("vault");
    let out = base.join("books");
    for d in [
        &root,
        &out,
        &out.join(".hidden"),
        &out.join("X.app/Contents"),
        &out.join("Deck.key/Data"),
        &root.join("Doc.Pages/Data"),
        &root.join("plain.dir"),
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    let put = |p: &Path, mode: u32| {
        std::fs::write(p, b"x").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    put(&root.join("a.pdf"), 0o644);
    put(&root.join("run.command"), 0o644);
    put(&out.join("教科書 1.pdf"), 0o644);
    put(&out.join("exec.pdf"), 0o755);
    put(&out.join("s.webloc"), 0o644);
    put(&out.join(".hidden/b.pdf"), 0o644);
    put(&out.join("openwith.pdf"), 0o644);
    let st = std::process::Command::new("/usr/bin/xattr")
        .args(["-w", "com.apple.LaunchServices.OpenWith", "x"])
        .arg(out.join("openwith.pdf"))
        .status()
        .unwrap();
    assert!(st.success());
    put(&out.join("noext"), 0o644);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    // vault の中
    assert_eq!(
        wiki_open_target(&root, "a.pdf", false).unwrap(),
        (root.join("a.pdf"), false)
    );
    assert_eq!(
        wiki_open_target(&root, &s(&root.join("a.pdf")), false).unwrap(),
        (root.join("a.pdf"), false)
    );
    assert!(wiki_open_target(&root, "run.command", false).unwrap().1);
    assert!(
        wiki_open_target(&root, "../books/教科書 1.pdf", false).is_err(),
        "相対の .. は中の検査で拒否"
    );
    assert!(wiki_open_target(&root, "nope.pdf", false).is_err());
    // vault の外（絶対パス）
    assert_eq!(
        wiki_open_target(&root, &s(&out.join("教科書 1.pdf")), false).unwrap(),
        (out.join("教科書 1.pdf"), false)
    );
    assert!(
        wiki_open_target(&root, &s(&out.join("exec.pdf")), false)
            .unwrap()
            .1,
        "実行ビット"
    );
    assert!(
        wiki_open_target(&root, &s(&out.join("s.webloc")), false)
            .unwrap()
            .1,
        "起動系の拡張子"
    );
    assert!(
        wiki_open_target(&root, &s(&out.join(".hidden/b.pdf")), false)
            .unwrap()
            .1,
        "隠しフォルダの中"
    );
    assert!(
        wiki_open_target(&root, &s(&out.join("openwith.pdf")), false)
            .unwrap()
            .1,
        "OpenWith の拡張属性付き"
    );
    assert!(!has_open_with_xattr(&out.join("教科書 1.pdf")));
    assert!(
        wiki_open_target(&root, &s(&out.join("noext")), false).unwrap().1,
        "拡張子なし"
    );
    assert!(wiki_open_target(&root, &s(&out), false).is_err(), "ディレクトリ");
    assert!(
        wiki_open_target(&root, &s(&out.join("X.app")), false).is_err(),
        "バンドル"
    );
    assert!(
        wiki_open_target(&root, &s(&out.join("missing.pdf")), false).is_err(),
        "存在しない"
    );
    // iWork のパッケージ形式（ディレクトリ）は拒否ではなく Finder 表示（vault の内外・大小無視）・ふつうのフォルダは拒否
    assert_eq!(
        wiki_open_target(&root, &s(&out.join("Deck.key")), false).unwrap(),
        (out.join("Deck.key"), true)
    );
    assert_eq!(
        wiki_open_target(&root, "Doc.Pages", false).unwrap(),
        (root.join("Doc.Pages"), true)
    );
    assert!(
        wiki_open_target(&root, "plain.dir", false).is_err(),
        "vault の中のフォルダ"
    );
    // getxattr の失敗: 属性が無い（ENOATTR）だけ「無い」・それ以外（存在しない = ENOENT 等）は「有る」側 = Finder 表示
    assert!(
        has_open_with_xattr(&out.join("missing.pdf")),
        "ENOENT は Finder 表示に倒す"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// 同値表の場（core の wiki::paths の試験と同じ配置）: base/home/Documents/vault = ワークスペース
fn paths_fixture() -> (PathBuf, PathBuf, Vec<(&'static str, String)>) {
    let base = std::env::temp_dir().join(format!(
        "openref-paths-app-{}-{}",
        std::process::id(),
        fastrand_u64()
    ));
    std::fs::create_dir_all(&base).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    let root = base.join("home/Documents/vault");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(root.join(".obsidian")).unwrap();
    std::fs::write(root.join("a.pdf"), "%PDF-1.4").unwrap();
    std::fs::write(root.join("sub/b.md"), "b").unwrap();
    std::fs::write(root.join(".obsidian/x.md"), "x").unwrap();
    std::fs::write(base.join("outside.md"), "o").unwrap();
    std::fs::create_dir_all(base.join(".hid")).unwrap();
    std::fs::write(base.join(".hid/y.md"), "y").unwrap();
    std::os::unix::fs::symlink(base.join("outside.md"), root.join("out-link.md")).unwrap();
    std::os::unix::fs::symlink(root.join("a.pdf"), root.join("in-link.pdf")).unwrap();
    std::os::unix::fs::symlink(root.join("a.pdf"), base.join("to-in.pdf")).unwrap();
    std::os::unix::fs::symlink(root.join("sub"), root.join(".alias")).unwrap();
    std::os::unix::fs::symlink(root.join(".obsidian/x.md"), root.join("vis-link.md")).unwrap();
    let r = |s: &str| root.join(s).to_string_lossy().into_owned();
    let b = |s: &str| base.join(s).to_string_lossy().into_owned();
    let inputs = vec![
        ("中(相対)", "a.pdf".into()),
        ("中(絶対)", r("a.pdf")),
        ("外", b("outside.md")),
        ("外の隠し", b(".hid/y.md")),
        ("隠し(相対)", ".obsidian/x.md".into()),
        ("隠し(絶対)", r(".obsidian/x.md")),
        ("隠し(相対・無い)", ".obsidian/nope.md".into()),
        ("隠しsymlinkで中(相対)", ".alias/b.md".into()),
        ("隠しsymlinkで中(絶対)", r(".alias/b.md")),
        ("symlinkで外", r("out-link.md")),
        ("symlinkで中", r("in-link.pdf")),
        ("symlinkで隠し", r("vis-link.md")),
        ("外のsymlinkで中", b("to-in.pdf")),
        ("..入り(相対)", "sub/../a.pdf".into()),
        ("..入り(絶対)", r("sub/../a.pdf")),
        ("..で外(相対)", "../../../outside.md".into()),
        ("./入り(相対)", "./a.pdf".into()),
        ("空白付き(相対)", " a.pdf".into()),
        ("存在しない", r("nope.pdf")),
        ("dir", r("sub")),
        ("末尾/(絶対)", format!("{}/", r("sub"))),
        ("末尾/(相対)", "sub/".into()),
        ("root", root.to_string_lossy().into_owned()),
    ];
    (base, root, inputs)
}

/// app の 3 箇所（wiki_open_target・wiki_ref_target・wiki_fs_plan）の同値表: 3-1 の前（直前の main）と後で同じ文字列
/// （commit 本文の手順で旧にも同じ試験を当てて確かめた）。参照フォルダはワークスペースと同じフォルダを渡す
#[test]
fn paths_sites_equivalence_table() {
    let (base, root, inputs) = paths_fixture();
    let mut out = String::new();
    for (label, s) in &inputs {
        let plan = wiki_fs_plan(&root, true, vec![PathBuf::from(s)], wiki_fs_kind, |_| false, |_, _| None, &[]);
        let rows = [
            ("wiki_open_target", format!("{:?}", wiki_open_target(&root, s, false))),
            ("wiki_ref_target", format!("{:?}", wiki_ref_target(std::slice::from_ref(&root), s, false))),
            ("wiki_fs_plan", format!("{:?}", (plan.picked, plan.full))),
        ];
        for (site, r) in rows {
            out.push_str(&format!(
                "{site} | {label} | {}\n",
                r.replace(&*base.to_string_lossy(), "<B>")
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    if std::env::var_os("PATHS_TABLE_DUMP").is_some() {
        print!("{out}");
    }
    assert_eq!(out, APP_PATHS_TABLE);
}

const APP_PATHS_TABLE: &str = r#"wiki_open_target | 中(相対) | Ok(("<B>/home/Documents/vault/a.pdf", false))
wiki_ref_target | 中(相対) | Err("絶対パスではありません: a.pdf")
wiki_fs_plan | 中(相対) | ([], false)
wiki_open_target | 中(絶対) | Ok(("<B>/home/Documents/vault/a.pdf", false))
wiki_ref_target | 中(絶対) | Ok(("<B>/home/Documents/vault", "<B>/home/Documents/vault/a.pdf"))
wiki_fs_plan | 中(絶対) | ([("a.pdf", "<B>/home/Documents/vault/a.pdf")], false)
wiki_open_target | 外 | Ok(("<B>/outside.md", false))
wiki_ref_target | 外 | Err("参照フォルダの中ではありません: <B>/outside.md")
wiki_fs_plan | 外 | ([], false)
wiki_open_target | 外の隠し | Ok(("<B>/.hid/y.md", true))
wiki_ref_target | 外の隠し | Err("参照フォルダの中ではありません: <B>/.hid/y.md")
wiki_fs_plan | 外の隠し | ([], false)
wiki_open_target | 隠し(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .obsidian/x.md")
wiki_ref_target | 隠し(相対) | Err("絶対パスではありません: .obsidian/x.md")
wiki_fs_plan | 隠し(相対) | ([], false)
wiki_open_target | 隠し(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/.obsidian/x.md")
wiki_ref_target | 隠し(絶対) | Err("参照フォルダの中ではありません: <B>/home/Documents/vault/.obsidian/x.md")
wiki_fs_plan | 隠し(絶対) | ([], false)
wiki_open_target | 隠し(相対・無い) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .obsidian/nope.md")
wiki_ref_target | 隠し(相対・無い) | Err("絶対パスではありません: .obsidian/nope.md")
wiki_fs_plan | 隠し(相対・無い) | ([], false)
wiki_open_target | 隠しsymlinkで中(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .alias/b.md")
wiki_ref_target | 隠しsymlinkで中(相対) | Err("絶対パスではありません: .alias/b.md")
wiki_fs_plan | 隠しsymlinkで中(相対) | ([], false)
wiki_open_target | 隠しsymlinkで中(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/.alias/b.md")
wiki_ref_target | 隠しsymlinkで中(絶対) | Err("参照フォルダの中ではありません: <B>/home/Documents/vault/.alias/b.md")
wiki_fs_plan | 隠しsymlinkで中(絶対) | ([], false)
wiki_open_target | symlinkで外 | Err("入力を解釈できません: ワークスペースの外を指しています（シンボリックリンク）: <B>/home/Documents/vault/out-link.md")
wiki_ref_target | symlinkで外 | Err("参照フォルダの外です（シンボリックリンク）: <B>/home/Documents/vault/out-link.md")
wiki_fs_plan | symlinkで外 | ([("out-link.md", "<B>/home/Documents/vault/out-link.md")], false)
wiki_open_target | symlinkで中 | Ok(("<B>/home/Documents/vault/a.pdf", false))
wiki_ref_target | symlinkで中 | Ok(("<B>/home/Documents/vault", "<B>/home/Documents/vault/a.pdf"))
wiki_fs_plan | symlinkで中 | ([("in-link.pdf", "<B>/home/Documents/vault/in-link.pdf")], false)
wiki_open_target | symlinkで隠し | Err("入力を解釈できません: このパスは読めません: <B>/home/Documents/vault/vis-link.md")
wiki_ref_target | symlinkで隠し | Err("参照フォルダの外です（シンボリックリンク）: <B>/home/Documents/vault/vis-link.md")
wiki_fs_plan | symlinkで隠し | ([("vis-link.md", "<B>/home/Documents/vault/vis-link.md")], false)
wiki_open_target | 外のsymlinkで中 | Ok(("<B>/home/Documents/vault/a.pdf", false))
wiki_ref_target | 外のsymlinkで中 | Err("参照フォルダの中ではありません: <B>/to-in.pdf")
wiki_fs_plan | 外のsymlinkで中 | ([], false)
wiki_open_target | ..入り(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: sub/../a.pdf")
wiki_ref_target | ..入り(相対) | Err("絶対パスではありません: sub/../a.pdf")
wiki_fs_plan | ..入り(相対) | ([], false)
wiki_open_target | ..入り(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/sub/../a.pdf")
wiki_ref_target | ..入り(絶対) | Err("絶対パスではありません: <B>/home/Documents/vault/sub/../a.pdf")
wiki_fs_plan | ..入り(絶対) | ([], false)
wiki_open_target | ..で外(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: ../../../outside.md")
wiki_ref_target | ..で外(相対) | Err("絶対パスではありません: ../../../outside.md")
wiki_fs_plan | ..で外(相対) | ([], false)
wiki_open_target | ./入り(相対) | Ok(("<B>/home/Documents/vault/a.pdf", false))
wiki_ref_target | ./入り(相対) | Err("絶対パスではありません: ./a.pdf")
wiki_fs_plan | ./入り(相対) | ([], false)
wiki_open_target | 空白付き(相対) | Ok(("<B>/home/Documents/vault/a.pdf", false))
wiki_ref_target | 空白付き(相対) | Err("絶対パスではありません:  a.pdf")
wiki_fs_plan | 空白付き(相対) | ([], false)
wiki_open_target | 存在しない | Err("not found: ワークスペースのファイル <B>/home/Documents/vault/nope.pdf")
wiki_ref_target | 存在しない | Err("見つかりません: <B>/home/Documents/vault/nope.pdf")
wiki_fs_plan | 存在しない | ([("nope.pdf", "<B>/home/Documents/vault/nope.pdf")], true)
wiki_open_target | dir | Err("通常のファイルではありません: <B>/home/Documents/vault/sub")
wiki_ref_target | dir | Err("通常のファイルではありません: <B>/home/Documents/vault/sub")
wiki_fs_plan | dir | ([("sub", "<B>/home/Documents/vault/sub")], true)
wiki_open_target | 末尾/(絶対) | Err("通常のファイルではありません: <B>/home/Documents/vault/sub")
wiki_ref_target | 末尾/(絶対) | Err("通常のファイルではありません: <B>/home/Documents/vault/sub/")
wiki_fs_plan | 末尾/(絶対) | ([("sub", "<B>/home/Documents/vault/sub/")], true)
wiki_open_target | 末尾/(相対) | Err("通常のファイルではありません: <B>/home/Documents/vault/sub")
wiki_ref_target | 末尾/(相対) | Err("絶対パスではありません: sub/")
wiki_fs_plan | 末尾/(相対) | ([], false)
wiki_open_target | root | Err("入力を解釈できません: このパスは読めません: <B>/home/Documents/vault")
wiki_ref_target | root | Err("参照フォルダの中ではありません: <B>/home/Documents/vault")
wiki_fs_plan | root | ([], true)
"#;

/// asset への遷移は中身が PDF の .pdf だけ（PDF の iframe の中のリンクで html へ行かせない）・asset 以外は今どおり
#[test]
fn nav_to_asset_only_real_pdf() {
    let base = std::env::temp_dir().join(format!(
        "openref-wiki-nav-{}-{}",
        std::process::id(),
        fastrand_u64()
    ));
    std::fs::create_dir_all(base.join("日本 語")).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    std::fs::write(base.join("日本 語/a b.pdf"), b"%PDF-1.7\n").unwrap();
    std::fs::write(
        base.join("fake.pdf"),
        b"<!doctype html><script>x()</script>",
    )
    .unwrap();
    std::fs::write(base.join("page.html"), b"<script>x()</script>").unwrap();
    std::fs::write(base.join("poly.html"), b"%PDF-<script>x()</script>").unwrap();
    std::fs::write(base.join("short.pdf"), b"%PD").unwrap();
    let nav = |u: &tauri::Url, d: Option<&tauri::Url>| wiki_nav_allowed(u, d, &|_| true);
    let asset = |p: &Path| {
        let enc: Vec<String> = p
            .to_string_lossy()
            .split('/')
            .map(|seg| {
                seg.bytes()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || b"-._~".contains(&c) {
                            (c as char).to_string()
                        } else {
                            format!("%{c:02X}")
                        }
                    })
                    .collect()
            })
            .collect();
        tauri::Url::parse(&format!("asset://localhost/{}", enc.join("/"))).unwrap()
    };
    assert!(
        nav(&asset(&base.join("日本 語/a b.pdf")), None),
        "本物の PDF（和文・空白）"
    );
    for bad in [
        "fake.pdf",
        "page.html",
        "poly.html",
        "short.pdf",
        "missing.pdf",
        "日本 語",
    ] {
        assert!(!nav(&asset(&base.join(bad)), None), "{bad}");
    }
    let u = |s: &str| tauri::Url::parse(s).unwrap();
    let page = asset(&base.join("page.html"));
    let pdf = asset(&base.join("日本 語/a b.pdf"));
    assert!(!nav(
        &u(&format!("http://asset.localhost{}", page.path())),
        None
    ));
    assert!(nav(
        &u(&format!("http://asset.localhost{}", pdf.path())),
        None
    ));
    // アプリ本体・空の iframe は通す
    for ok in [
        "tauri://localhost/",
        "tauri://localhost/index.html",
        "about:blank",
        "about:srcdoc",
    ] {
        assert!(nav(&u(ok), None), "{ok}");
    }
    // 許可リストの外（data: file: blob: 他のホストの tauri: 未知の scheme・about: の他）は止める
    let file_pdf = format!("file://{}", pdf.path().trim_start_matches('/'));
    for bad in [
        "data:text/html,<script>x()</script>",
        "data:application/pdf;base64,JVBERi0=",
        &file_pdf,
        "file:///etc/hosts",
        "blob:tauri://localhost/0b6e-1",
        "tauri://evil/",
        "javascript:alert(1)",
        "x-unknown://a/b",
        "about:config",
    ] {
        assert!(!nav(&u(bad), None), "{bad}");
    }
    // asset scope の外は中身が PDF でも止める（scope を先に確かめる）
    assert!(!wiki_nav_allowed(&pdf, None, &|_| false), "scope の外");
    // root の接頭辞（文字列だけ）: 中は可・root そのもの・隣の名前（/v2 は /v の中ではない）・外は不可
    let roots = vec![PathBuf::from("/Users/u/wiki")];
    assert!(wiki_under_roots(&roots, Path::new("/Users/u/wiki/a/b.pdf")));
    assert!(!wiki_under_roots(&roots, Path::new("/Users/u/wiki")));
    assert!(!wiki_under_roots(&roots, Path::new("/Users/u/wiki2/b.pdf")));
    assert!(!wiki_under_roots(&roots, Path::new("/net/autofs/b.pdf")));
    assert!(!wiki_under_roots(&[], Path::new("/Users/u/wiki/a.pdf")));
    assert!(
        !wiki_under_roots(&roots, Path::new("/Users/u/wiki/../secret/b.pdf")),
        ".."
    );
    assert!(
        !wiki_under_roots(&roots, Path::new("/Users/u/wiki/./b.pdf")),
        "."
    );
    assert!(
        wiki_under_roots(&roots, Path::new("/Users/u/wiki/..x/b.pdf")),
        "名前が .. で始まるだけなら可"
    );
    // asset の URL: %2E%2E は decode 後に .. として落とす（scope は通す設定でも）
    let enc =
        tauri::Url::parse("asset://localhost/%2FUsers%2Fu%2Fwiki%2F%2E%2E%2Fx.pdf").unwrap();
    assert!(!wiki_asset_is_pdf(&enc, &|_| true));
    // FIFO の x.pdf は開かない（main thread で待たない）。開いてしまえば書き手がいないので固まる = 戻ってくれば合格
    let fifo = base.join("pipe.pdf");
    let st = std::process::Command::new("/usr/bin/mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(!nav(&asset(&fifo), None), "FIFO");
    assert!(!wiki_is_pdf_file(&fifo, &|_| true), "FIFO（直接）");
    // http(s) は止める（外部・ローカルのサーバーとも）。dev の origin だけ通す（同じ origin のパスも）
    let dev = u("http://localhost:5173");
    for bad in [
        "https://pubmed.ncbi.nlm.nih.gov/1/",
        "http://127.0.0.1:8765/x",
        "http://localhost:1420/",
        "http://localhost:5174/",
        "https://localhost:5173/",
    ] {
        assert!(!nav(&u(bad), None), "{bad}");
        assert!(!nav(&u(bad), Some(&dev)), "{bad} (dev)");
    }
    assert!(nav(&u("http://localhost:5173/"), Some(&dev)));
    assert!(nav(&u("http://localhost:5173/src/main.ts?t=1"), Some(&dev)));
    assert!(
        !nav(&u("http://localhost:5173/"), None),
        "release では dev の origin も止める"
    );
    assert_eq!(percent_decode("%E6%97%A5%2"), "日%2");
    assert_eq!(percent_decode("a%zz%41"), "a%zzA");
    assert_eq!(
        percent_decode("%あ%4"),
        "%あ%4",
        "多バイト文字の前の % で落ちない"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// percent_decode の境界の値（percent_encoding に置き換えたときに旧の手書きと同値を確かめた入力の、今の出力を固定）:
/// 末尾の `%` `%2`・16 進でない・大小・壊れた UTF-8（置換文字）・`+` はそのまま・多バイト文字の前後の `%`
#[test]
fn percent_decode_golden() {
    for (c, want) in [
        ("", ""), ("%", "%"), ("%2", "%2"), ("%zz", "%zz"), ("%C3%A9", "é"), ("%c3%a9", "é"), ("%C3", "\u{fffd}"),
        ("%FF%FE", "\u{fffd}\u{fffd}"), ("%E6%97%A5%2", "日%2"), ("a%zz%41", "a%zzA"), ("%あ%4", "%あ%4"), ("+", "+"),
        ("a+b%20c", "a+b c"), ("%%41", "%A"), ("%4", "%4"), ("%41", "A"), ("x%41", "xA"), ("%2F..%2e%2E", "/...."),
        ("%00", "\0"), ("%E6%97", "\u{fffd}"), ("日本%E8%AA%9E", "日本語"), ("%G0", "%G0"), ("%0G", "%0G"), ("100%", "100%"),
        ("%%", "%%"), ("%%%", "%%%"), ("%é1", "%é1"), ("%1é", "%1é"),
    ] {
        assert_eq!(percent_decode(c), want, "{c:?}");
    }
}

/// percent_decode の性質（乱数）: `%` を含まない文字列はそのまま・全バイトを `%XX`（大小の混ざった 16 進）にした文字列は元に戻る・
/// 16 進 2 桁が続かない `%` は字のまま残る
#[test]
fn percent_decode_properties() {
    let alphabet: Vec<char> = "0123456789abcdefABCDEFgGz+/. 日é語\u{1f600}".chars().collect();
    let mut x = fastrand_u64() | 1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..5_000 {
        let n = next() % 12;
        let s: String = (0..n).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
        assert_eq!(percent_decode(&s), s, "% なし: {s:?}");
        let enc: String = s
            .bytes()
            .map(|b| if next() % 2 == 0 { format!("%{b:02X}") } else { format!("%{b:02x}") })
            .collect();
        assert_eq!(percent_decode(&enc), s, "{enc:?}");
        let lone = format!("%g{s}");
        assert!(percent_decode(&lone).starts_with("%g"), "{lone:?}");
    }
}

/// percent_decode の性質（乱数・`%` と 16 進と普通の字が混ざる形）: 入力を「期待の出力が分かっている区画」の並びで作る =
/// 普通の字（`%` 以外）はそのまま・`%HH`（16 進 2 桁・大小は任意）は 1 バイトに解ける（多バイトの字は全バイトを `%HH` にして元の字）・
/// `%` + 16 進でない字・`%H` + 16 進でない字は字のまま・末尾の不完全な `%`・`%H` も字のまま。区画の継ぎ目で新しい `%HH` はできない
/// （字のまま残る区画は 16 進でない字で終わる・末尾の区画は最後だけ）
#[test]
fn percent_decode_mixed_properties() {
    let hex = b"0123456789abcdefABCDEF";
    let plain: Vec<char> = "0123456789abcdefABCDEFgGz+/. 日é\u{1f600}".chars().collect();
    let non_hex: Vec<char> = "gGz+/. 日é".chars().collect();
    let multi: Vec<char> = "日é語\u{1f600}".chars().collect();
    let mut x = fastrand_u64() | 1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..20_000 {
        let (mut input, mut want) = (String::new(), String::new());
        for _ in 0..next() % 8 {
            match next() % 5 {
                0 => {
                    let c = plain[(next() % plain.len() as u64) as usize];
                    input.push(c);
                    want.push(c);
                }
                1 => {
                    // ASCII 1 バイト（`%` 自身の 0x25 も含む）を %HH に
                    let b = 0x20 + (next() % 0x5f) as u8;
                    input += &if next() % 2 == 0 { format!("%{b:02X}") } else { format!("%{b:02x}") };
                    want.push(b as char);
                }
                2 => {
                    let c = multi[(next() % multi.len() as u64) as usize];
                    let mut buf = [0u8; 4];
                    for b in c.encode_utf8(&mut buf).bytes() {
                        input += &if next() % 2 == 0 { format!("%{b:02X}") } else { format!("%{b:02x}") };
                    }
                    want.push(c);
                }
                3 => {
                    let g = non_hex[(next() % non_hex.len() as u64) as usize];
                    input += &format!("%{g}");
                    want += &format!("%{g}");
                }
                _ => {
                    let h = hex[(next() % hex.len() as u64) as usize] as char;
                    let g = non_hex[(next() % non_hex.len() as u64) as usize];
                    input += &format!("%{h}{g}");
                    want += &format!("%{h}{g}");
                }
            }
        }
        match next() % 3 {
            0 => {}
            1 => {
                input.push('%');
                want.push('%');
            }
            _ => {
                let h = hex[(next() % hex.len() as u64) as usize] as char;
                input += &format!("%{h}");
                want += &format!("%{h}");
            }
        }
        assert_eq!(percent_decode(&input), want, "{input:?}");
    }
}

/// has_open_with_xattr は libc の getxattr: 実ファイルに属性を付けて確かめる（OpenWith あり = true・quarantine だけ /
/// 属性なし = ENOATTR = false・無いファイル = true）。libc の定数は旧の自前の値と同じ
#[test]
fn open_with_xattr_on_real_files() {
    assert_eq!(libc::ENOATTR, 93, "旧 = sys/errno.h の 93");
    assert_eq!(libc::O_NONBLOCK, 0x0004, "旧 = sys/fcntl.h の 0x0004");
    let base = std::env::temp_dir().join(format!("openref-xattr-{}-{}", std::process::id(), fastrand_u64()));
    std::fs::create_dir_all(&base).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    let (plain, quar, ow) = (base.join("plain.pdf"), base.join("quar.pdf"), base.join("ow.pdf"));
    for p in [&plain, &quar, &ow] {
        std::fs::write(p, "%PDF-1.4").unwrap();
    }
    let xattr = |name: &str, p: &Path| {
        let st = std::process::Command::new("/usr/bin/xattr")
            .args(["-w", name, "0081;00000000;Safari;"])
            .arg(p)
            .status()
            .unwrap();
        assert!(st.success(), "xattr -w {name}");
    };
    xattr("com.apple.quarantine", &quar);
    xattr("com.apple.LaunchServices.OpenWith", &ow);
    assert!(!has_open_with_xattr(&plain), "属性なし");
    assert!(!has_open_with_xattr(&quar), "quarantine だけ = OpenWith は無い");
    assert!(has_open_with_xattr(&ow), "OpenWith あり");
    assert!(has_open_with_xattr(&base.join("missing.pdf")), "ENOENT は有る側");
    // open_target: OpenWith 付きは拡張子が pdf でも Finder 表示・quarantine だけは既定のアプリで開く
    let root = base.join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let s = |p: &Path| p.to_string_lossy().into_owned();
    assert_eq!(wiki_open_target(&root, &s(&ow), false).unwrap(), (ow.clone(), true));
    assert_eq!(wiki_open_target(&root, &s(&quar), false).unwrap(), (quar.clone(), false));
    let _ = std::fs::remove_dir_all(&base);
}

/// 参照フォルダの html の欄（core の body_fields に寄せたときに旧の組立てと同値を確かめた入力の、今の出力を固定）:
/// 題あり / 空白だけ（名前のまま）/ 無し・壊れた UTF-8（置換文字で数える）・max_chars で切る（truncated）・links は空
#[test]
fn ref_html_fields_golden() {
    use openref_core::wiki::scan::Kind;
    use serde_json::json;
    let page = &b"<html><head><title>T 1</title></head><body><p>\xe6\x9c\xac\xe6\x96\x87</p></body></html>"[..];
    let page_s = "<html><head><title>T 1</title></head><body><p>本文</p></body></html>";
    let broken = &b"\xff\xfe<title>t</title>\xe6\x97"[..];
    let broken_s = "\u{fffd}\u{fffd}<title>t</title>\u{fffd}";
    let f = |title: &str, chars: usize, content: &str, truncated: bool| {
        json!({"title": title, "chars": chars, "content": content, "truncated": truncated, "links": []})
    };
    for (h, max, want) in [
        (page, 200_000, f("T 1", 66, page_s, false)),
        (page, 3, f("T 1", 66, "<ht", true)),
        (&b"<title>  </title><p>x</p>"[..], 200_000, f("a.html", 25, "<title>  </title><p>x</p>", false)),
        (&b"<p>no title</p>"[..], 0, f("a.html", 15, "", true)),
        (&b""[..], 1, f("a.html", 0, "", false)),
        (broken, 200_000, f("t", 19, broken_s, false)),
        (broken, 5, f("t", 19, "\u{fffd}\u{fffd}<ti", true)),
    ] {
        let mut got = json!({"title": "a.html"});
        wiki_ref_html_fields(&mut got, Kind::Html, h, max);
        assert_eq!(got, want, "{:?} max {max}", String::from_utf8_lossy(h));
    }
}

fn fastrand_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

fn plan(
    root_is_dir: bool,
    paths: &[&str],
    dirs: &[&str],
    gone: &[&str],
    indexed: &[&str],
) -> FsPlan {
    let root = Path::new("/v");
    let kind = |p: &Path| {
        let s = p.to_string_lossy();
        if dirs.iter().any(|d| *d == s) {
            FsKind::Dir
        } else if gone.iter().any(|g| *g == s) {
            FsKind::Missing
        } else {
            FsKind::File
        }
    };
    wiki_fs_plan(
        root,
        root_is_dir,
        paths.iter().map(PathBuf::from).collect(),
        kind,
        |rel| indexed.contains(&rel),
        |_, _| None,
        &[],
    )
}
fn rels(p: &FsPlan) -> Vec<&str> {
    p.picked.iter().map(|(r, _)| r.as_str()).collect()
}

#[test]
fn fs_plan_filters_hidden_outside_root_and_dupes() {
    let p = plan(
        true,
        &[
            "/v/a.md",
            "/v/.obsidian/w.json",
            "/v/sub/.git/HEAD",
            "/other/x.md",
            "/v/a.md",
            "/vx/a.md",
        ],
        &[],
        &[],
        &[],
    );
    assert_eq!(rels(&p), vec!["a.md"]);
    assert!(!p.full && !p.root_gone);
    // 隠しだけ = 何も取り込まない
    assert!(plan(true, &["/v/.obsidian/a"], &[], &[], &[])
        .picked
        .is_empty());
    // 書込（wiki_save / wiki_write）の一時ファイル（`.openref-<pid>-<乱数>.tmp`）は拾わない = 書いた本体だけ
    assert_eq!(
        rels(&plan(
            true,
            &["/v/sub/.openref-1-00ff.tmp", "/v/sub/a.md"],
            &[],
            &["/v/sub/.openref-1-00ff.tmp"],
            &[]
        )),
        vec!["sub/a.md"]
    );
}

/// root そのもののパス（`/v`・`/v/`）= 全走査（フォルダの丸ごとの差し替え・MustScanSubDirs は root のパス 1 件で届く）。
/// root が消えていれば全走査でなく root_gone
#[test]
fn fs_plan_full_scan_for_root_path() {
    for paths in [&["/v"][..], &["/v/"], &["/v", "/v/"], &["/v/.obsidian/w.json", "/v"]] {
        let p = plan(true, paths, &["/v", "/v/"], &[], &[]);
        assert!(p.full && !p.root_gone, "{paths:?}");
        assert!(p.picked.is_empty(), "{paths:?}");
    }
    let p = plan(true, &["/v", "/v/a.md"], &["/v"], &[], &[]);
    assert!(p.full);
    assert_eq!(rels(&p), vec!["a.md"]);
    let gone = plan(false, &["/v"], &[], &[], &[]);
    assert!(gone.root_gone && !gone.full);
}

#[test]
fn fs_plan_switches_to_full_scan_for_dirs_and_vanished_unknown_paths() {
    assert!(
        plan(
            true,
            &["/v/a.md", "/v/new-folder"],
            &["/v/new-folder"],
            &[],
            &[]
        )
        .full,
        "フォルダの作成・改名"
    );
    assert!(
        plan(true, &["/v/old-folder"], &[], &["/v/old-folder"], &[]).full,
        "消えたフォルダ（索引に無い）"
    );
    let del = plan(true, &["/v/a.md"], &[], &["/v/a.md"], &["a.md"]);
    assert!(!del.full, "索引にあるファイルが消えた = そのパスだけ");
    assert_eq!(rels(&del), vec!["a.md"]);
}

#[test]
fn fs_plan_reports_root_gone() {
    let p = plan(
        false,
        &["/v", "/v/a.md"],
        &[],
        &["/v", "/v/a.md"],
        &["a.md"],
    );
    assert!(p.root_gone);
    assert!(
        p.picked.is_empty() && !p.full,
        "root が無いときは取り込まない（全部消えたことにしない）"
    );
}

#[cfg(unix)]
fn is_root_user() -> bool {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() == 0 }
}

/// payload の serde の形（画面の types.ts の WikiChanged と同じ鍵・all のとき配列は空）
#[test]
fn wiki_changed_serializes_as_contract() {
    use serde_json::json;
    assert_eq!(
        serde_json::to_value(WikiChanged::everything()).unwrap(),
        json!({"all": true, "added": [], "removed": [], "modified": []})
    );
    let c = WikiChanged::from(openref_core::wiki::index::Refreshed {
        added: vec!["n.md".into()],
        modified: vec!["a.md".into(), "b.md".into()],
        removed: vec!["g.md".into()],
    });
    assert_eq!(
        serde_json::to_value(&c).unwrap(),
        json!({"all": false, "added": ["n.md"], "removed": ["g.md"], "modified": ["a.md", "b.md"]})
    );
    assert!(!c.is_empty() && WikiChanged::default().is_empty() && !WikiChanged::everything().is_empty());
}

/// 仕様の固定（契約コメント :1731 と同じ）: core が同期で索引に入れた書込（wiki_write = edit::write_agent・GUI の保存も同じ reindex）は、
/// 監視が拾う時点で索引にある = 新しいページでも added ではなく modified で届く。画面側（reloadPlan の known）が増として扱う
#[test]
fn fs_ingest_reports_core_written_new_page_as_modified() {
    use openref_core::wiki::{edit, with_index, Ctx};
    let base = std::env::temp_dir().join(format!(
        "openref-app-fs-ingest-core-{}-{}",
        std::process::id(),
        fastrand_u64()
    ));
    let home = base.join("home");
    let root = home.join("Documents/vault");
    let app_dir = home.join("scratch-app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&app_dir).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    let home = base.join("home");
    let root = home.join("Documents/vault");
    let app_dir = home.join("scratch-app");
    let ctx = Ctx {
        enabled: true,
        root: Some(root.to_string_lossy().into()),
        app_dir: Some(app_dir.clone()),
        home: Some(home.clone()),
        watched: false,
        ..Default::default()
    };
    let at = ctx.ready().unwrap();
    let root = at.root.clone();
    with_index(&at, |ix| ix.refresh().map(|_| ())).unwrap();
    edit::write_agent(&ctx, &serde_json::json!({"path": "agent.md", "content": "# A\n"})).unwrap();
    let out = with_index(&at, |ix| {
        wiki_fs_ingest(ix, &root, vec![root.join("agent.md")], wiki_fs_kind)
    })
    .unwrap();
    assert_eq!(
        out,
        FsOutcome::Changed(WikiChanged {
            all: false,
            added: vec![],
            removed: vec![],
            modified: vec!["agent.md".into()],
        }),
        "core が先に索引へ入れた新規ページは監視では modified"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// 除外フォルダ: 配下のイベントは全走査の判定の前に捨てる・記録済みの除外フォルダそのものも捨てる・
/// 記録に無い（新しくできた）除外フォルダはフォルダのイベント = 全走査・消えた記録済みの除外フォルダも全走査・除外の外は今どおり
#[test]
fn fs_plan_drops_events_under_excluded_folders() {
    let root = Path::new("/v");
    // 除外 = 名前 node_modules・target（CACHEDIR.TAG の代わり）・proj/testdata（exclude）。上から辿って最初の物
    let ex = |rel: &str, self_is_dir: bool| -> Option<String> {
        let segs: Vec<&str> = rel.split('/').collect();
        let n = if self_is_dir { segs.len() } else { segs.len() - 1 };
        for i in 0..n {
            let cur = segs[..=i].join("/");
            if segs[i] == "node_modules" || cur == "target" || cur == "proj/testdata" {
                return Some(cur);
            }
        }
        None
    };
    let known = ["app/node_modules".to_string(), "target".into()];
    let run = |paths: &[&str], dirs: &[&str], gone: &[&str], indexed: &[&str]| {
        let kind = |p: &Path| {
            let s = p.to_string_lossy();
            if dirs.iter().any(|d| *d == s) {
                FsKind::Dir
            } else if gone.iter().any(|g| *g == s) {
                FsKind::Missing
            } else {
                FsKind::File
            }
        };
        wiki_fs_plan(root, true, paths.iter().map(PathBuf::from).collect(), kind, |rel| indexed.contains(&rel), ex, &known)
    };
    // cargo build: target の中のファイル・フォルダ・target そのもの（記録済み）= 全部捨てる = 全走査にならない
    let p = run(&["/v/target/debug/a.o", "/v/target/debug/deps", "/v/target", "/v/target/debug/gone.o"], &["/v/target/debug/deps", "/v/target"], &["/v/target/debug/gone.o"], &[]);
    assert!(p.picked.is_empty() && !p.full, "{p:?}");
    // npm install: node_modules の中の新しいフォルダ・消えたファイル（索引に無い）も捨てる
    let p = run(&["/v/app/node_modules/x/y.js", "/v/app/node_modules/x", "/v/app/node_modules/z.js"], &["/v/app/node_modules/x"], &["/v/app/node_modules/z.js"], &[]);
    assert!(p.picked.is_empty() && !p.full, "{p:?}");
    // exclude の中も同じ
    let p = run(&["/v/proj/testdata/1.dcm", "/v/proj/testdata/new"], &["/v/proj/testdata/new"], &[], &[]);
    assert!(p.picked.is_empty() && !p.full, "{p:?}");
    // 除外の外のファイルは今どおり取り込む（除外の配下と混ざっても）
    let p = run(&["/v/target/x.o", "/v/notes/a.md"], &[], &[], &[]);
    assert_eq!(rels(&p), ["notes/a.md"]);
    assert!(!p.full);
    // 記録に無い除外フォルダ（新しくできた node_modules）= フォルダのイベント = 全走査（記録に載せる）
    let p = run(&["/v/web/node_modules", "/v/web/node_modules/a.js"], &["/v/web/node_modules"], &[], &[]);
    assert!(p.full, "{p:?}");
    assert_eq!(rels(&p), ["web/node_modules"]);
    // 記録済みの除外フォルダが消えた（cargo clean）= 索引に無いパスが消えた = 全走査
    let p = run(&["/v/target", "/v/target/debug/a.o"], &[], &["/v/target", "/v/target/debug/a.o"], &[]);
    assert!(p.full, "{p:?}");
    assert_eq!(rels(&p), ["target"]);
    // 普通のフォルダのイベントは今どおり全走査
    assert!(run(&["/v/target/a.o", "/v/notes/new"], &["/v/notes/new"], &[], &[]).full);
}

/// 実の索引と実のフォルダで: 除外フォルダ（署名つき CACHEDIR.TAG・node_modules）の中でファイルとフォルダを作って消しても
/// 取り込まず（Nothing）・全走査もしない（meta last_scan が変わらない）。除外の外は今どおり取り込む
#[test]
fn fs_ingest_ignores_excluded_folders() {
    let base = std::env::temp_dir().join(format!("openref-app-fs-excl-{}-{}", std::process::id(), fastrand_u64()));
    let root = base.join("vault");
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::create_dir_all(root.join("web/node_modules/pkg")).unwrap();
    std::fs::create_dir_all(root.join("proj/testdata")).unwrap();
    std::fs::write(root.join("target/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
    std::fs::write(root.join("a.md"), "# A\n").unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let ip = base.join("ix/wiki-index.sqlite");
    // 利用者の exclude（proj/testdata）も索引の規則（Index::rules）で効く
    let rules = openref_core::wiki::scan::Rules::new([], ["proj/testdata".to_string()]);
    let mut ix = openref_core::wiki::Index::open(&openref_core::wiki::index::Ready::new(&ip, &root, rules)).unwrap();
    ix.refresh().unwrap();
    assert_eq!(ix.excluded().unwrap(), ["proj/testdata", "target", "web/node_modules"]);
    ix.refresh().unwrap();
    let last = ix.meta("last_scan").unwrap();
    // 作る
    std::fs::create_dir_all(root.join("target/debug/deps")).unwrap();
    std::fs::write(root.join("target/debug/deps/x.md"), "x").unwrap();
    std::fs::write(root.join("web/node_modules/pkg/r.md"), "r").unwrap();
    std::fs::create_dir_all(root.join("web/node_modules/new")).unwrap();
    std::fs::write(root.join("proj/testdata/1.md"), "d").unwrap();
    std::fs::create_dir_all(root.join("proj/testdata/newdir")).unwrap();
    let made = [
        root.join("proj/testdata/1.md"),
        root.join("proj/testdata/newdir"),
        root.join("target/debug/deps"),
        root.join("target/debug/deps/x.md"),
        root.join("target/debug"),
        root.join("target"),
        root.join("web/node_modules/pkg/r.md"),
        root.join("web/node_modules/new"),
        root.join("web/node_modules"),
    ];
    assert_eq!(wiki_fs_ingest(&mut ix, &root, made.to_vec(), wiki_fs_kind).unwrap(), FsOutcome::Nothing);
    // 消す
    std::fs::remove_dir_all(root.join("target/debug/deps")).unwrap();
    std::fs::remove_dir_all(root.join("web/node_modules/new")).unwrap();
    std::fs::remove_file(root.join("web/node_modules/pkg/r.md")).unwrap();
    assert_eq!(wiki_fs_ingest(&mut ix, &root, made.to_vec(), wiki_fs_kind).unwrap(), FsOutcome::Nothing);
    assert_eq!(ix.meta("last_scan").unwrap(), last, "全走査していない");
    assert_eq!(ix.all_paths().unwrap(), ["a.md"]);
    // 除外の外は今どおり
    std::fs::write(root.join("b.md"), "# B\n").unwrap();
    assert!(matches!(wiki_fs_ingest(&mut ix, &root, vec![root.join("b.md"), root.join("target/x")], wiki_fs_kind).unwrap(), FsOutcome::Changed(c) if c.added == ["b.md"]));
    let _ = std::fs::remove_dir_all(&base);
}

/// 規則と索引のずれ（CACHEDIR.TAG）: 索引に入っているフォルダに後から署名つき CACHEDIR.TAG ができ、同じバッチで中の索引にあるページが
/// 消える / 変わる = 捨てずに全走査 → そのページが索引から外れ、meta excluded にフォルダが載る。CACHEDIR.TAG を消すと（記録と食い違う）
/// 全走査で戻る。除外の配下で索引に無いファイルのイベントは今どおり捨てる（全走査にしない）
#[test]
fn fs_ingest_full_scan_when_rules_drift_from_index() {
    let base = std::env::temp_dir().join(format!("openref-app-fs-drift-{}-{}", std::process::id(), fastrand_u64()));
    let root = base.join("vault");
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
    std::fs::write(root.join("notes/a.md"), "# A\n").unwrap();
    std::fs::write(root.join("notes/b.md"), "# B\n").unwrap();
    std::fs::write(root.join("top.md"), "# T\n").unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let ip = base.join("ix/wiki-index.sqlite");
    let mut ix = openref_core::wiki::Index::open(&openref_core::wiki::index::Ready::new(&ip, &root, Default::default())).unwrap();
    ix.refresh().unwrap();
    assert_eq!(ix.excluded().unwrap(), ["target"]);
    assert!(ix.page("notes/a.md").unwrap().is_some());
    // 除外の配下で索引に無いファイル = 捨てる（全走査にしない）
    let last = ix.meta("last_scan").unwrap();
    std::fs::write(root.join("target/x.o"), "o").unwrap();
    assert_eq!(wiki_fs_ingest(&mut ix, &root, vec![root.join("target/x.o")], wiki_fs_kind).unwrap(), FsOutcome::Nothing);
    assert_eq!(ix.meta("last_scan").unwrap(), last);
    // notes/CACHEDIR.TAG の作成と notes/a.md の削除が同じバッチ = 全走査
    std::fs::write(root.join("notes/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
    std::fs::remove_file(root.join("notes/a.md")).unwrap();
    let out = wiki_fs_ingest(&mut ix, &root, vec![root.join("notes/CACHEDIR.TAG"), root.join("notes/a.md")], wiki_fs_kind).unwrap();
    assert_eq!(out, FsOutcome::Changed(WikiChanged::everything()));
    assert!(ix.page("notes/a.md").unwrap().is_none() && ix.page("notes/b.md").unwrap().is_none(), "notes の中は索引から外れる");
    assert_eq!(ix.excluded().unwrap(), ["notes", "target"]);
    // 記録どおりの CACHEDIR.TAG の変更（中身を書き直しただけ）は捨てる
    let last = ix.meta("last_scan").unwrap();
    std::fs::write(root.join("notes/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# again\n").unwrap();
    assert_eq!(wiki_fs_ingest(&mut ix, &root, vec![root.join("notes/CACHEDIR.TAG")], wiki_fs_kind).unwrap(), FsOutcome::Nothing);
    assert_eq!(ix.meta("last_scan").unwrap(), last);
    // CACHEDIR.TAG を消す = 親の除外の状態が記録と食い違う = 全走査で戻る
    std::fs::remove_file(root.join("notes/CACHEDIR.TAG")).unwrap();
    let out = wiki_fs_ingest(&mut ix, &root, vec![root.join("notes/CACHEDIR.TAG")], wiki_fs_kind).unwrap();
    assert_eq!(out, FsOutcome::Changed(WikiChanged::everything()));
    assert!(ix.page("notes/b.md").unwrap().is_some());
    assert_eq!(ix.excluded().unwrap(), ["target"]);
    // 署名つき CACHEDIR.TAG ができ、同じバッチで中の索引にあるページが変わった（消えずに）でも全走査
    std::fs::write(root.join("notes/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
    std::fs::write(root.join("notes/b.md"), "# B2\n").unwrap();
    let out = wiki_fs_ingest(&mut ix, &root, vec![root.join("notes/b.md")], wiki_fs_kind).unwrap();
    assert_eq!(out, FsOutcome::Changed(WikiChanged::everything()), "索引にあるページ = 捨てずに全走査");
    assert!(ix.page("notes/b.md").unwrap().is_none());
    // CACHEDIR.TAG の作成だけのバッチ（中のページのイベントが来ない）でも、親の除外の状態が記録と食い違えば全走査
    std::fs::remove_file(root.join("notes/CACHEDIR.TAG")).unwrap();
    wiki_fs_ingest(&mut ix, &root, vec![root.join("notes/CACHEDIR.TAG")], wiki_fs_kind).unwrap();
    assert!(ix.page("notes/b.md").unwrap().is_some() && ix.excluded().unwrap() == ["target"]);
    std::fs::write(root.join("notes/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
    let out = wiki_fs_ingest(&mut ix, &root, vec![root.join("notes/CACHEDIR.TAG")], wiki_fs_kind).unwrap();
    assert_eq!(out, FsOutcome::Changed(WikiChanged::everything()), "CACHEDIR.TAG の作成だけ = 記録と食い違う = 全走査");
    assert!(ix.page("notes/b.md").unwrap().is_none());
    assert_eq!(ix.excluded().unwrap(), ["notes", "target"]);
    let _ = std::fs::remove_dir_all(&base);
}

/// 規則と索引のずれ（CACHEDIR.TAG の無い除外 = exclude リスト）: 索引にある x/sub/a.md → exclude に x/sub を足した規則で
/// そのページのイベント = 1 回目は全走査（外れて meta excluded に載る）・2 回目は Nothing（1 回で収束）
#[test]
fn fs_ingest_full_scan_when_exclude_list_drifts_from_index() {
    let base = std::env::temp_dir().join(format!("openref-app-fs-drift-list-{}-{}", std::process::id(), fastrand_u64()));
    let root = base.join("vault");
    std::fs::create_dir_all(root.join("x/sub")).unwrap();
    std::fs::write(root.join("x/sub/a.md"), "# A\n").unwrap();
    std::fs::write(root.join("x/b.md"), "# B\n").unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let ip = base.join("ix/wiki-index.sqlite");
    let ready = |rules| openref_core::wiki::index::Ready::new(&ip, &root, rules);
    {
        let mut ix = openref_core::wiki::Index::open(&ready(Default::default())).unwrap();
        ix.refresh().unwrap();
        assert!(ix.page("x/sub/a.md").unwrap().is_some());
        assert!(ix.excluded().unwrap().is_empty());
    }
    // 設定に exclude: ["x/sub"] を足した後の規則（監視のバッチが読む規則 = Index::rules）
    let rules = openref_core::wiki::scan::Rules::new([], ["x/sub".to_string()]);
    let mut ix = openref_core::wiki::Index::open(&ready(rules)).unwrap();
    std::fs::write(root.join("x/sub/a.md"), "# A2\n").unwrap();
    let first = wiki_fs_ingest(&mut ix, &root, vec![root.join("x/sub/a.md")], wiki_fs_kind).unwrap();
    assert_eq!(first, FsOutcome::Changed(WikiChanged::everything()), "索引にあるページ = 規則とのずれ = 全走査");
    assert!(ix.page("x/sub/a.md").unwrap().is_none());
    assert!(ix.page("x/b.md").unwrap().is_some());
    assert_eq!(ix.excluded().unwrap(), ["x/sub"]);
    let last = ix.meta("last_scan").unwrap();
    std::fs::write(root.join("x/sub/a.md"), "# A3\n").unwrap();
    let second = wiki_fs_ingest(&mut ix, &root, vec![root.join("x/sub/a.md")], wiki_fs_kind).unwrap();
    assert_eq!(second, FsOutcome::Nothing, "1 回で収束（索引に無くなった = 捨てる）");
    assert_eq!(ix.meta("last_scan").unwrap(), last);
    let _ = std::fs::remove_dir_all(&base);
}

/// wiki_exclude_set の dir の検査: vault 相対（前後の `/`・NFD は NFC に）・root / `.` 始まり / `..` / 無い / ファイル / symlink / 途中の symlink は拒否
#[test]
fn exclude_dir_check_rules() {
    let base = std::env::temp_dir().join(format!("openref-app-excl-dir-{}-{}", std::process::id(), fastrand_u64()));
    let root = base.join("vault");
    std::fs::create_dir_all(root.join("proj/testdata")).unwrap();
    std::fs::create_dir_all(root.join("データ/sub")).unwrap();
    std::fs::create_dir_all(root.join(".git/x")).unwrap();
    std::fs::write(root.join("proj/f.md"), "x").unwrap();
    std::os::unix::fs::symlink(root.join("proj"), root.join("link")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let ok = |d: &str| wiki_exclude_dir_check(&root, d, false).map(|(r, _)| r);
    assert_eq!(ok("proj/testdata").unwrap(), "proj/testdata");
    assert_eq!(ok("/proj/testdata/").unwrap(), "proj/testdata");
    // NFD（デ = テ U+30C6 + 濁点 U+3099）で渡しても NFC の vault 相対に（NFC と NFD で字が変わる名前でないと確かめられない）
    let nfd: String = "\u{30c6}\u{3099}\u{30fc}\u{30bf}/sub".to_string();
    assert_ne!(nfd, "データ/sub");
    assert_eq!(ok(&nfd).unwrap(), "データ/sub");
    assert_eq!(wiki_exclude_dir_check(&root, "proj/testdata", false).unwrap().1, root.join("proj/testdata"));
    for (d, why) in [
        ("", "root"),
        ("/", "root"),
        (".git", "`.`"),
        (".git/x", "`.`"),
        ("proj/../proj", "`..`"),
        ("nope", "見つかりません"),
        ("proj/f.md", "フォルダではありません"),
        ("link", "フォルダではありません"),
        ("link/testdata", "シンボリックリンク"),
    ] {
        let e = ok(d).unwrap_err();
        assert!(e.contains(why), "{d}: {e}");
    }
    // 文は理由ごとに前と同じ字（ja / en・末尾に渡された dir）= 検査を core の paths::exclude_dir へ移しても変わらない
    for (d, ja, en) in [
        ("/", "ワークスペースの root は索引から除けません", "The workspace root cannot be excluded from the index"),
        (".git", "`.` 始まりの区画・`..` を含むフォルダは指定できません", "Folders with `.`-prefixed or `..` segments cannot be used"),
        ("proj/f.md", "フォルダではありません（シンボリックリンクは除けません）", "Not a folder (symlinks cannot be excluded)"),
        ("nope", "フォルダが見つかりません", "Folder not found"),
        ("link/testdata", "途中にシンボリックリンクがあるフォルダは指定できません", "Folders reached through a symlink cannot be used"),
    ] {
        assert_eq!(wiki_exclude_dir_check(&root, d, false).unwrap_err(), format!("{ja}: {d}"));
        assert_eq!(wiki_exclude_dir_check(&root, d, true).unwrap_err(), format!("{en}: {d}"));
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// "wiki-changed" の種別は core の取り込み結果から: 新規 = added / 上書き = modified / 削除 = removed / フォルダの作成（全走査）= all。
/// 実データの一時フォルダと実の索引（Index::open）で。取り込むものが無い・root が無い・stat 拒否の据え置きも固定
#[test]
fn fs_ingest_reports_kinds_from_index() {
    let base = std::env::temp_dir().join(format!(
        "openref-app-fs-ingest-{}-{}",
        std::process::id(),
        fastrand_u64()
    ));
    let root = base.join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let ip = base.join("ix/wiki-index.sqlite");
    std::fs::write(root.join("a.md"), "# A\n").unwrap();
    std::fs::write(root.join("b.md"), "# B\n").unwrap();
    let mut ix = openref_core::wiki::Index::open(&openref_core::wiki::index::Ready::new(&ip, &root, Default::default())).unwrap();
    ix.refresh().unwrap();
    let ingest = |ix: &mut openref_core::wiki::Index, paths: &[PathBuf]| {
        wiki_fs_ingest(ix, &root, paths.to_vec(), wiki_fs_kind).unwrap()
    };
    let changed = |added: &[&str], removed: &[&str], modified: &[&str]| {
        FsOutcome::Changed(WikiChanged {
            all: false,
            added: added.iter().map(|s| s.to_string()).collect(),
            removed: removed.iter().map(|s| s.to_string()).collect(),
            modified: modified.iter().map(|s| s.to_string()).collect(),
        })
    };
    // 新規
    std::fs::write(root.join("new.md"), "# N\n").unwrap();
    assert_eq!(
        ingest(&mut ix, &[root.join("new.md")]),
        changed(&["new.md"], &[], &[])
    );
    // 上書き（同じ内容を書いても「読み直した」= modified）
    std::fs::write(root.join("a.md"), "# A2\n").unwrap();
    assert_eq!(
        ingest(&mut ix, &[root.join("a.md")]),
        changed(&[], &[], &["a.md"])
    );
    // 削除（索引にあるファイル = そのパスだけ・全走査にしない）
    std::fs::remove_file(root.join("b.md")).unwrap();
    assert_eq!(
        ingest(&mut ix, &[root.join("b.md")]),
        changed(&[], &["b.md"], &[])
    );
    assert!(ix.page("b.md").unwrap().is_none());
    // 混在 = 1 つの payload に種別ごと（届いた順・重複なし）
    std::fs::write(root.join("c.md"), "# C\n").unwrap();
    std::fs::remove_file(root.join("new.md")).unwrap();
    assert_eq!(
        ingest(
            &mut ix,
            &[
                root.join("a.md"),
                root.join("c.md"),
                root.join("new.md"),
                root.join("a.md")
            ]
        ),
        changed(&["c.md"], &["new.md"], &["a.md"])
    );
    // フォルダの作成 = 全走査 = all（配列は空 = 画面は全部として扱う）。中のファイルは走査で索引に入る
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/d.md"), "# D\n").unwrap();
    assert_eq!(
        ingest(&mut ix, &[root.join("sub")]),
        FsOutcome::Changed(WikiChanged::everything())
    );
    assert!(ix.page("sub/d.md").unwrap().is_some());
    // root そのもののパスだけ（フォルダの丸ごとの差し替え・MustScanSubDirs）= 全走査 = all。個別に来ないファイルも索引に入る
    std::fs::write(root.join("e.md"), "# E\n").unwrap();
    std::fs::remove_file(root.join("c.md")).unwrap();
    assert_eq!(
        ingest(&mut ix, std::slice::from_ref(&root)),
        FsOutcome::Changed(WikiChanged::everything())
    );
    assert!(ix.page("e.md").unwrap().is_some() && ix.page("c.md").unwrap().is_none());
    // 隠しだけ = 取り込むものが無い = 知らせない（索引に無いパスが消えた = フォルダが消えたかもしれない = all は上の規則）
    assert_eq!(
        ingest(&mut ix, &[root.join(".obsidian/w.json")]),
        FsOutcome::Nothing
    );
    // stat が拒否された既知のページ（フォルダの権限 = 1-2 の据え置き）= 索引は何も変えない = 知らせない（消えたことにしない）
    #[cfg(unix)]
    if !is_root_user() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(root.join("locked")).unwrap();
        std::fs::write(root.join("locked/x.md"), "# X\n").unwrap();
        assert_eq!(
            ingest(&mut ix, &[root.join("locked/x.md")]),
            changed(&["locked/x.md"], &[], &[])
        );
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = ingest(&mut ix, &[root.join("locked/x.md")]);
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r, FsOutcome::Nothing, "stat 拒否の据え置き");
        assert!(ix.page("locked/x.md").unwrap().is_some(), "据え置き = 索引に残る");
    }
    // root が消えた
    let gone = base.join("gone");
    assert_eq!(
        wiki_fs_ingest(&mut ix, &gone, vec![gone.join("a.md")], wiki_fs_kind).unwrap(),
        FsOutcome::RootGone
    );
    drop(ix);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn enable_step_picks_only_when_needed_and_cancel_restores() {
    let mut called = false;
    assert_eq!(
        wiki_enable_step(true, || {
            called = true;
            None
        }),
        EnableStep::Go(None)
    );
    assert!(!called, "root が使えるときはダイアログを出さない");
    assert_eq!(
        wiki_enable_step(false, || None),
        EnableStep::Cancel,
        "取消 = チェックを戻す"
    );
    assert_eq!(
        wiki_enable_step(false, || Some(PathBuf::from("/v"))),
        EnableStep::Go(Some(PathBuf::from("/v")))
    );
}

// ---------- AppHandle 無しの段取りを tempdir の DB と偽の app_dir で ----------

/// tempdir の (base, 偽の HOME, app_dir, 共有の DB)。base は canonical（macOS の /var → /private/var を吸収 = check_root が通る）
fn ws_env(tag: &str) -> (PathBuf, PathBuf, PathBuf, Mutex<openref_core::Db>) {
    let base = std::fs::canonicalize(scratch(&format!("ws-{tag}"))).unwrap();
    let home = base.join("home");
    std::fs::create_dir_all(home.join("Documents")).unwrap();
    let app = base.join("app");
    let db = openref_core::Db::open(&app.join("openref.sqlite")).unwrap();
    (base, home, app, Mutex::new(db))
}

/// DB の設定から読んだ Ctx（HOME だけ偽の HOME に = このプロセスの環境変数は変えない）
fn ctx_home(db: &Mutex<openref_core::Db>, home: &Path) -> openref_core::wiki::Ctx {
    let mut c = openref_core::wiki::Ctx::from_db(&db.lock().unwrap()).unwrap();
    c.home = Some(home.to_path_buf());
    c
}

fn ws_map(db: &Mutex<openref_core::Db>) -> openref_core::wiki::workspaces::Map {
    let raw = setting(&db.lock().unwrap(), openref_core::wiki::workspaces::KEY);
    openref_core::wiki::workspaces::parse(raw.as_deref()).unwrap()
}

fn put_file(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"x").unwrap();
}

/// 設定 → 規則 → 索引のつなぎ（AppHandle に依らない所まで）: wiki_exclude_set の本体（wiki_exclude_write = 設定を書く →
/// 規則を読み直す・wiki_scan = 走査）・Ctx::from_db → ready() の規則・監視の規則の読み（wiki_rules_from）と取り込み
/// （wiki_on_fs_ingest）が、DB の設定 wiki_workspaces の exclude を既定の規則に化けさせずに効かせる。入れ直すと戻る
#[test]
fn exclude_set_flows_from_settings_to_index() {
    use openref_core::wiki::{index::Ready, workspaces, with_index};
    let (base, home, _app, db) = ws_env("excl-flow");
    let vault = home.join("Documents/vault");
    for rel in ["notes/a.md", "proj/testdata/x.md", "proj/testdata/sub/y.md", "web/node_modules/m/r.md"] {
        put_file(&vault.join(rel));
    }
    let vault_s = vault.to_string_lossy().into_owned();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &vault_s).unwrap();
        g.set_setting(workspaces::KEY, &serde_json::json!({ &vault_s: {"last_used": 7, "future": 1} }).to_string()).unwrap();
    }
    let paths = |at: &Ready| with_index(at, |ix| ix.all_paths()).unwrap();
    let at0 = ctx_home(&db, &home).ready().unwrap();
    assert!(at0.rules.exclude().is_empty() && at0.rules.include().is_empty());
    wiki_scan(&at0, WikiScan::Refresh, || {}).unwrap();
    assert_eq!(paths(&at0), ["notes/a.md", "proj/testdata/sub/y.md", "proj/testdata/x.md"]);
    // 除く: 設定に書かれ（他の項目は保つ）・返る Ready の規則・DB から読み直した Ctx の規則が同じ
    let held = std::cell::Cell::new(false);
    let (rel, at) = wiki_exclude_write(&db, ctx_home(&db, &home), None, "/proj/testdata/", true, 100, false).1.unwrap();
    assert_eq!(rel, "proj/testdata");
    assert_eq!(at.rules.exclude(), ["proj/testdata".to_string()]);
    assert_eq!(ws_map(&db)[&vault_s], serde_json::json!({"last_used": 7, "future": 1, "exclude": ["proj/testdata"]}));
    assert_eq!(ctx_home(&db, &home).ready().unwrap(), at, "Ctx::from_db → ready() も同じ規則");
    assert_eq!(wiki_scan(&at, WikiScan::Refresh, || held.set(true)).unwrap(), openref_core::wiki::Fresh::Ok);
    assert!(held.get(), "索引を握った後に知らせる");
    assert_eq!(paths(&at), ["notes/a.md"]);
    assert_eq!(with_index(&at, |ix| ix.excluded()).unwrap(), ["proj/testdata", "web/node_modules"]);
    // 書込の後の refresh_paths（ctx.ready() の入口）も除外の配下を無視する
    put_file(&vault.join("proj/testdata/new.md"));
    let r = with_index(&ctx_home(&db, &home).ready().unwrap(), |ix| ix.refresh_paths(&["proj/testdata/new.md"])).unwrap();
    assert!(r.is_empty(), "{r:?}");
    // 監視: 設定から読んだ規則（wiki_rules_from）で取り込む = 除外の中のフォルダの作成で全走査しない
    let rules = wiki_rules_from(&db.lock().unwrap(), &at.root).unwrap();
    assert_eq!(rules, at.rules, "監視の規則の読みが既定の規則に化けない");
    std::fs::create_dir_all(vault.join("proj/testdata/newdir")).unwrap();
    let last = with_index(&at, |ix| ix.meta("last_scan")).unwrap();
    let out = wiki_on_fs_ingest(&at.index, &at.root, rules, vec![vault.join("proj/testdata/newdir"), vault.join("proj/testdata/new.md")]).unwrap();
    assert_eq!(out, FsOutcome::Nothing);
    assert_eq!(with_index(&at, |ix| ix.meta("last_scan")).unwrap(), last, "全走査していない");
    // 入れ直す: exclude から外れ・走査で戻る（除いていた間にできた new.md も入る）
    let (_, at2) = wiki_exclude_write(&db, ctx_home(&db, &home), None, "proj/testdata", false, 200, false).1.unwrap();
    assert!(at2.rules.exclude().is_empty());
    wiki_scan(&at2, WikiScan::Refresh, || {}).unwrap();
    assert_eq!(paths(&at2), ["notes/a.md", "proj/testdata/new.md", "proj/testdata/sub/y.md", "proj/testdata/x.md"]);
    // 既定で除かれるフォルダを入れる = include に書かれ、走査に入る
    let (_, at3) = wiki_exclude_write(&db, ctx_home(&db, &home), None, "web/node_modules", false, 300, false).1.unwrap();
    assert_eq!(at3.rules.include(), ["web/node_modules".to_string()]);
    wiki_scan(&at3, WikiScan::Refresh, || {}).unwrap();
    assert!(paths(&at3).contains(&"web/node_modules/m/r.md".to_string()));
    // 検査に落ちる dir は設定を書かない
    let before = ws_map(&db);
    assert!(wiki_exclude_write(&db, ctx_home(&db, &home), None, ".git", true, 400, false).1.is_err());
    assert!(wiki_exclude_write(&db, ctx_home(&db, &home), None, "", true, 400, false).1.is_err());
    assert_eq!(ws_map(&db), before);
    let _ = std::fs::remove_dir_all(&base);
}

/// 退避の記録は失敗のときも返る: 壊れた wiki_workspaces を退避して書いた後に失敗する（書いた後の ready() が落ちる =
/// 呼び手の ctx は有効・DB の wiki_enabled は "0"）= Err と一緒に set_aside の入った note が返る（呼び手は知らせてからエラーを返す）。
/// 書く前の検査で落ちたときは退避もしない（空の note）
#[test]
fn exclude_write_returns_set_aside_note_on_failure() {
    use openref_core::wiki::workspaces;
    let (base, home, _app, db) = ws_env("excl-note");
    let vault = home.join("Documents/vault");
    put_file(&vault.join("proj/testdata/x.md"));
    let vault_s = vault.to_string_lossy().into_owned();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "0").unwrap();
        g.set_setting("wiki_root", &vault_s).unwrap();
        g.set_setting(workspaces::KEY, "{not json").unwrap();
    }
    let ctx = || {
        let mut c = ctx_home(&db, &home);
        c.enabled = true;
        c
    };
    // 書く前の検査で落ちる = 退避しない・空の note
    let (note, r) = wiki_exclude_write(&db, ctx(), None, ".git", true, 100, false);
    assert!(r.is_err());
    assert_eq!(note, WikiWsNote::default());
    assert_eq!(setting(&db.lock().unwrap(), workspaces::KEY).as_deref(), Some("{not json"), "検査で落ちたら設定に触らない");
    // 退避して書いた後に失敗
    let (note, r) = wiki_exclude_write(&db, ctx(), None, "proj/testdata", true, 100, false);
    let e = r.unwrap_err();
    assert!(e.contains("無効"), "書いた後の ready() の失敗: {e}");
    assert_eq!(note.set_aside.len(), 1, "{note:?}");
    assert_eq!(note.set_aside[0].0, workspaces::KEY);
    assert_eq!(setting(&db.lock().unwrap(), &note.set_aside[0].1).as_deref(), Some("{not json"), "退避先に元の値");
    // 書込そのものは済んでいる（壊れた値を退避した後の新しい設定）
    assert_eq!(ws_map(&db)[&vault_s]["exclude"], serde_json::json!(["proj/testdata"]));
    let _ = std::fs::remove_dir_all(&base);
}

/// 索引の本体と -wal を置く（偽の app_dir）
fn put_index(app: &Path, root: &str) -> PathBuf {
    let ip = openref_core::wiki::index::index_path(app, Path::new(root));
    put_file(&ip);
    put_file(&PathBuf::from(format!("{}-wal", ip.display())));
    ip
}

fn wal_of(ip: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", ip.display()))
}

/// 掃除の開始（これより前に作ったファイルは「前から有った」）
fn later() -> SystemTime {
    SystemTime::now() + Duration::from_secs(60)
}

/// 設定 wiki_root が symlink・末尾 `/` の綴りでも、起動の段取りの後は wiki_root = canonical・履歴は canonical の 1 件
/// （生の鍵の ref_dirs と未知の項目は寄せる）・[外す] は canonical / 生の綴りのどちらでも current を拒む・現在の索引は残る
#[test]
fn startup_canonicalizes_root_spelling() {
    use openref_core::wiki::workspaces;
    for kind in ["symlink", "slash"] {
        let (base, home, app, db) = ws_env(&format!("canon-{kind}"));
        let real = home.join("Documents/notes-real");
        std::fs::create_dir_all(&real).unwrap();
        let real_s = real.to_string_lossy().to_string();
        let spelled = if kind == "symlink" {
            let l = home.join("Documents/notes");
            std::os::unix::fs::symlink(&real, &l).unwrap();
            l.to_string_lossy().to_string()
        } else {
            format!("{real_s}/")
        };
        {
            let g = db.lock().unwrap();
            g.set_setting("wiki_enabled", "1").unwrap();
            g.set_setting("wiki_root", &spelled).unwrap();
            // 割れた履歴（再現: 生の鍵 = ref_dirs・last_used 0 / canonical の鍵 = 時刻）
            let ws = serde_json::json!({
                spelled.clone(): {"last_used": 0, "ref_dirs": ["/ref"], "future": 1},
                real_s.clone(): {"last_used": 50},
                "/v/other": {"last_used": 40},
            });
            g.set_setting(workspaces::KEY, &ws.to_string()).unwrap();
        }
        let cur_ix = put_index(&app, &real_s);
        let canon = wiki_root_canon(&ctx_home(&db, &home));
        assert_eq!(canon, Some((spelled.clone(), real_s.clone())), "{kind}");
        let (r, note) = wiki_startup_steps(&db, &app, true, Some(&spelled), canon.as_ref(), 1000, false, later());
        let done = r.unwrap();
        assert!(note.set_aside.is_empty());
        assert_eq!(done.rewrote, Some((spelled.clone(), real_s.clone())), "{kind}");
        assert_eq!(setting(&db.lock().unwrap(), "wiki_root").as_deref(), Some(real_s.as_str()), "{kind}: wiki_root は canonical に");
        let m = ws_map(&db);
        assert_eq!(
            workspaces::order(&m),
            vec![(real_s.clone(), 1000), ("/v/other".to_string(), 40)],
            "{kind}: 履歴は canonical の 1 件"
        );
        assert_eq!(workspaces::ref_dirs(&m, &real_s), vec!["/ref".to_string()], "{kind}: ref_dirs は寄せる");
        assert_eq!(m[&real_s]["future"], serde_json::json!(1), "{kind}: 未知の項目も寄せる");
        // [外す]: 書き直しの後の canon・書き直し前の canon のどちらでも current を拒み、現在の索引は残る
        let canon2 = wiki_root_canon(&ctx_home(&db, &home));
        assert_eq!(canon2, Some((real_s.clone(), real_s.clone())));
        for c in [canon2.as_ref(), canon.as_ref()] {
            let (r, _) = wiki_forget_steps(&db, Some(&app), c, &real_s, 0, false);
            assert_eq!(r.unwrap_err(), "現在のワークスペースは履歴から外せません", "{kind}");
        }
        let (r, _) = wiki_forget_steps(&db, Some(&app), canon.as_ref(), &spelled, 0, false);
        assert!(r.is_err(), "{kind}: 生の綴りも現在として守る");
        assert!(cur_ix.exists() && wal_of(&cur_ix).exists(), "{kind}: 使用中の索引は消えない");
        assert_eq!(ws_map(&db).len(), 2);
        // 2 回目の起動 = 書き直さない・時刻だけ
        let (r, _) = wiki_startup_steps(&db, &app, true, Some(&real_s), canon2.as_ref(), 2000, false, later());
        assert_eq!(r.unwrap().rewrote, None);
        assert_eq!(workspaces::order(&ws_map(&db))[0], (real_s.clone(), 2000));
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// 参照フォルダの読み（wiki_ref_dirs_read = 書込の段取りを通らない）は、前の読み（wiki_ws_db の中で wiki_ws_target → wiki_ref_dirs_of
/// → wiki_pin）と同じ一覧・同じ固定の root・同じ Err（履歴に無い root）。壊れた設定は空で、退避も書込もしない（退避は次の書込 / 起動時）
#[test]
fn ref_dirs_read_matches_the_transaction_path() {
    let (base, home, _app, db) = ws_env("ref-read");
    let real = home.join("Documents/notes-real");
    std::fs::create_dir_all(&real).unwrap();
    let real_s = real.to_string_lossy().to_string();
    let link = home.join("Documents/notes");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let link_s = link.to_string_lossy().to_string();
    let ws = serde_json::json!({ real_s.clone(): {"last_used": 2, "ref_dirs": ["/r1", "/r1b"]}, "/v/b": {"last_used": 1, "ref_dirs": ["/r2"]} });
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_root", &link_s).unwrap(); // 生の値 = symlink（履歴の鍵は canonical）
        g.set_setting(openref_core::wiki::workspaces::KEY, &ws.to_string()).unwrap();
    }
    let canon = wiki_root_canon(&ctx_home(&db, &home));
    assert_eq!(canon, Some((link_s.clone(), real_s.clone())));
    let before = setting(&db.lock().unwrap(), openref_core::wiki::workspaces::KEY);
    for root in [None, Some(real_s.as_str()), Some("/v/b"), Some("/v/none"), Some(link_s.as_str())] {
        let (old, _) = wiki_ws_db(&db, 0, false, canon.as_ref(), |m, cur| Ok(wiki_ref_dirs_of(m, wiki_ws_target(m, root, cur)?)));
        let old = old.and_then(|(dirs, ctx)| Ok((dirs, wiki_pin(ctx, root)?.root)));
        let new = wiki_ref_dirs_read(ctx_home(&db, &home), canon.as_ref(), root).map(|(dirs, ctx)| (dirs, ctx.root));
        assert_eq!(new, old, "{root:?}");
    }
    let (dirs, _) = wiki_ref_dirs_read(ctx_home(&db, &home), canon.as_ref(), None).unwrap();
    assert_eq!(dirs, vec![PathBuf::from("/r1"), PathBuf::from("/r1b")], "現在の root = canonical の鍵で引く");
    assert!(wiki_ref_dirs_read(ctx_home(&db, &home), canon.as_ref(), Some("/v/none")).unwrap_err().contains("unknown_root"));
    assert_eq!(setting(&db.lock().unwrap(), openref_core::wiki::workspaces::KEY), before, "読むだけ = 書かない");
    // 壊れた設定: 空・退避しない・書かない
    db.lock().unwrap().set_setting(openref_core::wiki::workspaces::KEY, "[broken").unwrap();
    let (dirs, _) = wiki_ref_dirs_read(ctx_home(&db, &home), canon.as_ref(), None).unwrap();
    assert!(dirs.is_empty());
    let g = db.lock().unwrap();
    assert_eq!(setting(&g, openref_core::wiki::workspaces::KEY).as_deref(), Some("[broken"));
    assert_eq!(setting(&g, "wiki_workspaces.broken-19700101T000000Z"), None);
    drop(g);
    // 入口（AppHandle あり）は書込の段取り（wiki_ws = BEGIN IMMEDIATE・退避・toast）を通らず、この純関数を通る
    let raw = crate::parity::squash(&crate::parity::fn_body(include_str!("../wiki_gui.rs"), "pub(crate) fn wiki_ref_dirs_raw("));
    assert!(raw.contains("wiki_ref_dirs_read(ctx,canon.as_ref(),root)") && !raw.contains("wiki_ws("), "{raw}");
    let _ = std::fs::remove_dir_all(&base);
}

/// ①: 読み書きの入口（wiki_ws_db）の root は canonical = f が書く参照フォルダも canonical の鍵へ（生の綴りの鍵を作らない）。
/// canon が無い（root が使えない）・canon の生の値が設定と違う（ロックの外で読んだ後に書き換わった）なら設定の生の値
#[test]
fn ws_db_root_is_canonical() {
    let (base, home, _app, db) = ws_env("canon-legacy");
    let real = home.join("Documents/notes-real");
    std::fs::create_dir_all(&real).unwrap();
    let real_s = real.to_string_lossy().to_string();
    let link = home.join("Documents/notes");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let link_s = link.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_root", &link_s).unwrap();
        g.set_setting("wiki_ref_dirs", r#"["/refdir"]"#).unwrap();
    }
    // 無効でも canonical は求まる（check_root は有効 / 無効を問わない）
    let canon = wiki_root_canon(&ctx_home(&db, &home));
    assert_eq!(canon, Some((link_s.clone(), real_s.clone())));
    let (r, note) = wiki_ws_db(&db, 0, false, canon.as_ref(), |m, root| {
        let before = wiki_ref_dirs_of(m, root);
        if let Some(r) = root {
            wiki_save_ref_dirs(m, r, &[PathBuf::from("/added")]);
        }
        Ok((root.map(str::to_string), before))
    });
    let ((root, before), ctx) = r.unwrap();
    assert_eq!(root.as_deref(), Some(real_s.as_str()));
    assert!(before.is_empty(), "旧キーは取り込まない: {before:?}");
    assert_eq!(note, WikiWsNote::default());
    assert_eq!(db.lock().unwrap().setting("wiki_ref_dirs").unwrap().as_deref(), Some(r#"["/refdir"]"#), "旧キーは消さない");
    assert_eq!(ctx.root.as_deref(), Some(link_s.as_str()), "Ctx は設定の値（書き直しは起動の家事）");
    let m = ws_map(&db);
    assert_eq!(m.keys().cloned().collect::<Vec<_>>(), vec![real_s.clone()], "生の綴りの鍵を作らない");
    let (r, _) = wiki_ws_db(&db, 0, false, None, |_, root| Ok(root.map(str::to_string)));
    assert_eq!(r.unwrap().0.as_deref(), Some(link_s.as_str()), "canon 無し = 生の値");
    let stale = ("/v/elsewhere".to_string(), "/v/elsewhere-real".to_string());
    let (r, _) = wiki_ws_db(&db, 0, false, Some(&stale), |_, root| Ok(root.map(str::to_string)));
    assert_eq!(r.unwrap().0.as_deref(), Some(link_s.as_str()), "canon の生の値が設定と違う = 生の値");
    // 消えた root = canonical が求まらない
    std::fs::remove_file(&link).unwrap();
    assert_eq!(wiki_root_canon(&ctx_home(&db, &home)), None);
    let _ = std::fs::remove_dir_all(&base);
}

/// 起動の段取り = 上限で落ちた root の索引と孤児だけ消え、履歴の root・現在の root の索引は残る（keep から履歴の鍵を外す
/// 変異はここで落ちる）・2.0.0 より前の置き場の索引（app_dir 直下）と DB は残る（旧の置き場は見ない）
#[test]
fn startup_steps_keep_history_and_drop_evicted() {
    use openref_core::wiki::workspaces;
    let (base, home, app, db) = ws_env("startup-keep");
    let cur = home.join("Documents/notes");
    std::fs::create_dir_all(&cur).unwrap();
    let cur_s = cur.to_string_lossy().to_string();
    let hist: Vec<String> = (0..workspaces::MAX).map(|i| format!("/Users/u/w{i}")).collect();
    let mut ws = serde_json::Map::new();
    for (i, r) in hist.iter().enumerate() {
        ws.insert(r.clone(), serde_json::json!({"last_used": 100 + i}));
    }
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &cur_s).unwrap();
        g.set_setting(workspaces::KEY, &serde_json::Value::Object(ws).to_string()).unwrap();
    }
    let ixs: Vec<PathBuf> = hist.iter().map(|r| put_index(&app, r)).collect();
    let cur_ix = put_index(&app, &cur_s);
    let orphan_ix = put_index(&app, "/Users/u/orphan");
    put_file(&app.join(openref_core::wiki::index::INDEX_FILE));
    let canon = wiki_root_canon(&ctx_home(&db, &home));
    assert_eq!(canon, Some((cur_s.clone(), cur_s.clone())));
    let (r, _) = wiki_startup_steps(&db, &app, true, Some(&cur_s), canon.as_ref(), 1000, false, later());
    let done = r.unwrap();
    assert_eq!(done.evicted, vec![hist[0].clone()], "最も古い w0 が落ちる");
    assert!(!ixs[0].exists() && !wal_of(&ixs[0]).exists(), "落ちた root の索引は消える");
    for ix in &ixs[1..] {
        assert!(ix.exists() && wal_of(ix).exists(), "履歴の root の索引は残る: {}", ix.display());
    }
    assert!(cur_ix.exists() && wal_of(&cur_ix).exists(), "現在の root の索引は残る");
    assert!(!orphan_ix.exists() && !wal_of(&orphan_ix).exists(), "孤児は消える");
    assert!(app.join(openref_core::wiki::index::INDEX_FILE).exists(), "旧の置き場の索引には触らない");
    assert!(app.join("openref.sqlite").exists(), "DB は残る");
    let order = workspaces::order(&ws_map(&db));
    assert_eq!(order.len(), workspaces::MAX);
    assert_eq!(order[0], (cur_s.clone(), 1000));
    // 無効 = touch しない（履歴の時刻は変わらない）が、掃除はする（履歴と現在は残す）
    let stray = put_index(&app, "/Users/u/stray");
    {
        db.lock().unwrap().set_setting("wiki_enabled", "0").unwrap();
    }
    let (r, _) = wiki_startup_steps(&db, &app, false, Some(&cur_s), canon.as_ref(), 5000, false, later());
    assert!(r.unwrap().evicted.is_empty());
    assert_eq!(workspaces::order(&ws_map(&db))[0], (cur_s.clone(), 1000));
    assert!(!stray.exists());
    assert!(ixs[1].exists() && cur_ix.exists());
    let _ = std::fs::remove_dir_all(&base);
}

/// [外す] の段取り = 外した root の索引だけ消える（現在と他の履歴の索引は残る）・現在は拒否して何も消さない
#[test]
fn forget_steps_drop_only_that_index() {
    use openref_core::wiki::workspaces;
    let (base, home, app, db) = ws_env("forget");
    let cur = home.join("Documents/notes");
    std::fs::create_dir_all(&cur).unwrap();
    let cur_s = cur.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_root", &cur_s).unwrap();
        let ws = serde_json::json!({cur_s.clone(): {"last_used": 3}, "/Users/u/a": {"last_used": 2, "ref_dirs": ["/r"]}, "/Users/u/b": {"last_used": 1}});
        g.set_setting(workspaces::KEY, &ws.to_string()).unwrap();
    }
    let (cur_ix, a_ix, b_ix) = (put_index(&app, &cur_s), put_index(&app, "/Users/u/a"), put_index(&app, "/Users/u/b"));
    let canon = wiki_root_canon(&ctx_home(&db, &home));
    let (r, _) = wiki_forget_steps(&db, Some(&app), canon.as_ref(), "/Users/u/a", 0, false);
    r.unwrap();
    assert!(!a_ix.exists() && !wal_of(&a_ix).exists());
    assert!(b_ix.exists() && cur_ix.exists());
    let m = ws_map(&db);
    assert_eq!(workspaces::order(&m).into_iter().map(|(r, _)| r).collect::<Vec<_>>(), vec![cur_s.clone(), "/Users/u/b".into()]);
    let (r, _) = wiki_forget_steps(&db, Some(&app), canon.as_ref(), &cur_s, 0, false);
    assert!(r.is_err());
    assert!(cur_ix.exists() && wal_of(&cur_ix).exists(), "拒否したら何も消さない");
    let (r, _) = wiki_forget_steps(&db, Some(&app), canon.as_ref(), "/Users/u/a", 0, false);
    assert_eq!(r.unwrap_err(), "履歴に無いワークスペースです");
    let _ = std::fs::remove_dir_all(&base);
}

/// root の確定（wiki_choose_root の設定の段） = wiki_root を書き・履歴に入れ（touch）・上限で落ちた root の索引だけ消す
#[test]
fn root_commit_writes_root_and_touches_history() {
    use openref_core::wiki::workspaces;
    let (base, _home, app, db) = ws_env("commit");
    let hist: Vec<String> = (0..workspaces::MAX).map(|i| format!("/Users/u/w{i}")).collect();
    let mut ws = serde_json::Map::new();
    for (i, r) in hist.iter().enumerate() {
        ws.insert(r.clone(), serde_json::json!({"last_used": 100 + i}));
    }
    db.lock().unwrap().set_setting(workspaces::KEY, &serde_json::Value::Object(ws).to_string()).unwrap();
    let ixs: Vec<PathBuf> = hist.iter().map(|r| put_index(&app, r)).collect();
    let (r, _) = wiki_root_commit(&db, Some(&app), "/Users/u/new", 1000, false);
    assert_eq!(r.unwrap(), vec![hist[0].clone()]);
    assert_eq!(setting(&db.lock().unwrap(), "wiki_root").as_deref(), Some("/Users/u/new"));
    let order = workspaces::order(&ws_map(&db));
    assert_eq!(order[0], ("/Users/u/new".to_string(), 1000));
    assert_eq!(order.len(), workspaces::MAX);
    assert!(!ixs[0].exists());
    assert!(ixs[1..].iter().all(|p| p.exists()));
    // 同じ root の選び直し = 何も落ちない・時刻だけ
    let (r, _) = wiki_root_commit(&db, Some(&app), "/Users/u/new", 2000, false);
    assert!(r.unwrap().is_empty());
    assert_eq!(workspaces::order(&ws_map(&db))[0], ("/Users/u/new".to_string(), 2000));
    let _ = std::fs::remove_dir_all(&base);
}

/// 掃除は一族ごと = 一族のどれかが開始より後に変更されていれば一族まるごと残す・変更時刻を読めないものは前から有った扱い・
/// keep の一族は時刻を問わず残す
#[test]
fn index_sweep_plan_at_keeps_fresh_families() {
    let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let old = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(500));
    let new = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(2000));
    let (a, b, c, d, k) = (
        "aaaaaaaaaaaaaaaa.sqlite",
        "bbbbbbbbbbbbbbbb.sqlite",
        "cccccccccccccccc.sqlite",
        "dddddddddddddddd.sqlite",
        "eeeeeeeeeeeeeeee.sqlite",
    );
    let e = |n: String, t: Option<SystemTime>| (n, t);
    let entries = vec![
        // a: 本体は古いが -wal が新しい = 一族ごと残す
        e(a.into(), old),
        e(format!("{a}-wal"), new),
        e(format!("{a}-shm"), old),
        // b: 全部古い = 一族ごと消す
        e(b.into(), old),
        e(format!("{b}-wal"), old),
        e(format!("{b}.corrupt-1"), old),
        // c: 時刻を読めない = 前から有った
        e(c.into(), None),
        e(format!("{c}-wal"), None),
        // d: 退避の -wal だけ新しい = 一族ごと残す
        e(d.into(), old),
        e(format!("{d}.corrupt-5-wal"), new),
        // k: keep（新旧を問わず残す）
        e(k.into(), old),
        e(format!("{k}-wal"), old),
        e("notes.txt".into(), old),
    ];
    let keep: HashSet<String> = [k.to_string()].into_iter().collect();
    let mut plan = wiki_index_sweep_plan_at(&entries, &keep, started);
    plan.sort();
    assert_eq!(
        plan,
        vec![b.to_string(), format!("{b}-wal"), format!("{b}.corrupt-1"), c.to_string(), format!("{c}-wal")]
    );
}

// ---------- タブごとのワークスペース: AppHandle 無しの段取りと純関数 ----------

/// wiki_err の JSON（{"code", "message"}）の code
fn err_code(e: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(e).ok()?["code"].as_str().map(str::to_string)
}

/// 段の選び方 = タブの行き来（Front）は last_used を変えず・チャットの子を止めない。明示の切替（Choose）は touch し、
/// 子の root が違えば止める（同じ root・子が無ければ止めない）
#[test]
fn root_change_kinds() {
    let (a, b) = (Path::new("/v/a"), Path::new("/v/b"));
    assert!(RootChange::Choose.touches() && !RootChange::Front.touches());
    assert!(RootChange::Choose.stops_chat(Some(a), b));
    assert!(!RootChange::Choose.stops_chat(Some(b), b));
    assert!(!RootChange::Choose.stops_chat(None, b));
    assert!(!RootChange::Front.stops_chat(Some(a), b), "タブの行き来では子を止めない");
    assert!(!RootChange::Front.stops_chat(Some(b), b));
    assert!(!RootChange::Front.stops_chat(None, b));
}

/// wiki_front_root の設定の段（Front） = 履歴にある root だけ（空・生の綴り・履歴に無い = unknown_root で何も書かない）・
/// wiki_root を canonical で書く・最近のワークスペース（last_used・順・ref_dirs）は設定の値ごと変えない。
/// wiki_choose_root の段（Choose）は従来どおり履歴の外のフォルダも選べて touch する
#[test]
fn front_root_steps_write_root_without_touch() {
    use openref_core::wiki::workspaces;
    let (base, home, app, db) = ws_env("front");
    let notes = home.join("Documents/notes");
    let site = home.join("Documents/site");
    let other = home.join("Documents/other");
    for d in [&notes, &site, &other] {
        std::fs::create_dir_all(d).unwrap();
    }
    let link = home.join("Documents/site-link");
    std::os::unix::fs::symlink(&site, &link).unwrap();
    let s = |p: &Path| p.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &s(&notes)).unwrap();
        let ws = serde_json::json!({s(&notes): {"last_used": 100, "ref_dirs": ["/r"]}, s(&site): {"last_used": 50}});
        g.set_setting(workspaces::KEY, &ws.to_string()).unwrap();
    }
    let ws_raw = || setting(&db.lock().unwrap(), workspaces::KEY);
    let root_now = || setting(&db.lock().unwrap(), "wiki_root");
    let before = ws_raw();
    let (r, note) = wiki_root_steps(&db, &ctx_home(&db, &home), &site, RootChange::Front, 999, false);
    assert_eq!(r.unwrap(), site);
    assert_eq!(note, WikiWsNote::default());
    assert_eq!(root_now(), Some(s(&site)));
    assert_eq!(ws_raw(), before, "last_used も順も変えない（設定の値そのまま）");
    // 履歴に無い = unknown_root（wiki_err の JSON）・何も書かない
    for p in [other.clone(), PathBuf::new(), link.clone(), PathBuf::from(format!("{}/", s(&notes)))] {
        let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &p, RootChange::Front, 999, false);
        let e = r.unwrap_err();
        assert_eq!(err_code(&e).as_deref(), Some("unknown_root"), "{p:?}: {e}");
    }
    assert_eq!((root_now(), ws_raw()), (Some(s(&site)), before.clone()));
    // Choose = 従来どおり（履歴の外も選べる・touch）
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &other, RootChange::Choose, 1000, false);
    assert_eq!(r.unwrap(), other);
    let order = || workspaces::order(&ws_map(&db));
    assert_eq!(order(), vec![(s(&other), 1000), (s(&notes), 100), (s(&site), 50)]);
    // Front で前のワークスペースへ戻す = wiki_root だけ・時刻は変わらない
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &notes, RootChange::Front, 2000, false);
    assert_eq!(r.unwrap(), notes);
    assert_eq!(root_now(), Some(s(&notes)));
    assert_eq!(order(), vec![(s(&other), 1000), (s(&notes), 100), (s(&site), 50)]);
    assert_eq!(workspaces::ref_dirs(&ws_map(&db), &s(&notes)), vec!["/r".to_string()]);
    // check_root に落ちる（履歴にはあるが消えた）= Err・何も書かない
    std::fs::remove_dir_all(&site).unwrap();
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &site, RootChange::Front, 3000, false);
    assert!(r.is_err());
    assert_eq!(root_now(), Some(s(&notes)));
    let _ = app;
    let _ = std::fs::remove_dir_all(&base);
}

/// 参照フォルダの対象の root = タブの root（履歴にあるものだけ = 無ければ unknown_root で履歴に項目を作らない）・None は設定の root。
/// root ごと = B に足しても A の一覧は変わらない
#[test]
fn ref_dirs_target_is_per_root() {
    use openref_core::wiki::workspaces;
    let db = openref_core::Db::open_in_memory().unwrap();
    db.set_setting("wiki_root", "/v/a").unwrap();
    db.set_setting(workspaces::KEY, r#"{"/v/a": {"last_used": 2, "ref_dirs": ["/ra"]}, "/v/b": {"last_used": 1}}"#).unwrap();
    let read = |root: Option<&str>| wiki_ws_tx(&db, 0, false, None, |m, cur| Ok(wiki_ref_dirs_of(m, wiki_ws_target(m, root, cur)?))).0;
    assert_eq!(read(None).unwrap(), vec![PathBuf::from("/ra")], "None = 設定の root");
    assert_eq!(read(Some("/v/a")).unwrap(), vec![PathBuf::from("/ra")]);
    assert!(read(Some("/v/b")).unwrap().is_empty());
    for bad in ["/v/c", "", "/v/b/"] {
        let e = read(Some(bad)).unwrap_err();
        assert_eq!(err_code(&e).as_deref(), Some("unknown_root"), "{bad:?}");
    }
    // B に足す（main.rs の wiki_ref_dir_add の f と同じ組み立て）= B だけ変わる
    let add = |root: Option<&str>, dir: &str| {
        wiki_ws_tx(&db, 0, false, None, |m, cur| {
            let r = wiki_ws_target(m, root, cur)?.ok_or("未設定")?;
            let plan = wiki_ref_dirs_for_add(m, r, Path::new(dir));
            if plan.changed {
                wiki_save_ref_dirs(m, r, &plan.dirs);
            }
            Ok(plan.changed)
        })
        .0
    };
    assert!(add(Some("/v/b"), "/rb").unwrap());
    assert_eq!(read(Some("/v/b")).unwrap(), vec![PathBuf::from("/rb")]);
    assert_eq!(read(Some("/v/a")).unwrap(), vec![PathBuf::from("/ra")]);
    // 履歴に無い root には足さない（項目を作らない）
    assert_eq!(err_code(&add(Some("/v/c"), "/rc").unwrap_err()).as_deref(), Some("unknown_root"));
    let m = workspaces::parse(setting(&db, workspaces::KEY).as_deref()).unwrap();
    assert_eq!(m.len(), 2);
    // B から外す = B だけ
    let removed = wiki_ws_tx(&db, 0, false, None, |m, cur| {
        Ok(wiki_ref_dirs_for_remove(m, wiki_ws_target(m, Some("/v/b"), cur)?, Path::new("/rb")))
    })
    .0
    .unwrap();
    assert!(removed.is_empty());
    assert_eq!(read(Some("/v/a")).unwrap(), vec![PathBuf::from("/ra")]);
}

/// command の引数 root = 無い・null は None・文字列はそのまま（空も = pin が拒否）・他の型は Err。wiki_pin = 履歴の鍵だけ（JSON の code）
#[test]
fn root_arg_and_pin() {
    use serde_json::json;
    assert_eq!(wiki_root_arg(&json!({})).unwrap(), None);
    assert_eq!(wiki_root_arg(&json!({"root": null})).unwrap(), None);
    assert_eq!(wiki_root_arg(&json!({"root": ""})).unwrap().as_deref(), Some(""));
    assert_eq!(wiki_root_arg(&json!({"root": "/v/a"})).unwrap().as_deref(), Some("/v/a"));
    assert!(wiki_root_arg(&json!({"root": 1})).is_err());
    let ctx = openref_core::wiki::Ctx {
        root: Some("/v/a".into()),
        workspaces: openref_core::wiki::workspaces::parse(Some(r#"{"/v/a": {}, "/v/b": {}}"#)).unwrap(),
        ..Default::default()
    };
    assert_eq!(wiki_pin(ctx.clone(), None).unwrap().root.as_deref(), Some("/v/a"));
    let b = wiki_pin(ctx.clone(), Some("/v/b")).unwrap();
    assert_eq!((b.root.as_deref(), b.setting_root()), (Some("/v/b"), Some("/v/a")));
    for bad in ["", "/v/c"] {
        assert_eq!(err_code(&wiki_pin(ctx.clone(), Some(bad)).unwrap_err()).as_deref(), Some("unknown_root"), "{bad:?}");
    }
}

/// 固定した root（args.root）が監視中の root と違えば入口の stat 走査を省かない（WikiLive::plan が root を比べる）・同じなら省く
#[test]
fn watched_is_off_for_pinned_other_root() {
    let live = WikiLive::default();
    let g = live.begin(Some("/v/notes".into()));
    live.armed(g);
    assert!(live.prime(g));
    let ctx = openref_core::wiki::Ctx {
        enabled: true,
        root: Some("/v/notes".into()),
        workspaces: openref_core::wiki::workspaces::parse(Some(r#"{"/v/notes": {}, "/v/site": {}}"#)).unwrap(),
        ..Default::default()
    };
    assert!(wiki_watched(&live, &ctx), "監視中の root = 省く");
    let mut site = ctx.clone();
    site.pin("/v/site").unwrap();
    assert!(!wiki_watched(&live, &site), "固定した別の root = 省かない");
    let mut notes = ctx.clone();
    notes.pin("/v/notes").unwrap();
    assert!(wiki_watched(&live, &notes), "固定した root が監視中と同じ = 省く");
}

/// "wiki-changed" の payload = 監視中の root（canonical な文字列）+ WikiChanged の鍵（平らに・形は従来のまま）
#[test]
fn wiki_changed_event_carries_root() {
    use serde_json::json;
    assert_eq!(
        serde_json::to_value(WikiChangedEvent::new(Path::new("/v/notes"), WikiChanged::everything())).unwrap(),
        json!({"root": "/v/notes", "all": true, "added": [], "removed": [], "modified": []})
    );
    let c = WikiChanged {
        added: vec!["n.md".into()],
        modified: vec!["a.md".into()],
        ..WikiChanged::default()
    };
    assert_eq!(
        serde_json::to_value(WikiChangedEvent::new(Path::new("/v/site"), c)).unwrap(),
        json!({"root": "/v/site", "all": false, "added": ["n.md"], "removed": [], "modified": ["a.md"]})
    );
}

/// チャットの [元に戻す] は記録した子の root で戻す（設定 wiki_root = 前面のタブの root が別でも）。設定の root のままだと
/// 「別のワークスペース」で断る（固定が要る理由）・子の root が履歴に無ければ戻さない・記録に root が無ければ設定のまま
#[test]
fn chat_undo_uses_child_root() {
    use crate::wiki_chat::{undo_record, Change, Record, Undo};
    use openref_core::wiki::{edit, workspaces, Ctx};
    let (base, home, app, _db) = ws_env("undo-root");
    let notes = home.join("Documents/notes");
    let site = home.join("Documents/site");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::create_dir_all(&site).unwrap();
    let (notes_s, site_s) = (notes.to_string_lossy().to_string(), site.to_string_lossy().to_string());
    let ws = |roots: &[&str]| {
        let m: serde_json::Map<String, serde_json::Value> = roots.iter().map(|r| (r.to_string(), serde_json::json!({}))).collect();
        workspaces::parse(Some(&serde_json::Value::Object(m).to_string())).unwrap()
    };
    // 設定 = notes（前面のタブ）・子 = site（Claude がそこにページを作った）
    let setting = Ctx {
        enabled: true,
        root: Some(notes_s.clone()),
        app_dir: Some(app.clone()),
        home: Some(home.clone()),
        workspaces: ws(&[&notes_s, &site_s]),
        ..Default::default()
    };
    let child = wiki_pin(setting.clone(), Some(&site_s)).unwrap();
    let w = edit::write_agent(&child, &serde_json::json!({"path": "c.md", "content": "x"})).unwrap();
    let rec = Record {
        change: Change::Create {
            path: "c.md".into(),
            sha256_after: w["sha256"].as_str().unwrap().into(),
        },
        root: Some(site.clone()),
        moved_sha: None,
        undone: false,
    };
    let no_trash = |_: &Path| -> Result<String, String> { panic!("呼ばない") };
    let no_item = |_: &str| -> Result<(), String> { panic!("呼ばない") };
    assert!(
        matches!(undo_record(&setting, &rec, None, false, no_trash, no_item), Undo::Refused(s) if s.contains("別のワークスペース")),
        "設定の root のまま = 戻せない"
    );
    // 履歴に無い子の root = 戻さない（理由の文）
    let lost = Ctx { workspaces: ws(&[&notes_s]), ..setting.clone() };
    let e = wiki_chat_undo_ctx(lost, Some(&site)).unwrap_err();
    assert!(e.contains("履歴に無いワークスペース"), "{e}");
    // 記録に root が無い = 設定のまま
    assert_eq!(wiki_chat_undo_ctx(setting.clone(), None).unwrap().root, Some(notes_s.clone()));
    // 子の root に固定 = 戻せる（ゴミ箱の代わりに消す）・設定の root（notes）には触らない
    let ctx = wiki_chat_undo_ctx(setting.clone(), Some(&site)).unwrap();
    let mut trashed = None;
    let out = undo_record(&ctx, &rec, None, false, |p: &Path| {
        trashed = Some(p.to_path_buf());
        std::fs::remove_file(p).map(|()| "test".to_string()).map_err(|e| e.to_string())
    }, no_item);
    assert!(matches!(&out, Undo::Done { path: Some(p), .. } if p == "c.md"), "{out:?}");
    assert_eq!(trashed, Some(site.join("c.md")));
    assert!(!site.join("c.md").exists() && std::fs::read_dir(&notes).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(&base);
}

// ---------- 有効化 = OpenNote の起動（OFF で起動した後でも現在の root が履歴に入る = args.root の pin が通る） ----------

/// OFF で起動（履歴は空のまま）→ 有効化（wiki_enable_with = 家事を touch で）→ 履歴に canonical な root が 1 件・pin が通る
#[test]
fn enable_after_off_startup_adds_root_to_history() {
    use openref_core::wiki::workspaces;
    let (base, home, _app, db) = ws_env("enable-off");
    let notes = home.join("Documents/notes");
    std::fs::create_dir_all(&notes).unwrap();
    let notes_s = notes.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "0").unwrap();
        g.set_setting("wiki_root", &notes_s).unwrap();
    }
    // 起動時（無効 = touch しない）
    let ctx = ctx_home(&db, &home);
    let (r, _) = wiki_chores_steps(&db, &ctx, ctx.root.as_deref(), ctx.enabled, 1000, false, later()).unwrap();
    assert!(r.unwrap().evicted.is_empty());
    assert!(ws_map(&db).is_empty(), "OFF の起動では履歴に入らない");
    assert!(ctx_home(&db, &home).pin(&notes_s).is_err(), "このままでは pin が通らない（直す前の不具合）");
    // 有効化 = 設定を書いてから家事（touch）
    db.lock().unwrap().set_setting("wiki_enabled", "1").unwrap();
    let touch = wiki_enable_chores(true, true).expect("有効化は家事を回す");
    let (r, note) = wiki_chores_steps(&db, &ctx_home(&db, &home), Some(&notes_s), touch, 2000, false, later()).unwrap();
    assert_eq!(r.unwrap().rewrote, None, "既に canonical");
    assert!(note.set_aside.is_empty());
    assert_eq!(workspaces::order(&ws_map(&db)), vec![(notes_s.clone(), 2000)]);
    ctx_home(&db, &home).pin(&notes_s).unwrap();
    let _ = std::fs::remove_dir_all(&base);
}

/// 有効化: 設定 wiki_root が symlink の綴りなら canonical に書き直し、履歴は canonical の 1 件（生の綴りの鍵を作らない）
#[test]
fn enable_rewrites_symlink_root_to_canonical() {
    use openref_core::wiki::workspaces;
    let (base, home, _app, db) = ws_env("enable-link");
    let real = home.join("Documents/notes-real");
    std::fs::create_dir_all(&real).unwrap();
    let link = home.join("Documents/notes");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let (real_s, link_s) = (real.to_string_lossy().to_string(), link.to_string_lossy().to_string());
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &link_s).unwrap();
    }
    let touch = wiki_enable_chores(true, true).unwrap();
    let (r, _) = wiki_chores_steps(&db, &ctx_home(&db, &home), Some(&link_s), touch, 3000, false, later()).unwrap();
    assert_eq!(r.unwrap().rewrote, Some((link_s.clone(), real_s.clone())));
    assert_eq!(setting(&db.lock().unwrap(), "wiki_root").as_deref(), Some(real_s.as_str()));
    assert_eq!(workspaces::order(&ws_map(&db)), vec![(real_s.clone(), 3000)]);
    let _ = std::fs::remove_dir_all(&base);
}

/// 無効化は家事を回さない（履歴を変えない）。有効化のフォルダ選択の経路（wiki_choose_root が touch 済み）は touch しない =
/// last_used は選んだときのまま（二重にしない）
#[test]
fn disable_and_chosen_enable_do_not_touch() {
    use openref_core::wiki::workspaces;
    assert_eq!(wiki_enable_chores(false, true), None);
    assert_eq!(wiki_enable_chores(false, false), None);
    assert_eq!(wiki_enable_chores(true, false), Some(false));
    let (base, home, app, db) = ws_env("enable-chosen");
    let notes = home.join("Documents/notes");
    std::fs::create_dir_all(&notes).unwrap();
    let notes_s = notes.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "0").unwrap();
        g.set_setting(workspaces::KEY, r#"{"/Users/u/old": {"last_used": 7}}"#).unwrap();
    }
    // メニューの有効化でフォルダを選んだ = wiki_choose_root（Choose = touch）→ 有効化の家事は touch = false
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &notes, RootChange::Choose, 1000, false);
    assert_eq!(r.unwrap(), notes);
    db.lock().unwrap().set_setting("wiki_enabled", "1").unwrap();
    let before = setting(&db.lock().unwrap(), workspaces::KEY);
    let touch = wiki_enable_chores(true, false).unwrap();
    let (r, _) = wiki_chores_steps(&db, &ctx_home(&db, &home), Some(&notes_s), touch, 5000, false, later()).unwrap();
    r.unwrap();
    assert_eq!(setting(&db.lock().unwrap(), workspaces::KEY), before, "last_used は選んだときのまま");
    assert_eq!(workspaces::order(&ws_map(&db)), vec![(notes_s.clone(), 1000), ("/Users/u/old".to_string(), 7)]);
    let _ = app;
    let _ = std::fs::remove_dir_all(&base);
}

// ---------- 起動時の家事は起動時の root にだけ効かせる・揃えの no-op ----------

/// 画面の揃え（Front）が起動時の家事より先に済んだ = 家事は起動時の root（A）だけを touch し、前面の root（B）の last_used は変えない・
/// 設定 wiki_root（B）を書き換えない・起動時の適用は省く。揃えが無ければ従来どおり（touch・適用する）
#[test]
fn startup_chores_touch_only_start_root() {
    use openref_core::wiki::workspaces;
    let (base, home, _app, db) = ws_env("startup-front");
    let a = home.join("Documents/notes");
    let b = home.join("Documents/site");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let link = home.join("Documents/notes-link");
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let (a_s, b_s, link_s) = (a.to_string_lossy().to_string(), b.to_string_lossy().to_string(), link.to_string_lossy().to_string());
    let reset = |root: &str| {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", root).unwrap();
        let ws = serde_json::json!({a_s.clone(): {"last_used": 100}, b_s.clone(): {"last_used": 50}});
        g.set_setting(workspaces::KEY, &ws.to_string()).unwrap();
    };
    let root_now = || setting(&db.lock().unwrap(), "wiki_root");
    for start in [a_s.clone(), link_s.clone()] {
        reset(&start);
        // setup が同期に読んだ root = start → 家事より先に画面が B へ揃えた
        let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &b, RootChange::Front, 999, false);
        r.unwrap();
        let (r, _) = wiki_chores_steps(&db, &ctx_home(&db, &home), Some(&start), true, 2000, false, later()).unwrap();
        let done = r.unwrap();
        assert_eq!(done.rewrote, None, "{start}: 揃えた後の設定を起動時の root で書き換えない");
        assert_eq!(root_now(), Some(b_s.clone()), "{start}");
        assert_eq!(
            workspaces::order(&ws_map(&db)),
            vec![(a_s.clone(), 2000), (b_s.clone(), 50)],
            "{start}: touch は起動時の root（canonical）だけ・前面の B は触らない"
        );
        assert!(!wiki_startup_apply(root_now().as_deref(), Some(&start), done.rewrote.as_ref()), "{start}: 適用を省く");
    }
    // 揃えが無い = 従来どおり（symlink の綴りは canonical に書き直し・touch・適用する）
    reset(&link_s);
    let (r, _) = wiki_chores_steps(&db, &ctx_home(&db, &home), Some(&link_s), true, 3000, false, later()).unwrap();
    let done = r.unwrap();
    assert_eq!(done.rewrote, Some((link_s.clone(), a_s.clone())));
    assert_eq!(root_now(), Some(a_s.clone()));
    assert_eq!(workspaces::order(&ws_map(&db))[0], (a_s.clone(), 3000));
    assert!(wiki_startup_apply(root_now().as_deref(), Some(&link_s), done.rewrote.as_ref()));
    assert!(wiki_startup_apply(Some(&a_s), Some(&a_s), None));
    assert!(wiki_startup_apply(None, None, None), "未設定のまま = 適用（無効・未設定の枝は wiki_apply が扱う）");
    let _ = std::fs::remove_dir_all(&base);
}

/// 揃えの no-op = 設定が既に root で、監視がその root に張られて生きている（Prime / Skip）ときだけ。設定が違う・監視が別の root・
/// 張り終える前・止めた・失敗した = 揃える
#[test]
fn front_noop_only_when_watched_same_root() {
    let live = WikiLive::default();
    assert!(!wiki_front_noop(&live, Some("/v/a"), "/v/a"), "監視なし");
    let g = live.begin(Some("/v/a".into()));
    assert!(!wiki_front_noop(&live, Some("/v/a"), "/v/a"), "張り終える前");
    live.armed(g);
    assert!(wiki_front_noop(&live, Some("/v/a"), "/v/a"), "張った後（prime 前）");
    assert!(live.prime(g));
    assert!(wiki_front_noop(&live, Some("/v/a"), "/v/a"));
    assert!(!wiki_front_noop(&live, Some("/v/b"), "/v/a"), "設定が違う");
    assert!(!wiki_front_noop(&live, Some("/v/b"), "/v/b"), "監視は別の root");
    assert!(!wiki_front_noop(&live, None, "/v/a"));
    live.fail(g);
    assert!(!wiki_front_noop(&live, Some("/v/a"), "/v/a"), "監視の失敗");
    let g2 = live.begin(Some("/v/a".into()));
    live.armed(g2);
    live.stop();
    assert!(!wiki_front_noop(&live, Some("/v/a"), "/v/a"), "止めた");
}

/// 起動時の段取り（鍵の中で 家事 → 適用 = 監視を張る）の最中に画面の揃え（wiki_front_root = 同じ鍵）が来ると、揃えは鍵を待ち、
/// 適用の後に no-op の判定をする = 同じ root なら何もせず返る（ページの門は起動時の適用の後に開く）。鍵を取らない揃えは張る前を見る（対照）
#[test]
fn front_waits_for_startup_apply_under_roots_lock() {
    use std::sync::mpsc;
    let rt = Arc::new(WikiRuntime::default());
    let (held_tx, held_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let startup = {
        let rt = rt.clone();
        std::thread::spawn(move || {
            let _roots = rt.lock_roots();
            held_tx.send(()).unwrap();
            // 対照を見終えるまで張らない = 混んだ CI でも時間に依らない
            go_rx.recv().unwrap();
            // 家事（canonicalize・掃除）の代わり → 適用（wiki_apply の監視を張る段 = begin → armed）
            std::thread::sleep(Duration::from_millis(50));
            let g = rt.live.begin(Some("/v/a".into()));
            rt.live.armed(g);
        })
    };
    held_rx.recv().unwrap();
    assert!(!wiki_front_noop(&rt.live, Some("/v/a"), "/v/a"), "鍵を取らなければ適用の前を見る（対照）");
    go_tx.send(()).unwrap();
    let noop = {
        let _roots = rt.lock_roots();
        wiki_front_noop(&rt.live, Some("/v/a"), "/v/a")
    };
    assert!(noop, "揃えは起動時の段取りを待つ = 適用の後に同じ root なら no-op");
    startup.join().unwrap();
    // 毒（段の途中の panic）は無視して握れる
    let rt2 = rt.clone();
    let _ = std::thread::spawn(move || {
        let _roots = rt2.lock_roots();
        panic!("段の途中で落ちた");
    })
    .join();
    drop(rt.lock_roots());
}

/// 「索引を作り直す」の失敗から戻る段（Settings › 索引を削除 の手もここに含む）:
/// 書けない索引（chmod 444 = 別のユーザー / sudo で作られた・権限の違う復元）は core が退避もやり直しもしない → 段が消して 1 回だけやり直す。
/// 索引は一時のフォルダの作り物だけ（本物の索引・vault に触れない）
#[cfg(unix)]
#[test]
fn rebuild_steps_delete_and_retry_once() {
    use openref_core::wiki::index::{self, Ready};
    use openref_core::wiki::Fresh;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("rebuild-steps");
    let make = |name: &str| {
        let root = d.join(name).join("vault");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.md"), "# A\n[[b]]\n").unwrap();
        std::fs::write(root.join("b.md"), "# B\n").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let ip = index::index_path(&d.join(name).join("app"), &root);
        Ready::new(ip, root, Default::default())
    };
    let removes = Cell::new(0);
    let rebuilds = Cell::new(0);
    let real_remove = |p: &Path| {
        removes.set(removes.get() + 1);
        index::remove_index(p)
    };
    let real_rebuild = |at: &Ready| {
        rebuilds.set(rebuilds.get() + 1);
        wiki_scan(at, WikiScan::Rebuild, || {})
    };
    let reset = || {
        removes.set(0);
        rebuilds.set(0);
    };
    // 正常 = 消さない・やり直さない
    let at = make("ok");
    let other = make("other");
    let (r, retried) = wiki_rebuild_steps(&at.index, || real_rebuild(&at), real_remove);
    assert!(matches!(r, Ok(Fresh::Ok)), "{r:?}");
    assert_eq!((retried, removes.get(), rebuilds.get()), (None, 0, 1), "正常なのに消した / やり直した");
    // 書けない索引: このプロセスの接続を別の索引へ移してから（開いた接続は書けてしまう）索引のファイルを 444 に
    wiki_scan(&other, WikiScan::Rebuild, || {}).unwrap();
    for s in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{s}", at.index.display()));
        if p.exists() {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();
        }
    }
    let ro = std::fs::OpenOptions::new().write(true).open(&at.index).is_err();
    if ro {
        // 前提: core の作り直しだけでは戻らない（退避もやり直しもしない = 消さないと画面から戻れない）
        assert!(wiki_scan(&at, WikiScan::Rebuild, || {}).is_err(), "前提: 書けない索引を core が作り直せた");
        wiki_scan(&other, WikiScan::Rebuild, || {}).unwrap();
        reset();
        let (r, retried) = wiki_rebuild_steps(&at.index, || real_rebuild(&at), real_remove);
        assert!(matches!(r, Ok(Fresh::Ok)), "消してやり直しても作れない: {r:?}");
        assert_eq!((removes.get(), rebuilds.get()), (1, 2), "消す / やり直すが 1 回ずつでない");
        assert!(retried.as_deref().is_some_and(|w| w.to_lowercase().contains("readonly")), "最初の失敗の文が残らない: {retried:?}");
        assert!(std::fs::OpenOptions::new().write(true).open(&at.index).is_ok(), "やり直した索引が書けない");
    } else {
        eprintln!("rebuild_steps: 444 でも書ける（root で走っている）= 書けない索引の場面は飛ばす");
    }
    // 別のプロセスが作り直し中（Ok(Rebuilding)）= 失敗でない = 消さない（その作り直しを壊さない）
    reset();
    let (r, retried) = wiki_rebuild_steps(&at.index, || Ok(Fresh::Rebuilding(30)), real_remove);
    assert!(matches!(r, Ok(Fresh::Rebuilding(30))), "{r:?}");
    assert_eq!((retried, removes.get()), (None, 0), "別のプロセスの作り直しの最中に索引を消した");
    // 消しても直らない（やり直しも失敗）= Err・消すのもやり直すのも 1 回だけ。索引の所為の失敗の本物の値 = ディレクトリを索引として開く
    // （CANTOPEN）を core の Db::open で作る
    reset();
    let cantopen = || openref_core::Db::open(&d).err().expect("ディレクトリを DB として開けた");
    assert!(wiki_index_file_fault(&cantopen()), "前提: CANTOPEN が索引の所為と判定されない: {}", cantopen());
    let fail = || -> openref_core::Result<Fresh> {
        rebuilds.set(rebuilds.get() + 1);
        Err(cantopen())
    };
    let (r, retried) = wiki_rebuild_steps(&at.index, fail, |_| {
        removes.set(removes.get() + 1);
        Ok(())
    });
    assert!(r.is_err() && retried.is_some(), "{r:?} {retried:?}");
    assert_eq!((removes.get(), rebuilds.get()), (1, 2), "2 回以上やり直した / 消した");
    // 消せない（索引の置き場のフォルダが書けない等）= その Err・やり直さない
    reset();
    let (r, retried) = wiki_rebuild_steps(&at.index, fail, |_| {
        removes.set(removes.get() + 1);
        Err(openref_core::CoreError::BadInput("消せない".into()))
    });
    assert!(r.as_ref().is_err_and(|e| e.to_string().contains("消せない")) && retried.is_some(), "{r:?}");
    assert_eq!((removes.get(), rebuilds.get()), (1, 1), "消せないのにやり直した");
    // 索引の所為でない失敗 = 消さない・その失敗をそのまま返す
    reset();
    let other_fail = || -> openref_core::Result<Fresh> {
        rebuilds.set(rebuilds.get() + 1);
        Err(openref_core::CoreError::BadInput("入力".into()))
    };
    let (r, retried) = wiki_rebuild_steps(&at.index, other_fail, real_remove);
    assert!(r.as_ref().is_err_and(|e| e.to_string().contains("入力")) && retried.is_none(), "{r:?} {retried:?}");
    assert_eq!((removes.get(), rebuilds.get()), (0, 1), "索引の所為でない失敗で索引を消した / やり直した");
    // vault が読めない（本物 = root を 000 に = scan::walk の Io「ワークスペース … を読めません」）= 消さない・文が vault の読みの失敗を言う
    let vault = make("vault-unreadable");
    wiki_scan(&vault, WikiScan::Rebuild, || {}).unwrap();
    std::fs::set_permissions(&vault.root, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&vault.root).is_err() {
        reset();
        let (r, retried) = wiki_rebuild_steps(&vault.index, || real_rebuild(&vault), real_remove);
        let msg = wiki_rebuild_message(false, &r, retried.as_deref(), WIKI_SCAN_FAIL_BUILD).unwrap_or_default();
        std::fs::set_permissions(&vault.root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err() && retried.is_none(), "vault が読めないのに消した / やり直した: {r:?}");
        assert_eq!((removes.get(), rebuilds.get()), (0, 1), "vault が読めないのに索引を消した");
        assert!(msg.contains("を読めません") && !msg.contains("消してやり直しても"), "文が vault の読みの失敗を言わない: {msg}");
        assert!(vault.index.exists(), "vault が読めないのに索引のファイルが消えた");
    } else {
        std::fs::set_permissions(&vault.root, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("rebuild_steps: 000 でも読める（root で走っている）= vault が読めない場面は飛ばす");
    }
    // 消してやり直して直ったときだけ toast（普通の成功・失敗・Rebuilding では出さない）
    assert!(wiki_rebuild_recovered(&Ok(Fresh::Ok), Some("ro")));
    assert!(!wiki_rebuild_recovered(&Ok(Fresh::Ok), None), "普通の成功で「索引のファイルを消して作り直しました」と出す");
    assert!(!wiki_rebuild_recovered(&Ok(Fresh::Rebuilding(3)), Some("ro")));
    assert!(!wiki_rebuild_recovered(&Err(cantopen()), Some("ro")));
    // 知らせの文 = やり直したことと最初の失敗を添える（握りつぶさない）・やり直していない / 成功なら前の文のまま
    let err = || -> openref_core::Result<Fresh> { Err(openref_core::CoreError::BadInput("x".into())) };
    let ja = wiki_rebuild_message(false, &err(), Some("attempt to write a readonly database"), WIKI_SCAN_FAIL_BUILD).unwrap();
    assert!(ja.starts_with("索引を作れません: ") && ja.contains("消してやり直しても") && ja.contains("readonly"), "{ja}");
    let en = wiki_rebuild_message(true, &err(), Some("ro"), WIKI_SCAN_FAIL_BUILD).unwrap();
    assert!(en.contains("retrying") && en.contains("first error: ro"), "{en}");
    assert_eq!(wiki_rebuild_message(false, &err(), None, WIKI_SCAN_FAIL_BUILD), wiki_scan_message(false, &err(), WIKI_SCAN_FAIL_BUILD));
    assert_eq!(wiki_rebuild_message(false, &Ok(Fresh::Ok), Some("ro"), WIKI_SCAN_FAIL_BUILD), None, "やり直して直ったのに失敗を知らせた");
    let _ = std::fs::remove_dir_all(&d);
}

/// 索引のファイルを消す種類: SQLite の主な結果コードのうち索引のファイルそのものの所為の物だけ。
/// 起こせる種類は本物のエラー（READONLY は rebuild_steps の試験・CANTOPEN = ディレクトリを DB として開く・NOTADB = DB でないファイル）、
/// 起こせない種類（BUSY・LOCKED・IOERR・CORRUPT・FULL）は結果コードの値を直接渡す
#[test]
fn index_file_fault_kinds() {
    for (code, want, name) in [
        (5, false, "BUSY"),
        (6, false, "LOCKED"),
        (8, true, "READONLY"),
        (10, true, "IOERR"),
        (11, true, "CORRUPT"),
        (13, true, "FULL"),
        (14, true, "CANTOPEN"),
        (26, true, "NOTADB"),
        (1, false, "ERROR"),
        (19, false, "CONSTRAINT"),
        (0, false, "OK"),
    ] {
        assert_eq!(wiki_index_fault_code(code), want, "{name}（{code}）");
    }
    let d = scratch("fault-kinds");
    // CANTOPEN（本物）
    let e = openref_core::Db::open(&d).err().expect("ディレクトリを DB として開けた");
    assert!(wiki_index_file_fault(&e), "CANTOPEN を索引の所為と見ない: {e}");
    // NOTADB（本物 = DB でないファイル）
    let junk = d.join("junk.sqlite");
    std::fs::write(&junk, vec![0x41u8; 8192]).unwrap();
    let e = openref_core::Db::open(&junk).err().expect("DB でないファイルを開けた");
    assert!(wiki_index_file_fault(&e), "NOTADB を索引の所為と見ない: {e}");
    // SQLite の外の失敗（vault の読み = Io・入力）= 索引の所為でない
    assert!(!wiki_index_file_fault(&openref_core::CoreError::Io(std::io::Error::other("ワークスペース /v を読めません"))));
    assert!(!wiki_index_file_fault(&openref_core::CoreError::BadInput("x".into())));
    let _ = std::fs::remove_dir_all(&d);
}

/// OpenNote とチャットの画面へ返す失敗の文 = ワークスペース・参照フォルダのパスはフォルダ名まで（親のパス = 偽の HOME の
/// base を含まない）・理由は残す・エージェント向けの手がかり（「root には wiki_status の … をそのまま渡す」）は出さない。
/// core の文（MCP / CLI）は手がかりもフルパスも持ったまま（ここでも確かめる）
#[test]
fn failures_hide_root_parents_and_agent_hints() {
    use openref_core::wiki::workspaces;
    let (base, home, app, db) = ws_env("x29");
    let b = base.to_string_lossy().to_string();
    let notes = home.join("Documents/notes");
    let site = home.join("Documents/site");
    let other = home.join("Documents/other");
    let hidden = home.join(".秘密/x");
    for d in [&notes, &site, &other, &hidden] {
        std::fs::create_dir_all(d).unwrap();
    }
    let s = |p: &Path| p.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &s(&notes)).unwrap();
        let ws = serde_json::json!({s(&notes): {"last_used": 100}, s(&site): {"last_used": 50}});
        g.set_setting(workspaces::KEY, &ws.to_string()).unwrap();
    }
    // 画面の文 = 親のパスも手がかりも無い・want を含む
    let ok = |e: &str, want: &str| {
        assert!(!e.contains(&b) && !e.contains("_status") && !e.contains("そのまま渡す") && e.contains(want), "{e}");
    };
    let msg = |e: &str| match serde_json::from_str::<serde_json::Value>(e) {
        Ok(v) => v["message"].as_str().unwrap().to_string(),
        Err(_) => e.to_string(),
    };
    // core の文（MCP / CLI）は手がかりもフルパスも持つ = 変えていない
    let core = openref_core::wiki::commands::known_root(&ws_map(&db), &s(&other)).unwrap_err();
    assert_eq!(core.to_string(), format!("履歴に無いワークスペースです: {}（{}）", s(&other), openref_core::wiki::commands::ROOT_HINT));
    // wiki_err_in = JSON の message を畳む・手がかりを外す（code は残す）
    let e = wiki_err_in(core, &[&other]);
    assert_eq!(err_code(&e).as_deref(), Some("unknown_root"));
    assert_eq!(msg(&e), "履歴に無いワークスペースです: other");
    // wiki_err（畳むパス無し）も手がかりは外す
    let e = wiki_err(openref_core::wiki::commands::known_root(&ws_map(&db), "/x/y").unwrap_err());
    assert_eq!(msg(&e), "履歴に無いワークスペースです: /x/y");
    // root の段（Front = 履歴に無い・Choose = 無い / 隠し / 消えた）
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &other, RootChange::Front, 999, false);
    let e = r.unwrap_err();
    assert_eq!(err_code(&e).as_deref(), Some("unknown_root"));
    assert_eq!(msg(&e), "履歴に無いワークスペースです: other");
    for (p, want) in [
        (home.join("Documents/nope"), "nope は使えません（見つかりません）"),
        (hidden.clone(), "x は使えません（`.` で始まるフォルダ（隠しフォルダ）の中です）"),
    ] {
        let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &p, RootChange::Choose, 999, false);
        ok(&r.unwrap_err(), want);
    }
    // 履歴の root の解決（wiki_pin・wiki_ws_target）= 手がかりを外し名前まで
    let e = wiki_pin(ctx_home(&db, &home), Some(&s(&other))).unwrap_err();
    assert_eq!(msg(&e), "履歴に無いワークスペースです: other");
    let e = wiki_ws_target(&ws_map(&db), Some(&s(&other)), None).unwrap_err();
    assert_eq!(msg(&e), "履歴に無いワークスペースです: other");
    // チャットの [元に戻す] の root（履歴から外した子の root）
    let e = wiki_chat_undo_ctx(ctx_home(&db, &home), Some(&other)).unwrap_err();
    assert_eq!(e, "履歴に無いワークスペースです: other");
    // ready（設定の root が消えた）= 名前と理由
    std::fs::remove_dir_all(&site).unwrap();
    let mut c = ctx_home(&db, &home);
    c.root = Some(s(&site));
    let e = wiki_ready(&c).err().unwrap();
    ok(&e, "ワークスペースのフォルダに site は使えません（見つかりません）");
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &site, RootChange::Front, 999, false);
    ok(&r.unwrap_err(), "site は使えません（見つかりません）");
    // wiki_run の畳むパス = その呼び出しの root・引数の root・引数の abs（絶対パスだけ）
    let a = serde_json::json!({"root": "/v/b", "abs": "/Users/me/Downloads/x.pdf", "path": "/p"});
    assert_eq!(
        wiki_run_paths(Some("/v/a"), Some(Path::new("/Users/me/Library/Application Support/OpenRef")), &a),
        vec![PathBuf::from("/v/a"), PathBuf::from("/v/b"), PathBuf::from("/Users/me/Downloads/x.pdf"), PathBuf::from("/Users/me/Library/Application Support/OpenRef")]
    );
    assert_eq!(wiki_run_paths(None, None, &serde_json::json!({"abs": "x.pdf"})), Vec::<PathBuf>::new());
    // 参照フォルダの検査の文（[追加…] の殻が選んだフォルダで畳む）: 無い・隠し・ワークスペースの中（symlink 越し = 文は canonical）
    let canon_home = std::fs::canonicalize(&home).unwrap();
    let canon_notes = std::fs::canonicalize(&notes).unwrap();
    std::fs::create_dir_all(notes.join("sub")).unwrap();
    let link = home.join("近道");
    std::os::unix::fs::symlink(notes.join("sub"), &link).unwrap();
    for (p, want) in [
        (home.join("無い"), "参照フォルダに 無い は使えません（見つかりません）"),
        (hidden.clone(), "参照フォルダに x は使えません"),
        (link.clone(), "参照フォルダに sub は使えません（ワークスペースの中です"),
    ] {
        let e = wiki_ref_dir_check(&p, &canon_home, Some(&canon_notes), None, false).unwrap_err();
        assert!(e.contains(&b), "畳む前の文（stderr のログと同じ）はフルパス: {e}");
        ok(&crate::hide_paths(e, &[&p]), want);
    }
    // 殻の配線（command の本体が畳む = 呼び手の AppHandle 無しでは走らせられない所）
    let main = include_str!("../main.rs");
    let body = |src: &'static str, head: &str| -> String {
        let s = &src[src.find(head).unwrap_or_else(|| panic!("無い: {head}"))..];
        s[..s.find("\n}\n").unwrap()].chars().filter(|c| !c.is_whitespace()).collect()
    };
    // 字で確かめる配線 = AppHandle / asset scope / 実物の claude の検出が要る所だけ（出口の本体は下の
    // exits_fold_real_failures が実際に失敗を起こして確かめる）
    for (src, head, want) in [
        (main, "async fn wiki_ref_dir_add(", "wiki_ref_dir_add_picked(&app,&ctx,root,&p,en).map_err(|e|hide_paths(e,&[&p]))"),
        (main, "async fn wiki_ref_dir_remove(", "wiki_ref_dir_remove_now(&app,root.as_deref(),&path).map_err(|e|hide_paths(e,&[&path]))"),
        (main, "async fn wiki_read_ref(", "blocking(move||wiki_read_ref_now(&wiki_ref_dirs(&app,root.as_deref())?,&abs,max_chars,probe,en))"),
        (main, "async fn wiki_open_path(", "let(abs,reveal)=wiki_open_resolve(&wiki_ready(&ctx)?.root,&path,reveal==Some(true),en)?;"),
        (main, "async fn wiki_pick_root(", "letdetail=wiki_root_toast_detail(en,&root,stale);"),
        (main, "async fn wiki_switch_root(", "letdetail=wiki_root_toast_detail(en,&root,stale);"),
        (main, "async fn shelf_set_opts(", "r.map_err(|e|shelf_hide_arg_root(e,&args))"),
        // apply は asset scope に足す = AppHandle が要る
        (include_str!("../wiki_gui.rs"), "fn wiki_change_root_locked(", "wiki_apply(app,WikiScan::Refresh).map_err(|e|hide_paths(e,&[path,&root]))?;"),
        (include_str!("../wiki_gui.rs"), "pub(crate) fn wiki_enable_with(", "wiki_enable_check(wiki_ctx(app)?)?;"),
        (include_str!("../wiki_gui.rs"), "pub(crate) fn wiki_front_workspace(", "wiki_front_check(&ctx,root)?;"),
        (include_str!("../wiki_gui.rs"), "pub(crate) async fn wiki_run(", "blocking(move||wiki_run_now(&db,&live,validate,args,f))"),
        // チャットの状態は実物の claude を探して起こす（auth status）= 走らせない
        (include_str!("../wiki_chat_gui.rs"), "pub(crate) fn wiki_chat_status_now(", "letready=wiki_ready(&ctx);letreason=wiki_chat_reason("),
    ] {
        assert!(body(src, head).contains(want), "{head} が畳まない: {want}");
    }
    // 除外の書き込みは ready を 2 度読む（検査と書いた後）= どちらも wiki_ready（1 度目は exits_fold_real_failures が実際に落とす）
    assert_eq!(body(include_str!("../wiki_gui.rs"), "pub(crate) fn wiki_exclude_write(").matches("wiki_ready(&wiki_pin(ctx,root_arg)?)?").count(), 2);
    // "wiki-error"（監視・索引の取り込みの失敗）= stderr はフルパス・画面へは root を畳んでから
    let gui = include_str!("../wiki_gui.rs");
    assert!(
        body(gui, "fn wiki_error(").contains("eprintln!(\"openref-app:OpenNote:{msg}\");wiki_emit(app,\"wiki-error\",hide_paths(msg,&[root,index]));"),
        "wiki_error が畳まない"
    );
    let chat = include_str!("../wiki_chat.rs");
    let chat_sq: String = chat.chars().filter(|c| !c.is_whitespace()).collect();
    for want in [
        "crate::hide_paths(format!(\"{}:{e}\",l.cli.path.display()),&[l.cli.path.as_path()])",
        "Some(lg)=>crate::hide_paths(e,&[lg.root.as_path()]),",
        "Err(e)=>returnUndo::Refused(crate::wiki_gui::wiki_ready_err(ctx,e)),",
        "Err(e)=>returnrefused(crate::wiki_gui::wiki_ready_err(ctx,e)),",
    ] {
        assert!(chat_sq.contains(want), "チャットの文が畳まない: {want}");
    }
    let _ = app;
    let _ = std::fs::remove_dir_all(&base);
}

/// wiki_err（GUI の出口）はエージェント向けの手がかりを外す・code は残す（core の文 = MCP / CLI は手がかりまで）
#[test]
fn wiki_err_drops_agent_hint() {
    let m = openref_core::wiki::workspaces::parse(Some(r#"{"/v/a": {"last_used": 1}}"#)).unwrap();
    let e = openref_core::wiki::commands::known_root(&m, "/v/b").unwrap_err();
    assert_eq!(e.to_string(), format!("履歴に無いワークスペースです: /v/b（{}）", openref_core::wiki::commands::ROOT_HINT));
    let v: serde_json::Value = serde_json::from_str(&wiki_err(e)).unwrap();
    assert_eq!(v, serde_json::json!({"code": "unknown_root", "message": "履歴に無いワークスペースです: /v/b"}));
}

/// 画面へ返す失敗の出口ごとに実フォルダのフルパスで実際に失敗を起こし、返る字に親のパス（偽の HOME の base）が無い（名前と理由は残る）こと
/// （字面の照合でなく走らせる。AppHandle の要る所の配線は failures_hide_root_parents_and_agent_hints が字で見る）
#[test]
fn exits_fold_real_failures() {
    use openref_core::wiki::workspaces;
    let (base, home, app, db) = ws_env("x29-exits");
    let b = base.to_string_lossy().to_string();
    let notes = home.join("Documents/notes");
    let gone = home.join("Documents/消えた vault");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::create_dir_all(&gone).unwrap();
    let s = |p: &Path| p.to_string_lossy().to_string();
    let set = |root: &Path| {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &s(root)).unwrap();
        let ws = serde_json::json!({s(&notes): {"last_used": 2}, s(&gone): {"last_used": 1}});
        g.set_setting(workspaces::KEY, &ws.to_string()).unwrap();
    };
    let ok = |e: &str, want: &str| {
        assert!(!e.contains(&b) && !e.contains("そのまま渡す") && e.contains(want), "{e}");
    };
    let msg = |e: &str| match serde_json::from_str::<serde_json::Value>(e) {
        Ok(v) => v["message"].as_str().unwrap_or(e).to_string(),
        Err(_) => e.to_string(),
    };
    std::fs::remove_dir_all(&gone).unwrap();
    let live = WikiLive::default();
    // wiki_status の Ok の応答の error（サイドバーにずっと出る）= 名前まで・root の欄（データ）はフルパスのまま
    set(&gone);
    let v = wiki_run_now(&db, &live, Some("wiki_status"), None, openref_core::wiki::commands::status).unwrap();
    ok(v["error"].as_str().unwrap(), "ワークスペースのフォルダに 消えた vault は使えません（見つかりません）");
    assert_eq!(v["root"], serde_json::json!(s(&gone)), "データの欄は変えない");
    // core の文（MCP）はフルパスのまま
    let core = openref_core::wiki::commands::status(&ctx_home(&db, &home), &serde_json::json!({})).unwrap();
    assert!(core["error"].as_str().unwrap().contains(&s(&gone)));
    // wiki_run の Err: 履歴に無い args.root・ワークスペースの外から取り込む無いファイル（args.abs）
    set(&notes);
    let other = home.join("Documents/other");
    let e = wiki_run_now(&db, &live, None, Some(serde_json::json!({"root": s(&other)})), openref_core::wiki::commands::status).unwrap_err();
    assert_eq!(msg(&e), "履歴に無いワークスペースです: other");
    // core の関数の中の失敗（ready = この試験の root は本物の HOME の外 = 「ホームの外です」で落ちる）も root は名前まで。
    // args.abs を畳む物に入れる事は wiki_run_paths の表明（本物の HOME を偽にせずに取り込みまで進めない）
    let abs = home.join("Downloads/無い 図.png");
    let e = wiki_run_now(&db, &live, None, Some(serde_json::json!({"abs": s(&abs)})), openref_core::wiki::edit::import_file).unwrap_err();
    ok(&msg(&e), "ワークスペースのフォルダに notes は使えません");
    // 有効化の検査（wiki_enable_with）・タブの行き来（wiki_front_workspace）
    set(&gone);
    ok(&wiki_enable_check(ctx_home(&db, &home)).unwrap_err(), "消えた vault は使えません（見つかりません）");
    set(&notes);
    let e = wiki_front_check(&ctx_home(&db, &home), &s(&other)).unwrap_err();
    assert_eq!(err_code(&e).as_deref(), Some("unknown_root"));
    assert_eq!(msg(&e), "履歴に無いワークスペースです: other");
    // 索引から除く（wiki_exclude_write の 1 度目の ready = 固定した root が消えた）
    let (_, r) = wiki_exclude_write(&db, ctx_home(&db, &home), Some(&s(&gone)), "sub", true, 9, false);
    ok(&r.unwrap_err(), "消えた vault は使えません（見つかりません）");
    // 選んだ / 切り替えた toast の detail
    assert_eq!(wiki_root_toast_detail(false, &notes, false), "notes");
    // 外部で開く / Finder で表示（本文の file:// の外の無いファイル・中の無いファイル）
    let canon_notes = std::fs::canonicalize(&notes).unwrap();
    let outside = home.join("Downloads/資料 1.pdf");
    ok(&wiki_open_resolve(&canon_notes, &s(&outside), false, false).unwrap_err(), "資料 1.pdf");
    ok(&wiki_open_resolve(&canon_notes, &s(&canon_notes.join("無い.pdf")), true, false).unwrap_err(), "無い.pdf");
    // 参照フォルダの中のファイル（参照フォルダの外 = 外した後に復元したタブ・中の無いファイル）
    let refd = home.join("Refs");
    std::fs::create_dir_all(&refd).unwrap();
    let refs = [std::fs::canonicalize(&refd).unwrap()];
    ok(&crate::wiki_read_ref_now(&refs, &s(&home.join("Old refs/x.pdf")), 10, false, false).unwrap_err(), "参照フォルダの中ではありません: x.pdf");
    ok(&crate::wiki_read_ref_now(&refs, &s(&refs[0].join("sub/無い.pdf")), 10, false, false).unwrap_err(), "無い.pdf");
    // チャット: ログインのスクリプトを書けない（app_dir が無い = 書けない = Terminal は開かない）
    let cli = wiki_chat::Cli { path: PathBuf::from("/nonexistent/claude"), path_env: Default::default() };
    ok(&wiki_chat::open_login_terminal(&cli, &base.join("無い app")).unwrap_err(), "claude-login.command: ");
    let _ = app;
    let _ = std::fs::remove_dir_all(&base);
}

/// Ok の応答の失敗の欄を畳むのは決まった位置だけ（最上位の error / index_error・改名の
/// skipped[].reason）・利用者のデータ（frontmatter の同じ名前の鍵）は畳まない・開けない索引の文のアプリの置き場（app_dir =
/// 利用者名を含む）も名前まで。wiki_run_now と同じ組み（core の関数 → wiki_hide_fields / wiki_err_in に wiki_run_paths）を
/// 偽の HOME の Ctx で走らせる（wiki_run_now は本物の HOME を読む = 偽の HOME の root は「ホームの外」で落ちる）
#[test]
fn fail_fields_positions_and_app_dir() {
    use std::os::unix::fs::PermissionsExt;
    let (base, home, app, db) = ws_env("x29-fields");
    let b = base.to_string_lossy().to_string();
    let notes = std::fs::canonicalize({
        let k = home.join("Documents/notes");
        std::fs::create_dir_all(&k).unwrap();
        k
    })
    .unwrap();
    let s = |p: &Path| p.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &s(&notes)).unwrap();
    }
    let ctx = ctx_home(&db, &home);
    let run = |f: WikiFn, a: serde_json::Value| -> Result<serde_json::Value, String> {
        let paths = wiki_run_paths(ctx.root.as_deref(), ctx.app_dir.as_deref(), &a);
        let paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        f(&ctx, &a).map(|v| wiki_hide_fields(v, &paths)).map_err(|e| wiki_err_in(e, &paths))
    };
    let clean = |t: &str| assert!(!t.contains(&b), "親のパスが残る: {t}");
    // frontmatter に error / reason の鍵（値にワークスペースのフルパス）があっても、読んだ値はそのまま
    let fm_val = format!("{}/x", s(&notes));
    std::fs::write(notes.join("fm.md"), format!("---\nerror: \"{fm_val}\"\nmeta:\n  reason: \"{fm_val}\"\n---\n# FM\n")).unwrap();
    let v = run(openref_core::wiki::commands::read, serde_json::json!({"path": "fm.md"})).unwrap();
    let fm = serde_json::to_string(&v["frontmatter"]).unwrap();
    assert!(fm.matches(&*fm_val).count() >= 2, "利用者のデータを畳んだ: {fm}");
    // 改名の skipped[].reason（実際に断らせる = 書き換える先のページを読み取り専用に）
    std::fs::write(notes.join("a.md"), "# A\n").unwrap();
    std::fs::write(notes.join("b.md"), "[[a]]\n").unwrap();
    run(openref_core::wiki::commands::list, serde_json::json!({})).unwrap(); // 索引を作る（改名の書き換えの計画は索引から）
    std::fs::set_permissions(notes.join("b.md"), std::fs::Permissions::from_mode(0o444)).unwrap();
    let v = run(openref_core::wiki::edit::rename, serde_json::json!({"from": "a.md", "to": "c.md", "rewrite": true})).unwrap();
    let reason = v["skipped"][0]["reason"].as_str().unwrap_or_else(|| panic!("断らない: {v}")).to_string();
    assert!(reason.contains("書込権がありません"), "{reason}");
    // core の今の reason はパスを持たない（io の文・固定の文）= 位置の表明は実物の結果の reason に vault のパスを足して確かめる
    let mut v2 = v.clone();
    v2["skipped"][0]["reason"] = serde_json::json!(format!("{reason}: {}", s(&notes.join("b.md"))));
    v2["frontmatter"] = serde_json::json!({"reason": s(&notes)}); // 決まった位置の外 = 畳まない
    let paths = wiki_run_paths(ctx.root.as_deref(), ctx.app_dir.as_deref(), &serde_json::json!({}));
    let paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let v2 = wiki_hide_fields(v2, &paths);
    assert_eq!(v2["skipped"][0]["reason"], serde_json::json!(format!("{reason}: notes/b.md")));
    assert_eq!(v2["frontmatter"]["reason"], serde_json::json!(s(&notes)), "決まった位置の外を畳んだ");
    std::fs::set_permissions(notes.join("b.md"), std::fs::Permissions::from_mode(0o644)).unwrap();
    // 索引の置き場を開けない（chmod 000）= 書いた後の index_error に <app_dir>/wiki-index/….sqlite。
    // 索引の接続はプロセスの中で使い回されうる = まだ開いていない別のワークスペース（notes2）で
    let notes2 = std::fs::canonicalize({
        let k = home.join("Documents/notes2");
        std::fs::create_dir_all(&k).unwrap();
        k
    })
    .unwrap();
    let mut ctx2 = ctx_home(&db, &home);
    ctx2.root = Some(s(&notes2));
    let run2 = |f: WikiFn, a: serde_json::Value| -> Result<serde_json::Value, String> {
        let paths = wiki_run_paths(ctx2.root.as_deref(), ctx2.app_dir.as_deref(), &a);
        let paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        f(&ctx2, &a).map(|v| wiki_hide_fields(v, &paths)).map_err(|e| wiki_err_in(e, &paths))
    };
    let wi = app.join("wiki-index");
    std::fs::set_permissions(&wi, std::fs::Permissions::from_mode(0o000)).unwrap();
    let r = run2(openref_core::wiki::edit::write, serde_json::json!({"path": "d.md", "content": "# D\n"}));
    let r2 = run2(openref_core::wiki::commands::list, serde_json::json!({}));
    let core = openref_core::wiki::edit::write(&ctx2, &serde_json::json!({"path": "e.md", "content": "# E\n"}));
    std::fs::set_permissions(&wi, std::fs::Permissions::from_mode(0o755)).unwrap();
    let v = r.unwrap();
    let ie = v["index_error"].as_str().unwrap_or_else(|| panic!("index_error が無い: {v}"));
    clean(ie);
    assert!(ie.contains("app/wiki-index/") && ie.contains(".sqlite"), "{ie}");
    // core の文（MCP）は置き場のフルパスを持つ = 変えていない
    let core = core.unwrap();
    assert!(core["index_error"].as_str().unwrap().contains(&s(&app)), "{core}");
    // 索引を開けない読み（一覧）= Err の文も
    let e = r2.unwrap_err();
    clean(&e);
    assert!(e.contains("app/wiki-index/"), "{e}");
    let _ = std::fs::remove_dir_all(&base);
}

/// 索引を開く（core の索引の層をそのまま = 開けなければ「database error: unable to open database file: <app_dir>/wiki-index/….sqlite」）。
/// Ctx::ready の HOME の検査は通さない（wiki_run_now は本物の HOME を読む = 偽の HOME の root は ready の手前で落ちる）
fn open_index_of(ctx: &openref_core::wiki::Ctx, _a: &serde_json::Value) -> openref_core::Result<serde_json::Value> {
    let root = PathBuf::from(ctx.root.clone().unwrap());
    let ip = openref_core::wiki::index::index_path(ctx.app_dir.as_deref().unwrap(), &root);
    let at = openref_core::wiki::index::Ready::new(ip, root, openref_core::wiki::scan::Rules::default());
    openref_core::wiki::with_index(&at, |_| Ok(())).map(|()| serde_json::json!({}))
}

/// アプリの置き場（app_dir = 利用者名を含む）の下のフルパスを含む失敗を実際に起こし、
/// 本体の出口（wiki_run_now・wiki_ready_err・[元に戻す] の undo_change）を通した字にそのパスが無い事。索引の置き場を chmod 000 に
/// して索引を開かせる（root は app_dir の外 = root を畳んでも置き場のパスは残る = app_dir を畳む配線だけが消す）
#[test]
fn app_dir_folds_through_real_exits() {
    use std::os::unix::fs::PermissionsExt;
    let (base, home, app, db) = ws_env("x29-appdir");
    let notes = std::fs::canonicalize({
        let k = home.join("Documents/置き場の外の vault");
        std::fs::create_dir_all(&k).unwrap();
        k
    })
    .unwrap();
    let ks = notes.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &ks).unwrap();
        g.set_setting(openref_core::wiki::workspaces::KEY, &serde_json::json!({&ks: {"last_used": 1}}).to_string()).unwrap();
    }
    let app_s = app.to_string_lossy().to_string();
    let wi = app.join("wiki-index");
    std::fs::create_dir_all(&wi).unwrap();
    let ctx = ctx_home(&db, &home);
    std::fs::set_permissions(&wi, std::fs::Permissions::from_mode(0o000)).unwrap();
    // 起こした失敗そのもの（core の文 = MCP / CLI）は置き場のフルパスを持つ = 試験が本物の失敗を使っている事の確認
    let raw = open_index_of(&ctx, &serde_json::json!({})).unwrap_err();
    // wiki_run_now（本体）の Err
    let run = wiki_run_now(&db, &WikiLive::default(), None, None, open_index_of);
    // wiki_ready_err（本体）に同じ種類の失敗を渡す
    let ready = wiki_ready_err(&ctx, open_index_of(&ctx, &serde_json::json!({})).unwrap_err());
    // [元に戻す] の断る文（undo_change = 本体）= ゴミ箱へ移す所で置き場の下へ動かせない（chmod 000）失敗の文
    std::fs::write(notes.join("p.md"), "x").unwrap();
    let sha = openref_core::wiki::index::sha256_hex(b"x");
    let trash = |p: &Path| -> Result<String, String> {
        let dest = wi.join("trash/p.md");
        std::fs::rename(p, &dest).map(|()| String::new()).map_err(|e| format!("{}: {e}", dest.display()))
    };
    let undo = wiki_chat::undo_change(
        &ctx,
        &wiki_chat::Change::Create { path: "p.md".into(), sha256_after: sha },
        false,
        trash,
        |_: &str| -> Result<(), String> { panic!("呼ばない") },
    );
    std::fs::set_permissions(&wi, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(raw.to_string().contains(&format!("{app_s}/wiki-index/")), "起こした失敗が置き場のパスを持たない: {raw}");
    let e = run.unwrap_err();
    assert!(!e.contains(&app_s) && e.contains("app/wiki-index/"), "wiki_run_now が置き場を畳まない: {e}");
    assert!(!ready.contains(&app_s) && ready.contains("app/wiki-index/"), "wiki_ready_err が置き場を畳まない: {ready}");
    match undo {
        wiki_chat::Undo::Refused(s) => {
            assert!(!s.contains(&app_s) && s.contains("app/wiki-index/trash/p.md"), "[元に戻す] の断る文が置き場を畳まない: {s}")
        }
        other => panic!("断らない: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&base);
}
