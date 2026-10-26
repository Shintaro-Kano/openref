use openref_core::{import, search, Db};
use serde_json::json;

#[test]
fn paperpile_import_roundtrip() {
    // 一時 mount + JSON を用意
    let tmp = std::env::temp_dir().join(format!("openref-pp-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mount = tmp.join("Paperpile");
    let pdf_rel = "All Papers/T/Test et al. 2026 - Import.pdf";
    let pdf_abs = mount.join(pdf_rel);
    std::fs::create_dir_all(pdf_abs.parent().unwrap()).unwrap();
    std::fs::write(&pdf_abs, b"%PDF-1.4 fake").unwrap();

    let lib = json!([
        {
            "_id": "r1", "citekey": "Test2026-aa", "pubtype": "PP_ARTICLE",
            "title": "Import Test Article",
            "author": [{"first": "Taro", "last": "Test"}],
            "journal": "Test J", "journalfull": "Test Journal",
            "volume": "1", "issue": "2", "pages": "3-4",
            "doi": "10.5555/import.test", "pmid": "55555555",
            "published": {"year": "2026", "month": "7"},
            "keywords": "import; test",
            "abstract": "An abstract about importing references.",
            "foldersNamed": ["Parent/Child"],
            "attachments": [{"article_pdf": 1, "filename": pdf_rel}],
            "created": 1750000000.0
        },
        {
            "_id": "r2", "pubtype": "PP_ARTICLE",
            "title": "和文誌テスト論文（DOI・PMID なし）",
            "author": [{"first": "花子", "last": "検証"}],
            "journalfull": "日本テスト放射線学会雑誌",
            "published": {"year": "2025"},
            "foldersNamed": ["和文誌"],
            "attachments": [{"article_pdf": 1, "filename": "All Papers/K/検証 2025 - 存在しない.pdf"}],
            "created": 1750000001.0
        }
    ]);
    let jp = tmp.join("lib.json");
    std::fs::write(&jp, serde_json::to_string(&lib).unwrap()).unwrap();

    let mut db = Db::open_in_memory().unwrap();

    // dry-run: 書き込まれない
    let rep = import::paperpile::import(&mut db, &jp, Some(mount.clone()), true).unwrap();
    assert!(rep.dry_run);
    assert_eq!(rep.inserted, 2);
    assert_eq!(search::counts(&db).unwrap().items, 0);

    // 本 import
    let rep = import::paperpile::import(&mut db, &jp, Some(mount.clone()), false).unwrap();
    assert_eq!(rep.inserted, 2);
    assert_eq!(rep.merged, 0);
    assert_eq!(rep.collections_created, 3); // Parent, Child, 和文誌
    assert_eq!(rep.pdf_linked, 1);
    assert_eq!(rep.pdf_missing.len(), 1); // 存在しない和文 PDF
    // リンクした PDF は本文索引を試みる（偽 PDF なので抽出失敗 = 未索引に数える。実 PDF なら indexed）
    assert_eq!(rep.fulltext_indexed + rep.fulltext_unindexed, rep.pdf_linked);
    assert_eq!(rep.fulltext_unindexed, 1);
    assert_eq!(rep.without_ids, 1);

    let c = search::counts(&db).unwrap();
    assert_eq!(c.items, 2);
    assert_eq!(c.attachments, 1);
    assert_eq!(c.collections, 3);

    // 和文 FTS（cjk per-char phrase）: 部分語・著者名でヒット
    assert_eq!(search::search(&db, "テスト論文", 5).unwrap().len(), 1);
    assert_eq!(search::search(&db, "胆道", 5).unwrap().len(), 0); // 収載外の語は当然ヒットしない
    assert_eq!(search::search(&db, "検証", 5).unwrap().len(), 1); // 和文著者 family
    assert_eq!(search::search(&db, "放射線学会雑誌", 5).unwrap().len(), 1); // 誌名の部分一致

    // FTS・date_added 保存（1750000000 = 2025-06-15 UTC）
    assert_eq!(search::search(&db, "importing references", 5).unwrap().len(), 1);
    let rows = search::list(&db, 10).unwrap();
    let t1 = rows.iter().find(|r| r.key.len() == 8 && openref_core::model::s(&r.csl, "PMID").is_some()).unwrap();
    assert!(t1.date_added.starts_with("2025-06-15"));

    // 再 import → 全 merge（DOI/PMID なしは paperpile._id で突合）・二重付与なし
    let rep2 = import::paperpile::import(&mut db, &jp, Some(mount.clone()), false).unwrap();
    assert_eq!(rep2.inserted, 0);
    assert_eq!(rep2.merged, 2);
    let c2 = search::counts(&db).unwrap();
    assert_eq!(c2.items, 2);
    assert_eq!(c2.attachments, 1);
    assert_eq!(c2.collections, 3);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// 兄弟の照合は大小無視: GUI で "Liver" → "liver" と改名した後に Paperpile を再 import しても、
/// 同じ親の下に "Liver" をもう 1 つ作らない（edit::ensure_collection_path・collections::create_collection と同じ基準）
#[test]
fn paperpile_reimport_after_case_rename_does_not_duplicate_folders() {
    use openref_core::collections;
    let tmp = std::env::temp_dir().join(format!("openref-pp-nocase-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let lib = json!([{
        "_id": "n1", "pubtype": "PP_ARTICLE",
        "title": "Case-insensitive folder matching",
        "author": [{"first": "Taro", "last": "Test"}],
        "doi": "10.5555/nocase.folder",
        "published": {"year": "2026"},
        "foldersNamed": ["Liver/MRI"]
    }]);
    let jp = tmp.join("lib.json");
    std::fs::write(&jp, serde_json::to_string(&lib).unwrap()).unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let rep = import::paperpile::import(&mut db, &jp, None, false).unwrap();
    assert_eq!(rep.collections_created, 2);
    let id_of = |db: &Db, name: &str| -> i64 {
        db.conn.query_row("SELECT id FROM collections WHERE name = ?1", [name], |r| r.get(0)).unwrap()
    };
    let liver = id_of(&db, "Liver");
    let mri = id_of(&db, "MRI");
    collections::rename_collection(&db, liver, "liver").unwrap();
    collections::rename_collection(&db, mri, "mri").unwrap();
    let rep2 = import::paperpile::import(&mut db, &jp, None, false).unwrap();
    assert_eq!(rep2.merged, 1);
    assert_eq!(rep2.collections_created, 0, "大小違いの兄弟を作らない");
    assert_eq!(search::counts(&db).unwrap().collections, 2);
    let n: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM collection_items WHERE collection_id = ?1", [mri], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "所属は改名後のフォルダのまま（二重所属なし）");
    let _ = std::fs::remove_dir_all(&tmp);
}
