use openref_core::Db;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Command, Output},
};

struct Scratch {
    root: PathBuf,
    db: PathBuf,
    json: PathBuf,
}
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "openref-cli-import-{}-{}",
            std::process::id(),
            openref_core::keys::new_key()
        ));
        std::fs::create_dir(&root).unwrap();
        let db = root.join("app/openref.sqlite");
        let json = root.join("library.json");
        std::fs::write(&json,json!([{"_id":"one","title":"CLI fixture","attachments":[{"_id":"a","filename":"a.pdf"}]}]).to_string()).unwrap();
        Self { root, db, json }
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_openref"))
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .output()
            .unwrap()
    }
    fn import(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_openref"));
        cmd.arg("--db")
            .arg(&self.db)
            .args(["import", "paperpile"])
            .arg(&self.json)
            .args(args);
        cmd.output().unwrap()
    }
    fn decode(output: Output) -> Value {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn zotero_cli_uses_rdf_parent_for_files_and_defaults_to_managed_copy() {
    let s = Scratch::new();
    let rdf = s.root.join("export.rdf");
    std::fs::create_dir_all(s.root.join("files/1")).unwrap();
    let source_pdf = s.root.join("files/1/paper.pdf");
    std::fs::write(
        &source_pdf,
        openref_core::pdftools::minimal_pdf("Zotero CLI fixture", None),
    )
    .unwrap();
    std::fs::write(&rdf,r##"<r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:b="http://purl.org/net/biblio#" xmlns:d="http://purl.org/dc/elements/1.1/" xmlns:l="http://purl.org/rss/1.0/modules/link/" xmlns:z="http://www.zotero.org/namespaces/export#"><b:Article r:about="#1"><z:itemType>journalArticle</z:itemType><d:title>CLI RDF fixture</d:title><l:link r:resource="#2"/></b:Article><z:Attachment r:about="#2"><z:path r:resource="files/1/paper.pdf"/><l:type>application/pdf</l:type></z:Attachment></r:RDF>"##).unwrap();
    let path = rdf.to_str().unwrap();
    let plan = Scratch::decode(s.cli(&["import", "zotero", path, "--dry-run", "--report-json"]));
    assert_eq!(plan["attachment_mode"], "copy");
    assert_eq!(plan["counts"]["available_pdfs"], 1);
    assert!(!s.db.parent().unwrap().exists());
    let report = Scratch::decode(s.cli(&["import", "zotero", path, "--report-json"]));
    assert_eq!(report["state"], "finished");
    let run = report["run_id"].as_str().unwrap();
    let repeated =
        Scratch::decode(s.cli(&["import", "zotero", path, "--resume", run, "--report-json"]));
    assert_eq!(repeated["results"][0]["status"], "merged");
    let db = Db::open(&s.db).unwrap();
    let (mode, path): (String, String) = db
        .conn
        .query_row("SELECT mode,path FROM attachments", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(mode, "managed");
    assert_ne!(PathBuf::from(path), source_pdf);
}

#[test]
fn legacy_dry_run_and_history_do_not_create_a_database() {
    let s = Scratch::new();
    let out = s.import(&["--mount", s.root.to_str().unwrap(), "--dry-run"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!s.db.parent().unwrap().exists());
    assert_eq!(Scratch::decode(s.cli(&["import", "history"])), json!([]));
    assert!(!s.db.parent().unwrap().exists());
}

#[test]
fn legacy_dry_run_reads_old_wal_without_upgrading_original_or_creating_backup() {
    let s = Scratch::new();
    let db = Db::open(&s.db).unwrap();
    db.conn.execute_batch("DROP INDEX IF EXISTS idx_tags_import_key; ALTER TABLE tags DROP COLUMN import_key; DROP TRIGGER IF EXISTS import_match_insert; DROP TRIGGER IF EXISTS import_match_invalidate; DROP TABLE IF EXISTS import_match_keys; DROP INDEX IF EXISTS idx_import_doi; DROP INDEX IF EXISTS idx_import_pmid; DROP INDEX IF EXISTS idx_import_unusual_ids; DROP TABLE import_files; DROP TABLE import_results; DROP TABLE import_objects; DROP TABLE import_runs; DROP TABLE import_sources; PRAGMA user_version=9;").unwrap();
    let paths = [
        s.db.clone(),
        PathBuf::from(format!("{}-wal", s.db.display())),
        PathBuf::from(format!("{}-shm", s.db.display())),
    ];
    let before: Vec<_> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let out = s.import(&["--mount", s.root.to_str().unwrap(), "--dry-run"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        paths
            .iter()
            .map(|p| std::fs::read(p).unwrap())
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(
        db.conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert!(!s.db.parent().unwrap().join("backup").exists());
}

#[test]
fn common_cli_preview_copy_resume_and_report_share_core_results() {
    let s = Scratch::new();
    let root = s.root.to_str().unwrap();
    let plan = Scratch::decode(s.import(&[
        "--files",
        root,
        "--attachments",
        "copy",
        "--dry-run",
        "--report-json",
    ]));
    assert_eq!(plan["counts"]["new_items"], 1);
    assert_eq!(plan["counts"]["missing_pdfs"], 1);
    assert!(!s.db.parent().unwrap().exists());
    let first =
        Scratch::decode(s.import(&["--files", root, "--attachments", "copy", "--report-json"]));
    assert_eq!(first["results"][0]["status"], "inserted");
    std::fs::write(s.root.join("a.pdf"), b"%PDF-1.4 CLI").unwrap();
    let run = first["run_id"].as_str().unwrap();
    let second = Scratch::decode(s.import(&["--files", root, "--resume", run, "--report-json"]));
    assert_eq!(second["results"][0]["status"], "merged");
    assert_eq!(second["results"][0]["attachments"][0]["status"], "copied");
    assert_eq!(second["fulltext_pending"], 1);
    let db = Db::open(&s.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.conn
            .query_row("SELECT mode FROM attachments", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "managed"
    );
    let report = Scratch::decode(s.cli(&["import", "report", second["run_id"].as_str().unwrap()]));
    assert_eq!(report["report"], second);
    let history = Scratch::decode(s.cli(&["import", "history"]));
    assert_eq!(history.as_array().unwrap().len(), 2);
}

#[test]
fn old_arguments_use_safe_common_apply_with_link_default_and_history() {
    let s = Scratch::new();
    std::fs::write(s.root.join("a.pdf"), b"%PDF-linked fixture").unwrap();
    // No new flags: --mount still selects a root and default remains linked.
    let out = s.import(&["--mount", s.root.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let db = Db::open(&s.db).unwrap();
    let mode: String = db
        .conn
        .query_row("SELECT mode FROM attachments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "linked");
    let backup: String = db
        .conn
        .query_row("SELECT backup_path FROM import_runs", [], |r| r.get(0))
        .unwrap();
    assert!(std::path::Path::new(&backup).is_file());
    assert!(!s.db.parent().unwrap().join("storage").exists());
    drop(db);
    assert!(s
        .import(&["--mount", s.root.to_str().unwrap()])
        .status
        .success());
    let db = Db::open(&s.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn old_arguments_block_crossed_identifiers_and_backup_failure() {
    let s = Scratch::new();
    let db = Db::open(&s.db).unwrap();
    for (key, pmid, doi) in [("A", "111", "10.1234/a"), ("B", "222", "10.1234/b")] {
        db.conn
            .execute(
                "INSERT INTO items(key,csl) VALUES(?1,?2)",
                [
                    key.to_string(),
                    json!({"title":key,"PMID":pmid,"DOI":doi}).to_string(),
                ],
            )
            .unwrap();
    }
    std::fs::write(&s.json,json!([{"_id":"cross","title":"Cross","pmid":"111","doi":"10.1234/b","foldersNamed":["Cross"]}]).to_string()).unwrap();
    drop(db);
    let out = s.import(&["--mount", s.root.to_str().unwrap()]);
    assert!(out.status.success());
    let history = Scratch::decode(s.cli(&["import", "history"]));
    assert_eq!(history[0]["report"]["results"][0]["status"], "blocked");
    let db = Db::open(&s.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT count(*) FROM collection_items", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(db);
    // A fresh DB whose backup directory is a regular file must not import.
    let other = Scratch::new();
    let db = Db::open(&other.db).unwrap();
    drop(db);
    std::fs::write(other.db.parent().unwrap().join("backup"), b"block backup").unwrap();
    assert!(!other
        .import(&["--mount", other.root.to_str().unwrap()])
        .status
        .success());
    let db = Db::open(&other.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn explicit_decisions_preview_and_apply_use_identical_safe_core() {
    let s = Scratch::new();
    let first = Scratch::decode(s.import(&["--source-id", "choices-cli", "--report-json"]));
    let key = first["results"][0]["item_key"].as_str().unwrap();
    let raw = std::fs::read_to_string(&s.json).unwrap();
    std::fs::write(&s.json, format!("{raw}\n")).unwrap();
    let a = s.root.join("a");
    let b = s.root.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::write(a.join("a.pdf"), b"%PDF-selected").unwrap();
    std::fs::write(b.join("a.pdf"), b"%PDF-other").unwrap();
    let args = [
        "--source-id",
        "choices-cli",
        "--files",
        a.to_str().unwrap(),
        "--files",
        b.to_str().unwrap(),
        "--attachments",
        "copy",
        "--report-json",
    ];
    let mut dry = args.to_vec();
    dry.push("--dry-run");
    let plan = Scratch::decode(s.import(&dry));
    assert_eq!(plan["counts"]["review"], 1);
    assert_eq!(plan["counts"]["ambiguous_pdfs"], 1);
    let decision = s.root.join("choices.json");
    std::fs::write(&decision,json!({"source_id":plan["source_id"],"input_sha256":plan["input_sha256"],"items":[{"object_id":plan["items"][0]["object_id"],"item_key":key,"csl_sha256":plan["items"][0]["candidate_details"][0]["csl_sha256"]}],"files":[{"object_id":plan["items"][0]["object_id"],"attachment_id":plan["items"][0]["attachments"][0]["id"],"path":a.join("a.pdf").canonicalize().unwrap()}]}).to_string()).unwrap();
    dry.extend(["--decisions", decision.to_str().unwrap()]);
    let before = std::fs::read(&s.db).unwrap();
    let preview = Scratch::decode(s.import(&dry));
    assert_eq!(preview["counts"]["review"], 0);
    assert_eq!(preview["counts"]["ambiguous_pdfs"], 0);
    assert_eq!(std::fs::read(&s.db).unwrap(), before);
    assert!(!s.db.parent().unwrap().join("storage").exists());
    let mut run = args.to_vec();
    run.extend(["--decisions", decision.to_str().unwrap()]);
    let applied = Scratch::decode(s.import(&run));
    assert_eq!(applied["results"][0]["status"], "merged");
    assert_eq!(applied["results"][0]["attachments"][0]["status"], "copied");
    let repeated = Scratch::decode(s.import(&run));
    assert_eq!(repeated["results"][0]["status"], "merged");
    let db = Db::open(&s.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.conn
            .query_row("SELECT count(*) FROM attachments", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn n1_legacy_mount_reexport_reuses_source_and_tree_after_rename() {
    let s = Scratch::new();
    let first =
        json!({"_id":"a","title":"Alpha","doi":"10.1234/a","foldersNamed":["Radiology/CT"]});
    std::fs::write(&s.json, json!([first.clone()]).to_string()).unwrap();
    let args = ["--mount", s.root.to_str().unwrap(), "--report-json"];
    let one = Scratch::decode(s.import(&args));
    let db = Db::open(&s.db).unwrap();
    db.conn
        .execute("UPDATE collections SET name='Renamed' WHERE name='CT'", [])
        .unwrap();
    drop(db);
    std::fs::write(
        &s.json,
        json!([first,{"_id":"b","title":"Beta","doi":"10.1234/b","foldersNamed":["Radiology/CT"]}])
            .to_string(),
    )
    .unwrap();
    let two = Scratch::decode(s.import(&args));
    assert_eq!(one["source_id"], two["source_id"]);
    assert_eq!(two["results"][0]["status"], "review");
    let db = Db::open(&s.db).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT count(*) FROM collections", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(db.conn.query_row("SELECT count(*) FROM collection_items ci JOIN collections c ON c.id=ci.collection_id WHERE c.name='Renamed'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
}

#[test]
fn saved_cli_decision_rejects_changed_candidate_until_reselected() {
    let s = Scratch::new();
    Scratch::decode(s.import(&["--report-json"]));
    let raw = std::fs::read_to_string(&s.json).unwrap();
    std::fs::write(&s.json, format!("{raw}\n")).unwrap();
    let plan = Scratch::decode(s.import(&["--dry-run", "--report-json"]));
    let candidate = &plan["items"][0]["candidate_details"][0];
    let mut choice = json!({"source_id":plan["source_id"],"input_sha256":plan["input_sha256"],"items":[{"object_id":plan["items"][0]["object_id"],"item_key":candidate["key"],"csl_sha256":candidate["csl_sha256"]}]});
    let file = s.root.join("saved-choice.json");
    std::fs::write(&file, choice.to_string()).unwrap();
    let db = Db::open(&s.db).unwrap();
    db.conn
        .execute(
            "UPDATE items SET csl=json_set(csl,'$.title','Different candidate')",
            [],
        )
        .unwrap();
    drop(db);
    let args = ["--decisions", file.to_str().unwrap(), "--report-json"];
    assert!(
        !s.import(&args).status.success(),
        "saved hash must not be rebound to a fresh preview"
    );
    choice["items"][0]
        .as_object_mut()
        .unwrap()
        .remove("csl_sha256");
    std::fs::write(&file, choice.to_string()).unwrap();
    assert!(
        !s.import(&args).status.success(),
        "unbound existing choices require reselection"
    );
    let fresh = Scratch::decode(s.import(&["--dry-run", "--report-json"]));
    choice["items"][0]["csl_sha256"] =
        fresh["items"][0]["candidate_details"][0]["csl_sha256"].clone();
    std::fs::write(&file, choice.to_string()).unwrap();
    assert_eq!(
        Scratch::decode(s.import(&args))["results"][0]["status"],
        "merged"
    );
    assert_eq!(
        Scratch::decode(s.import(&args))["results"][0]["status"],
        "merged"
    );
}
