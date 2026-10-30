use openref_core::{
    import::{
        self, apply,
        files::{AttachmentRoots, FileFingerprint},
        AttachmentMode, ImportFolder, ImportNote, ImportSource, ImportTag,
    },
    Db,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

struct Fixture {
    dir: PathBuf,
    db: PathBuf,
    json: PathBuf,
}
impl Fixture {
    fn new(records: Value) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "openref-apply-{}-{}",
            std::process::id(),
            openref_core::keys::new_key()
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).unwrap();
        let db = dir.join("library/openref.sqlite");
        let json = dir.join("library.json");
        std::fs::write(&json, records.to_string()).unwrap();
        Self { dir, db, json }
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
    fn input(&self) -> import::ImportInput {
        import::paperpile::read_input(
            &self.json,
            ImportSource {
                id: "test-library".into(),
                format: "paperpile".into(),
            },
        )
        .unwrap()
    }
    fn roots(&self) -> AttachmentRoots {
        AttachmentRoots::new(std::slice::from_ref(&self.dir)).unwrap()
    }
    fn run(&self, input: &import::ImportInput, mode: AttachmentMode) -> apply::ImportReport {
        let roots = self.roots();
        let preview = apply::prepare(&self.db, input, &roots, mode).unwrap();
        apply::execute(
            &self.db,
            &self.json,
            input,
            &preview,
            &roots,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        let db = Db::open(&self.db).unwrap();
        db.conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}
/// 履歴の run_report が数える run_id の索引待ち。CLI（私用コピー）と GUI（開いている接続）の 2 経路が同じ値を返すことも確かめる
fn history_pending(db: &std::path::Path, run_id: &str) -> Value {
    let snapshot = apply::run_report_snapshot(db, run_id)
        .unwrap()
        .expect("run in history");
    let live = apply::run_report(&Db::open(db).unwrap().conn, run_id)
        .unwrap()
        .expect("run in history");
    assert_eq!(snapshot, live);
    snapshot["report"]["fulltext_pending"].clone()
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn missing_database_preview_creates_no_user_database_or_storage() {
    let f = Fixture::new(json!([{"_id":"one","title":"Bibliography only"}]));
    let plan = apply::prepare(&f.db, &f.input(), &f.roots(), AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.new_items, 1);
    assert!(!f.db.parent().unwrap().exists());
    assert_eq!(
        plan.managed_storage,
        Some(
            f.db.parent()
                .unwrap()
                .canonicalize()
                .unwrap_or_else(|_| std::fs::canonicalize(&f.dir).unwrap().join("library"))
                .join("storage")
        )
    );
}

#[test]
fn active_wal_preview_includes_commits_without_touching_source_sidecars() {
    let f = Fixture::new(json!([{"title":"incoming","doi":"10.5555/existing"}]));
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "INSERT INTO items(key,csl) VALUES('EXISTING',?1)",
            [json!({"title":"Saved","DOI":"10.5555/existing"}).to_string()],
        )
        .unwrap();
    let names = [
        f.db.clone(),
        PathBuf::from(format!("{}-wal", f.db.display())),
        PathBuf::from(format!("{}-shm", f.db.display())),
    ];
    let before: Vec<_> = names.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let plan = apply::prepare(&f.db, &f.input(), &f.roots(), AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.existing_items, 1);
    assert_eq!(
        names
            .iter()
            .map(|p| std::fs::read(p).unwrap())
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn closed_database_preview_never_creates_wal_shm_or_backups() {
    let f = Fixture::new(json!([{"title":"incoming"}]));
    drop(Db::open(&f.db).unwrap());
    let before = std::fs::read(&f.db).unwrap();
    let names: Vec<_> = std::fs::read_dir(f.db.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    apply::prepare(&f.db, &f.input(), &f.roots(), AttachmentMode::Copy).unwrap();
    assert_eq!(std::fs::read(&f.db).unwrap(), before);
    assert_eq!(
        std::fs::read_dir(f.db.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>(),
        names
    );
}

#[test]
fn copy_ignores_drive_setting_repeats_without_duplicates_and_preserves_dates() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Paper","created":1750000000.0,"attachments":[{"_id":"main","filename":"main.pdf"},{"_id":"supp","filename":"supp.pdf"}]},{"_id":"two","title":"No identifiers or PDF"}]),
    );
    let main = f.file("main.pdf", b"%PDF-1.4 main");
    f.file("supp.pdf", b"%PDF-1.4 supplement");
    let db = Db::open(&f.db).unwrap();
    db.set_setting("pdf_write_mode", "drive").unwrap();
    drop(db);
    let input = f.input();
    let one = f.run(&input, AttachmentMode::Copy);
    assert_eq!(
        one.results
            .iter()
            .filter(|r| r.status == "inserted")
            .count(),
        2
    );
    assert_eq!(f.count("attachments"), 2);
    assert_eq!(f.count("import_objects"), 6); // Includes memberships and bibliography key history.
    let two = f.run(&input, AttachmentMode::Copy);
    assert_eq!(
        two.results.iter().filter(|r| r.status == "merged").count(),
        2
    );
    assert_eq!(f.count("items"), 2);
    assert_eq!(f.count("attachments"), 2);
    assert_eq!(f.count("import_runs"), 2);
    let db = Db::open(&f.db).unwrap();
    assert_eq!(db.pdf_write_mode(), "drive");
    let rows: Vec<(String, String)> = db
        .conn
        .prepare("SELECT mode,path FROM attachments")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert!(rows.iter().all(|r| r.0 == "managed"
        && PathBuf::from(&r.1).starts_with(
            f.db.parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join("storage")
        )));
    assert_eq!(
        FileFingerprint::read(&main).unwrap(),
        FileFingerprint::read(&PathBuf::from(&rows[0].1)).unwrap()
    );
    let date: String = db
        .conn
        .query_row(
            "SELECT date_added FROM items WHERE title='Paper'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(date.starts_with("2025-06-15"));
    assert!(one.backup_path.unwrap().exists());
    std::fs::remove_file(&main).unwrap();
    assert!(PathBuf::from(&rows[0].1).is_file());
}

#[test]
fn missing_pdf_can_be_added_later_without_duplicating_other_attachments() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Paper","attachments":[{"_id":"a","filename":"a.pdf"},{"_id":"b","filename":"b.pdf"}]}]),
    );
    f.file("a.pdf", b"%PDF-a");
    let input = f.input();
    let one = f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("attachments"), 1);
    assert!(one.results[0]
        .attachments
        .iter()
        .any(|a| a.status == "unresolved"));
    f.file("b.pdf", b"%PDF-b");
    f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("items"), 1);
    assert_eq!(f.count("attachments"), 2);
}

#[test]
fn cancel_at_boundary_and_during_copy_then_reexecute_resumes_consistently() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"One"},{"_id":"two","title":"Two","attachments":[{"_id":"a","filename":"large.pdf"}]}]),
    );
    let mut bytes = b"%PDF-".to_vec();
    bytes.resize(200000, b'x');
    f.file("large.pdf", &bytes);
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let cancel = AtomicBool::new(false);
    let report = apply::execute(&f.db, &f.json, &input, &plan, &roots, &cancel, |p| {
        if p.phase == "copy" {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(report.state, "cancelled");
    assert_eq!(f.count("items"), 1);
    assert_eq!(f.count("attachments"), 0);
    assert_eq!(f.count("import_results"), 1);
    let db = Db::open(&f.db).unwrap();
    let unfinished: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM import_files WHERE state IN ('planned','ready')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unfinished, 0);
    drop(db);
    let report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(report.state, "finished");
    assert_eq!(f.count("items"), 2);
    assert_eq!(f.count("attachments"), 1);
    assert_eq!(report.results[0].status, "merged");
    assert_eq!(report.results[1].status, "inserted");
}

#[test]
fn linked_attachment_is_promoted_in_place_and_keeps_annotation_fk() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Paper","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    f.file("a.pdf", b"%PDF-a");
    let input = f.input();
    f.run(&input, AttachmentMode::Link);
    assert!(!f.db.parent().unwrap().join("storage").exists());
    let db = Db::open(&f.db).unwrap();
    let (id, key): (i64, String) = db
        .conn
        .query_row("SELECT id,key FROM attachments", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    db.conn.execute("INSERT INTO annotations(key,attachment_id,author,kind,comment) VALUES('ANNOTATE',?1,'user','note','Preserve this')",[id]).unwrap();
    drop(db);
    let report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(report.results[0].attachments[0].status, "relocated");
    let db = Db::open(&f.db).unwrap();
    assert_eq!(f.count("attachments"), 1);
    assert_eq!(f.count("annotations"), 1);
    let updated: (i64, String, String) = db
        .conn
        .query_row("SELECT id,key,mode FROM attachments", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(updated, (id, key, "managed".into()));
}

#[test]
fn folders_notes_tags_retain_edits_and_tombstones_on_reexecution() {
    let f = Fixture::new(json!([{"_id":"one","title":"Original"}]));
    let mut input = f.input();
    input.folders = vec![
        ImportFolder {
            id: "parent".into(),
            stable_id: true,
            name: "Name/with slash".into(),
            parent_id: None,
            raw: json!({}),
        },
        ImportFolder {
            id: "empty".into(),
            stable_id: true,
            name: "Empty".into(),
            parent_id: Some("parent".into()),
            raw: json!({}),
        },
    ];
    input.items[0].folder_ids.push("parent".into());
    input.items[0].notes = vec![
        ImportNote {
            stable_id: false,
            id: "n1".into(),
            content_md: "First".into(),
            raw: json!({"original":"First"}),
            date_added: None,
            date_modified: None,
        },
        ImportNote {
            stable_id: false,
            id: "n2".into(),
            content_md: "Second".into(),
            raw: json!({}),
            date_added: None,
            date_modified: None,
        },
    ];
    input.items[0].tags.push(ImportTag {
        id: "test-tag".into(),
        stable_id: false,
        name: "Label".into(),
        color: Some("#112233".into()),
        raw: json!({}),
    });
    f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("collections"), 2);
    assert_eq!(f.count("notes"), 2);
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "UPDATE notes SET content_md='User edits' WHERE content_md='First'",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "UPDATE collections SET name='Renamed' WHERE name='Name∕with slash'",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "UPDATE collections SET deleted_at=datetime('now') WHERE name='Empty'",
            [],
        )
        .unwrap();
    drop(db);
    input.items[0].notes[0].content_md = "Changed source note".into();
    let report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("notes"), 2);
    assert_eq!(f.count("collections"), 2);
    assert_eq!(f.count("tags"), 1);
    assert!(report.results[0]
        .warnings
        .iter()
        .any(|w| w.contains("差分候補")));
    let db = Db::open(&f.db).unwrap();
    assert_eq!(
        db.conn
            .query_row(
                "SELECT count(*) FROM notes WHERE content_md='User edits'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        db.conn
            .query_row(
                "SELECT count(*) FROM collections WHERE name='Renamed'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        db.conn
            .query_row(
                "SELECT count(*) FROM collections WHERE deleted_at IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn changed_input_or_pdf_aborts_before_creating_database() {
    let f = Fixture::new(json!([{"title":"Paper","attachments":[{"_id":"a","filename":"a.pdf"}]}]));
    let p = f.file("a.pdf", b"%PDF-before");
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    std::fs::write(p, b"%PDF-after").unwrap();
    assert!(apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {}
    )
    .is_err());
    assert!(!f.db.parent().unwrap().exists());
}

#[test]
fn write_transaction_rechecks_identifiers_after_preview() {
    let f = Fixture::new(json!([{"title":"Incoming","doi":"10.5555/same"}]));
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "INSERT INTO items(key,csl) VALUES('RACED',?1)",
            [json!({"title":"Keep edits","DOI":"10.5555/same"}).to_string()],
        )
        .unwrap();
    drop(db);
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.results[0].status, "merged");
    assert_eq!(f.count("items"), 1);
    let db = Db::open(&f.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT title FROM items", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Keep edits"
    );
}

#[test]
fn backup_failure_prevents_items_sources_and_run_creation() {
    let f = Fixture::new(json!([{"title":"Incoming"}]));
    drop(Db::open(&f.db).unwrap());
    std::fs::write(f.db.parent().unwrap().join("backup"), b"not a directory").unwrap();
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert!(apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {}
    )
    .is_err());
    assert_eq!(f.count("items"), 0);
    assert_eq!(f.count("import_runs"), 0);
    assert_eq!(f.count("import_sources"), 0);
}

/// 移行前のスナップショットは pre-import- 固定の接頭辞で新しい 3 本だけ残る（同じ秒に続いても上書きしない）。
/// 旧版の run id 入りの名前（pre-import-<run>-<日時>.sqlite）は数えも消しもしない
#[test]
fn pre_import_backups_keep_newest_three() {
    let f = Fixture::new(json!([{"_id":"one","title":"Backup","created":1750000000.0}]));
    let backups = f.db.parent().unwrap().join("backup");
    std::fs::create_dir_all(&backups).unwrap();
    let legacy = backups.join("pre-import-ABCDEFGH23456789-20261029-101010.sqlite");
    std::fs::write(&legacy, b"legacy").unwrap();
    let input = f.input();
    let mut paths = Vec::new();
    for _ in 0..5 {
        paths.push(f.run(&input, AttachmentMode::Copy).backup_path.unwrap());
    }
    let mut names: Vec<_> = std::fs::read_dir(&backups)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("pre-import-"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 4, "新しい 3 本 + 旧名 1 本: {names:?}");
    assert!(legacy.exists());
    for (n, path) in paths.iter().enumerate() {
        assert_eq!(path.exists(), n >= 2, "{n}: {}", path.display());
    }
    let recorded: String = Db::open(&f.db)
        .unwrap()
        .conn
        .query_row(
            "SELECT backup_path FROM import_runs ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(PathBuf::from(recorded), paths[4]);
}

#[test]
fn job_lock_covers_aliases_and_is_released_on_drop() {
    let f = Fixture::new(json!([]));
    let lock = apply::JobLock::acquire(&f.db).unwrap();
    assert!(apply::JobLock::acquire(&f.db).is_err());
    drop(lock);
    assert!(apply::JobLock::acquire(&f.db).is_ok());
}

#[test]
fn schema_upgrade_saves_v9_snapshot_and_backup_failure_leaves_v9_intact() {
    let f = Fixture::new(json!([]));
    let db = Db::open(&f.db).unwrap();
    db.conn.execute_batch("DROP INDEX IF EXISTS idx_tags_import_key; ALTER TABLE tags DROP COLUMN import_key; DROP TRIGGER IF EXISTS import_match_insert; DROP TRIGGER IF EXISTS import_match_invalidate; DROP TABLE IF EXISTS import_match_keys; DROP INDEX IF EXISTS idx_import_doi; DROP INDEX IF EXISTS idx_import_pmid; DROP INDEX IF EXISTS idx_import_unusual_ids; DROP TABLE import_files; DROP TABLE import_results; DROP TABLE import_objects; DROP TABLE import_runs; DROP TABLE import_sources; INSERT INTO items(key,csl) VALUES('KEEP','{\"title\":\"Old edits\"}'); PRAGMA user_version=9;").unwrap();
    drop(db);
    let db = Db::open(&f.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
    drop(db);
    let backup = std::fs::read_dir(f.db.parent().unwrap().join("backup"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let c = rusqlite::Connection::open(&backup).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert_eq!(
        c.query_row("SELECT title FROM items WHERE key='KEEP'", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Old edits"
    );
    drop(c);
    let other = Fixture::new(json!([]));
    let db = Db::open(&other.db).unwrap();
    db.conn.execute_batch("DROP INDEX IF EXISTS idx_tags_import_key; ALTER TABLE tags DROP COLUMN import_key; DROP TRIGGER IF EXISTS import_match_insert; DROP TRIGGER IF EXISTS import_match_invalidate; DROP TABLE IF EXISTS import_match_keys; DROP INDEX IF EXISTS idx_import_doi; DROP INDEX IF EXISTS idx_import_pmid; DROP INDEX IF EXISTS idx_import_unusual_ids; DROP TABLE import_files; DROP TABLE import_results; DROP TABLE import_objects; DROP TABLE import_runs; DROP TABLE import_sources; PRAGMA user_version=9;").unwrap();
    drop(db);
    std::fs::write(other.db.parent().unwrap().join("backup"), b"blocked").unwrap();
    assert!(Db::open(&other.db).is_err());
    let c = rusqlite::Connection::open(&other.db).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
}

#[test]
fn failed_copy_keeps_bibliography_and_never_falls_back_to_link() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Paper","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    f.file("a.pdf", b"%PDF-a");
    drop(Db::open(&f.db).unwrap());
    let storage = f.db.parent().unwrap().join("storage");
    std::fs::write(&storage, b"not a folder").unwrap();
    let input = f.input();
    let report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(report.results[0].status, "inserted");
    assert_eq!(report.results[0].attachments[0].status, "failed");
    assert_eq!(f.count("items"), 1);
    assert_eq!(f.count("attachments"), 0);
    std::fs::rename(&storage, f.dir.join("saved-test-file")).unwrap();
    let report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(report.results[0].status, "merged");
    assert_eq!(f.count("attachments"), 1);
}

#[test]
fn failed_item_transaction_rolls_back_mapping_and_cleans_only_its_copy() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Paper","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    let original = f.file("a.pdf", b"%PDF-a");
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let report=apply::execute(&f.db,&f.json,&input,&plan,&roots,&AtomicBool::new(false),|p|{if p.phase=="item" {let db=Db::open(&f.db).unwrap();db.conn.execute_batch("CREATE TRIGGER fail_item BEFORE INSERT ON import_objects WHEN new.kind='item' BEGIN SELECT RAISE(ABORT,'injected item failure'); END;").unwrap();}}).unwrap();
    assert_eq!(report.results[0].status, "failed");
    assert!(report.results[0].item_key.is_none());
    assert_eq!(f.count("items"), 0);
    assert_eq!(f.count("attachments"), 0);
    assert!(original.exists());
    let db = Db::open(&f.db).unwrap();
    let copied_path: String = db
        .conn
        .query_row("SELECT path FROM import_files", [], |r| r.get(0))
        .unwrap();
    assert!(!PathBuf::from(copied_path).exists());
    assert_eq!(
        db.conn
            .query_row(
                "SELECT count(*) FROM import_objects WHERE kind='item'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn interrupted_results_are_readable_and_previous_unreferenced_files_are_recovered() {
    let f = Fixture::new(json!([{"_id":"one","title":"Paper"}]));
    let input = f.input();
    let first = f.run(&input, AttachmentMode::Copy);
    let storage =
        f.db.parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("storage");
    let run = first.run_id.clone();
    let orphan = storage.join("ORPHAN23/file.pdf");
    let part = storage
        .join(".imports")
        .join(&run)
        .join("ORPHAN23")
        .join("file.part");
    std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    std::fs::write(&orphan, b"%PDF-orphan").unwrap();
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "UPDATE import_runs SET state='running',report_json=NULL WHERE id=?1",
            [&run],
        )
        .unwrap();
    db.conn.execute("INSERT INTO import_files(run_id,key,path,staging_path,sha256,state,file_device,file_inode) VALUES(?1,'ORPHAN23',?2,?3,'test','ready',?4,?5)",rusqlite::params![run,orphan.to_string_lossy(),part.to_string_lossy(),{use std::os::unix::fs::MetadataExt;std::fs::metadata(&orphan).unwrap().dev().to_string()},{use std::os::unix::fs::MetadataExt;std::fs::metadata(&orphan).unwrap().ino().to_string()}]).unwrap();
    drop(db);
    let history = apply::run_report_snapshot(&f.db, &run).unwrap().unwrap();
    assert_eq!(history["report"]["results"].as_array().unwrap().len(), 1);
    let untouched = f.file("unrelated.pdf", b"%PDF-keep");
    f.run(&input, AttachmentMode::Copy);
    assert!(!orphan.exists());
    assert!(untouched.exists());
    assert_eq!(f.count("items"), 1);
    let db = Db::open(&f.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT state FROM import_runs WHERE id=?1", [run], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "interrupted"
    );
}

#[test]
fn item_deleted_after_preview_is_not_automatically_recreated() {
    let f = Fixture::new(json!([{"title":"Incoming","doi":"10.5555/same"}]));
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "INSERT INTO items(key,csl) VALUES('BEFORE',?1)",
            [json!({"DOI":"10.5555/same","title":"Existing"}).to_string()],
        )
        .unwrap();
    drop(db);
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.existing_items, 1);
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |p| {
            if p.phase == "item" {
                let db = Db::open(&f.db).unwrap();
                db.conn
                    .execute("DELETE FROM items WHERE key='BEFORE'", [])
                    .unwrap();
            }
        },
    )
    .unwrap();
    assert_eq!(report.results[0].status, "blocked");
    assert_eq!(f.count("items"), 0);
}

#[test]
fn normalized_unicode_identifiers_are_rechecked_with_the_same_rules_as_preview() {
    let f = Fixture::new(json!([{"title":"Incoming","doi":"10.5555/k","pmid":"123"}]));
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("INSERT INTO items(key,csl) VALUES('UNICODE',?1)",[json!({"DOI":"  DOI:   10.5555/K\u{00a0}","PMID":"\u{00a0}000123","title":"Preserved"}).to_string()]).unwrap();
    drop(db);
    let report = f.run(&f.input(), AttachmentMode::Copy);
    assert_eq!(report.results[0].status, "merged");
    assert_eq!(f.count("items"), 1);
}

#[test]
fn legacy_writers_invalidate_weak_match_index_without_losing_review_candidates() {
    let f = Fixture::new(
        json!([{"title":"Incoming","author":[{"last":"Author"}],"published":{"year":"2026"}}]),
    );
    let db = Db::open(&f.db).unwrap();
    openref_core::edit::create_manual(&mut Db::open(&f.db).unwrap(),&json!({"type":"article-journal","title":"Different","author":[{"family":"Other"}],"issued":{"date-parts":[[2026]]}})).unwrap();
    drop(db);
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.new_items, 1);
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |p| {
            if p.phase == "item" {
                let db = Db::open(&f.db).unwrap();
                db.conn
                    .execute("UPDATE items SET csl=?1", [input.items[0].csl.to_string()])
                    .unwrap();
            }
        },
    )
    .unwrap();
    assert_eq!(report.results[0].status, "review");
    assert_eq!(f.count("items"), 1);
}

#[test]
fn another_writer_can_commit_while_pdf_bytes_are_being_copied() {
    let f =
        Fixture::new(json!([{"title":"Paper","attachments":[{"_id":"a","filename":"large.pdf"}]}]));
    let mut bytes = b"%PDF-".to_vec();
    bytes.resize(200000, b'x');
    f.file("large.pdf", &bytes);
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let mut writes = 0;
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |p| {
            if p.phase == "copy" {
                let db = Db::open(&f.db).unwrap();
                db.set_setting("during_import", "responsive").unwrap();
                writes += 1;
            }
        },
    )
    .unwrap();
    assert!(writes > 1);
    assert_eq!(report.results[0].status, "inserted");
}

#[test]
fn preview_cancellation_during_hashing_creates_no_database() {
    let f =
        Fixture::new(json!([{"title":"Paper","attachments":[{"_id":"a","filename":"large.pdf"}]}]));
    let mut bytes = b"%PDF-".to_vec();
    bytes.resize(200000, b'x');
    f.file("large.pdf", &bytes);
    let cancel = AtomicBool::new(false);
    let result = apply::prepare_controlled(
        &f.db,
        &f.input(),
        &f.roots(),
        AttachmentMode::Copy,
        &cancel,
        |p| {
            if p.phase == "preview" && p.bytes > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    );
    assert!(result.is_err());
    assert!(!f.db.parent().unwrap().exists());
}

#[test]
fn already_copied_pdf_is_reused_when_original_has_been_removed() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Paper","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    let source = f.file("a.pdf", b"%PDF-original");
    let input = f.input();
    f.run(&input, AttachmentMode::Copy);
    let files = f.count("import_files");
    std::fs::remove_file(source).unwrap();
    let plan = apply::prepare(&f.db, &input, &f.roots(), AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.missing_pdfs, 0);
    assert_eq!(plan.counts.existing_pdfs, 1);
    assert_eq!(plan.counts.copy_bytes_upper_bound, 0);
    let report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(report.results[0].attachments[0].status, "existing");
    assert_eq!(f.count("attachments"), 1);
    assert_eq!(f.count("import_files"), files);
}

#[test]
fn source_arriving_after_existing_copy_preview_requires_reconfirmation() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Paper","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    let source = f.file("a.pdf", b"%PDF-original");
    let input = f.input();
    f.run(&input, AttachmentMode::Copy);
    std::fs::remove_file(&source).unwrap();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    std::fs::write(source, b"%PDF-with new annotations").unwrap();
    assert!(apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {}
    )
    .is_err());
    assert_eq!(f.count("import_runs"), 1);
}

#[test]
fn recovery_never_deletes_a_preexisting_file_with_a_colliding_name() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new(json!([{"_id":"one","title":"Paper"}]));
    let input = f.input();
    let first = f.run(&input, AttachmentMode::Copy);
    let storage =
        f.db.parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("storage");
    let run = first.run_id;
    let final_path = storage.join("COLLIDE2/preexisting.pdf");
    std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    std::fs::write(&final_path, b"%PDF-preserve preexisting bytes").unwrap();
    let part = storage
        .join(".imports")
        .join(&run)
        .join("COLLIDE2/file.part");
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::write(&part, b"%PDF-unfinished copy").unwrap();
    let meta = std::fs::metadata(&part).unwrap();
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("INSERT INTO import_files(run_id,key,path,staging_path,sha256,state,owns_staging,file_device,file_inode) VALUES(?1,'COLLIDE2',?2,?3,'expected','planned',1,?4,?5)",rusqlite::params![run,final_path.to_string_lossy(),part.to_string_lossy(),meta.dev().to_string(),meta.ino().to_string()]).unwrap();
    drop(db);
    let report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(
        std::fs::read(final_path).unwrap(),
        b"%PDF-preserve preexisting bytes"
    );
    assert!(!part.exists());
    assert!(report
        .warnings
        .iter()
        .any(|w| w.contains("作成したファイルではない")));
}

#[test]
fn automatic_ingestion_serializes_duplicate_checks_with_its_insert() {
    use std::sync::{Arc, Barrier};
    let f = Fixture::new(json!([]));
    drop(Db::open(&f.db).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let threads:Vec<_>=(0..2).map(|_|{let path=f.db.clone();let barrier=barrier.clone();std::thread::spawn(move||{let mut db=Db::open(&path).unwrap();barrier.wait();openref_core::ingest::add_item(&mut db,&openref_core::resolver::Resolved{csl:json!({"type":"article-journal","title":"Concurrent automatic import","DOI":"10.5555/concurrent"}),source:"test",retracted:None})})}).collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(openref_core::CoreError::Duplicate { .. })))
            .count(),
        1
    );
    assert_eq!(f.count("items"), 1);
}

#[test]
fn tag_renames_are_preserved_and_deleted_tag_ids_are_not_rebound() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Paper","labels":["source-tag"],"labelsNamed":["Original label"]}]),
    );
    let input = f.input();
    f.run(&input, AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute("UPDATE tags SET name='User rename'", [])
        .unwrap();
    drop(db);
    f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("tags"), 1);
    let db = Db::open(&f.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT name FROM tags", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "User rename"
    );
    db.conn
        .execute_batch("DELETE FROM tags; INSERT INTO tags(name) VALUES('Different tag');")
        .unwrap();
    drop(db);
    let report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("tags"), 1);
    assert_eq!(f.count("item_tags"), 0);
    assert!(report.results[0]
        .warnings
        .iter()
        .any(|w| w.contains("削除されたタグ")));
}

#[test]
#[ignore = "10,000-record / 1,000-PDF scale measurement in an isolated scratch DB"]
fn scale_10000_records_1000_pdfs() {
    let records:Vec<_>=(0..10000).map(|n|{let mut rec=json!({"_id":format!("scale-{n}"),"title":format!("Scale article {n}"),"doi":format!("10.5555/scale-{n}"),"citekey":format!("Scale{n}"),"created":1750000000.0,"author":[{"last":format!("Author{n}")}],"published":{"year":"2026"}});if n<1000{rec["attachments"]=json!([{"_id":format!("pdf-{n}"),"filename":format!("pdfs/{n}.pdf")}]);}rec}).collect();
    let f = Fixture::new(json!(records));
    let pdf = openref_core::pdftools::minimal_pdf("Generated scale fixture", None);
    for n in 0..1000 {
        f.file(&format!("pdfs/{n}.pdf"), &pdf);
    }
    let start = std::time::Instant::now();
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let preview_seconds = start.elapsed().as_secs_f64();
    let applied = std::time::Instant::now();
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |p| {
            if p.phase == "committed" && p.completed % 2000 == 0 {
                println!("scale progress: {}", p.completed);
            }
        },
    )
    .unwrap();
    assert_eq!(report.results.len(), 10000);
    assert!(report.results.iter().all(|r| r.status == "inserted"));
    assert_eq!(f.count("items"), 10000);
    assert_eq!(f.count("attachments"), 1000);
    println!("scale: preview={preview_seconds:.3}s apply={:.3}s total={:.3}s, 10000 records / 1000 PDFs / {} PDF bytes",applied.elapsed().as_secs_f64(),start.elapsed().as_secs_f64(),pdf.len()*1000);
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: rusage points to writable storage and is read only on success.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0 {
            let usage = unsafe { usage.assume_init() };
            println!(
                "scale maxrss={} ({})",
                usage.ru_maxrss,
                if cfg!(target_os = "macos") {
                    "bytes"
                } else {
                    "KiB"
                }
            );
        }
    }
}

#[test]
fn legacy_paperpile_ids_are_adopted_only_when_confirmed_and_rechecked() {
    let f = Fixture::new(json!([{"_id":"old-paperpile-id","title":"Source metadata"}]));
    let mut input = f.input();
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("INSERT INTO items(key,csl) VALUES('legacy',?1)",[json!({"type":"article-journal","title":"User edited title","custom":{"paperpile":{"id":"old-paperpile-id"}}}).to_string()]).unwrap();
    drop(db);
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.new_items, 1);
    input.adopt_legacy_paperpile_ids = true;
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.existing_items, 1);
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.results[0].item_key.as_deref(), Some("legacy"));
    assert_eq!(f.count("items"), 1);
    let db = Db::open(&f.db).unwrap();
    let title: String = db
        .conn
        .query_row("SELECT title FROM items WHERE key='legacy'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(title, "User edited title");
    drop(db);
    // A second row with the same unscoped legacy ID appears after preview.
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("INSERT INTO items(key,csl) VALUES('collision',?1)",[json!({"type":"document","title":"Other library","custom":{"paperpile":{"id":"old-paperpile-id"}}}).to_string()]).unwrap();
    drop(db);
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.results[0].status, "blocked");
}

#[test]
fn fulltext_is_independent_cancellable_and_reports_failures_without_losing_items() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Indexed fixture","attachments":[{"_id":"a","filename":"a.pdf"},{"_id":"b","filename":"bad.pdf"}]}]),
    );
    f.file(
        "a.pdf",
        &openref_core::pdftools::minimal_pdf("Searchable migration fixture", None),
    );
    f.file("bad.pdf", b"%PDF-invalid header-only fixture");
    let mut report = f.run(&f.input(), AttachmentMode::Copy);
    assert_eq!(report.fulltext_pending, 2);
    apply::index_report(&f.db, &mut report, &AtomicBool::new(true), |_| {}).unwrap();
    assert_eq!(report.fulltext_pending, 2);
    apply::index_report(&f.db, &mut report, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.fulltext_pending, 1);
    assert_eq!(f.count("items"), 1);
    assert_eq!(f.count("attachments"), 2);
    assert_eq!(report.state, "finished");
    assert_eq!(history_pending(&f.db, &report.run_id), 1);
    assert!(report.warnings.iter().any(|w| w.contains("全文索引")));
}

/// execute 末・index_report・history の索引待ちが同じ定義（添付が無い = 待ちなし）で数える。
/// 利用者が消した添付（再実行で blocked・target_key は消えた key）が index_report 後も待ちに残ると、
/// 全文索引ボタンが消えない（履歴から開き直すと 0 に戻る）。
#[test]
fn fulltext_pending_agrees_across_execute_index_and_history_for_deleted_attachments() {
    let f = Fixture::new(json!([
        {"_id":"good","title":"Indexed","attachments":[{"_id":"g","filename":"good.pdf"}]},
        {"_id":"bad","title":"Unindexable","attachments":[{"_id":"b","filename":"bad.pdf"}]},
        {"_id":"gone","title":"Attachment removed by user","attachments":[{"_id":"r","filename":"gone.pdf"}]},
        {"_id":"bare","title":"No attachment"}
    ]));
    f.file(
        "good.pdf",
        &openref_core::pdftools::minimal_pdf("Searchable pending fixture", None),
    );
    f.file("bad.pdf", b"%PDF-invalid header-only fixture");
    f.file(
        "gone.pdf",
        &openref_core::pdftools::minimal_pdf("Removed pending fixture", None),
    );
    let input = f.input();
    let first = f.run(&input, AttachmentMode::Copy);
    assert_eq!(first.fulltext_pending, 3);
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute("DELETE FROM attachments WHERE path LIKE '%gone.pdf'", [])
        .unwrap();
    drop(db);
    assert_eq!(f.count("attachments"), 2);

    let mut report = f.run(&input, AttachmentMode::Copy);
    let gone = &report.results[2];
    assert_eq!(gone.attachments[0].status, "blocked");
    assert!(gone.attachments[0].target_key.is_some());
    assert!(report.results[3].attachments.is_empty());
    // 未索引 = good・bad（消えた添付と添付の無い文献は数えない）
    assert_eq!(report.fulltext_pending, 2);
    assert_eq!(history_pending(&f.db, &report.run_id), 2);

    apply::index_report(&f.db, &mut report, &AtomicBool::new(false), |_| {}).unwrap();
    // good は索引済み・bad は失敗で待ちに残る
    assert_eq!(report.fulltext_pending, 1);
    assert_eq!(history_pending(&f.db, &report.run_id), 1);
    assert_eq!(f.count("attachments"), 2);
}

#[test]
#[ignore = "requires explicitly provided real Paperpile JSON/PDF sample; scratch DB only"]
fn real_paperpile_copy_roundtrip_in_scratch_database() {
    let json =
        PathBuf::from(std::env::var_os("OPENREF_TEST_PAPERPILE_JSON").expect("set JSON path"));
    let files =
        PathBuf::from(std::env::var_os("OPENREF_TEST_PAPERPILE_FILES").expect("set files root"));
    let f = Fixture::new(json!([]));
    let input = import::paperpile::read_input(
        &json,
        ImportSource {
            id: "real-sample".into(),
            format: "paperpile".into(),
        },
    )
    .unwrap();
    let roots = AttachmentRoots::new(&[files]).unwrap();
    let before = FileFingerprint::read(&json).unwrap();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let report = apply::execute(
        &f.db,
        &json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.results.len(), 3);
    assert_eq!(f.count("items"), 3);
    assert_eq!(f.count("attachments"), 2);
    assert_eq!(report.fulltext_pending, 2);
    before.verify(&json).unwrap();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let report = apply::execute(
        &f.db,
        &json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert!(report.results.iter().all(|r| r.status == "merged"));
    assert_eq!(f.count("attachments"), 2);
    println!("Real sample copied into scratch DB: 3 records / 2 PDFs; reexecution did not duplicate records or PDFs");
}

#[test]
fn review_reexport_preserves_deleted_objects_and_holds_purged_items() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Original","doi":"10.1234/reexport","foldersNamed":["Folder"],"labels":["tag"],"labelsNamed":["Tag"]}]),
    );
    let add_note = |mut input: import::ImportInput| {
        input.items[0].notes.push(ImportNote {
            id: "note".into(),
            stable_id: false,
            content_md: "Original note".into(),
            raw: json!({}),
            date_added: None,
            date_modified: None,
        });
        input
    };
    let input = add_note(f.input());
    f.run(&input, AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("DELETE FROM notes", []).unwrap();
    db.conn.execute("DELETE FROM tags", []).unwrap();
    db.conn
        .execute("DELETE FROM collections WHERE name='Folder'", [])
        .unwrap();
    drop(db);
    let bytes = std::fs::read_to_string(&f.json).unwrap();
    std::fs::write(&f.json, format!("{bytes}\n")).unwrap();
    let changed = add_note(f.input());
    let plan = apply::prepare(&f.db, &changed, &f.roots(), AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.review, 1);
    let report = f.run(&changed, AttachmentMode::Copy);
    assert_eq!(report.results[0].status, "review");
    assert_eq!(f.count("notes"), 0);
    assert_eq!(f.count("tags"), 0);
    assert_eq!(f.count("collections"), 0);
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("DELETE FROM items", []).unwrap();
    drop(db);
    assert_eq!(
        f.run(&changed, AttachmentMode::Copy).results[0].status,
        "blocked"
    );
    assert_eq!(f.count("items"), 0);
}

#[test]
fn review_reexport_keeps_edits_and_accepts_only_unrelated_new_records() {
    let f = Fixture::new(json!([{"_id":"one","title":"Original","doi":"10.1234/old"}]));
    let input = f.input();
    f.run(&input, AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute("UPDATE items SET csl=json_set(csl,'$.title','Edited')", [])
        .unwrap();
    drop(db);
    std::fs::write(&f.json,json!([{"_id":"changed-id","title":"Original","doi":"10.1234/old"},{"_id":"new","title":"New","doi":"10.1234/new"}]).to_string()).unwrap();
    let changed = f.input();
    let report = f.run(&changed, AttachmentMode::Copy);
    assert_eq!(report.results[0].status, "review");
    assert_eq!(report.results[1].status, "inserted");
    assert_eq!(f.count("items"), 2);
    let repeated = f.run(&changed, AttachmentMode::Copy);
    assert_eq!(repeated.results[0].status, "review");
    assert_eq!(repeated.results[1].status, "merged");
}

#[test]
fn review_removed_memberships_are_not_restored_and_missing_pdf_still_is() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Membership","foldersNamed":["Folder"],"labels":["tag"],"labelsNamed":["Tag"],"attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    let input = f.input();
    f.run(&input, AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("DELETE FROM collection_items", []).unwrap();
    db.conn.execute("DELETE FROM item_tags", []).unwrap();
    drop(db);
    f.file("a.pdf", b"%PDF-added later");
    let mut report = f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("collection_items"), 0);
    assert_eq!(f.count("item_tags"), 0);
    assert_eq!(f.count("attachments"), 1);
    // 整合の確認: index_report 後と history の索引待ちが一致する。添付を消していないので
    // 旧新の定義の差は出ない（差の検出は fulltext_pending_agrees_… が担う）
    apply::index_report(&f.db, &mut report, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(
        history_pending(&f.db, &report.run_id),
        report.fulltext_pending
    );
    assert!(report.results[0]
        .warnings
        .iter()
        .any(|w| w.contains("解除済み")));
}

#[test]
fn review_reused_collection_id_is_not_a_valid_membership_target() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Race","foldersNamed":["Folder"],"attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    f.file("a.pdf", b"%PDF-race");
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let mut replaced = false;
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |p| {
            if p.phase == "copy" && !replaced {
                replaced = true;
                let db = Db::open(&f.db).unwrap();
                let id: i64 = db
                    .conn
                    .query_row("SELECT id FROM collections WHERE name='Folder'", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                openref_core::collections::delete_collection(&db, id).unwrap();
                db.conn
                    .execute(
                        "INSERT INTO collections(key,name) VALUES('UNRELATED','Unrelated')",
                        [],
                    )
                    .unwrap();
                assert_eq!(db.conn.last_insert_rowid(), id);
            }
        },
    )
    .unwrap();
    let db = Db::open(&f.db).unwrap();
    let wrong:i64=db.conn.query_row("SELECT count(*) FROM collection_items ci JOIN collections c ON c.id=ci.collection_id WHERE c.key='UNRELATED'",[],|r|r.get(0)).unwrap();
    assert_eq!(wrong, 0);
    assert!(report.results[0]
        .warnings
        .iter()
        .any(|w| w.contains("削除された所属先")));
}

#[test]
fn review_citation_key_is_preserved_across_formats_in_both_directions() {
    for existing_pp in [false, true] {
        let f = Fixture::new(
            json!([{"_id":"one","title":"Citekey","doi":"10.1234/key","citekey":"Incoming"}]),
        );
        let db = Db::open(&f.db).unwrap();
        let stored = if existing_pp {
            json!({"title":"Existing","DOI":"10.1234/key","custom":{"paperpile":{"citekey":"Keep"}}})
        } else {
            json!({"title":"Existing","DOI":"10.1234/key","citation-key":"Keep"})
        };
        db.conn
            .execute(
                "INSERT INTO items(key,csl) VALUES('EXISTING',?1)",
                [stored.to_string()],
            )
            .unwrap();
        drop(db);
        let mut input = f.input();
        if existing_pp {
            input.items[0].csl.as_object_mut().unwrap().remove("custom");
            input.items[0].csl["citation-key"] = json!("Incoming");
        }
        let report = f.run(&input, AttachmentMode::Copy);
        let db = Db::open(&f.db).unwrap();
        let csl: String = db
            .conn
            .query_row("SELECT csl FROM items WHERE key='EXISTING'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            openref_core::bibtex::citekey(&serde_json::from_str(&csl).unwrap(), "EXISTING"),
            "Keep"
        );
        assert!(report.results[0]
            .warnings
            .iter()
            .any(|w| w.contains("既存の引用キーを保持")));
    }
}

/// 形式の固定の注意は notices に分かれ、complete を妨げない。注意の無い実行は complete = true
/// （PDF があれば全文索引の後に true）。事前確認専用の文言は最終報告に出ない
#[test]
fn clean_run_is_complete_and_fixed_caveats_are_notices() {
    let f = Fixture::new(json!([
        {"_id":"one","title":"Clean","doi":"10.1234/clean","created":1750000000.0},
        {"_id":"two","title":"With PDF","doi":"10.1234/pdf","created":1750000000.0,
         "attachments":[{"_id":"pdf","filename":"pdfs/two.pdf"}]}
    ]));
    f.file(
        "pdfs/two.pdf",
        &openref_core::pdftools::minimal_pdf("Complete fixture text", None),
    );
    let input = f.input();
    assert!(input.warnings.is_empty());
    assert!(!input.notices.is_empty());
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    assert_eq!(plan.notices, input.notices);
    let mut report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.notices, input.notices);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.results.iter().all(|r| r.warnings.is_empty()));
    assert!(!report
        .notices
        .iter()
        .chain(&report.warnings)
        .any(|w| w.contains("事前確認")));
    // 索引待ちの PDF が 1 本 → まだ完了ではない
    assert_eq!(report.fulltext_pending, 1);
    assert!(!report.complete);
    apply::index_report(&f.db, &mut report, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.fulltext_pending, 0);
    assert!(report.complete, "{:?}", report.warnings);
    // PDF の無い実行は終わった時点で完了
    let g = Fixture::new(json!([{"_id":"one","title":"Clean","doi":"10.1234/clean","created":1750000000.0}]));
    let report = g.run(&g.input(), AttachmentMode::Copy);
    assert!(report.complete);
}

/// 実 Paperpile 書出しの構造（tests/fixtures/paperpile-2026-10-27）で、元レコードに保持しただけの情報
/// （未反映のフィールド・複数 URL）は文献ごとの warnings にならず notices の件数になる → 索引の後 complete = true
#[test]
fn actual_paperpile_export_completes() {
    let records: Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/paperpile-2026-10-27/library.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let f = Fixture::new(records);
    for n in 0..2 {
        f.file(
            &format!("All Papers/F/Fixture{n} - Article.pdf"),
            &openref_core::pdftools::minimal_pdf(&format!("Fixture article {n} text"), None),
        );
    }
    let input = f.input();
    assert!(input.items.iter().all(|i| i.warnings.is_empty()), "{:?}", input.items.iter().map(|i| &i.warnings).collect::<Vec<_>>());
    assert!(input.notices.iter().any(|w| w.starts_with("3 件で UI/引用に未反映のフィールドを元レコードに保持しました（")), "{:?}", input.notices);
    assert!(input.notices.iter().any(|w| w.starts_with("3 件は URL が複数ある")), "{:?}", input.notices);
    let mut report = f.run(&input, AttachmentMode::Copy);
    assert!(report.results.iter().all(|r| r.status == "inserted" && r.warnings.is_empty()), "{:?}", report.results);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.fulltext_pending, 2);
    assert!(!report.complete);
    apply::index_report(&f.db, &mut report, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.fulltext_pending, 0, "{:?}", report.warnings);
    assert!(report.complete, "{:?}", report.warnings);
}

/// 引用キーの衝突は既存の文献と、同じ run で先に書いた文献の両方と照合する。
/// 照合は文字列だけ（数値の citation-key 42 は "42" と衝突しない）・自分自身とは衝突しない
#[test]
fn citekey_collision_checks_existing_and_earlier_items_in_the_run() {
    let f = Fixture::new(json!([
        {"_id":"a","title":"A","doi":"10.1234/a","citekey":"Shared"},
        {"_id":"b","title":"B","doi":"10.1234/b","citekey":"Fresh"},
        {"_id":"c","title":"C","doi":"10.1234/c","citekey":"Fresh"},
        {"_id":"d","title":"D","doi":"10.1234/d","citekey":"42"},
        {"_id":"e","title":"E","doi":"10.1234/e","citekey":"Own"}
    ]));
    let db = Db::open(&f.db).unwrap();
    for (key, csl) in [
        ("OTHER", json!({"title":"Other","citation-key":"Shared"})),
        ("NUMBER", json!({"title":"Number","citation-key":42})),
        ("OWN", json!({"title":"E","DOI":"10.1234/e","custom":{"paperpile":{"citekey":"Own"}}})),
    ] {
        db.conn
            .execute("INSERT INTO items(key,csl) VALUES(?1,?2)", [key, &csl.to_string()])
            .unwrap();
    }
    drop(db);
    let report = f.run(&f.input(), AttachmentMode::Copy);
    let collided: Vec<_> = report
        .results
        .iter()
        .map(|r| r.warnings.iter().any(|w| w.contains("引用キーが衝突しています")))
        .collect();
    assert_eq!(collided, [true, false, true, false, false]);
    assert_eq!(report.results[4].status, "merged");
    // 再実行: 全部が自分の書いた文献へ merged。a・b/c は互いに / 既存 OTHER と衝突し続ける
    let again = f.run(&f.input(), AttachmentMode::Copy);
    let collided: Vec<_> = again
        .results
        .iter()
        .map(|r| r.warnings.iter().any(|w| w.contains("引用キーが衝突しています")))
        .collect();
    assert_eq!(collided, [true, true, true, false, false]);
}

#[cfg(unix)]
#[test]
fn review_replaced_staging_and_unknown_ownership_are_preserved() {
    use std::os::unix::fs::MetadataExt;
    for known in [true, false] {
        let f = Fixture::new(json!([{"_id":"one","title":"Staging"}]));
        let input = f.input();
        let report = f.run(&input, AttachmentMode::Copy);
        let storage =
            f.db.parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join("storage");
        let part = storage
            .join(".imports")
            .join(&report.run_id)
            .join("REVIEW1/file.part");
        std::fs::create_dir_all(part.parent().unwrap()).unwrap();
        std::fs::write(&part, b"owned file").unwrap();
        let meta = std::fs::metadata(&part).unwrap();
        std::fs::rename(&part, part.with_file_name("owned-old.part")).unwrap();
        std::fs::write(&part, b"foreign file").unwrap();
        let db = Db::open(&f.db).unwrap();
        db.conn.execute("INSERT INTO import_files(run_id,key,path,staging_path,sha256,state,owns_staging,file_device,file_inode) VALUES(?1,'REVIEW1',?2,?3,'unused','planned',1,?4,?5)",rusqlite::params![report.run_id,storage.join("REVIEW1/file.pdf").to_string_lossy(),part.to_string_lossy(),known.then(||meta.dev().to_string()),known.then(||meta.ino().to_string())]).unwrap();
        drop(db);
        let report = f.run(&input, AttachmentMode::Copy);
        assert_eq!(std::fs::read(part).unwrap(), b"foreign file");
        assert!(report.warnings.iter().any(|w| w.contains("保持")));
    }
}

#[cfg(unix)]
#[test]
fn review_storage_replacement_during_copy_is_not_committed() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Storage","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    f.file("a.pdf", b"%PDF-storage");
    let input = f.input();
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let storage = plan.managed_storage.clone().unwrap();
    let moved = f.dir.canonicalize().unwrap().join("unapproved");
    let mut changed = false;
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |p| {
            if p.phase == "copy" && !changed {
                changed = true;
                std::fs::rename(&storage, &moved).unwrap();
                std::os::unix::fs::symlink(&moved, &storage).unwrap();
            }
        },
    )
    .unwrap();
    assert_eq!(f.count("attachments"), 0);
    assert_eq!(f.count("items"), 1);
    assert_eq!(report.results[0].attachments[0].status, "failed");
    assert_eq!(
        std::fs::read_dir(&moved).unwrap().count(),
        1,
        "only staging exists; no published copy"
    );
    // Restore the original directory. Recovery cleans its own unfinished file.
    std::fs::remove_file(&storage).unwrap();
    std::fs::rename(&moved, &storage).unwrap();
    assert_eq!(
        f.run(&input, AttachmentMode::Copy).results[0].attachments[0].status,
        "copied"
    );
}

#[test]
fn review_changed_ids_without_identifiers_cannot_recreate_deleted_records() {
    let f = Fixture::new(json!([{"_id":"old","title":"No identifiers"}]));
    f.run(&f.input(), AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("DELETE FROM items", []).unwrap();
    drop(db);
    std::fs::write(
        &f.json,
        json!([{"_id":"new","title":"No identifiers"}]).to_string(),
    )
    .unwrap();
    assert_eq!(
        f.run(&f.input(), AttachmentMode::Copy).results[0].status,
        "blocked"
    );
    assert_eq!(f.count("items"), 0);
}

#[test]
fn review_new_record_cannot_restore_deleted_tag_from_another_export() {
    let f =
        Fixture::new(json!([{"_id":"old","title":"Old","labels":["tag"],"labelsNamed":["Tag"]}]));
    f.run(&f.input(), AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("DELETE FROM tags", []).unwrap();
    drop(db);
    std::fs::write(
        &f.json,
        json!([{"_id":"new","title":"New","labels":["tag"],"labelsNamed":["Tag"]}]).to_string(),
    )
    .unwrap();
    let report = f.run(&f.input(), AttachmentMode::Copy);
    assert_eq!(report.results[0].status, "inserted");
    assert_eq!(f.count("tags"), 0);
    assert!(!report.results[0].warnings.is_empty());
}

#[test]
fn review_legacy_import_without_membership_markers_preserves_removed_edges() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Legacy","foldersNamed":["Folder"],"labels":["tag"],"labelsNamed":["Tag"]}]),
    );
    let input = f.input();
    f.run(&input, AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute("DELETE FROM import_objects WHERE kind='membership'", [])
        .unwrap();
    db.conn.execute("DELETE FROM collection_items", []).unwrap();
    db.conn.execute("DELETE FROM item_tags", []).unwrap();
    drop(db);
    f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("collection_items"), 0);
    assert_eq!(f.count("item_tags"), 0);
}

fn choose_item(
    f: &Fixture,
    input: &import::ImportInput,
    object: &str,
    key: &str,
) -> import::decisions::Decisions {
    import::decisions::Decisions {
        source_id: input.source.id.clone(),
        input_sha256: input.input_sha256.clone(),
        items: vec![import::decisions::ItemChoice {
            object_id: object.into(),
            item_key: key.into(),
            csl_sha256: if key.is_empty() {
                None
            } else {
                Db::open(&f.db)
                    .unwrap()
                    .conn
                    .query_row("SELECT csl FROM items WHERE key=?1", [key], |r| {
                        r.get::<_, String>(0)
                    })
                    .ok()
                    .map(|raw| {
                        use sha2::{Digest, Sha256};
                        format!(
                            "{:x}",
                            Sha256::digest(
                                serde_json::from_str::<Value>(&raw)
                                    .unwrap()
                                    .to_string()
                                    .as_bytes()
                            )
                        )
                    })
            },
        }],
        files: vec![],
    }
}

#[test]
fn choices_allow_cross_export_pdf_complement_without_restoring_metadata() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Original","doi":"10.1234/choice","labels":["tag"],"labelsNamed":["Tag"],"attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    let initial = f.run(&f.input(), AttachmentMode::Copy);
    let key = initial.results[0].item_key.as_ref().unwrap();
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("DELETE FROM item_tags", []).unwrap();
    drop(db);
    let raw = std::fs::read_to_string(&f.json).unwrap();
    std::fs::write(&f.json, format!("{raw}\n")).unwrap();
    f.file("a.pdf", b"%PDF-choice");
    let mut input = f.input();
    input.items[0].notes.push(ImportNote {
        id: "note".into(),
        stable_id: false,
        content_md: "Unresolved note".into(),
        raw: json!({}),
        date_added: None,
        date_modified: None,
    });
    let roots = f.roots();
    let mut plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.review, 1);
    assert_eq!(plan.items[0].candidate_details[0].key, *key);
    let choices = choose_item(&f, &input, &input.items[0].id, key);
    import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &choices,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(plan.counts.review, 0);
    assert_eq!(plan.counts.existing_items, 1);
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.results[0].status, "merged");
    assert_eq!(f.count("attachments"), 1);
    assert_eq!(f.count("items"), 1);
    assert_eq!(f.count("item_tags"), 0);
    assert_eq!(f.count("notes"), 0);
    let repeated = f.run(&input, AttachmentMode::Copy);
    assert_eq!(repeated.results[0].status, "merged");
    assert_eq!(f.count("notes"), 0);
    assert_eq!(f.count("item_tags"), 0);
    assert!(apply::history_list_snapshot(&f.db)
        .unwrap()
        .iter()
        .any(
            |h| apply::run_report_snapshot(&f.db, h["id"].as_str().unwrap())
                .unwrap()
                .unwrap()
                .to_string()
                .contains("利用者が確認した対応")
        ));
}

#[test]
fn choices_cannot_override_deleted_target_conflicts_or_wrong_input() {
    let f = Fixture::new(json!([{"_id":"one","title":"Protected","doi":"10.1234/protect"}]));
    let first = f.run(&f.input(), AttachmentMode::Copy);
    let key = first.results[0].item_key.as_ref().unwrap();
    let raw = std::fs::read_to_string(&f.json).unwrap();
    std::fs::write(&f.json, format!("{raw}\n")).unwrap();
    let input = f.input();
    let roots = f.roots();
    let choices = choose_item(&f, &input, &input.items[0].id, key);
    let mut plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let mut wrong = choices.clone();
    wrong.input_sha256 = "other input".into();
    assert!(import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &wrong,
        &AtomicBool::new(false)
    )
    .is_err());
    wrong = choices.clone();
    wrong.source_id = "another library".into();
    assert!(import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &wrong,
        &AtomicBool::new(false)
    )
    .is_err());
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "UPDATE items SET csl=json_set(csl,'$.DOI','10.1234/conflict')",
            [],
        )
        .unwrap();
    drop(db);
    let mut conflict = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert!(import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut conflict,
        &choices,
        &AtomicBool::new(false)
    )
    .is_err());
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("DELETE FROM items", []).unwrap();
    drop(db);
    let mut deleted = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(deleted.counts.blocked, 1);
    assert!(import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut deleted,
        &choices,
        &AtomicBool::new(false)
    )
    .is_err());
}

#[test]
fn choices_are_rechecked_inside_commit_after_concurrent_edit_delete_or_identifier_collision() {
    for change in ["edit", "delete", "collision"] {
        let f = Fixture::new(
            json!([{"_id":"one","title":"Race","doi":"10.1234/race","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
        );
        let first = f.run(&f.input(), AttachmentMode::Copy);
        let key = first.results[0].item_key.as_ref().unwrap();
        let raw = std::fs::read_to_string(&f.json).unwrap();
        std::fs::write(&f.json, format!("{raw}\n")).unwrap();
        f.file("a.pdf", b"%PDF-race-choice");
        let input = f.input();
        let roots = f.roots();
        let mut plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
        import::decisions::resolve(
            &f.db,
            &input,
            &roots,
            &mut plan,
            &choose_item(&f, &input, &input.items[0].id, key),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut changed = false;
        let report=apply::execute(&f.db,&f.json,&input,&plan,&roots,&AtomicBool::new(false),|p|{
            if p.phase=="copy" && !changed {changed=true;let db=Db::open(&f.db).unwrap();match change {
                "delete"=>{db.conn.execute("DELETE FROM items",[]).unwrap();},
                "edit"=>{db.conn.execute("UPDATE items SET csl=json_set(csl,'$.title','Concurrent edit')",[]).unwrap();},
                _=>{db.conn.execute("INSERT INTO items(key,csl) VALUES('COLLIDE',?1)",[json!({"title":"Another","DOI":"10.1234/race"}).to_string()]).unwrap();}
            }}
        }).unwrap();
        assert!(changed);
        assert_eq!(report.results[0].status, "blocked", "{change}");
        assert_eq!(f.count("attachments"), 0);
    }
}

#[test]
fn file_choices_are_readonly_bound_to_candidates_and_revalidated() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Ambiguous","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    let a = f.file("a/a.pdf", b"%PDF-A").canonicalize().unwrap();
    let b = f.file("b/a.pdf", b"%PDF-B").canonicalize().unwrap();
    let roots =
        AttachmentRoots::new(&[a.parent().unwrap().into(), b.parent().unwrap().into()]).unwrap();
    let input = f.input();
    let mut plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.ambiguous_pdfs, 1);
    let mut choices = choose_item(&f, &input, "", "");
    choices.items.clear();
    choices.files.push(import::decisions::FileChoice {
        object_id: input.items[0].id.clone(),
        attachment_id: input.items[0].attachments[0].id.clone(),
        path: a.clone(),
    });
    let mut invalid = choices.clone();
    invalid.files[0].path = f.file("outside.pdf", b"%PDF-outside");
    assert!(import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &invalid,
        &AtomicBool::new(false)
    )
    .is_err());
    import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &choices,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!f.db.parent().unwrap().exists());
    assert_eq!(plan.counts.ambiguous_pdfs, 0);
    assert_eq!(plan.counts.available_pdfs, 1);
    assert_eq!(plan.counts.copy_bytes_upper_bound, 6);
    std::fs::write(&a, b"%PDF-modified").unwrap();
    assert!(apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {}
    )
    .is_err());
    assert!(!f.db.exists());
    std::fs::write(&a, b"%PDF-A").unwrap();
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.results[0].attachments[0].status, "copied");
    let db = Db::open(&f.db).unwrap();
    let dest: String = db
        .conn
        .query_row("SELECT path FROM attachments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(std::fs::read(dest).unwrap(), b"%PDF-A");
    assert_eq!(std::fs::read(b).unwrap(), b"%PDF-B");
}

#[test]
fn weak_match_choice_adds_new_source_metadata_once_and_rejects_stale_preview() {
    let f = Fixture::new(
        json!([{"_id":"one","title":"Weak match","labels":["tag"],"labelsNamed":["Tag"]}]),
    );
    let mut input = f.input();
    input.items[0].csl =
        json!({"title":"Weak match","author":[{"family":"Doe"}],"issued":{"date-parts":[[2024]]}});
    input.items[0].notes.push(ImportNote {
        id: "note".into(),
        stable_id: false,
        content_md: "Imported note".into(),
        raw: json!({}),
        date_added: None,
        date_modified: None,
    });
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "INSERT INTO items(key,csl) VALUES('WEAK',?1)",
            [input.items[0].csl.to_string()],
        )
        .unwrap();
    drop(db);
    let roots = f.roots();
    let mut plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.review, 1);
    let choices = choose_item(&f, &input, &input.items[0].id, "WEAK");
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "UPDATE items SET csl=json_set(csl,'$.abstract','Edited during confirmation')",
            [],
        )
        .unwrap();
    drop(db);
    assert!(import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &choices,
        &AtomicBool::new(false)
    )
    .is_err());
    let choices = choose_item(&f, &input, &input.items[0].id, "WEAK");
    let mut plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &choices,
        &AtomicBool::new(false),
    )
    .unwrap();
    let report = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.results[0].status, "merged");
    assert_eq!(f.count("notes"), 1);
    assert_eq!(f.count("item_tags"), 1);
    f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("notes"), 1);
    assert_eq!(f.count("items"), 1);
}

#[test]
fn n2_deleted_and_cleared_bibliography_stays_deleted_including_legacy_history() {
    for legacy in [false, true] {
        let f = Fixture::new(
            json!([{"_id":"p","title":"Fields","doi":"10.1234/wrong","abstract":"Wrong","journal":"Wrong J"}]),
        );
        let input = f.input();
        f.run(&input, AttachmentMode::Copy);
        let db = Db::open(&f.db).unwrap();
        db.conn.execute("UPDATE items SET csl=json_remove(csl,'$.DOI','$.abstract','$.\"container-title\"')", []).unwrap();
        if legacy {
            db.conn
                .execute("DELETE FROM import_objects WHERE kind='bibliography'", [])
                .unwrap();
        }
        drop(db);
        f.run(&input, AttachmentMode::Copy);
        let db = Db::open(&f.db).unwrap();
        let raw: String = db
            .conn
            .query_row("SELECT csl FROM items", [], |r| r.get(0))
            .unwrap();
        let csl: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for key in ["DOI", "abstract", "container-title"] {
            assert!(csl.get(key).is_none(), "{csl}");
        }
        db.conn
            .execute("UPDATE items SET csl=json_set(csl,'$.abstract','')", [])
            .unwrap();
        drop(db);
        f.run(&input, AttachmentMode::Copy);
        let db = Db::open(&f.db).unwrap();
        assert_eq!(
            db.conn
                .query_row(
                    "SELECT json_extract(csl,'$.abstract') FROM items",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            ""
        );
    }
}

#[test]
fn n3_outside_root_link_is_promoted_without_losing_annotation_and_unknown_is_held() {
    let f = Fixture::new(
        json!([{"_id":"p","title":"Linked","attachments":[{"_id":"a","filename":"a.pdf"}]}]),
    );
    f.file("drive/a.pdf", b"%PDF-identical");
    f.file("export/a.pdf", b"%PDF-identical");
    let input = f.input();
    let run = |root: &str, mode| {
        let roots = AttachmentRoots::new(&[f.dir.join(root)]).unwrap();
        let plan = apply::prepare(&f.db, &input, &roots, mode).unwrap();
        apply::execute(
            &f.db,
            &f.json,
            &input,
            &plan,
            &roots,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap()
    };
    run("drive", AttachmentMode::Link);
    let db = Db::open(&f.db).unwrap();
    let id: i64 = db
        .conn
        .query_row("SELECT id FROM attachments", [], |r| r.get(0))
        .unwrap();
    db.conn.execute("INSERT INTO annotations(key,attachment_id,author,kind,comment) VALUES('N3',?1,'user','note','Keep')",[id]).unwrap();
    drop(db);
    // Unknown existing bytes must not create a second attachment.
    std::fs::write(f.dir.join("drive/a.pdf"), b"not PDF").unwrap();
    assert_eq!(
        run("export", AttachmentMode::Copy).results[0].attachments[0].status,
        "retry"
    );
    assert_eq!(f.count("attachments"), 1);
    std::fs::write(f.dir.join("drive/a.pdf"), b"%PDF-identical").unwrap();
    assert_eq!(
        run("export", AttachmentMode::Copy).results[0].attachments[0].status,
        "relocated"
    );
    assert_eq!(f.count("attachments"), 1);
    let db = Db::open(&f.db).unwrap();
    assert_eq!(
        db.conn
            .query_row(
                "SELECT attachment_id FROM annotations WHERE key='N3'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        id
    );
    assert_eq!(
        db.conn
            .query_row("SELECT mode FROM attachments", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "managed"
    );
}

#[test]
fn n4_reexport_new_item_joins_existing_renamed_folders_and_tags() {
    let first = json!({"_id":"a","title":"Alpha","doi":"10.1234/a","foldersNamed":["Neuro"],"labels":["l1"],"labelsNamed":["Read"]});
    let f = Fixture::new(json!([first.clone()]));
    f.run(&f.input(), AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "UPDATE collections SET name='Renamed' WHERE name='Neuro'",
            [],
        )
        .unwrap();
    drop(db);
    std::fs::write(&f.json,json!([first,{"_id":"n","title":"New","doi":"10.1234/n","foldersNamed":["Neuro","Chest"],"labels":["l1","l2"],"labelsNamed":["Read","Fresh"]}]).to_string()).unwrap();
    f.run(&f.input(), AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    let folders:i64=db.conn.query_row("SELECT count(*) FROM collection_items ci JOIN items i ON i.id=ci.item_id WHERE i.title='New'",[],|r|r.get(0)).unwrap();
    let tags: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM item_tags it JOIN items i ON i.id=it.item_id WHERE i.title='New'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((folders, tags), (2, 2));
    assert_eq!(f.count("collections"), 2);
}

#[test]
fn n5_second_record_in_run_adds_its_own_memberships() {
    let f = Fixture::new(json!([{"_id":"e","title":"Existing","doi":"10.1234/e"}]));
    let mut seed = f.input();
    seed.source.id = "seed".into();
    f.run(&seed, AttachmentMode::Copy);
    std::fs::write(&f.json,json!([
        {"_id":"x","title":"X","doi":"10.1234/e","foldersNamed":["FolderX"],"labelsNamed":["TagX"],"labels":["x"]},
        {"_id":"y","title":"Y","doi":"10.1234/e","foldersNamed":["FolderY"],"labelsNamed":["TagY"],"labels":["y"]}
    ]).to_string()).unwrap();
    let input = f.input();
    let r = f.run(&input, AttachmentMode::Copy);
    assert!(r.results.iter().all(|r| r.status == "merged"));
    assert_eq!(f.count("item_tags"), 2);
    assert_eq!(f.count("collection_items"), 2);
    f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("item_tags"), 2);
}

#[test]
fn n6_input_duplicate_can_explicitly_register_and_merge_without_duplicating() {
    let f = Fixture::new(
        json!([{"_id":"x","title":"X","doi":"10.1234/same"},{"_id":"y","title":"Y","doi":"10.1234/same"}]),
    );
    let input = f.input();
    let roots = f.roots();
    let mut plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    let choices = import::decisions::Decisions {
        source_id: input.source.id.clone(),
        input_sha256: input.input_sha256.clone(),
        items: input
            .items
            .iter()
            .map(|i| import::decisions::ItemChoice {
                object_id: i.id.clone(),
                item_key: "@new".into(),
                csl_sha256: None,
            })
            .collect(),
        files: vec![],
    };
    import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &choices,
        &AtomicBool::new(false),
    )
    .unwrap();
    let r = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(r.results[0].status, "inserted");
    assert_eq!(r.results[1].status, "merged");
    assert_eq!(f.count("items"), 1);
    let mut retry = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut retry,
        &choices,
        &AtomicBool::new(false),
    )
    .unwrap();
    apply::execute(
        &f.db,
        &f.json,
        &input,
        &retry,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("items"), 1);
}

#[test]
fn n7_literal_date_is_atomic_during_complement() {
    let f = Fixture::new(
        json!([{"_id":"x","title":"Seasonal","doi":"10.1234/date","published":{"year":"2021"}}]),
    );
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "INSERT INTO items(key,csl) VALUES('DATE',?1)",
            [
                json!({"title":"Seasonal","DOI":"10.1234/date","issued":{"literal":"Spring 2019"}})
                    .to_string(),
            ],
        )
        .unwrap();
    drop(db);
    f.run(&f.input(), AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    let raw: String = db
        .conn
        .query_row("SELECT csl FROM items", [], |r| r.get(0))
        .unwrap();
    let csl: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(csl["issued"], json!({"literal":"Spring 2019"}));
}

#[test]
fn n1_default_source_reuses_one_and_requires_choice_for_multiple() {
    let f = Fixture::new(json!([{"_id":"a","title":"Alpha"}]));
    assert_eq!(
        apply::default_source(&f.db, "paperpile").unwrap(),
        "paperpile-default"
    );
    f.run(&f.input(), AttachmentMode::Copy);
    assert_eq!(
        apply::default_source(&f.db, "paperpile").unwrap(),
        "test-library"
    );
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "INSERT INTO import_sources(id,format) VALUES('other','paperpile')",
            [],
        )
        .unwrap();
    drop(db);
    assert!(apply::default_source(&f.db, "paperpile").is_err());
}

#[test]
fn n6_weak_input_duplicates_can_be_registered_as_distinct_papers() {
    let f = Fixture::new(json!([
        {"_id":"x","title":"Shared title","author":[{"first":"A","last":"Author"}],"published":{"year":"2020"}},
        {"_id":"y","title":"Shared title","author":[{"first":"A","last":"Author"}],"published":{"year":"2020"}}
    ]));
    let input = f.input();
    let roots = f.roots();
    let mut plan = apply::prepare(&f.db, &input, &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.review, 2);
    let choices = import::decisions::Decisions {
        source_id: input.source.id.clone(),
        input_sha256: input.input_sha256.clone(),
        items: input
            .items
            .iter()
            .map(|i| import::decisions::ItemChoice {
                object_id: i.id.clone(),
                item_key: "@new".into(),
                csl_sha256: None,
            })
            .collect(),
        files: vec![],
    };
    import::decisions::resolve(
        &f.db,
        &input,
        &roots,
        &mut plan,
        &choices,
        &AtomicBool::new(false),
    )
    .unwrap();
    let r = apply::execute(
        &f.db,
        &f.json,
        &input,
        &plan,
        &roots,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert!(r.results.iter().all(|r| r.status == "inserted"), "{r:?}");
    assert_eq!(f.count("items"), 2);
    f.run(&input, AttachmentMode::Copy);
    assert_eq!(f.count("items"), 2);
}

#[cfg(unix)]
#[test]
fn r9_database_hardlink_is_rejected_before_pdf_open() {
    let f = Fixture::new(
        json!([{"_id":"x","title":"DB alias","attachments":[{"_id":"a","filename":"alias.pdf"}]}]),
    );
    let db = Db::open(&f.db).unwrap();
    std::fs::hard_link(&f.db, f.dir.join("alias.pdf")).unwrap();
    assert!(apply::reject_database_file(&f.dir.join("alias.pdf"), &f.db).is_err());
    let roots = f.roots();
    let plan = apply::prepare(&f.db, &f.input(), &roots, AttachmentMode::Copy).unwrap();
    assert_eq!(plan.counts.failed_pdfs, 1);
    assert!(format!("{:?}", plan.items[0].attachments).contains("DB・WAL・SHM"));
    db.conn
        .execute(
            "INSERT INTO settings(key,value) VALUES('still-locked','yes')",
            [],
        )
        .unwrap();
}

#[test]
fn review_n4_delimited_tag_identity_does_not_join_an_unrelated_tag() {
    let f = Fixture::new(json!([{"_id":"old","title":"Old","labels":["a:b"],"labelsNamed":["c"]}]));
    f.run(&f.input(), AttachmentMode::Copy);
    std::fs::write(
        &f.json,
        json!([{"_id":"new","title":"New","labels":["a"],"labelsNamed":["b:c"]}]).to_string(),
    )
    .unwrap();
    f.run(&f.input(), AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    let tags: Vec<String> = db.conn.prepare("SELECT t.name FROM item_tags it JOIN tags t ON t.id=it.tag_id JOIN items i ON i.id=it.item_id WHERE i.title='New'").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(tags, vec!["b:c"]);
}

#[cfg(unix)]
#[test]
fn recovery_rejects_database_alias_even_with_matching_ledger_identity() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new(json!([{"_id":"one","title":"Recovery alias"}]));
    let report = f.run(&f.input(), AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    let storage = f.db.parent().unwrap().canonicalize().unwrap().join("storage");
    let part = storage.join(".imports").join(&report.run_id).join("DBALIAS/file.part");
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::hard_link(&f.db, &part).unwrap();
    let meta = std::fs::metadata(&part).unwrap();
    db.conn.execute("INSERT INTO import_files(run_id,key,path,staging_path,sha256,state,owns_staging,file_device,file_inode) VALUES(?1,'DBALIAS',?2,?3,'unused','planned',1,?4,?5)",rusqlite::params![report.run_id,storage.join("DBALIAS/file.pdf").to_string_lossy(),part.to_string_lossy(),meta.dev().to_string(),meta.ino().to_string()]).unwrap();
    drop(db);
    let next = f.run(&f.input(), AttachmentMode::Copy);
    assert!(part.exists());
    assert!(next.warnings.iter().any(|w| w.contains("DB・WAL・SHM")), "{:?}", next.warnings);
}

#[test]
fn folders_go_to_top_level_and_join_same_named_existing_folder() {
    let f = Fixture::new(json!([
        {"_id":"a","title":"Alpha","doi":"10.1234/top-a","foldersNamed":["Neuro/CT","Chest"]},
        {"_id":"b","title":"Beta","doi":"10.1234/top-b"}
    ]));
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute("INSERT INTO collections(key,name) VALUES('MINE','neuro')", [])
        .unwrap();
    drop(db);
    let report = f.run(&f.input(), AttachmentMode::Copy);
    assert!(report.results.iter().all(|r| r.status == "inserted"));
    assert!(!report
        .warnings
        .iter()
        .chain(report.results.iter().flat_map(|r| r.warnings.iter()))
        .any(|w| w.contains("表示名を変更") || w.contains("所属先")));
    assert!(report
        .warnings
        .iter()
        .any(|w| w == "同名の既存フォルダへ追加: Neuro → neuro"));
    assert!(!report.warnings.iter().any(|w| w.contains("以前の移行で作った")));
    let db = Db::open(&f.db).unwrap();
    let tree: Vec<(String, Option<String>)> = db
        .conn
        .prepare("SELECT c.name,p.name FROM collections c LEFT JOIN collections p ON p.id=c.parent_id ORDER BY c.name COLLATE NOCASE")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        tree,
        vec![
            ("Chest".into(), None),
            ("CT".into(), Some("neuro".into())),
            ("neuro".into(), None)
        ]
    );
    let memberships: Vec<(String, String)> = db
        .conn
        .prepare("SELECT i.title,c.name FROM collection_items ci JOIN items i ON i.id=ci.item_id JOIN collections c ON c.id=ci.collection_id ORDER BY c.name COLLATE NOCASE")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        memberships,
        vec![("Alpha".into(), "Chest".into()), ("Alpha".into(), "CT".into())]
    );
    drop(db);
    f.run(&f.input(), AttachmentMode::Copy);
    assert_eq!(f.count("collections"), 3);
    assert_eq!(f.count("collection_items"), 2);
}

#[test]
fn same_named_folders_within_one_input_stay_separate() {
    let f = Fixture::new(json!([
        {"_id":"a","title":"Alpha","doi":"10.1234/dup-a","foldersNamed":["Neuro/CT"]},
        {"_id":"b","title":"Beta","doi":"10.1234/dup-b","foldersNamed":["neuro/MR"]}
    ]));
    let report = f.run(&f.input(), AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    let top: Vec<String> = db
        .conn
        .prepare("SELECT name FROM collections WHERE parent_id IS NULL ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(top.len(), 2, "{top:?}");
    assert!(top.iter().any(|n| n.contains(" (")), "{top:?}");
    assert!(report.warnings.iter().any(|w| w.contains("表示名を変更")));
    assert!(!report.warnings.iter().any(|w| w.contains("同名の既存フォルダへ追加")));
    assert_eq!(f.count("collections"), 4);
}

#[test]
fn source_root_left_by_earlier_version_is_kept_and_reported() {
    let first = json!({"_id":"a","title":"Alpha","doi":"10.1234/legacy-root","foldersNamed":["Neuro"]});
    let f = Fixture::new(json!([first.clone()]));
    f.run(&f.input(), AttachmentMode::Copy);
    // 1.1.0 の移行が残した状態: 移行元名の親フォルダ（その下に元フォルダ）と kind = source_root の対応
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute("INSERT INTO collections(key,name) VALUES('OLDROOT','Paperpile')", [])
        .unwrap();
    db.conn
        .execute("UPDATE collections SET parent_id=(SELECT id FROM collections WHERE key='OLDROOT') WHERE name='Neuro'", [])
        .unwrap();
    db.conn.execute("INSERT INTO import_objects(source_id,kind,scope,object_id,target_key,raw_json,content_sha256) VALUES('test-library','source_root','','root','OLDROOT','{\"generated_source_root\":true}','')",[]).unwrap();
    drop(db);
    let same = f.run(&f.input(), AttachmentMode::Copy);
    assert!(!same.warnings.iter().any(|w| w.contains("以前の移行で作った")));
    assert_eq!(f.count("collections"), 2);
    // 再書出しで最上位のフォルダが増えると知らせる。元フォルダ「Paperpile」は旧親フォルダへ合流させない
    std::fs::write(
        &f.json,
        json!([first,{"_id":"b","title":"Beta","doi":"10.1234/legacy-root-b","foldersNamed":["Chest","Paperpile"]}]).to_string(),
    )
    .unwrap();
    let report = f.run(&f.input(), AttachmentMode::Copy);
    assert!(report
        .warnings
        .iter()
        .any(|w| w.contains("親フォルダ「Paperpile」はそのまま残します")));
    assert!(!report.warnings.iter().any(|w| w.contains("同名の既存フォルダへ追加")));
    let db = Db::open(&f.db).unwrap();
    let in_root: i64 = db
        .conn
        .query_row("SELECT count(*) FROM collection_items ci JOIN collections c ON c.id=ci.collection_id WHERE c.key='OLDROOT'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(in_root, 0);
    assert_eq!(f.count("collections"), 4);
}

#[test]
fn reexport_case_variant_folder_does_not_join_the_mapped_one_regardless_of_order() {
    let first = json!({"_id":"a","title":"Alpha","doi":"10.1234/order-a","foldersNamed":["Neuro"]});
    let f = Fixture::new(json!([first.clone()]));
    f.run(&f.input(), AttachmentMode::Copy);
    // BTreeMap 順で "NEURO" が "Neuro" より先に処理される
    std::fs::write(
        &f.json,
        json!([first,{"_id":"b","title":"Beta","doi":"10.1234/order-b","foldersNamed":["NEURO"]}]).to_string(),
    )
    .unwrap();
    let report = f.run(&f.input(), AttachmentMode::Copy);
    assert!(!report.warnings.iter().any(|w| w.contains("同名の既存フォルダへ追加")));
    let db = Db::open(&f.db).unwrap();
    let names: Vec<String> = db
        .conn
        .prepare("SELECT name FROM collections ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(names[0], "Neuro");
    assert!(names[1].starts_with("NEURO ("), "{names:?}");
}

#[test]
fn removed_membership_of_joined_user_folder_is_not_restored() {
    let f = Fixture::new(json!([{"_id":"a","title":"Alpha","doi":"10.1234/join-r7","foldersNamed":["Neuro"]}]));
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute("INSERT INTO collections(key,name) VALUES('MINE','Neuro')", [])
        .unwrap();
    drop(db);
    f.run(&f.input(), AttachmentMode::Copy);
    assert_eq!(f.count("collection_items"), 1);
    let db = Db::open(&f.db).unwrap();
    db.conn.execute("DELETE FROM collection_items", []).unwrap();
    drop(db);
    let report = f.run(&f.input(), AttachmentMode::Copy);
    assert_eq!(f.count("collection_items"), 0);
    assert_eq!(f.count("collections"), 1);
    assert!(report.results[0]
        .warnings
        .iter()
        .any(|w| w.contains("解除済み")));
}

/// report_json が object でない JSON（壊れた保存結果）でも run_report は panic せず Err・一覧は読める
/// （GUI は DB のロックを持ったまま読むので、panic すると AppState の DB が poison される）
#[test]
fn run_report_rejects_non_object_report_json_without_panic() {
    let f = Fixture::new(json!([{"_id":"one","title":"Broken report fixture"}]));
    let input = f.input();
    let report = f.run(&input, AttachmentMode::Copy);
    let db = Db::open(&f.db).unwrap();
    db.conn
        .execute(
            "UPDATE import_runs SET report_json='[]' WHERE id=?1",
            [&report.run_id],
        )
        .unwrap();
    assert!(apply::run_report(&db.conn, &report.run_id).is_err());
    drop(db);
    assert!(apply::run_report_snapshot(&f.db, &report.run_id).is_err());
    let list = apply::history_list_snapshot(&f.db).unwrap();
    assert_eq!(list[0]["id"], report.run_id.as_str());
    assert_eq!(list[0]["total"], 1);
}
