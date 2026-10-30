use openref_core::{ingest, model, resolver::Resolved, search, CoreError, Db};
use serde_json::json;

fn fake_resolved() -> Resolved {
    Resolved {
        csl: json!({
            "type": "article-journal",
            "title": "Pancreatic cancer imaging: a test review",
            "author": [
                {"family": "Kano", "given": "S"},
                {"family": "Yamada", "given": "T"}
            ],
            "container-title": "Test Journal of Radiology",
            "issued": {"date-parts": [[2026, 7]]},
            "DOI": "10.9999/openref.test.1",
            "PMID": "99999999",
            "abstract": "MRI and CT findings of pancreatic ductal adenocarcinoma.",
            "custom": {"mesh": ["Pancreatic Neoplasms"], "source": "test"}
        }),
        retracted: None,
        source: "test",
        warnings: vec![],
    }
}

#[test]
fn add_search_dedup_roundtrip() {
    let mut db = Db::open_in_memory().unwrap();
    let r = fake_resolved();

    let added = ingest::add_item(&mut db, &r).unwrap();
    assert_eq!(added.key.len(), 8);

    // FTS: abstract 語で hit
    let hits = search::search(&db, "pancreatic adenocarcinoma", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].key, added.key);

    // FTS: 著者名で hit
    let hits = search::search(&db, "kano", 10).unwrap();
    assert_eq!(hits.len(), 1);

    // 生成列（year / doi / pmid）が効いている
    let item = search::get_by_key(&db, &added.key).unwrap().unwrap();
    assert_eq!(model::year(&item.csl), Some(2026));

    // 著者チップ / 誌名チップ（list_items の author_family + author_initial / journal）
    use openref_core::search::{list_items, ListFilter};
    let n = |f: ListFilter| list_items(&db, &f).unwrap().len();
    assert_eq!(n(ListFilter { author_family: Some("kano".into()), ..Default::default() }), 1, "姓は大小無視");
    assert_eq!(n(ListFilter { author_family: Some("Kano".into()), author_initial: Some("S".into()), ..Default::default() }), 1);
    assert_eq!(n(ListFilter { author_family: Some("Kano".into()), author_initial: Some("T".into()), ..Default::default() }), 0, "頭文字違いは別人");
    assert_eq!(n(ListFilter { author_family: Some("Yamada".into()), ..Default::default() }), 1, "第 2 著者でも hit（Paperpile と同じ）");
    assert_eq!(n(ListFilter { author_family: Some("Suzuki".into()), ..Default::default() }), 0);
    assert_eq!(n(ListFilter { journal: Some("test journal of radiology".into()), ..Default::default() }), 1);
    assert_eq!(n(ListFilter { journal: Some("Radiology".into()), ..Default::default() }), 0, "誌名は完全一致");
    assert_eq!(
        n(ListFilter { search: Some("pancreatic".into()), author_family: Some("Kano".into()), journal: Some("Test Journal of Radiology".into()), ..Default::default() }),
        1,
        "FTS + チップの AND"
    );
    assert_eq!(n(ListFilter { search: Some("pancreatic".into()), author_family: Some("Suzuki".into()), ..Default::default() }), 0);
    assert_eq!(model::first_author_given(&item.csl).as_deref(), Some("S"));

    // dedup: PMID 一致で拒否
    match ingest::add_item(&mut db, &r).unwrap_err() {
        CoreError::Duplicate { field, existing_key } => {
            assert_eq!(field, "PMID");
            assert_eq!(existing_key, added.key);
        }
        e => panic!("unexpected error: {e}"),
    }

    // dedup: PMID なし・DOI 一致でも拒否
    let mut r2 = fake_resolved();
    r2.csl.as_object_mut().unwrap().remove("PMID");
    match ingest::add_item(&mut db, &r2).unwrap_err() {
        CoreError::Duplicate { field, .. } => assert_eq!(field, "DOI"),
        e => panic!("unexpected error: {e}"),
    }

    let c = search::counts(&db).unwrap();
    assert_eq!(c.items, 1);
}

#[test]
fn edit_and_filtered_list() {
    use openref_core::{edit, search::ListFilter};
    let mut db = Db::open_in_memory().unwrap();
    let added = ingest::add_item(&mut db, &fake_resolved()).unwrap();
    let key = &added.key;

    // star
    edit::set_starred(&db, key, true).unwrap();
    let starred = search::list_items(
        &db,
        &ListFilter { starred_only: true, ..Default::default() },
    )
    .unwrap();
    assert_eq!(starred.len(), 1);

    // tags
    edit::add_tag(&db, key, "pancreas").unwrap();
    edit::add_tag(&db, key, "pancreas").unwrap(); // 冪等
    let d = search::item_detail(&db, key).unwrap().unwrap();
    assert_eq!(d.tags.len(), 1);
    assert_eq!(d.tags[0].name, "pancreas");
    edit::remove_tag(&db, key, "pancreas").unwrap();
    assert!(search::item_detail(&db, key).unwrap().unwrap().tags.is_empty());
    // 名前の前後の空白は core で落とす（登録表 = trim・GUI / CLI = しない、のずれを無くす。parity test は名前しか見ない）
    edit::add_tag(&db, key, "  liver ").unwrap();
    assert_eq!(search::item_detail(&db, key).unwrap().unwrap().tags[0].name, "liver");
    edit::remove_tag(&db, key, " liver  ").unwrap();
    assert!(search::item_detail(&db, key).unwrap().unwrap().tags.is_empty(), "空白付きの名前でも外れる");

    // collection
    db.conn
        .execute_batch("INSERT INTO collections (key, name) VALUES ('AAAA1111', 'TestCol')")
        .unwrap();
    let cid: i64 = db
        .conn
        .query_row("SELECT id FROM collections WHERE name='TestCol'", [], |r| r.get(0))
        .unwrap();
    edit::add_to_collection(&db, key, cid).unwrap();
    let in_col = search::list_items(
        &db,
        &ListFilter { collection_id: Some(cid), ..Default::default() },
    )
    .unwrap();
    assert_eq!(in_col.len(), 1);
    assert!(in_col[0].item.starred);

    // search + collection 併用
    let hits = search::list_items(
        &db,
        &ListFilter {
            search: Some("pancreatic".into()),
            collection_id: Some(cid),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);

    // trash → 一覧から消え、trash ビューに現れ、restore で戻る
    edit::trash(&db, key).unwrap();
    assert_eq!(search::list_items(&db, &ListFilter::default()).unwrap().len(), 0);
    let trashed = search::list_items(&db, &ListFilter { trashed: true, ..Default::default() }).unwrap();
    assert_eq!(trashed.len(), 1);
    let vc = search::view_counts(&db).unwrap();
    assert_eq!((vc.all, vc.trash), (0, 1));
    edit::restore(&db, key).unwrap();
    assert_eq!(search::list_items(&db, &ListFilter::default()).unwrap().len(), 1);

    // purge: ゴミ箱外は拒否 → trash 後は完全に消える（一覧・ゴミ箱・FTS・key 解決・所属行すべて）
    match edit::purge(&db, key).unwrap_err() {
        CoreError::BadInput(m) => assert!(m.contains("ゴミ箱"), "{m}"),
        e => panic!("unexpected error: {e}"),
    }
    edit::trash(&db, key).unwrap();
    let purged = edit::purge(&db, key).unwrap();
    assert_eq!(purged, edit::Purged { key: key.to_string(), files_removed: 0, files_kept: 0, files_failed: 0 });
    assert_eq!(search::list_items(&db, &ListFilter { trashed: true, ..Default::default() }).unwrap().len(), 0);
    assert_eq!(search::view_counts(&db).unwrap().trash, 0);
    assert_eq!(search::search(&db, "pancreatic", 10).unwrap().len(), 0);
    assert!(search::get_by_key(&db, key).unwrap().is_none());
    let n_rows = |sql: &str| -> i64 { db.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(n_rows("SELECT COUNT(*) FROM collection_items"), 0);
    assert_eq!(n_rows("SELECT COUNT(*) FROM item_tags"), 0);
    assert_eq!(n_rows("SELECT COUNT(*) FROM creators_index"), 0);
    assert_eq!(n_rows("SELECT COUNT(*) FROM items_fts"), 0);
    // タグ・コレクション自体は残る（item だけが消える）
    assert_eq!(n_rows("SELECT COUNT(*) FROM collections WHERE name='TestCol'"), 1);
    // 二重 purge は NotFound
    assert!(matches!(edit::purge(&db, key).unwrap_err(), CoreError::NotFound(_)));
}

#[test]
fn managed_attach_roundtrip() {
    use openref_core::edit;
    let tmp = std::env::temp_dir().join(format!("openref-attach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut db = Db::open(&tmp.join("t.sqlite")).unwrap();
    let added = ingest::add_item(&mut db, &fake_resolved()).unwrap();
    let src = tmp.join("drop.pdf");
    std::fs::write(&src, b"%PDF-1.4 fake").unwrap();
    edit::attach_auto(&db, &added.key, &src).unwrap();
    let d = search::item_detail(&db, &added.key).unwrap().unwrap();
    assert_eq!(d.attachments.len(), 1);
    assert_eq!(d.attachments[0].mode, "managed");
    assert!(d.attachments[0].exists);
    assert!(d.attachments[0].path.contains("storage"));

    // purge は managed ファイル（storage/<添付key>/…）を消し、空になったディレクトリも畳む
    let managed_path = std::path::PathBuf::from(&d.attachments[0].path);
    edit::trash(&db, &added.key).unwrap();
    let purged = edit::purge(&db, &added.key).unwrap();
    assert_eq!((purged.files_removed, purged.files_kept, purged.files_failed), (1, 0, 0));
    assert!(!managed_path.exists());
    assert!(!managed_path.parent().unwrap().exists());
    assert!(tmp.join("storage").exists(), "storage ルート自体は残す");
    let n_att: i64 = db.conn.query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get(0)).unwrap();
    assert_eq!(n_att, 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn drive_mode_attach_writes_compat_layout() {
    use openref_core::edit;
    let tmp = std::env::temp_dir().join(format!("openref-cutover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mount = tmp.join("Paperpile");
    std::fs::create_dir_all(mount.join("All Papers")).unwrap();
    std::env::set_var("OPENREF_MOUNT", &mount);

    let mut db = Db::open(&tmp.join("t.sqlite")).unwrap();
    db.set_setting("pdf_write_mode", "drive").unwrap();
    let added = ingest::add_item(&mut db, &fake_resolved()).unwrap();
    let attached = edit::attach_auto_bytes(&db, &added.key, "raw.pdf", b"%PDF-1.4 fake").unwrap();
    assert_eq!(attached.md5, openref_core::pdffetch::md5_hex(b"%PDF-1.4 fake"), "添付関数は実体の md5 を返す");
    let placement = attached.placement;

    // 二著者規約 = "Kano and Yamada"・`: ` → ` - `・頭文字 K フォルダ
    let expected = mount
        .join("All Papers/K")
        .join("Kano and Yamada 2026 - Pancreatic cancer imaging - a test review.pdf");
    match &placement {
        edit::Placement::Drive(p) => assert_eq!(p, &expected),
        other => panic!("Drive 配置になっていない: {other:?}"),
    }
    assert_eq!(std::fs::read(&expected).unwrap(), b"%PDF-1.4 fake");
    let d = search::item_detail(&db, &added.key).unwrap().unwrap();
    assert_eq!(d.attachments[0].mode, "linked");
    assert!(d.attachments[0].exists);

    // 同名衝突は " (2)" 付番
    let p2 = edit::attach_auto_bytes(&db, &added.key, "raw.pdf", b"%PDF-1.4 second").unwrap().placement;
    if let edit::Placement::Drive(p) = &p2 {
        assert!(p.to_string_lossy().ends_with("a test review (2).pdf"), "{p:?}");
    } else {
        panic!("Drive 配置になっていない: {p2:?}");
    }

    // disable で managed へ戻る
    db.set_setting("pdf_write_mode", "managed").unwrap();
    let p3 = edit::attach_auto_bytes(&db, &added.key, "raw.pdf", b"%PDF-1.4 third").unwrap().placement;
    assert!(matches!(p3, edit::Placement::Managed(_)), "{p3:?}");

    // purge: linked 2 本（Drive 側の実体 = cutover 前は Paperpile の持ち物）は**残し**、managed 1 本だけ消す
    let managed_path = match &p3 {
        edit::Placement::Managed(p) => p.clone(),
        _ => unreachable!(),
    };
    edit::trash(&db, &added.key).unwrap();
    let purged = edit::purge(&db, &added.key).unwrap();
    assert_eq!((purged.files_removed, purged.files_kept, purged.files_failed), (1, 2, 0));
    assert!(expected.exists(), "linked ファイルは削除しない");
    assert!(!managed_path.exists(), "managed ファイルは削除する");

    std::env::remove_var("OPENREF_MOUNT");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn manual_reference_create_and_edit() {
    use openref_core::{edit, search, types};
    let mut db = Db::open_in_memory().unwrap();

    // 型カタログに無いフィールド（PMID）は book では落ちる
    let book = json!({
        "type": "book",
        "title": "画像診断コンパクトナビ",
        "author": [{"family": "山田", "given": "太郎"}],
        "publisher": "医学書院",
        "publisher-place": "東京",
        "issued": {"date-parts": [[2025]]},
        "ISBN": "978-4-260-00000-0",
        "PMID": "99999",
        "volume": "",
        "custom": {"subtype": "Review"}
    });
    let key = edit::create_manual(&mut db, &book).unwrap();
    let d = search::item_detail(&db, &key).unwrap().unwrap();
    assert_eq!(d.item.csl["publisher"], "医学書院");
    assert!(d.item.csl.get("PMID").is_none(), "book に PMID は入らない");
    assert!(d.item.csl.get("volume").is_none(), "空文字は落とす");
    assert_eq!(d.item.csl["custom"]["subtype"], "Review");

    // 和文の FTS（cjk 列）に載る
    assert_eq!(search::search(&db, "画像診断", 5).unwrap().len(), 1);
    assert_eq!(search::search(&db, "山田", 5).unwrap().len(), 1);

    // 編集: タイトル変更 → 再索引される（旧語では引けない）
    let mut edited = d.item.csl.clone();
    edited["title"] = json!("腹部画像診断ハンドブック");
    edit::update_csl(&mut db, &key, &edited).unwrap();
    assert_eq!(search::search(&db, "ハンドブック", 5).unwrap().len(), 1);
    assert_eq!(search::search(&db, "コンパクトナビ", 5).unwrap().len(), 0);
    // custom は温存
    let d2 = search::item_detail(&db, &key).unwrap().unwrap();
    assert_eq!(d2.item.csl["custom"]["subtype"], "Review");

    // タイトル無しは拒否
    assert!(edit::create_manual(&mut db, &json!({"type": "webpage"})).is_err());
    // 未知の型は拒否
    assert!(edit::create_manual(&mut db, &json!({"type": "hologram", "title": "x"})).is_err());

    // DOI 正規化 + 重複拒否
    let w = json!({"type": "webpage", "title": "RSNA", "URL": "https://rsna.org",
                   "DOI": "https://doi.org/10.1000/ABC"});
    let wk = edit::create_manual(&mut db, &w).unwrap();
    let wd = search::item_detail(&db, &wk).unwrap().unwrap();
    assert_eq!(wd.item.csl["DOI"], "10.1000/abc");
    assert!(edit::create_manual(&mut db, &w).is_err(), "同一 DOI は拒否");
    // 接頭辞・大小・空白が違っても同じ DOI（add_item の重複判定と同じ照合）
    let dup = |db: &mut Db, csl: serde_json::Value| match edit::create_manual(db, &csl) {
        Err(openref_core::CoreError::Duplicate { existing_key, field }) => Some((existing_key, field)),
        Err(e) => panic!("{e}"),
        Ok(k) => {
            edit::trash(db, &k).unwrap();
            None
        }
    };
    for doi in ["doi:10.1000/abc", " DOI: 10.1000/ABC ", "HTTPS://DX.DOI.ORG/10.1000/abc", "http://doi.org/10.1000/abc"] {
        let csl = json!({"type": "webpage", "title": "RSNA", "DOI": doi});
        assert_eq!(dup(&mut db, csl), Some((wk.clone(), "DOI")), "{doi:?}");
    }
    assert_eq!(search::find_by_ids(&db, None, Some("doi:10.1000/ABC")).unwrap().map(|r| r.1), Some(wk.clone()));
    // 保存も同じ正規化（編集でも）・接頭辞だけの DOI は空値と同じに落とす
    let mut we = wd.item.csl.clone();
    we["DOI"] = json!("doi: 10.1000/XYZ");
    edit::update_csl(&mut db, &wk, &we).unwrap();
    assert_eq!(search::item_detail(&db, &wk).unwrap().unwrap().item.csl["DOI"], "10.1000/xyz");
    // 保存は接頭辞が重なっていても全部剥がす（BibTeX・Word に生で出るため）→ 同じ DOI として重複
    for doi in ["https://doi.org/https://doi.org/10.1000/XYZ", "doi:https://doi.org/10.1000/xyz"] {
        assert_eq!(dup(&mut db, json!({"type": "webpage", "title": "R", "DOI": doi})), Some((wk.clone(), "DOI")), "{doi:?}");
    }
    let only_prefix = edit::create_manual(&mut db, &json!({"type": "webpage", "title": "P", "DOI": "doi:"})).unwrap();
    assert!(search::item_detail(&db, &only_prefix).unwrap().unwrap().item.csl.get("DOI").is_none());
    // PMID は先頭の 0 を揃える・一致した方の欄を返す（PMID を持つ入力でも DOI で一致したら DOI）
    let a = edit::create_manual(&mut db, &json!({"type": "article-journal", "title": "A", "PMID": "123"})).unwrap();
    assert_eq!(dup(&mut db, json!({"type": "article-journal", "title": "A", "PMID": "0123"})), Some((a.clone(), "PMID")));
    assert_eq!(search::find_by_ids(&db, Some("00123"), None).unwrap().map(|r| r.1), Some(a.clone()));
    assert_eq!(
        dup(&mut db, json!({"type": "article-journal", "title": "B", "PMID": "456", "DOI": "doi:10.1000/xyz"})),
        Some((wk.clone(), "DOI"))
    );
    // ゴミ箱の文献とは重複にしない
    edit::trash(&db, &a).unwrap();
    assert_eq!(dup(&mut db, json!({"type": "article-journal", "title": "A", "PMID": "123"})), None);
    assert!(search::find_by_ids(&db, Some("123"), None).unwrap().is_none());

    assert!(types::catalog().iter().any(|t| t.id == "chapter"));
}

#[test]
fn annotation_crud() {
    use openref_core::{annotate, edit};
    let tmp = std::env::temp_dir().join(format!("openref-annot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut db = Db::open(&tmp.join("t.sqlite")).unwrap();
    let added = ingest::add_item(&mut db, &fake_resolved()).unwrap();
    let src = tmp.join("a.pdf");
    std::fs::write(&src, b"%PDF-1.4 x").unwrap();
    edit::attach_auto(&db, &added.key, &src).unwrap();

    let k = annotate::add_annotation(
        &db,
        &added.key,
        &annotate::NewAnnotation {
            kind: "highlight".into(),
            color: Some("purple".into()),
            comment: Some("重要".into()),
            page: Some(3),
            quote: Some("pancreatic ductal adenocarcinoma".into()),
            author: "claude:fable-5".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let list = annotate::list_annotations(&db, &added.key).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].page, Some(3));
    assert_eq!(list[0].author, "claude:fable-5");

    // update_annotation: comment 変更 + date_modified 更新（sentinel を仕込んで検証・sleep 不要）
    db.conn
        .execute(
            "UPDATE annotations SET date_modified = '2000-01-01 00:00:00' WHERE key = ?1",
            [&k],
        )
        .unwrap();
    annotate::update_annotation(
        &db,
        &k,
        &annotate::AnnotationPatch {
            comment: Some(Some("更新後コメント".into())),
            ..Default::default()
        },
    )
    .unwrap();
    let dm: String = db
        .conn
        .query_row("SELECT date_modified FROM annotations WHERE key = ?1", [&k], |r| {
            r.get(0)
        })
        .unwrap();
    assert_ne!(dm, "2000-01-01 00:00:00", "update 時に date_modified が更新される");
    let list = annotate::list_annotations(&db, &added.key).unwrap();
    assert_eq!(list[0].comment.as_deref(), Some("更新後コメント"));

    // color 変更
    annotate::update_annotation(
        &db,
        &k,
        &annotate::AnnotationPatch {
            color: Some(Some("blue".into())),
            ..Default::default()
        },
    )
    .unwrap();
    let list = annotate::list_annotations(&db, &added.key).unwrap();
    assert_eq!(list[0].color.as_deref(), Some("blue"));

    // color 消去（Some(None) → NULL）
    annotate::update_annotation(
        &db,
        &k,
        &annotate::AnnotationPatch {
            color: Some(None),
            ..Default::default()
        },
    )
    .unwrap();
    let list = annotate::list_annotations(&db, &added.key).unwrap();
    assert_eq!(list[0].color, None, "Some(None) で NULL へ消去される");

    // comment を明示的な空文字に設定（NULL 消去とは区別される）
    annotate::update_annotation(
        &db,
        &k,
        &annotate::AnnotationPatch {
            comment: Some(Some(String::new())),
            ..Default::default()
        },
    )
    .unwrap();
    let list = annotate::list_annotations(&db, &added.key).unwrap();
    assert_eq!(
        list[0].comment.as_deref(),
        Some(""),
        "空文字は Some(Some(\"\")) で NULL 消去 Some(None) と区別される"
    );

    // kind 変更
    annotate::update_annotation(
        &db,
        &k,
        &annotate::AnnotationPatch {
            kind: Some("underline".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let list = annotate::list_annotations(&db, &added.key).unwrap();
    assert_eq!(list[0].kind, "underline");

    // kind 不正は拒否・DB は変更されない
    assert!(annotate::update_annotation(
        &db,
        &k,
        &annotate::AnnotationPatch {
            kind: Some("shape".into()),
            ..Default::default()
        },
    )
    .is_err());
    let list = annotate::list_annotations(&db, &added.key).unwrap();
    assert_eq!(list[0].kind, "underline", "不正 kind 拒否後も変更されない");

    // 存在しない key は NotFound
    assert!(matches!(
        annotate::update_annotation(
            &db,
            "doesnotexist",
            &annotate::AnnotationPatch {
                kind: Some("note".into()),
                ..Default::default()
            },
        ),
        Err(CoreError::NotFound(_))
    ));

    // 空パッチ（フィールド指定なし）は拒否
    assert!(annotate::update_annotation(&db, &k, &annotate::AnnotationPatch::default()).is_err());

    annotate::delete_annotation(&db, &k).unwrap();
    assert!(annotate::list_annotations(&db, &added.key).unwrap().is_empty());

    // v4: strikethrough は許容、未知 kind は拒否（annotations.kind の CHECK 制約）
    let sk = annotate::add_annotation(
        &db,
        &added.key,
        &annotate::NewAnnotation {
            kind: "strikethrough".into(),
            page: Some(1),
            quote: Some("obsolete finding".into()),
            author: "user".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let list = annotate::list_annotations(&db, &added.key).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, "strikethrough");
    annotate::delete_annotation(&db, &sk).unwrap();

    assert!(annotate::add_annotation(
        &db,
        &added.key,
        &annotate::NewAnnotation {
            kind: "shape".into(),
            quote: Some("x".into()),
            author: "user".into(),
            ..Default::default()
        },
    )
    .is_err());

    // メモ: add → 一覧の has_note が立つ → update で本文差し替え → 空は拒否 → delete で消え has_note が落ちる
    use openref_core::search::{list_items, ListFilter};
    assert!(!list_items(&db, &ListFilter::default()).unwrap()[0].has_note);
    let nk = annotate::add_note(&db, &added.key, "user", "読む: 図 3 の造影パターン").unwrap();
    assert!(list_items(&db, &ListFilter::default()).unwrap()[0].has_note);
    annotate::update_note(&db, &nk, "読んだ。図 3 は動脈相の wash-in が要点").unwrap();
    let notes = annotate::list_notes(&db, &added.key).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content_md, "読んだ。図 3 は動脈相の wash-in が要点");
    assert!(matches!(annotate::update_note(&db, &nk, "  "), Err(CoreError::BadInput(_))));
    assert!(matches!(annotate::update_note(&db, "NOPE0000", "x"), Err(CoreError::NotFound(_))));
    annotate::delete_note(&db, &nk).unwrap();
    assert!(!list_items(&db, &ListFilter::default()).unwrap()[0].has_note);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn retraction_flag_persisted() {
    let mut db = Db::open_in_memory().unwrap();
    let mut r = fake_resolved();
    r.retracted = Some("RetractionIn: Test Journal 2027".into());
    let added = ingest::add_item(&mut db, &r).unwrap();
    let flag = search::retraction_of(&db, added.id).unwrap();
    assert!(flag.unwrap().contains("RetractionIn"));
}

/// resolver の応答に見立てる（撤回なし・PubMed 由来）
fn enriched(csl: serde_json::Value) -> Resolved {
    Resolved { csl, retracted: None, source: "pubmed", warnings: vec![] }
}

/// refresh / enrich（merge_enrichment）が resolver の撤回情報を印に写す（以前は取込時の add_item しか書かず、
/// 登録後に撤回された論文は refresh しても印が付かなかった）。None は「分からない」なので既存の印を消さない
#[test]
fn refresh_writes_retraction_flag() {
    let mut db = Db::open_in_memory().unwrap();
    let added = ingest::add_item(&mut db, &fake_resolved()).unwrap();
    assert!(search::retraction_of(&db, added.id).unwrap().is_none());
    let flag_row = |db: &Db| -> (String, String, String) {
        db.conn
            .query_row(
                "SELECT status, detail, source FROM retraction_flags WHERE item_id = ?1",
                [added.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    };

    // 書誌は同じ・撤回だけが新しく付く → 変化あり
    let mut r = fake_resolved();
    r.source = "pubmed";
    r.retracted = Some("PubMed PublicationType: Retracted Publication".into());
    assert!(ingest::merge_enrichment(&mut db, added.id, &r).unwrap());
    assert_eq!(
        flag_row(&db),
        ("retracted".into(), "PubMed PublicationType: Retracted Publication".into(), "pubmed".into())
    );
    assert!(search::list_items(&db, &search::ListFilter::default()).unwrap()[0].retracted);
    // 同じ撤回をもう一度 → 変化なし（行は 1 つのまま）
    assert!(!ingest::merge_enrichment(&mut db, added.id, &r).unwrap());
    // 撤回の詳細が変わった（撤回通知が付いた）→ UPSERT で更新
    r.retracted = Some("RetractionIn: Test Journal 2027".into());
    assert!(ingest::merge_enrichment(&mut db, added.id, &r).unwrap());
    assert_eq!(flag_row(&db).1, "RetractionIn: Test Journal 2027");
    let n: i64 = db
        .conn
        .query_row("SELECT count(*) FROM retraction_flags WHERE item_id = ?1", [added.id], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    // resolver が撤回を返さない（Crossref は撤回を見ない）→ 印は残す
    r.retracted = None;
    r.source = "crossref";
    assert!(!ingest::merge_enrichment(&mut db, added.id, &r).unwrap());
    assert_eq!(flag_row(&db).1, "RetractionIn: Test Journal 2027");

    // 同一性ガードで弾かれた（識別子が別論文を指す）ときは撤回も写さない
    let mut other = fake_resolved();
    other.csl["DOI"] = json!("10.9999/openref.test.2");
    other.csl["PMID"] = json!("99999998");
    other.csl["title"] = json!("Another unrelated paper about cardiac MRI");
    let k2 = ingest::add_item(&mut db, &other).unwrap();
    let mut wrong = fake_resolved();
    wrong.retracted = Some("PubMed PublicationType: Retracted Publication".into());
    assert!(matches!(
        ingest::merge_enrichment(&mut db, k2.id, &wrong),
        Err(CoreError::BadInput(_))
    ));
    assert!(search::retraction_of(&db, k2.id).unwrap().is_none());
}

/// reindex_keys の件数は実際に貼り直した item の数（同じ key を重ねても 1 件）・知らない key があれば何も書かない
#[test]
fn reindex_keys_counts_distinct_items() {
    let mut db = Db::open_in_memory().unwrap();
    let k = ingest::add_item(&mut db, &fake_resolved()).unwrap().key;
    assert_eq!(ingest::reindex_keys(&mut db, &[k.clone(), k.clone()]).unwrap(), 1);
    assert_eq!(ingest::reindex_keys(&mut db, std::slice::from_ref(&k)).unwrap(), 1);
    assert!(matches!(
        ingest::reindex_keys(&mut db, &[k.clone(), "NOPE0000".into()]),
        Err(CoreError::NotFound(_))
    ));
    let creators: i64 = db
        .conn
        .query_row("SELECT count(*) FROM creators_index", [], |r| r.get(0))
        .unwrap();
    assert_eq!(creators, 2, "重ねて貼り直しても著者の索引は 2 行のまま");
}

#[test]
fn membership_counts_for_bulk_toolbar() {
    use openref_core::{edit, tags};
    let mut db = Db::open_in_memory().unwrap();
    let k1 = ingest::add_item(&mut db, &fake_resolved()).unwrap().key;
    let mut r2 = fake_resolved();
    {
        let o = r2.csl.as_object_mut().unwrap();
        o.insert("DOI".into(), json!("10.9999/openref.test.2"));
        o.insert("PMID".into(), json!("99999998"));
        o.insert("title".into(), json!("Second test article"));
    }
    let k2 = ingest::add_item(&mut db, &r2).unwrap().key;
    let keys = vec![k1.clone(), k2.clone()];

    // 所属なし → 空
    let m = search::membership(&db, &keys).unwrap();
    assert!(m.collections.is_empty() && m.tags.is_empty());

    // フォルダ: 1 件だけ所属 → (cid, 1) = 「一部」、両方 → (cid, 2) = 「全部」
    let (cid, created) = edit::ensure_collection_path(&db, "Bulk/Child").unwrap();
    assert_eq!(created, vec!["Bulk", "Child"]);
    edit::add_to_collection(&db, &k1, cid).unwrap();
    let m = search::membership(&db, &keys).unwrap();
    assert_eq!(m.collections, vec![(cid, 1)]);
    edit::add_to_collection(&db, &k2, cid).unwrap();
    let m = search::membership(&db, &keys).unwrap();
    assert_eq!(m.collections, vec![(cid, 2)]);

    // タグ: 両方に付与 → (tid, 2)。keys 外の item は数えない
    let tid = tags::create_tag(&db, "bulk", None).unwrap();
    tags::attach_tag(&db, &k1, tid).unwrap();
    tags::attach_tag(&db, &k2, tid).unwrap();
    let m = search::membership(&db, &keys).unwrap();
    assert_eq!(m.tags, vec![(tid, 2)]);
    let m = search::membership(&db, &[k1.clone()]).unwrap();
    assert_eq!(m.tags, vec![(tid, 1)]);

    // 空 keys → 空
    let m = search::membership(&db, &[]).unwrap();
    assert!(m.collections.is_empty() && m.tags.is_empty());

    // 一覧の行ラベル（バッジ）と id での絞り込み（改名しても同じ一覧のまま）
    let zeta = tags::create_tag(&db, "zeta", Some("forest")).unwrap();
    tags::attach_tag(&db, &k1, zeta).unwrap();
    let rows = search::list_items(&db, &search::ListFilter { tag_id: Some(tid), ..Default::default() }).unwrap();
    assert_eq!(rows.len(), 2);
    let r1 = rows.iter().find(|r| r.item.key == k1).unwrap();
    let names: Vec<&str> = r1.tags.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["bulk", "zeta"], "名前順");
    assert_eq!(r1.tags[1].color.as_deref(), Some("forest"));
    let r2 = rows.iter().find(|r| r.item.key == k2).unwrap();
    assert_eq!(r2.tags.len(), 1);
    tags::rename_tag(&db, tid, "renamed").unwrap();
    assert_eq!(search::list_items(&db, &search::ListFilter { tag_id: Some(tid), ..Default::default() }).unwrap().len(), 2);
    assert_eq!(search::list_items(&db, &search::ListFilter { tag_id: Some(zeta), ..Default::default() }).unwrap().len(), 1);
    // 名前（大小無視）→ id → 絞り込み（MCP / CLI の経路）。綴り違いは None = 呼び出し側でエラーにできる
    let rid = tags::tag_by_name(&db.conn, "RENAMED").unwrap().expect("大小無視で引ける").0;
    assert_eq!(rid, tid);
    assert_eq!(search::list_items(&db, &search::ListFilter { tag_id: Some(rid), ..Default::default() }).unwrap().len(), 2);
    assert!(tags::tag_by_name(&db.conn, "renamd").unwrap().is_none());
}

#[test]
fn attach_or_skip_by_md5() {
    use openref_core::pdfingest::{attach_or_skip, PdfIngestOutcome as O};
    let tmp = std::env::temp_dir().join(format!("openref-md5-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let mut db = Db::open(&tmp.join("t.sqlite")).unwrap();
    let key = ingest::add_item(&mut db, &fake_resolved()).unwrap().key;
    let a = tmp.join("a.pdf");
    std::fs::write(&a, b"%PDF-1.4 fake A").unwrap();
    let b = tmp.join("b.pdf");
    std::fs::write(&b, b"%PDF-1.4 fake B").unwrap();

    // 未添付 → 添付 / 同一ファイル → スキップ / 別ファイル → 2 本目
    assert!(matches!(attach_or_skip(&db, &key, &a).unwrap(), O::AttachedToExisting { .. }));
    assert!(matches!(attach_or_skip(&db, &key, &a).unwrap(), O::DuplicateFile { .. }));
    assert!(matches!(attach_or_skip(&db, &key, &b).unwrap(), O::AttachedAdditional { .. }));
    let with_md5 = |db: &Db| -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM attachments WHERE md5 IS NOT NULL", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(with_md5(&db), 2, "新規添付は md5 が埋まる");

    // md5 列が空の既存添付（過去データ）は実体から遅延バックフィルされ、判定にも使われる
    db.conn.execute("UPDATE attachments SET md5 = NULL", []).unwrap();
    assert!(matches!(attach_or_skip(&db, &key, &b).unwrap(), O::DuplicateFile { .. }));
    assert_eq!(with_md5(&db), 2);
    std::fs::remove_dir_all(&tmp).ok();
}

/// 実弾（ネットワーク）: `cargo test -p openref-core --test integration -- --ignored crossref_title_search_live`
#[test]
#[ignore]
fn crossref_title_search_live() {
    let text = "RadioGraphics 2026 Original Research\nImaging of Deep Endometriosis Involving\nLateral Pelvic Structures\nBrandão AC, …";
    let cands = openref_core::resolver::search_bibliographic(
        "Imaging of Deep Endometriosis Involving Lateral Pelvic Structures",
        5,
    )
    .unwrap();
    assert!(!cands.is_empty(), "Crossref から候補が返るはず");
    let m = openref_core::pdfingest::pick_match(&cands, text).expect("閾値を満たす候補があるはず");
    assert_eq!(m.doi, "10.1148/rg.250127");
    eprintln!("matched: {} ({})", m.doi, m.reason);
}

/// 実弾（手元の実 PDF）: `OPENREF_TEST_PDF=/path/to.pdf OPENREF_PDFTOTEXT=off cargo test -p openref-core --test integration -- --ignored builtin_extract_live --nocapture`
/// 内蔵抽出（pdf-extract）だけで冒頭 2 頁から DOI/PMID が取れることを、抽出テキストそのもので確認する。
#[test]
#[ignore]
fn builtin_extract_live() {
    let Some(p) = openref_core::env_os("TEST_PDF") else {
        eprintln!("OPENREF_TEST_PDF 未設定のためスキップ");
        return;
    };
    let ex = openref_core::pdftools::extract_pages(std::path::Path::new(&p), 1, 2).unwrap();
    let text = ex.pages.join("\n");
    eprintln!("backend={} pages={} chars={}", ex.backend, ex.pages.len(), text.chars().count());
    eprintln!("head: {:?}", text.chars().take(200).collect::<String>());
    let ids = openref_core::pdfingest::find_ids_in_text(&text);
    eprintln!("ids={ids:?}");
    assert_eq!(ex.backend, "builtin");
    assert!(ids.is_some(), "内蔵抽出テキストから DOI/PMID が取れるはず");
}

/// 誌名チップの根本対策: 表記ゆれ（Paperpile "Abdom. Radiol. (NY)" vs PubMed "Abdom Radiol (NY)"、大小、記号）・
/// print/electronic ISSN の違い・short の無い item を journal_keys の推移閉包で 1 つの誌に束ねる。
/// 一方で "Radiology" と "Radiology. Artificial Intelligence" のような別誌は混ぜない
#[test]
fn journal_chip_merges_name_variants() {
    use openref_core::edit;
    use openref_core::search::{list_items, ListFilter};
    let mut db = Db::open_in_memory().unwrap();
    fn add(db: &mut Db, n: u32, ct: &str, cts: Option<&str>, issn: Option<&str>) -> String {
        let mut r = fake_resolved();
        let o = r.csl.as_object_mut().unwrap();
        o.insert("DOI".into(), json!(format!("10.9999/j.{n}")));
        o.insert("PMID".into(), json!(format!("8800{n:04}")));
        o.insert("title".into(), json!(format!("Journal variant paper {n}")));
        o.insert("container-title".into(), json!(ct));
        match cts {
            Some(v) => { o.insert("container-title-short".into(), json!(v)); }
            None => { o.remove("container-title-short"); }
        }
        match issn {
            Some(v) => { o.insert("ISSN".into(), json!(v)); }
            None => { o.remove("ISSN"); }
        }
        ingest::add_item(db, &r).unwrap().key
    }
    let _a = add(&mut db, 1, "Abdominal Radiology (New York)", Some("Abdom. Radiol. (NY)"), Some("2366-004X"));
    let _b = add(&mut db, 2, "Abdominal Radiology (New York)", Some("Abdom. Radiol. (NY)"), Some("2366-0058"));
    let _c = add(&mut db, 3, "Abdominal radiology (New York)", Some("Abdom Radiol (NY)"), Some("2366-0058"));
    let d = add(&mut db, 4, "Abdominal Radiology (New York)", None, None);
    let _e = add(&mut db, 5, "Radiology", Some("Radiology"), Some("0033-8419"));
    let _f = add(&mut db, 6, "Radiology. Artificial Intelligence", Some("Radiol. Artif. Intell."), Some("2638-6100"));
    let _g = add(&mut db, 7, "Radiographics : a review publication of the Radiological Society of North America, Inc", Some("Radiographics"), Some("1527-1323"));
    let _h = add(&mut db, 8, "Radiographics: A Review Publication of the Radiological Society of North America, Inc", Some("Radiographics"), Some("0271-5333"));
    // print ISSN だけが共通で誌名表記が全く違う（ISSN 経由で繋がることの確認）
    let _i = add(&mut db, 9, "ABDOMINAL RADIOLOGY", None, Some("2366-004X"));

    fn n(db: &Db, j: &str) -> usize {
        list_items(db, &ListFilter { journal: Some(j.into()), ..Default::default() }).unwrap().len()
    }
    assert_eq!(n(&db, "Abdom Radiol (NY)"), 5, "PubMed 表記からでも Paperpile 表記・short なし・ISSN のみ一致を束ねる");
    assert_eq!(n(&db, "Abdom. Radiol. (NY)"), 5);
    assert_eq!(n(&db, "abdominal radiology (new york)"), 5);
    assert_eq!(n(&db, "Abdominal Radiology"), 5, "ISSN だけ共通の item からも全体に届く（推移閉包）");
    assert_eq!(n(&db, "Radiology"), 1, "Radiology と Radiology. Artificial Intelligence は別誌");
    assert_eq!(n(&db, "Radiol. Artif. Intell."), 1);
    assert_eq!(n(&db, "Radiographics"), 2, "full の記号・大小ゆれと print/electronic ISSN を吸収");
    assert_eq!(n(&db, "Nope Journal"), 0);
    assert_eq!(n(&db, "   "), 9, "空ラベルは未指定扱い");

    // update で誌名を変えると journal_keys も貼り替わる（d: Abdominal → Radiology）
    let mut csl = openref_core::search::get_by_key(&db, &d).unwrap().unwrap().csl;
    csl.as_object_mut().unwrap().insert("container-title".into(), json!("Radiology"));
    edit::update_csl(&mut db, &d, &csl).unwrap();
    assert_eq!(n(&db, "Radiology"), 2);
    assert_eq!(n(&db, "Abdom Radiol (NY)"), 4);

    // purge で journal_keys も消える（FK CASCADE）
    edit::trash(&db, &d).unwrap();
    edit::purge(&db, &d).unwrap();
    let orphan: i64 = db.conn.query_row("SELECT COUNT(*) FROM journal_keys jk WHERE NOT EXISTS (SELECT 1 FROM items i WHERE i.id = jk.item_id)", [], |r| r.get(0)).unwrap();
    assert_eq!(orphan, 0);
    assert_eq!(n(&db, "Radiology"), 1);

    // 正規化関数そのもの
    assert_eq!(model::journal_norm("Abdom. Radiol. (NY)"), "abdom radiol ny");
    assert_eq!(model::journal_norm("Abdom Radiol (NY)"), "abdom radiol ny");
    assert_eq!(model::journal_norm("The Lancet"), "lancet");
    assert_eq!(model::journal_norm("日小児放線会誌"), "日小児放線会誌");
    assert_eq!(model::journal_norm("Ｊｐｎ　Ｊ　Ｒａｄｉｏｌ"), "jpn j radiol", "全角は NFKC で半角へ");
    let ks = model::journal_keys(&json!({"container-title": "Radiology", "container-title-short": "Radiology", "ISSN": "0033-8419, 1527-1315"}));
    assert_eq!(ks, vec!["issn:0033-8419", "issn:1527-1315", "t:radiology"]);
}

/// PDF 本文の全文検索（migration v7: fulltext + fulltext_fts）。
/// 本文にしか無い語で hit・メタ hit が先・和文本文は per-char で hit・スキャン PDF は空で記録、を固定する
#[test]
fn fulltext_search_hits_pdf_body() {
    use openref_core::{edit, fulltext, pdftools, search::{list_items, ListFilter}};
    let tmp = std::env::temp_dir().join(format!("openref-fulltext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut db = Db::open(&tmp.join("t.sqlite")).unwrap();
    let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert!(v >= 8, "migration v7/v8 が当たっていない: {v}");
    let integrity_ok = |db: &Db| fulltext::integrity_check(db).unwrap().is_none();

    // A = メタデータに pancreatic（PDF なし）、B = 別題名 + 本文に pancreatic と zebrafish
    let a = ingest::add_item(&mut db, &fake_resolved()).unwrap();
    let mut rb = fake_resolved();
    rb.csl["title"] = json!("Liver fibrosis staging with elastography");
    rb.csl["abstract"] = json!("Shear wave elastography of the liver.");
    rb.csl["DOI"] = json!("10.9999/openref.test.2");
    rb.csl["PMID"] = json!("99999998");
    rb.csl["custom"] = json!({"source": "test"});
    let b = ingest::add_item(&mut db, &rb).unwrap();
    let pdf_b = tmp.join("b.pdf");
    std::fs::write(&pdf_b, pdftools::minimal_pdf("Pancreatic zebrafish xenograft model results and discussion", None)).unwrap();
    // 添付直後に本文が索引される（attach_auto → write_managed → fulltext::index_bytes_quietly = 手元のバイト列から）
    edit::attach_auto(&db, &b.key, &pdf_b).unwrap();
    let (status, n_fts): (String, i64) = db
        .conn
        .query_row(
            "SELECT f.status, (SELECT COUNT(*) FROM fulltext_fts WHERE fulltext_fts MATCH 'zebrafish') FROM fulltext f",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((status.as_str(), n_fts), ("ok", 1));

    let q = |s: &str| list_items(&db, &ListFilter { search: Some(s.into()), ..Default::default() }).unwrap();
    // 本文にしか無い語で hit・抜粋に一致マーカー
    let hits = q("zebrafish");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.key, b.key);
    let snip = hits[0].fulltext_snippet.as_deref().expect("本文 hit には抜粋");
    assert!(snip.contains(&format!("{}zebrafish{}", fulltext::HL_START, fulltext::HL_END)), "{snip}");
    // メタ hit（A）が先・本文のみ hit（B）が後。A に抜粋は無い
    let hits = q("pancreatic");
    assert_eq!(hits.iter().map(|h| h.item.key.as_str()).collect::<Vec<_>>(), vec![a.key.as_str(), b.key.as_str()]);
    assert!(hits[0].fulltext_snippet.is_none());
    assert!(hits[1].fulltext_snippet.is_some());
    // 複数語は AND（本文とメタを跨いでは成立しない）
    assert_eq!(q("zebrafish elastography").len(), 0, "語は同じ索引（本文 or メタ）内で全て一致する必要がある");
    // metadata_only（Word の引用検索）は本文 hit を出さない
    assert_eq!(list_items(&db, &ListFilter { search: Some("zebrafish".into()), metadata_only: true, ..Default::default() }).unwrap().len(), 0);
    assert_eq!(list_items(&db, &ListFilter { search: Some("pancreatic".into()), metadata_only: true, ..Default::default() }).unwrap().len(), 1);
    // チップ・ビューとは AND のまま
    assert_eq!(list_items(&db, &ListFilter { search: Some("zebrafish".into()), starred_only: true, ..Default::default() }).unwrap().len(), 0);
    assert_eq!(list_items(&db, &ListFilter { search: Some("zebrafish".into()), author_family: Some("Kano".into()), ..Default::default() }).unwrap().len(), 1);
    // ゴミ箱の item の本文は通常ビューに出ない
    edit::trash(&db, &b.key).unwrap();
    assert_eq!(q("zebrafish").len(), 0);
    assert_eq!(list_items(&db, &ListFilter { search: Some("zebrafish".into()), trashed: true, ..Default::default() }).unwrap().len(), 1);
    edit::restore(&db, &b.key).unwrap();

    // 和文本文: cjk 列（per-char）で部分一致。抜粋は分かち書きを戻して返す
    let att_b: i64 = db.conn.query_row("SELECT id FROM attachments LIMIT 1", [], |r| r.get(0)).unwrap();
    fulltext::store_text(&db.conn, att_b, "小児の胆道疾患の画像診断。\u{c}MRI 検査の適応。", Some("x"), "ok", Some("test")).unwrap();
    let hits = q("胆道");
    assert_eq!(hits.len(), 1);
    let snip = hits[0].fulltext_snippet.as_deref().unwrap();
    assert!(snip.contains(&format!("{}胆道{}疾患", fulltext::HL_START, fulltext::HL_END)), "{snip}");
    assert_eq!(q("胆嚢").len(), 0);
    assert_eq!(q("zebrafish").len(), 0, "貼り替え後は旧本文で hit しない（トリガーで索引同期）");
    assert!(integrity_ok(&db), "貼り替え後の external content 索引が整合");
    // 合字は素の綴りに畳んで索引・保存（"ﬁ" → "fi"・"ﬂ" → "fl"）。本文に紛れた抜粋マーカーは除く。
    // （"fibrosis" は B の title にもあるので、本文だけの語 "flowchart" で本文 hit を見る）
    fulltext::store_text(&db.conn, att_b, &format!("liver ﬁbrosis ﬂowchart{} marker{}", fulltext::HL_START, fulltext::HL_END), Some("x"), "ok", None).unwrap();
    assert_eq!(q("flowchart").len(), 1, "合字 ﬂ を畳んだ語で hit");
    let stored: String = db.conn.query_row("SELECT text FROM fulltext WHERE attachment_id = ?1", [att_b], |r| r.get(0)).unwrap();
    assert_eq!(stored, "liver fibrosis flowchart marker");
    // 旧バイナリ（v7 より前の MCP get_fulltext）の INSERT OR REPLACE でも索引が壊れない（v8 BEFORE INSERT トリガー）
    db.conn
        .execute("INSERT OR REPLACE INTO fulltext (attachment_id, text) VALUES (?1, 'legacy replaced body')", [att_b])
        .unwrap();
    assert!(integrity_ok(&db), "INSERT OR REPLACE 後も整合");
    assert_eq!(q("flowchart").len(), 0, "旧本文の語は消える");
    assert_eq!(q("legacy").len(), 1);
    // 旧バイナリ由来の行（md5 無し）は backfill で抽出し直される（cjk も無いので）
    let rep = fulltext::backfill(&db, false, |_, _, _, _| {}).unwrap();
    assert_eq!((rep.total, rep.already, rep.indexed), (1, 0, 1), "md5 無し行は再索引対象");
    assert_eq!(q("zebrafish").len(), 1, "実体（zebrafish の PDF）から抽出し直されている");
    assert!(integrity_ok(&db));
    // 旧 attachments.md5 が実体とずれていても、索引時に実体の md5 で更新され、次回はスキップされる（毎回抽出のループにならない）
    db.conn.execute("UPDATE attachments SET md5 = 'stale' WHERE id = ?1", [att_b]).unwrap();
    let rep = fulltext::backfill(&db, false, |_, _, _, _| {}).unwrap();
    assert_eq!((rep.already, rep.indexed), (0, 1));
    let rep = fulltext::backfill(&db, false, |_, _, _, _| {}).unwrap();
    assert_eq!((rep.already, rep.indexed), (1, 0));
    // 文字化け（U+FFFD）した本文が status='ok' で入っていても backfill が取り直す（ゲート導入前の索引の自己修復）。
    // status='partial'（内蔵が化けて pdftotext 無し）は pdftotext が無い間はスキップ、見つかれば取り直す
    let real_md5: String = db.conn.query_row("SELECT md5 FROM attachments WHERE id = ?1", [att_b], |r| r.get(0)).unwrap();
    let garbled = format!("{} {}", "\u{FFFD}".repeat(600), "zebrafish".repeat(50));
    fulltext::store_text(&db.conn, att_b, &garbled, Some(&real_md5), "ok", Some("builtin")).unwrap();
    let rep = fulltext::backfill(&db, false, |_, _, _, _| {}).unwrap();
    assert_eq!((rep.already, rep.indexed), (0, 1), "化けた ok 行は取り直す");
    let st: String = db.conn.query_row("SELECT status FROM fulltext WHERE attachment_id = ?1", [att_b], |r| r.get(0)).unwrap();
    assert_eq!(st, "ok");
    fulltext::store_text(&db.conn, att_b, &garbled, Some(&real_md5), "partial", Some("test")).unwrap();
    std::env::set_var("OPENREF_PDFTOTEXT", "off");
    let rep = fulltext::backfill(&db, false, |_, _, _, _| {}).unwrap();
    assert_eq!((rep.already, rep.indexed), (1, 0), "pdftotext 無し → partial はそのまま");
    std::env::remove_var("OPENREF_PDFTOTEXT");
    if openref_core::pdftools::find_pdftotext().is_some() {
        let rep = fulltext::backfill(&db, false, |_, _, _, _| {}).unwrap();
        assert_eq!((rep.already, rep.indexed), (0, 1), "pdftotext が見つかれば partial を取り直す");
    }
    // 和文の貼り替えに戻してから backfill の md5 ロジックへ
    fulltext::store_text(&db.conn, att_b, "小児の胆道疾患の画像診断。\u{c}MRI 検査の適応。", Some("x"), "ok", Some("test")).unwrap();


    // backfill: 行があれば md5 一致（or 検証不能 = attachments.md5 が空の旧添付）でスキップ、--force で再抽出。
    // 再抽出時は空だった attachments.md5 を実体から埋める（遅延バックフィル）
    db.conn.execute("UPDATE attachments SET md5 = NULL WHERE id = ?1", [att_b]).unwrap();
    let rep = fulltext::backfill(&db, false, |_, _, _, _| {}).unwrap();
    assert_eq!((rep.total, rep.already, rep.indexed), (1, 1, 0));
    let rep = fulltext::backfill(&db, true, |_, _, _, _| {}).unwrap();
    assert_eq!((rep.total, rep.already, rep.indexed), (1, 0, 1));
    assert_eq!(q("zebrafish").len(), 1);
    let (a_md5, f_md5): (Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT a.md5, f.md5 FROM attachments a JOIN fulltext f ON f.attachment_id = a.id WHERE a.id = ?1",
            [att_b],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(a_md5.is_some() && a_md5 == f_md5, "attachments.md5 を実体から埋め fulltext.md5 と一致: {a_md5:?} / {f_md5:?}");

    // スキャン PDF（テキスト層なし）: 空で記録（status='empty'）・検索に出ない・再実行はスキップ
    let pdf_a = tmp.join("a.pdf");
    std::fs::write(&pdf_a, pdftools::minimal_pdf("", None)).unwrap();
    edit::attach_auto(&db, &a.key, &pdf_a).unwrap();
    let (status, text): (String, String) = db
        .conn
        .query_row(
            "SELECT f.status, f.text FROM fulltext f JOIN attachments at ON at.id = f.attachment_id JOIN items i ON i.id = at.item_id WHERE i.key = ?1",
            [&a.key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((status.as_str(), text.as_str()), ("empty", ""));
    let rep = fulltext::backfill(&db, false, |_, _, _, _| {}).unwrap();
    assert_eq!((rep.total, rep.already, rep.indexed, rep.empty), (2, 2, 0, 0));
    // MCP get_fulltext は空本文を返す（再抽出しない）
    let ft = fulltext::get_or_extract(&db, &a.key).unwrap();
    assert_eq!(ft.text, "");

    // purge → fulltext が CASCADE で消え、索引も追随する（トリガー）
    edit::trash(&db, &b.key).unwrap();
    edit::purge(&db, &b.key).unwrap();
    assert_eq!(q("zebrafish").len(), 0);
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM fulltext_fts WHERE fulltext_fts MATCH 'zebrafish'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    assert!(integrity_ok(&db), "purge 後も整合");
    fulltext::rebuild(&db).unwrap();
    assert!(integrity_ok(&db));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// 実弾（手元の全 PDF・読み取りのみ）:
/// `OPENREF_TEST_DB=/path/scratch.sqlite cargo test -p openref-core --test integration -- --ignored title_guard_survey --nocapture`
/// 題名ガード（`pdfingest::title_in_text`）の誤拒否率と「先頭 DOI = item の DOI」の一致率を蔵書全体で測る
/// （本文 DOI 経路の取り違え対策の副作用を実データで見るため。assert は無く数値を出すだけ = 回帰防止は
/// pdfingest の unit `first_candidate_guard` が担う。母集団の題名は Paperpile 由来なので「別論文を登録する」
/// 退行はここでは検出できない）
#[test]
#[ignore]
fn title_guard_survey() {
    use openref_core::pdfingest::{find_dois_in_text, guard_first_candidate, lenient_reason, normalize, title_in_text, Guard};
    use openref_core::{model, pdftools};
    let Some(p) = openref_core::env_os("TEST_DB") else {
        eprintln!("OPENREF_TEST_DB 未設定のためスキップ");
        return;
    };
    let db = Db::open(std::path::Path::new(&p)).unwrap();
    let rows: Vec<(String, String, String)> = {
        let mut stmt = db
            .conn
            .prepare("SELECT i.key, i.csl, a.path FROM attachments a JOIN items i ON i.id = a.item_id WHERE i.deleted_at IS NULL AND a.kind = 'pdf' ORDER BY i.id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(|r| r.unwrap()).collect()
    };
    let (mut n, mut missing, mut err, mut no_doi_item, mut no_doi_text, mut first_ok, mut first_ng, mut guard_ok, mut guard_ng, mut lenient_n) =
        (0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    for (key, csl, path) in rows {
        let csl: serde_json::Value = serde_json::from_str(&csl).unwrap();
        let title = model::s(&csl, "title").unwrap_or("").to_string();
        let item_doi = model::s(&csl, "DOI").map(|d| d.to_lowercase());
        let pth = std::path::Path::new(&path);
        if !pth.exists() {
            missing += 1;
            continue;
        }
        n += 1;
        let ex = match pdftools::extract_pages(pth, 1, 2) {
            Ok(e) => e,
            Err(e) => {
                err += 1;
                eprintln!("ERR  {key} {e}");
                continue;
            }
        };
        let text = ex.pages.join("\n");
        // 本番と同じ判定経路（lenient 規則・ガード）を通す = lenient 規則を変えるときはこれで測る
        let lenient = lenient_reason(&ex.pages, &text).is_some();
        if lenient {
            lenient_n += 1;
        }
        let full = normalize(&text);
        let guard = guard_first_candidate(&title, &text, lenient) != Guard::Reject;
        if guard {
            guard_ok += 1;
        } else {
            guard_ng += 1;
        }
        // 変種の計測: 冒頭 3000 字 / 6000 字に絞った場合（実測: 3000 字では Springer 系で誤拒否が出る）
        let pos = full.find(&normalize(&title)).map(|i| i as i64).unwrap_or(-1);
        let head3: String = full.chars().take(3000).collect();
        let whole = title_in_text(&title, &head3);
        let head6: String = full.chars().take(6000).collect();
        let g6 = title_in_text(&title, &head6);
        let dois = find_dois_in_text(&text);
        let first = dois.first().map(|d| d.to_lowercase());
        let doi_note = match (&item_doi, &first) {
            (None, _) => {
                no_doi_item += 1;
                "item に DOI なし".to_string()
            }
            (Some(_), None) => {
                no_doi_text += 1;
                "本文に DOI なし".to_string()
            }
            (Some(a), Some(b)) if a == b => {
                first_ok += 1;
                "先頭 DOI 一致".to_string()
            }
            (Some(a), Some(b)) => {
                first_ng += 1;
                format!("先頭 DOI 不一致 item={a} text={b} 候補={dois:?}")
            }
        };
        if !guard || doi_note.starts_with("先頭 DOI 不一致") {
            eprintln!(
                "{} {key} guard(全文)={guard} head3000={whole} head6000={g6} pos={pos} len={} lenient={lenient} backend={} | {doi_note} | {}",
                if guard { "DOI " } else { "NG  " },
                full.chars().count(),
                ex.backend,
                title.chars().take(90).collect::<String>()
            );
            if !guard {
                eprintln!("      head: {}", full.chars().take(400).collect::<String>());
            }
        }
    }
    eprintln!(
        "=== PDF {n}（欠損 {missing}・抽出エラー {err}）: 題名ガード 通過 {guard_ok} / 不通過 {guard_ng}（lenient {lenient_n}）| 先頭 DOI 一致 {first_ok} / 不一致 {first_ng} / 本文に DOI なし {no_doi_text} / item に DOI なし {no_doi_item}"
    );
}

/// 実弾（手元 PDF の抽出統計）: `OPENREF_TEST_PDFS=a.pdf:b.pdf cargo test -p openref-core --test integration -- --ignored text_stats_live --nocapture`
/// 冒頭 2 頁の文字数・空白率・CJK 数・ラテン文字数・文字化け率・lenient 判定を出す（題名ガードの lenient 規則の根拠づけ用）
#[test]
#[ignore]
fn text_stats_live() {
    use openref_core::{model, pdftools};
    let Some(list) = openref_core::env_os("TEST_PDFS") else {
        eprintln!("OPENREF_TEST_PDFS 未設定のためスキップ");
        return;
    };
    for p in list.to_string_lossy().split(':').filter(|s| !s.is_empty()) {
        let ex = match pdftools::extract_pages(std::path::Path::new(p), 1, 2) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("{p}: ERR {e}");
                continue;
            }
        };
        let text = ex.pages.join("\n");
        let total = text.chars().count();
        let ws = text.chars().filter(|c| c.is_whitespace()).count();
        let cjk = text.chars().filter(|c| model::is_cjk(*c)).count();
        let latin = text.chars().filter(|c| c.is_alphabetic() && !model::is_cjk(*c)).count();
        eprintln!(
            "{}: backend={} chars={total} ws={ws} ({:.0}%) cjk={cjk} latin={latin} garbled={:.3} cjk_heavy={} | head: {:?}",
            std::path::Path::new(p).file_name().unwrap().to_string_lossy(),
            ex.backend,
            100.0 * ws as f64 / total.max(1) as f64,
            pdftools::garbled_ratio(&ex.pages),
            openref_core::pdfingest::is_cjk_heavy(&text),
            text.chars().filter(|c| !c.is_whitespace()).take(120).collect::<String>()
        );
    }
}

/// 3 段 API と doi_check の印: 取得中に別経路で添付されたら attach は拒否する / 印は直前に添付した行にだけ・上書きしない
#[test]
fn fetch_pdf_three_phase_and_doi_check_mark() {
    use openref_core::{edit, pdffetch};
    let tmp = std::env::temp_dir().join(format!("openref-fetch3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut db = Db::open(&tmp.join("t.sqlite")).unwrap();
    let added = ingest::add_item(&mut db, &fake_resolved()).unwrap();

    // ① prepare は item の CSL を返す
    let plan = pdffetch::fetch_pdf_prepare(&db, &added.key).unwrap();
    assert_eq!(plan.key, added.key);
    assert_eq!(plan.csl["DOI"], "10.9999/openref.test.1");

    // ② の間（ロック外）に別経路で添付された → ③ は拒否し、取得物は捨てる
    let other = b"%PDF-1.4 other";
    let other_id = edit::attach_auto_bytes(&db, &added.key, "other.pdf", other).unwrap().id;
    let dl = pdffetch::Downloaded { bytes: b"%PDF-1.4 fetched".to_vec(), source: "test".into(), doi_check: Some("match") };
    let err = pdffetch::fetch_pdf_attach(&db, &plan, dl).unwrap_err();
    assert!(matches!(err, CoreError::Conflict(_)), "{err}");
    assert!(err.to_string().contains("別経路"), "{err}");
    assert!(!err.to_string().starts_with("入力を解釈できません"), "{err}");
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    // ① も既添付を拒否
    assert!(pdffetch::fetch_pdf_prepare(&db, &added.key).is_err());

    // 印: 直前に添付した行（添付行 id）にだけ書き、既に印のある行は上書きしない・別の id には書かない
    let mark = |db: &Db| -> Option<String> {
        db.conn
            .query_row(
                "SELECT doi_check FROM attachments WHERE item_id = (SELECT id FROM items WHERE key = ?1)",
                [&added.key],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(mark(&db), None, "手動添付は未照合 = NULL");
    pdffetch::mark_doi_check(&db, other_id + 1000, "match");
    assert_eq!(mark(&db), None, "別の添付 id には書かない");
    pdffetch::mark_doi_check(&db, other_id, "unknown");
    assert_eq!(mark(&db).as_deref(), Some("unknown"));
    pdffetch::mark_doi_check(&db, other_id, "match");
    assert_eq!(mark(&db).as_deref(), Some("unknown"), "既に印のある行は上書きしない");

    // 正常系: ③ が添付して印を書く（doi_check None なら NULL のまま）
    let mut r2 = fake_resolved();
    r2.csl["DOI"] = json!("10.9999/openref.test.2");
    r2.csl["PMID"] = json!("99999998");
    r2.csl["title"] = json!("Second test item");
    let added2 = ingest::add_item(&mut db, &r2).unwrap();
    let plan2 = pdffetch::fetch_pdf_prepare(&db, &added2.key).unwrap();
    let f = pdffetch::fetch_pdf_attach(
        &db,
        &plan2,
        pdffetch::Downloaded { bytes: b"%PDF-1.4 two".to_vec(), source: "publisher-page (x)".into(), doi_check: Some("match") },
    )
    .unwrap();
    assert_eq!(f.bytes_len, 12);
    assert_eq!(f.source, "publisher-page (x)");
    let m2: Option<String> = db
        .conn
        .query_row(
            "SELECT doi_check FROM attachments WHERE item_id = (SELECT id FROM items WHERE key = ?1)",
            [&added2.key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(m2.as_deref(), Some("match"));
    let d = search::item_detail(&db, &added2.key).unwrap().unwrap();
    assert_eq!(d.attachments[0].doi_check.as_deref(), Some("match"));

    let mut r3 = fake_resolved();
    r3.csl["DOI"] = json!("10.9999/openref.test.3");
    r3.csl["PMID"] = json!("99999997");
    let added3 = ingest::add_item(&mut db, &r3).unwrap();
    let plan3 = pdffetch::fetch_pdf_prepare(&db, &added3.key).unwrap();
    pdffetch::fetch_pdf_attach(
        &db,
        &plan3,
        pdffetch::Downloaded { bytes: b"%PDF-1.4 three".to_vec(), source: "pmc-aws (CC BY)".into(), doi_check: None },
    )
    .unwrap();
    let d3 = search::item_detail(&db, &added3.key).unwrap().unwrap();
    assert_eq!(d3.attachments[0].doi_check, None, "PMC の Unknown は印なし");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// 実弾（フォルダ管理・本番 DB の scratch コピーに書く）:
/// `OPENREF_TEST_DB=/path/scratch.sqlite cargo test -p openref-core --test integration -- --ignored folders_live --nocapture`
/// 作成 → サブ作成 → 孫作成 → 改名 → find_replace → 削除 を実 DB で流し、各段の行を SQL で直接確かめる。
/// 終わると collections / collection_items は元に戻り、items は一度も変わらない（外からも sqlite3 で前後を突き合わせる）。
/// 本番 DB（データディレクトリの DB・`pdf_write_mode=drive`）では走らない
#[test]
#[ignore]
fn folders_live() {
    use openref_core::{collections, datadir, edit};
    let Some(p) = openref_core::env_os("TEST_DB") else {
        eprintln!("OPENREF_TEST_DB 未設定のためスキップ");
        return;
    };
    let path = std::path::PathBuf::from(&p);
    let canon = |q: &std::path::Path| std::fs::canonicalize(q).unwrap_or_else(|_| q.to_path_buf());
    // 既定の DB パスは datadir::db_path を呼ばずに組み立てる（あちらは旧ディレクトリからの移行を走らせうる）
    if let Some(home) = std::env::var_os("HOME") {
        let prod = std::path::Path::new(&home).join("Library/Application Support").join(datadir::DIR_NAME).join(datadir::DB_FILE);
        assert_ne!(canon(&path), canon(&prod), "本番 DB では走らせない（scratch コピーを使う）");
    }
    let db = Db::open(&path).unwrap();
    assert_ne!(db.pdf_write_mode().unwrap(), "drive", "scratch コピーは直後に pdf_write_mode=managed にしてから使う");
    let q = |sql: &str| -> i64 { db.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    // items の指紋（行数 + key・csl・削除状態の連結の長さと合計）。一度も変わらないこと
    let items_sig = || -> (i64, String) {
        db.conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(key) + length(csl) + COALESCE(length(deleted_at), 0) + starred), 0) || ':' ||
                        COALESCE(MAX(date_modified), '') FROM items",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    };
    let (n_col0, n_ci0) = (q("SELECT COUNT(*) FROM collections"), q("SELECT COUNT(*) FROM collection_items"));
    let sig0 = items_sig();
    eprintln!("開始: collections {n_col0} / collection_items {n_ci0} / items {}", sig0.0);

    // 作成 → サブ → 孫（root の名前は既存と衝突しない印つき）
    let root = collections::create_collection(&db, "F4LIVE 実弾ルート", None).unwrap();
    let sub = collections::create_collection(&db, "F4LIVE 子", Some(root)).unwrap();
    let grand = collections::create_collection(&db, "f4live 孫", Some(sub)).unwrap();
    assert!(collections::create_collection(&db, "f4live 子", Some(root)).is_err(), "兄弟の大小違いは拒否");
    // 所属: 蔵書の先頭 2 件（ゴミ箱外）を子と孫へ
    let keys: Vec<String> = {
        let mut st = db.conn.prepare("SELECT key FROM items WHERE deleted_at IS NULL ORDER BY id LIMIT 2").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    for k in &keys {
        edit::add_to_collection(&db, k, sub).unwrap();
        edit::add_to_collection(&db, k, grand).unwrap();
    }
    let n_add = 2 * keys.len() as i64;
    assert_eq!(q("SELECT COUNT(*) FROM collections"), n_col0 + 3);
    assert_eq!(q("SELECT COUNT(*) FROM collection_items"), n_ci0 + n_add);
    let tree = |id: i64| -> (String, Option<i64>) {
        db.conn.query_row("SELECT name, parent_id FROM collections WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    };
    eprintln!("作成: {root} {:?} / {sub} {:?} / {grand} {:?}", tree(root), tree(sub), tree(grand));
    assert_eq!(tree(sub).1, Some(root));
    assert_eq!(tree(grand).1, Some(sub));

    // 改名（自分の大小違いは可・兄弟なし）
    collections::rename_collection(&db, root, "F4LIVE 改名ルート").unwrap();
    collections::rename_collection(&db, grand, "F4LIVE 孫").unwrap();
    assert_eq!(tree(root).0, "F4LIVE 改名ルート");
    assert_eq!(tree(grand).0, "F4LIVE 孫");

    // find_replace: 予定（dry run）は作ったフォルダだけに当たり、書かない → 実行
    let plan = collections::find_replace_collections(&db, "f4live", "F4DONE", true).unwrap();
    let mine = [root, sub, grand];
    assert!(plan.iter().all(|o| mine.contains(&o.id)), "本番のフォルダに当たらない: {plan:?}");
    assert_eq!(plan.iter().filter(|o| o.status == "renamed").count(), 3);
    assert_eq!(tree(sub).0, "F4LIVE 子", "dry run は書かない");
    let done = collections::find_replace_collections(&db, "f4live", "F4DONE", false).unwrap();
    assert_eq!(done.iter().filter(|o| o.status == "renamed").count(), 3);
    eprintln!("改名 + find_replace: {:?} / {:?} / {:?}", tree(root).0, tree(sub).0, tree(grand).0);
    assert_eq!(tree(sub).0, "F4DONE 子");
    let shown = openref_core::search::collections(&db).unwrap();
    let n_items_of = |id: i64| shown.iter().find(|r| r.id == id).map(|r| r.n_items).unwrap();
    assert_eq!((n_items_of(root), n_items_of(sub), n_items_of(grand)), (0, keys.len() as i64, keys.len() as i64), "件数は直下だけ");

    // 削除: サブツリーごと消え、所属は外れ、論文は残る
    assert_eq!(collections::delete_collection(&db, root).unwrap(), 3);
    assert_eq!(q("SELECT COUNT(*) FROM collections"), n_col0);
    assert_eq!(q("SELECT COUNT(*) FROM collection_items"), n_ci0);
    assert_eq!(q(&format!("SELECT COUNT(*) FROM collections WHERE id IN ({root}, {sub}, {grand})")), 0);
    assert_eq!(q("SELECT COUNT(*) FROM pragma_foreign_key_check"), 0);
    assert_eq!(items_sig(), sig0, "items は一度も変わらない");
    eprintln!("削除後: collections {} / collection_items {} / items {}（開始時と一致）", n_col0, n_ci0, sig0.0);
}

/// connector が誌面の着地ページ（RSNA の epdf）を webpage として渡した item は、PubMed / Crossref の補完で type も
/// article-journal に置き換わる。
/// URL など非権威フィールドは既存を保持し、CSL に無い生の Crossref 型名（posted-content 等）は受けない。
#[test]
fn enrich_replaces_scraped_type_with_registry_type() {
    let mut db = Db::open_in_memory().unwrap();
    let title = "Imaging of Deep Endometriosis Involving Lateral Pelvic Structures";
    let scraped = Resolved {
        csl: json!({
            "type": "webpage",
            "title": title,
            "URL": "https://pubs.rsna.org/doi/epdf/10.1148/rg.250127",
            "DOI": "10.1148/rg.250127",
            "PMID": "42490331"
        }),
        retracted: None,
        source: "connector",
        warnings: vec![],
    };
    let added = ingest::add_item(&mut db, &scraped).unwrap();
    let richer = json!({
        "type": "article-journal",
        "title": title,
        "container-title": "Radiographics : a review publication of the Radiological Society of North America, Inc",
        "volume": "46",
        "page": "e250127",
        "issued": {"date-parts": [[2026, 8]]},
        "DOI": "10.1148/rg.250127",
        "PMID": "42490331",
        "custom": {"source": "pubmed"}
    });
    assert!(ingest::merge_enrichment(&mut db, added.id, &enriched(richer.clone())).unwrap());
    let item = search::get_by_key(&db, &added.key).unwrap().unwrap();
    assert_eq!(item.csl["type"], "article-journal");
    assert_eq!(item.csl["URL"], "https://pubs.rsna.org/doi/epdf/10.1148/rg.250127", "URL は既存を保持");
    let t: String = db
        .conn
        .query_row("SELECT item_type FROM items WHERE id = ?1", [added.id], |r| r.get(0))
        .unwrap();
    assert_eq!(t, "article-journal", "生成列も追随");
    // 2 回目は変更なし
    assert!(!ingest::merge_enrichment(&mut db, added.id, &enriched(richer.clone())).unwrap());
    // CSL に無い型名は受けない（type は据え置き・他に差分が無ければ false）
    let odd = json!({"type": "posted-content", "title": title});
    assert!(!ingest::merge_enrichment(&mut db, added.id, &enriched(odd)).unwrap());
    let item = search::get_by_key(&db, &added.key).unwrap().unwrap();
    assert_eq!(item.csl["type"], "article-journal");
}

/// enrich が著者を差し替えたら creators_index（著者チップの絞り込み）も追随する（以前は FTS と
/// journal_keys だけを貼り替え、creators_index は connector の壊れた姓名分割のまま残っていた）。
#[test]
fn enrich_reindexes_creators() {
    let mut db = Db::open_in_memory().unwrap();
    let title = "Diagnostic Performance of PET in Recurrent Disease";
    let scraped = Resolved {
        csl: json!({
            "type": "article-journal",
            "title": title,
            "DOI": "10.1234/x1",
            "author": [{"family": "S", "given": "Houshmand"}]
        }),
        retracted: None,
        source: "connector",
        warnings: vec![],
    };
    let added = ingest::add_item(&mut db, &scraped).unwrap();
    let creators = |db: &Db| -> Vec<(i64, Option<String>, Option<String>)> {
        let mut st = db
            .conn
            .prepare("SELECT position, family, given FROM creators_index WHERE item_id = ?1 AND role = 'author' ORDER BY position")
            .unwrap();
        st.query_map([added.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(creators(&db), vec![(0, Some("S".into()), Some("Houshmand".into()))]);
    let richer = json!({
        "type": "article-journal",
        "title": title,
        "DOI": "10.1234/x1",
        "author": [{"family": "Houshmand", "given": "Sina"}, {"family": "Alavi", "given": "Abass"}]
    });
    assert!(ingest::merge_enrichment(&mut db, added.id, &enriched(richer.clone())).unwrap());
    assert_eq!(
        creators(&db),
        vec![
            (0, Some("Houshmand".into()), Some("Sina".into())),
            (1, Some("Alavi".into()), Some("Abass".into())),
        ],
        "creators_index が CSL の author と一致する"
    );
    // 著者チップ（creators_index の family）で引ける・古い姓では引けない
    let by_family = |fam: &str| {
        search::list_items(
            &db,
            &search::ListFilter { author_family: Some(fam.into()), ..Default::default() },
        )
        .unwrap()
        .len()
    };
    assert_eq!(by_family("Houshmand"), 1);
    assert_eq!(by_family("S"), 0);
}

/// 移行（apply）で取り込んだ書誌も和文 FTS（cjk per-char phrase）と英語の複語で引ける。
/// 旧 Paperpile importer のテスト（tests/import_paperpile.rs・削除済み）が守っていた検査を、現行の取込経路で守る。
#[test]
fn migrated_records_are_searchable_in_japanese_and_english() {
    use openref_core::import::{self, apply, files::AttachmentRoots, ImportSource};
    use std::sync::atomic::AtomicBool;
    let dir = std::env::temp_dir().join(format!(
        "openref-fts-apply-{}-{}",
        std::process::id(),
        openref_core::keys::new_key()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let json_path = dir.join("lib.json");
    std::fs::write(
        &json_path,
        json!([
            {
                "_id": "r1", "pubtype": "PP_ARTICLE",
                "title": "Import Test Article",
                "author": [{"first": "Taro", "last": "Test"}],
                "journalfull": "Test Journal",
                "doi": "10.5555/import.test", "pmid": "55555555",
                "published": {"year": "2026", "month": "7"},
                "abstract": "An abstract about importing references."
            },
            {
                "_id": "r2", "pubtype": "PP_ARTICLE",
                "title": "和文誌テスト論文（DOI・PMID なし）",
                "author": [{"first": "花子", "last": "検証"}],
                "journalfull": "日本テスト放射線学会雑誌",
                "published": {"year": "2025"}
            }
        ])
        .to_string(),
    )
    .unwrap();
    let input = import::paperpile::read_input(
        &json_path,
        ImportSource { id: "fts-test".into(), format: "paperpile".into() },
    )
    .unwrap();
    let roots = AttachmentRoots::new(std::slice::from_ref(&dir)).unwrap();
    let db_path = dir.join("library/openref.sqlite");
    let preview = apply::prepare(&db_path, &input, &roots, import::AttachmentMode::Copy).unwrap();
    apply::execute(&db_path, &json_path, &input, &preview, &roots, &AtomicBool::new(false), |_| {})
        .unwrap();

    let db = Db::open(&db_path).unwrap();
    assert_eq!(search::counts(&db).unwrap().items, 2);
    assert_eq!(search::search(&db, "テスト論文", 5).unwrap().len(), 1);
    assert_eq!(search::search(&db, "胆道", 5).unwrap().len(), 0); // 収載外の語は当然ヒットしない
    assert_eq!(search::search(&db, "検証", 5).unwrap().len(), 1); // 和文著者 family
    assert_eq!(search::search(&db, "放射線学会雑誌", 5).unwrap().len(), 1); // 誌名の部分一致
    assert_eq!(search::search(&db, "importing references", 5).unwrap().len(), 1); // 英語の複語
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 読めない（表が無い・行の型が違う）SQL を既定値 / 空 / false に畳まない: settings を消した DB で設定の読み手が Err を返し、
/// 保存先を判定できない添付は storage/ にも書かずに Err。linked の行が読めない sync・要確認 stub の判定も Err
#[test]
fn unreadable_sql_is_an_error_not_a_default() {
    use openref_core::{citestyle, edit, pdfingest, sync, update};
    let tmp = std::env::temp_dir().join(format!("openref-sqlerr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut db = Db::open(&tmp.join("t.sqlite")).unwrap();
    let added = ingest::add_item(&mut db, &fake_resolved()).unwrap();
    let item_id: i64 = db.conn.query_row("SELECT id FROM items WHERE key = ?1", [&added.key], |r| r.get(0)).unwrap();

    // 普段の経路: 読めれば今までどおり（未設定 = 既定・stub でない = false）
    assert_eq!(db.setting("pdf_write_mode").unwrap(), None);
    assert_eq!(db.pdf_write_mode().unwrap(), "managed");
    assert_eq!(citestyle::prefs(&db).unwrap()["style"], citestyle::DEFAULT_STYLE);
    assert!(update::cached(&db).unwrap().enabled);
    assert!(!pdfingest::is_review_stub(&db, &added.key).unwrap());
    let mount = tmp.join("mount");
    std::fs::create_dir_all(&mount).unwrap();
    assert_eq!(sync::scan(&db, &mount).unwrap().linked, 0);

    // linked の path が TEXT でない行 = 読めない行（今までは黙って数から落ち、孤立の報告からも漏れた）
    db.conn
        .execute("INSERT INTO attachments (key, item_id, mode, path) VALUES ('ATTBLOB', ?1, 'linked', x'00ff')", [item_id])
        .unwrap();
    assert!(sync::scan(&db, &mount).is_err(), "読めない linked 行は Err");
    db.conn.execute("DELETE FROM attachments WHERE key = 'ATTBLOB'", []).unwrap();

    db.conn.execute_batch("DROP TABLE settings").unwrap();
    assert!(db.setting("pdf_write_mode").is_err());
    assert!(db.pdf_write_mode().is_err(), "保存先が読めないのに managed と答えない");
    assert!(update::cached(&db).is_err(), "読めないまま「前回なし = 取りに行く番」にしない");
    assert!(citestyle::prefs(&db).is_err());
    assert!(
        citestyle::set_prefs_partial(&db, None, None, Some(true)).is_err(),
        "部分更新は読めなかった項目を既定で書き戻さない"
    );
    assert!(edit::attach_auto_bytes(&db, &added.key, "raw.pdf", b"%PDF-1.4 fake").is_err());
    let stored = walk_files(&tmp.join("storage"));
    assert!(stored.is_empty(), "保存先を判定できない添付は managed にも書かない: {stored:?}");

    db.conn.execute_batch("DROP TABLE item_tags").unwrap();
    assert!(pdfingest::is_review_stub(&db, &added.key).is_err(), "数えられないのを「stub でない」にしない");
    drop(db);
    let _ = std::fs::remove_dir_all(&tmp);
}

fn walk_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .flat_map(|e| if e.path().is_dir() { walk_files(&e.path()) } else { vec![e.path()] })
        .collect()
}
