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

/// 旧キー wiki_ref_dirs（以前の形・取り込みだけが読む）: 未設定・空・`[]` は空。解釈できない値は Err（黙って空にしない = 退避してから消す）
#[test]
fn ref_dirs_setting_is_a_json_array() {
    assert_eq!(wiki_ref_dirs_parse(None, false).unwrap(), Vec::<PathBuf>::new());
    assert_eq!(wiki_ref_dirs_parse(Some(""), false).unwrap(), Vec::<PathBuf>::new());
    assert_eq!(wiki_ref_dirs_parse(Some("  \n"), false).unwrap(), Vec::<PathBuf>::new());
    assert_eq!(wiki_ref_dirs_parse(Some("[]"), false).unwrap(), Vec::<PathBuf>::new());
    for broken in ["not json", "{\"a\":1}", "[1, 2]", "[\"/a\", null]", "\"/a\""] {
        let e = wiki_ref_dirs_parse(Some(broken), false).unwrap_err();
        assert!(e.starts_with("設定 wiki_ref_dirs を解釈できません"), "{broken}: {e}");
        let e = wiki_ref_dirs_parse(Some(broken), true).unwrap_err();
        assert!(e.starts_with("The wiki_ref_dirs setting could not be read"), "{broken}: {e}");
    }
    assert_eq!(
        wiki_ref_dirs_parse(Some("[\"/a/b\", \"\", \"/c\"]"), false).unwrap(),
        vec![PathBuf::from("/a/b"), PathBuf::from("/c")]
    );
    // 重複は 1 つに（一覧の描画の key）
    assert_eq!(
        wiki_ref_dirs_parse(Some("[\"/a\", \"/a\", \"/b\"]"), false).unwrap(),
        vec![PathBuf::from("/a"), PathBuf::from("/b")]
    );
}

/// [追加…] / [外す]: 現在の root の生の一覧に足す / から外す（検査に通らないものも残す）・すでにあれば書かない・他の root の一覧に触らない
#[test]
fn ref_dirs_for_add_rules() {
    use openref_core::wiki::workspaces;
    let kb = PathBuf::from("/kb");
    let gone = PathBuf::from("/Volumes/gone");
    let mut m = workspaces::parse(Some(
        r#"{"/a": {"last_used": 1, "ref_dirs": ["/Volumes/gone"]}, "/b": {"ref_dirs": ["/other"]}}"#,
    ))
    .unwrap();
    // 見えないディスクの参照フォルダは残したまま足す
    let plan = wiki_ref_dirs_for_add(&m, "/a", &kb);
    assert_eq!(
        plan,
        WikiRefAdd {
            dirs: vec![gone.clone(), kb.clone()],
            changed: true,
            kind: WikiDir::Ref,
        }
    );
    wiki_save_ref_dirs(&mut m, "/a", &plan.dirs);
    // すでにある = 書かない
    let again = wiki_ref_dirs_for_add(&m, "/a", &kb);
    assert!(!again.changed);
    assert_eq!(again.dirs, vec![gone.clone(), kb.clone()]);
    // 項目の無い root = 1 件
    let first = wiki_ref_dirs_for_add(&m, "/c", &kb);
    assert_eq!((first.dirs.clone(), first.changed), (vec![kb.clone()], true));
    // 他の root（/b）と last_used は変わらない
    assert_eq!(wiki_ref_dirs_of(&m, Some("/b")), vec![PathBuf::from("/other")]);
    assert_eq!(m["/a"]["last_used"], serde_json::json!(1));
    // [外す]: 外す・無いものは書かない・root 未設定は空
    assert_eq!(wiki_ref_dirs_for_remove(&mut m, Some("/a"), &gone), vec![kb.clone()]);
    assert_eq!(wiki_ref_dirs_of(&m, Some("/a")), vec![kb.clone()]);
    let before = m.clone();
    assert_eq!(wiki_ref_dirs_for_remove(&mut m, Some("/a"), Path::new("/nope")), vec![kb.clone()]);
    assert_eq!(m, before);
    assert!(wiki_ref_dirs_for_remove(&mut m, Some("/none"), &kb).is_empty());
    assert!(!m.contains_key("/none"), "外すだけで項目を作らない");
    assert!(wiki_ref_dirs_for_remove(&mut m, None, &kb).is_empty());
    assert!(wiki_ref_dirs_of(&m, None).is_empty());
}

/// 壊れた設定の退避先の鍵 = `<鍵>.broken-<UTC の日時>`
#[test]
fn ref_dirs_broken_key_is_utc_stamp() {
    assert_eq!(wiki_broken_key(WIKI_REF_DIRS_KEY, 0), "wiki_ref_dirs.broken-19700101T000000Z");
    assert_eq!(
        wiki_broken_key(WIKI_REF_DIRS_KEY, 1_793_491_200),
        "wiki_ref_dirs.broken-20261101T000000Z"
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

/// wiki_ws_tx: 旧キー wiki_ref_dirs の取り込み（現在の root に ref_dirs が無いときだけ・値を失わない）と削除・root 未設定なら触らない・
/// 読めない旧キーは退避して消す
#[test]
fn ws_tx_adopts_legacy_ref_dirs_once() {
    use openref_core::wiki::workspaces;
    let db = openref_core::Db::open_in_memory().unwrap();
    let read = |db: &openref_core::Db| {
        wiki_ws_tx(db, 0, false, None, |m, root| Ok(wiki_ref_dirs_of(m, root))).0.unwrap()
    };
    // root 未設定 = 取り込む先が無い = 旧キーは残す・wiki_workspaces は書かない
    db.set_setting(WIKI_REF_DIRS_KEY, r#"["/kb", "/Volumes/gone"]"#).unwrap();
    assert!(read(&db).is_empty());
    assert!(setting(&db, WIKI_REF_DIRS_KEY).is_some());
    assert_eq!(setting(&db, workspaces::KEY), None);
    // root を設定 → 読むときに取り込んで旧キーを消す（現在の root の分だけ）
    db.set_setting("wiki_root", "/v/a").unwrap();
    let (dirs, note) = wiki_ws_tx(&db, 0, false, None, |m, root| Ok(wiki_ref_dirs_of(m, root)));
    assert_eq!(dirs.unwrap(), vec![PathBuf::from("/kb"), PathBuf::from("/Volumes/gone")]);
    assert!(note.adopted_legacy && note.set_aside.is_empty());
    assert_eq!(setting(&db, WIKI_REF_DIRS_KEY), None, "旧キーは消える = 二重に持たない");
    let m = workspaces::parse(setting(&db, workspaces::KEY).as_deref()).unwrap();
    assert_eq!(workspaces::ref_dirs(&m, "/v/a"), vec!["/kb".to_string(), "/Volumes/gone".into()]);
    assert!(m["/v/a"].get("last_used").is_none(), "取り込みは履歴の時刻を作らない");
    // 別の root では見えない
    db.set_setting("wiki_root", "/v/b").unwrap();
    assert!(read(&db).is_empty());
    // 旧キーが（古い版で）また書かれても、現在の root に ref_dirs があれば採用せずに消す
    db.set_setting("wiki_root", "/v/a").unwrap();
    db.set_setting(WIKI_REF_DIRS_KEY, r#"["/stale"]"#).unwrap();
    let (dirs, note) = wiki_ws_tx(&db, 0, false, None, |m, root| Ok(wiki_ref_dirs_of(m, root)));
    assert_eq!(dirs.unwrap(), vec![PathBuf::from("/kb"), PathBuf::from("/Volumes/gone")]);
    assert!(!note.adopted_legacy);
    assert_eq!(setting(&db, WIKI_REF_DIRS_KEY), None);
    // 読めない旧キー = 退避して消す（値は退避先に残る）
    db.set_setting(WIKI_REF_DIRS_KEY, "not json").unwrap();
    let (_, note) = wiki_ws_tx(&db, 1_793_491_200, false, None, |m, root| Ok(wiki_ref_dirs_of(m, root)));
    let to = "wiki_ref_dirs.broken-20261101T000000Z";
    assert_eq!(note.set_aside.len(), 1);
    assert_eq!((note.set_aside[0].0.as_str(), note.set_aside[0].1.as_str()), (WIKI_REF_DIRS_KEY, to));
    assert_eq!(setting(&db, to).as_deref(), Some("not json"));
    assert_eq!(setting(&db, WIKI_REF_DIRS_KEY), None);
    assert_eq!(read(&db), vec![PathBuf::from("/kb"), PathBuf::from("/Volumes/gone")], "ref_dirs は無傷");
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
    db.set_setting(workspaces::KEY, r#"{"/v/a": {"last_used": 2}, "/v/b": {"last_used": 1, "ref_dirs": ["/kb"]}}"#).unwrap();
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
    assert_eq!(wiki_toast_detail(false, "blog".into(), None), "blog");
    assert_eq!(wiki_toast_detail(false, "blog".into(), Some("前の".into())), "blog（前の）");
    assert_eq!(wiki_toast_detail(true, "blog".into(), Some("Files".into())), "blog (Files)");
    assert_eq!(wiki_workspace_name(Path::new("/Users/u/Documents/blog")), "blog");
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
    // 旧の置き場（app_dir 直下）= wiki-index.sqlite の一族だけ。DB の openref.sqlite・-wal には触らない
    let app: Vec<String> = [
        "openref.sqlite",
        "openref.sqlite-wal",
        "openref.sqlite-shm",
        "wiki-index.sqlite",
        "wiki-index.sqlite-wal",
        "wiki-index.sqlite-shm",
        "wiki-index.sqlite.corrupt-1700000000",
        "wiki-index.sqlite.corrupt-1700000000-wal",
        "wiki-index",
        "wiki-history",
        "storage",
        "wiki-index.sqlite.bak",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut legacy = wiki_legacy_index_plan(&app);
    legacy.sort();
    assert_eq!(
        legacy,
        vec![
            "wiki-index.sqlite",
            "wiki-index.sqlite-shm",
            "wiki-index.sqlite-wal",
            "wiki-index.sqlite.corrupt-1700000000",
            "wiki-index.sqlite.corrupt-1700000000-wal",
        ]
    );
}

/// 起動時の掃除（実ファイル・偽の app_dir）: 旧の索引の一族は消え DB は残る・wiki-index/ は履歴と現在の root の索引を残して孤児だけ消す・
/// 掃除の開始より後に変更されたファイルは残す
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
    let hist = "/Users/u/Documents/blog";
    let orphan = "/Users/u/Documents/old";
    for r in [cur, hist, orphan] {
        let ip = index_path(&app, Path::new(r));
        touch(&ip);
        touch(&PathBuf::from(format!("{}-wal", ip.display())));
    }
    touch(&app.join(INDEX_DIR).join("README.txt"));
    let started = SystemTime::now() + Duration::from_secs(60); // ここまでに作ったものは「前から有った」
    let gone = wiki_drop_legacy_index(&app);
    assert_eq!(gone.len(), 4, "{gone:?}");
    for n in [INDEX_FILE, "wiki-index.sqlite-wal", "wiki-index.sqlite-shm", "wiki-index.sqlite.corrupt-1"] {
        assert!(!app.join(n).exists(), "{n}");
    }
    for n in ["openref.sqlite", "openref.sqlite-wal", "openref.sqlite-shm"] {
        assert!(app.join(n).exists(), "DB は残る: {n}");
    }
    assert!(wiki_drop_legacy_index(&app).is_empty(), "2 回目は何もしない");
    let keep = wiki_index_keep(&app, [cur, hist]);
    let swept = wiki_index_sweep(&app, &keep, started);
    assert_eq!(swept.len(), 2, "{swept:?}");
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
    assert!(wiki_drop_legacy_index(&empty).is_empty());
    let _ = std::fs::remove_dir_all(&app);
    let _ = std::fs::remove_dir_all(&empty);
}

/// 前の root の配信が残るか: 同じ root の選び直し = false・別の root = true・参照フォルダは数えない
#[test]
fn root_stale_counts_roots_only() {
    let a = PathBuf::from("/v/a");
    let b = PathBuf::from("/v/b");
    let kb = PathBuf::from("/v/kb");
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
    assert!(al.insert(&kb, WikiDir::Ref));
    assert!(!wiki_root_stale(&al, &a));
    assert!(wiki_root_stale(&al, &b));
    // 参照フォルダだけが配信中（root はまだ）= 選んだ root が何であれ stale でない
    let mut refs_only = WikiAllowed::default();
    refs_only.insert(&kb, WikiDir::Ref);
    assert!(!wiki_root_stale(&refs_only, &a));
    // 配信そのものは両方を数える
    assert!(al.contains(&kb) && al.contains(&a) && !al.contains(&b));
    assert!(!al.is_empty());
    assert_eq!(al.all(), vec![a.clone(), kb.clone()]);
    // 同じフォルダを 2 度足しても scope には 1 回・記録も 1 つ
    assert!(!al.insert(&a, WikiDir::Root));
    assert_eq!(al.roots, vec![a.clone()]);
    // 参照フォルダだった場所を root に選ぶ: scope には既にある（false）が root として記録される = その root の選び直しは stale でない
    assert!(!al.insert(&kb, WikiDir::Root));
    assert_eq!(al.roots, vec![a.clone(), kb.clone()]);
    assert_eq!(al.refs, vec![kb.clone()]);
    assert!(wiki_root_stale(&al, &kb), "前の root a の配信は残る");
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
        wiki_apply_refs(Ok(vec![PathBuf::from("/kb"), PathBuf::from("/gone")]), &exists),
        (vec![(PathBuf::from("/kb"), WikiDir::Ref)], None)
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
/// scope の中の symlink で外を指すものは 403（照合も open も canonicalize した 1 つの値 = A-12）
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

/// 監視の印（A-16）: 張り終えるまで・張った後の stat 走査（prime）の前は省かない。止めた・失敗・root が違う = 今どおり。
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
    let (ip, croot) = ctx.ready().unwrap();
    let live = WikiLive::default();
    let g = live.begin(ctx.root.clone());
    live.armed(g);
    // 別のスレッドが索引を 1.5 秒握る（背景の走査の代わり）
    let (tx, rx) = std::sync::mpsc::channel();
    let held = {
        let (ip, croot) = (ip.clone(), croot.clone());
        std::thread::spawn(move || {
            with_index(&ip, &croot, |_| {
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
    assert!(with_index(&ip, &croot, |ix| ix.page("a.md")).unwrap().is_some(), "prime の走査で索引に入る");
    let _ = std::fs::remove_dir_all(&base);
}

/// 参照フォルダの検査: root と同じ規則 + vault の中は不可
#[test]
fn ref_dir_check_rules() {
    let d = scratch("check");
    let home = d.join("home");
    let kb = home.join("kb");
    let vault = home.join("Documents").join("vault");
    let hidden = home.join(".claude");
    for p in [&kb, &vault, &hidden] {
        std::fs::create_dir_all(p).unwrap();
    }
    let canon_home = std::fs::canonicalize(&home).unwrap();
    let canon_vault = std::fs::canonicalize(&vault).unwrap();
    let ok = wiki_ref_dir_check(&kb, &canon_home, Some(&canon_vault), None, false).unwrap();
    assert_eq!(ok, std::fs::canonicalize(&kb).unwrap());
    // 相対・無い・ファイル・隠し・ホームそのもの・vault そのもの・vault の中
    assert!(wiki_ref_dir_check(Path::new("kb"), &canon_home, None, None, false).is_err());
    assert!(wiki_ref_dir_check(&home.join("nope"), &canon_home, None, None, false).is_err());
    std::fs::write(kb.join("f.txt"), "x").unwrap();
    assert!(wiki_ref_dir_check(&kb.join("f.txt"), &canon_home, None, None, false).is_err());
    assert!(wiki_ref_dir_check(&hidden, &canon_home, None, None, false)
        .unwrap_err()
        .contains("隠し"));
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
    let kb = std::fs::canonicalize(&d).unwrap().join("kb");
    let outside = std::fs::canonicalize(&d).unwrap().join("outside");
    std::fs::create_dir_all(kb.join("journals")).unwrap();
    std::fs::create_dir_all(kb.join(".git")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(kb.join("journals/a.pdf"), "%PDF").unwrap();
    std::fs::write(kb.join(".git/h.pdf"), "%PDF").unwrap();
    std::fs::write(outside.join("o.pdf"), "%PDF").unwrap();
    std::os::unix::fs::symlink(outside.join("o.pdf"), kb.join("journals/link.pdf")).unwrap();
    std::os::unix::fs::symlink(&outside, kb.join("odir")).unwrap();
    let dirs = vec![kb.clone()];
    let a = kb.join("journals/a.pdf").to_string_lossy().into_owned();
    let (root, canon) = wiki_ref_target(&dirs, &a, true).unwrap();
    assert_eq!((root, canon), (kb.clone(), kb.join("journals/a.pdf")));
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
            &format!("{}/journals/../journals/a.pdf", kb.display()),
            true,
        )
        .is_err(),
        "`..` の区画"
    );
    assert!(
        wiki_ref_target(&dirs, &kb.join(".git/h.pdf").to_string_lossy(), true).is_err(),
        "隠しの区画"
    );
    assert!(
        wiki_ref_target(&dirs, &kb.join("journals/link.pdf").to_string_lossy(), true)
            .unwrap_err()
            .contains("symlink"),
        "symlink の先が外"
    );
    assert!(
        wiki_ref_target(&dirs, &kb.join("odir/o.pdf").to_string_lossy(), true)
            .unwrap_err()
            .contains("symlink"),
        "symlink のフォルダ経由"
    );
    assert!(
        wiki_ref_target(&dirs, &kb.join("journals").to_string_lossy(), true).is_err(),
        "フォルダ"
    );
    assert!(
        wiki_ref_target(&dirs, &kb.to_string_lossy(), true).is_err(),
        "参照フォルダそのもの"
    );
    assert!(
        wiki_ref_target(&dirs, &outside.join("o.pdf").to_string_lossy(), true)
            .unwrap_err()
            .contains("not under"),
        "外"
    );
    assert!(
        wiki_ref_target(&dirs, &kb.join("journals/nope.pdf").to_string_lossy(), true)
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
        let plan = wiki_fs_plan(&root, true, vec![PathBuf::from(s)], wiki_fs_kind, |_| false);
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
wiki_open_target | symlinkで外 | Err("入力を解釈できません: ワークスペースの外を指しています（symlink）: <B>/home/Documents/vault/out-link.md")
wiki_ref_target | symlinkで外 | Err("参照フォルダの外です（symlink）: <B>/home/Documents/vault/out-link.md")
wiki_fs_plan | symlinkで外 | ([("out-link.md", "<B>/home/Documents/vault/out-link.md")], false)
wiki_open_target | symlinkで中 | Ok(("<B>/home/Documents/vault/a.pdf", false))
wiki_ref_target | symlinkで中 | Ok(("<B>/home/Documents/vault", "<B>/home/Documents/vault/a.pdf"))
wiki_fs_plan | symlinkで中 | ([("in-link.pdf", "<B>/home/Documents/vault/in-link.pdf")], false)
wiki_open_target | symlinkで隠し | Err("入力を解釈できません: このパスは読めません: <B>/home/Documents/vault/vis-link.md")
wiki_ref_target | symlinkで隠し | Err("参照フォルダの外です（symlink）: <B>/home/Documents/vault/vis-link.md")
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

/// 旧の手書きの percent_decode（percent_encoding に置き換える前の写し）= 同値の基準
fn percent_decode_old(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// percent_decode は旧の手書きと同じ（境界 = 末尾の `%` `%2`・16 進でない・大小・壊れた UTF-8・`+`・空 + 乱数）
#[test]
fn percent_decode_matches_old() {
    for c in [
        "", "%", "%2", "%zz", "%C3%A9", "%c3%a9", "%C3", "%FF%FE", "%E6%97%A5%2", "a%zz%41", "%あ%4",
        "+", "a+b%20c", "%%41", "%4", "%41", "x%41", "%2F..%2e%2E", "%00", "%E6%97", "日本%E8%AA%9E", "%G0",
        "%0G", "100%", "%%", "%%%", "%é1", "%1é",
    ] {
        assert_eq!(percent_decode(c), percent_decode_old(c), "{c:?}");
    }
    let alphabet: Vec<char> = "%%%%%0123456789abcdefABCDEFgGz+/. 日é".chars().collect();
    let mut x = fastrand_u64() | 1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..50_000 {
        let n = next() % 14;
        let s: String = (0..n)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        assert_eq!(percent_decode(&s), percent_decode_old(&s), "{s:?}");
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

/// 旧の wiki_read_ref の html の欄（core の body_fields に寄せる前の写し）
fn ref_html_fields_old(
    out: &mut serde_json::Value,
    kind: openref_core::wiki::scan::Kind,
    bytes: &[u8],
    max_chars: usize,
) {
    let parsed = openref_core::wiki::index::parse_bytes(kind, bytes);
    let text = String::from_utf8_lossy(bytes);
    let chars = text.chars().count();
    if let Some(t) = parsed.title.filter(|t| !t.trim().is_empty()) {
        out["title"] = serde_json::json!(t);
    }
    out["chars"] = serde_json::json!(chars);
    out["truncated"] = serde_json::json!(chars > max_chars);
    out["content"] = serde_json::json!(text.chars().take(max_chars).collect::<String>());
    out["links"] = serde_json::json!([]);
}

/// 参照フォルダの html の JSON は旧と byte 同一（題あり / 空 / 無し・壊れた UTF-8・max_chars の境界）
#[test]
fn ref_html_fields_match_old() {
    use openref_core::wiki::scan::Kind;
    let html: [&[u8]; 6] = [
        b"<html><head><title>T 1</title></head><body><p>\xe6\x9c\xac\xe6\x96\x87</p></body></html>",
        b"<title>  </title><p>x</p>",
        b"<p>no title</p>",
        b"",
        b"\xff\xfe<title>t</title>\xe6\x97",
        b"<meta name=\"wiki:tags\" content=\"a, b\"><title>tags</title>",
    ];
    for h in html {
        let n = String::from_utf8_lossy(h).chars().count();
        for max in [0, 1, n.saturating_sub(1), n, n + 1, 200_000] {
            let base = serde_json::json!({
                "path": "/r/a.html", "abs_path": "/r/a.html", "external": "/r", "kind": "html",
                "size": h.len(), "mtime": 1, "title": "a.html",
            });
            let (mut new, mut old) = (base.clone(), base);
            wiki_ref_html_fields(&mut new, Kind::Html, h, max);
            ref_html_fields_old(&mut old, Kind::Html, h, max);
            assert_eq!(
                serde_json::to_string(&new).unwrap(),
                serde_json::to_string(&old).unwrap(),
                "{:?} max {max}",
                String::from_utf8_lossy(h)
            );
        }
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
    let (ip, root) = ctx.ready().unwrap();
    with_index(&ip, &root, |ix| ix.refresh().map(|_| ())).unwrap();
    edit::write_agent(&ctx, &serde_json::json!({"path": "agent.md", "content": "# A\n"})).unwrap();
    let out = with_index(&ip, &root, |ix| {
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
    let mut ix = openref_core::wiki::Index::open(&ip, &root).unwrap();
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
        let real = home.join("Documents/kb-real");
        std::fs::create_dir_all(&real).unwrap();
        let real_s = real.to_string_lossy().to_string();
        let spelled = if kind == "symlink" {
            let l = home.join("Documents/kb");
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

/// ①: 読み書きの入口（wiki_ws_db）の root は canonical = 旧キーの取り込みも canonical の鍵へ（生の綴りの鍵を作らない）。
/// canon が無い（root が使えない）・canon の生の値が設定と違う（ロックの外で読んだ後に書き換わった）なら設定の生の値
#[test]
fn ws_db_root_is_canonical() {
    let (base, home, _app, db) = ws_env("canon-legacy");
    let real = home.join("Documents/kb-real");
    std::fs::create_dir_all(&real).unwrap();
    let real_s = real.to_string_lossy().to_string();
    let link = home.join("Documents/kb");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let link_s = link.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_root", &link_s).unwrap();
        g.set_setting(WIKI_REF_DIRS_KEY, r#"["/refdir"]"#).unwrap();
    }
    // 無効でも canonical は求まる（check_root は有効 / 無効を問わない）
    let canon = wiki_root_canon(&ctx_home(&db, &home));
    assert_eq!(canon, Some((link_s.clone(), real_s.clone())));
    let (r, note) = wiki_ws_db(&db, 0, false, canon.as_ref(), |m, root| {
        Ok((root.map(str::to_string), wiki_ref_dirs_of(m, root)))
    });
    let ((root, dirs), ctx) = r.unwrap();
    assert_eq!(root.as_deref(), Some(real_s.as_str()));
    assert_eq!(dirs, vec![PathBuf::from("/refdir")]);
    assert!(note.adopted_legacy);
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
/// 変異はここで落ちる）・旧の置き場の索引は消え DB は残る
#[test]
fn startup_steps_keep_history_and_drop_evicted() {
    use openref_core::wiki::workspaces;
    let (base, home, app, db) = ws_env("startup-keep");
    let cur = home.join("Documents/kb");
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
    assert!(!app.join(openref_core::wiki::index::INDEX_FILE).exists());
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
    let cur = home.join("Documents/kb");
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
    let kb = home.join("Documents/kb");
    let blog = home.join("Documents/blog");
    let other = home.join("Documents/other");
    for d in [&kb, &blog, &other] {
        std::fs::create_dir_all(d).unwrap();
    }
    let link = home.join("Documents/blog-link");
    std::os::unix::fs::symlink(&blog, &link).unwrap();
    let s = |p: &Path| p.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "1").unwrap();
        g.set_setting("wiki_root", &s(&kb)).unwrap();
        let ws = serde_json::json!({s(&kb): {"last_used": 100, "ref_dirs": ["/r"]}, s(&blog): {"last_used": 50}});
        g.set_setting(workspaces::KEY, &ws.to_string()).unwrap();
    }
    let ws_raw = || setting(&db.lock().unwrap(), workspaces::KEY);
    let root_now = || setting(&db.lock().unwrap(), "wiki_root");
    let before = ws_raw();
    let (r, note) = wiki_root_steps(&db, &ctx_home(&db, &home), &blog, RootChange::Front, 999, false);
    assert_eq!(r.unwrap(), blog);
    assert_eq!(note, WikiWsNote::default());
    assert_eq!(root_now(), Some(s(&blog)));
    assert_eq!(ws_raw(), before, "last_used も順も変えない（設定の値そのまま）");
    // 履歴に無い = unknown_root（wiki_err の JSON）・何も書かない
    for p in [other.clone(), PathBuf::new(), link.clone(), PathBuf::from(format!("{}/", s(&kb)))] {
        let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &p, RootChange::Front, 999, false);
        let e = r.unwrap_err();
        assert_eq!(err_code(&e).as_deref(), Some("unknown_root"), "{p:?}: {e}");
    }
    assert_eq!((root_now(), ws_raw()), (Some(s(&blog)), before.clone()));
    // Choose = 従来どおり（履歴の外も選べる・touch）
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &other, RootChange::Choose, 1000, false);
    assert_eq!(r.unwrap(), other);
    let order = || workspaces::order(&ws_map(&db));
    assert_eq!(order(), vec![(s(&other), 1000), (s(&kb), 100), (s(&blog), 50)]);
    // Front で前のワークスペースへ戻す = wiki_root だけ・時刻は変わらない
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &kb, RootChange::Front, 2000, false);
    assert_eq!(r.unwrap(), kb);
    assert_eq!(root_now(), Some(s(&kb)));
    assert_eq!(order(), vec![(s(&other), 1000), (s(&kb), 100), (s(&blog), 50)]);
    assert_eq!(workspaces::ref_dirs(&ws_map(&db), &s(&kb)), vec!["/r".to_string()]);
    // check_root に落ちる（履歴にはあるが消えた）= Err・何も書かない
    std::fs::remove_dir_all(&blog).unwrap();
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &blog, RootChange::Front, 3000, false);
    assert!(r.is_err());
    assert_eq!(root_now(), Some(s(&kb)));
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
    let g = live.begin(Some("/v/kb".into()));
    live.armed(g);
    assert!(live.prime(g));
    let ctx = openref_core::wiki::Ctx {
        enabled: true,
        root: Some("/v/kb".into()),
        workspaces: openref_core::wiki::workspaces::parse(Some(r#"{"/v/kb": {}, "/v/blog": {}}"#)).unwrap(),
        ..Default::default()
    };
    assert!(wiki_watched(&live, &ctx), "監視中の root = 省く");
    let mut blog = ctx.clone();
    blog.pin("/v/blog").unwrap();
    assert!(!wiki_watched(&live, &blog), "固定した別の root = 省かない");
    let mut kb = ctx.clone();
    kb.pin("/v/kb").unwrap();
    assert!(wiki_watched(&live, &kb), "固定した root が監視中と同じ = 省く");
}

/// "wiki-changed" の payload = 監視中の root（canonical な文字列）+ WikiChanged の鍵（平らに・形は従来のまま）
#[test]
fn wiki_changed_event_carries_root() {
    use serde_json::json;
    assert_eq!(
        serde_json::to_value(WikiChangedEvent::new(Path::new("/v/kb"), WikiChanged::everything())).unwrap(),
        json!({"root": "/v/kb", "all": true, "added": [], "removed": [], "modified": []})
    );
    let c = WikiChanged {
        added: vec!["n.md".into()],
        modified: vec!["a.md".into()],
        ..WikiChanged::default()
    };
    assert_eq!(
        serde_json::to_value(WikiChangedEvent::new(Path::new("/v/blog"), c)).unwrap(),
        json!({"root": "/v/blog", "all": false, "added": ["n.md"], "removed": [], "modified": ["a.md"]})
    );
}

/// チャットの [元に戻す] は記録した子の root で戻す（設定 wiki_root = 前面のタブの root が別でも）。設定の root のままだと
/// 「別のワークスペース」で断る（固定が要る理由）・子の root が履歴に無ければ戻さない・記録に root が無ければ設定のまま
#[test]
fn chat_undo_uses_child_root() {
    use crate::wiki_chat::{undo_record, Change, Record, Undo};
    use openref_core::wiki::{edit, workspaces, Ctx};
    let (base, home, app, _db) = ws_env("undo-root");
    let kb = home.join("Documents/kb");
    let blog = home.join("Documents/blog");
    std::fs::create_dir_all(&kb).unwrap();
    std::fs::create_dir_all(&blog).unwrap();
    let (kb_s, blog_s) = (kb.to_string_lossy().to_string(), blog.to_string_lossy().to_string());
    let ws = |roots: &[&str]| {
        let m: serde_json::Map<String, serde_json::Value> = roots.iter().map(|r| (r.to_string(), serde_json::json!({}))).collect();
        workspaces::parse(Some(&serde_json::Value::Object(m).to_string())).unwrap()
    };
    // 設定 = kb（前面のタブ）・子 = blog（Claude がそこにページを作った）
    let setting = Ctx {
        enabled: true,
        root: Some(kb_s.clone()),
        app_dir: Some(app.clone()),
        home: Some(home.clone()),
        workspaces: ws(&[&kb_s, &blog_s]),
        ..Default::default()
    };
    let child = wiki_pin(setting.clone(), Some(&blog_s)).unwrap();
    let w = edit::write_agent(&child, &serde_json::json!({"path": "c.md", "content": "x"})).unwrap();
    let rec = Record {
        change: Change::Create {
            path: "c.md".into(),
            sha256_after: w["sha256"].as_str().unwrap().into(),
        },
        root: Some(blog.clone()),
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
    let lost = Ctx { workspaces: ws(&[&kb_s]), ..setting.clone() };
    let e = wiki_chat_undo_ctx(lost, Some(&blog)).unwrap_err();
    assert!(e.contains("履歴に無いワークスペース"), "{e}");
    // 記録に root が無い = 設定のまま
    assert_eq!(wiki_chat_undo_ctx(setting.clone(), None).unwrap().root, Some(kb_s.clone()));
    // 子の root に固定 = 戻せる（ゴミ箱の代わりに消す）・設定の root（kb）には触らない
    let ctx = wiki_chat_undo_ctx(setting.clone(), Some(&blog)).unwrap();
    let mut trashed = None;
    let out = undo_record(&ctx, &rec, None, false, |p: &Path| {
        trashed = Some(p.to_path_buf());
        std::fs::remove_file(p).map(|()| "test".to_string()).map_err(|e| e.to_string())
    }, no_item);
    assert!(matches!(&out, Undo::Done { path: Some(p), .. } if p == "c.md"), "{out:?}");
    assert_eq!(trashed, Some(blog.join("c.md")));
    assert!(!blog.join("c.md").exists() && std::fs::read_dir(&kb).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(&base);
}

// ---------- 有効化 = OpenNote の起動（OFF で起動した後でも現在の root が履歴に入る = args.root の pin が通る） ----------

/// OFF で起動（履歴は空のまま）→ 有効化（wiki_enable_with = 家事を touch で）→ 履歴に canonical な root が 1 件・pin が通る
#[test]
fn enable_after_off_startup_adds_root_to_history() {
    use openref_core::wiki::workspaces;
    let (base, home, _app, db) = ws_env("enable-off");
    let kb = home.join("Documents/kb");
    std::fs::create_dir_all(&kb).unwrap();
    let kb_s = kb.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "0").unwrap();
        g.set_setting("wiki_root", &kb_s).unwrap();
    }
    // 起動時（無効 = touch しない）
    let ctx = ctx_home(&db, &home);
    let (r, _) = wiki_chores_steps(&db, &ctx, ctx.root.as_deref(), ctx.enabled, 1000, false, later()).unwrap();
    assert!(r.unwrap().evicted.is_empty());
    assert!(ws_map(&db).is_empty(), "OFF の起動では履歴に入らない");
    assert!(ctx_home(&db, &home).pin(&kb_s).is_err(), "このままでは pin が通らない（直す前の不具合）");
    // 有効化 = 設定を書いてから家事（touch）
    db.lock().unwrap().set_setting("wiki_enabled", "1").unwrap();
    let touch = wiki_enable_chores(true, true).expect("有効化は家事を回す");
    let (r, note) = wiki_chores_steps(&db, &ctx_home(&db, &home), Some(&kb_s), touch, 2000, false, later()).unwrap();
    assert_eq!(r.unwrap().rewrote, None, "既に canonical");
    assert!(note.set_aside.is_empty());
    assert_eq!(workspaces::order(&ws_map(&db)), vec![(kb_s.clone(), 2000)]);
    ctx_home(&db, &home).pin(&kb_s).unwrap();
    let _ = std::fs::remove_dir_all(&base);
}

/// 有効化: 設定 wiki_root が symlink の綴りなら canonical に書き直し、履歴は canonical の 1 件（生の綴りの鍵を作らない）
#[test]
fn enable_rewrites_symlink_root_to_canonical() {
    use openref_core::wiki::workspaces;
    let (base, home, _app, db) = ws_env("enable-link");
    let real = home.join("Documents/kb-real");
    std::fs::create_dir_all(&real).unwrap();
    let link = home.join("Documents/kb");
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
    let kb = home.join("Documents/kb");
    std::fs::create_dir_all(&kb).unwrap();
    let kb_s = kb.to_string_lossy().to_string();
    {
        let g = db.lock().unwrap();
        g.set_setting("wiki_enabled", "0").unwrap();
        g.set_setting(workspaces::KEY, r#"{"/Users/u/old": {"last_used": 7}}"#).unwrap();
    }
    // メニューの有効化でフォルダを選んだ = wiki_choose_root（Choose = touch）→ 有効化の家事は touch = false
    let (r, _) = wiki_root_steps(&db, &ctx_home(&db, &home), &kb, RootChange::Choose, 1000, false);
    assert_eq!(r.unwrap(), kb);
    db.lock().unwrap().set_setting("wiki_enabled", "1").unwrap();
    let before = setting(&db.lock().unwrap(), workspaces::KEY);
    let touch = wiki_enable_chores(true, false).unwrap();
    let (r, _) = wiki_chores_steps(&db, &ctx_home(&db, &home), Some(&kb_s), touch, 5000, false, later()).unwrap();
    r.unwrap();
    assert_eq!(setting(&db.lock().unwrap(), workspaces::KEY), before, "last_used は選んだときのまま");
    assert_eq!(workspaces::order(&ws_map(&db)), vec![(kb_s.clone(), 1000), ("/Users/u/old".to_string(), 7)]);
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
    let a = home.join("Documents/kb");
    let b = home.join("Documents/blog");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let link = home.join("Documents/kb-link");
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
