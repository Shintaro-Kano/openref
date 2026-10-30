use openref_core::import::{
    self,
    files::{AttachmentRoots, FileFingerprint, ResolvedPath},
    plan::{self, AttachmentPlan, ExistingItem, ExistingLibrary, ExistingObject, ItemAction},
    AttachmentMode, ImportSource,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "openref-import-plan-{}-{}",
            std::process::id(),
            openref_core::keys::new_key()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn write(&self, name: &str, contents: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents).unwrap();
        p
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn source() -> ImportSource {
    ImportSource {
        id: "chosen-source-1".into(),
        format: "paperpile".into(),
    }
}
fn input(s: &Scratch, records: Value) -> import::ImportInput {
    let p = s.write("library.json", records.to_string().as_bytes());
    import::paperpile::read_input(&p, source()).unwrap()
}
fn old(key: &str, csl: Value) -> ExistingItem {
    ExistingItem {
        key: key.into(),
        csl,
        deleted: false,
    }
}
fn empty_roots() -> AttachmentRoots {
    AttachmentRoots::new(&[]).unwrap()
}
fn preview(i: &import::ImportInput, old: Vec<ExistingItem>) -> import::plan::ImportPlan {
    plan::preview(
        i,
        &ExistingLibrary {
            items: old,
            objects: vec![],
        },
        &empty_roots(),
        AttachmentMode::Copy,
    )
    .unwrap()
}

#[test]
fn actual_paperpile_shape_preserves_all_records_and_exact_zip_layout() {
    let s = Scratch::new();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/paperpile-2026-10-27/library.json");
    let before = std::fs::read(&path).unwrap();
    let input = import::paperpile::read_input(&path, source()).unwrap();
    assert_eq!(input.items.len(), 3);
    assert_eq!(
        input
            .items
            .iter()
            .map(|i| i.attachments.len())
            .sum::<usize>(),
        2
    );
    assert_eq!(input.items[0].csl["author"].as_array().unwrap().len(), 10);
    assert_eq!(input.items[1].csl["author"].as_array().unwrap().len(), 5);
    assert_eq!(input.items[2].csl["author"].as_array().unwrap().len(), 3);
    assert!(input.items.iter().all(|i| !i.stable_id));
    assert!(input.items[0].raw.get("owner").is_some());
    assert!(input.items[0].csl.get("owner").is_none());
    // Tiny header-only file bytes exercise resolution and hashing, not PDF.js.
    s.write(
        "Paperpile files/F/Fixture0 - Article.pdf",
        b"%PDF-1.4 fixture-0",
    );
    s.write(
        "Paperpile files/F/Fixture1 - Article.pdf",
        b"%PDF-1.4 fixture-1",
    );
    let roots = AttachmentRoots::new(std::slice::from_ref(&s.0)).unwrap();
    let plan = plan::preview(
        &input,
        &ExistingLibrary::default(),
        &roots,
        AttachmentMode::Copy,
    )
    .unwrap();
    assert_eq!(plan.counts.new_items, 3);
    assert_eq!(plan.counts.available_pdfs, 2);
    assert_eq!(plan.counts.missing_pdfs, 0);
    assert_eq!(plan.counts.copy_bytes_upper_bound, 36);
    assert!(!s.0.join("storage").exists());
    assert!(!s.0.join("backup").exists());
    assert_eq!(std::fs::read(path).unwrap(), before);
    // The attachment root may also be the ZIP's Paperpile files directory.
    let roots = AttachmentRoots::new(&[s.0.join("Paperpile files")]).unwrap();
    let plan = plan::preview(
        &input,
        &ExistingLibrary::default(),
        &roots,
        AttachmentMode::Link,
    )
    .unwrap();
    assert_eq!(plan.counts.available_pdfs, 2);
    assert_eq!(plan.counts.copy_bytes_upper_bound, 0);
}

/// JSON の添付パスが絶対パスでルートの外を指すとき、事前確認は Failed にする(読まない・Available にしない)
#[test]
fn absolute_attachment_path_outside_root_fails_in_preview() {
    let root = Scratch::new();
    let external = Scratch::new();
    let outside = external.write("outside.pdf", b"%PDF-1.4 external");
    let i = input(
        &root,
        json!([{"_id":"bad","title":"Outside root",
            "attachments":[{"article_pdf":1,"filename":outside.to_str().unwrap()}]}]),
    );
    let roots = AttachmentRoots::new(std::slice::from_ref(&root.0)).unwrap();
    let plan = plan::preview(&i, &ExistingLibrary::default(), &roots, AttachmentMode::Link).unwrap();
    assert_eq!(plan.counts.failed_pdfs, 1);
    assert_eq!(plan.counts.available_pdfs, 0);
    assert!(matches!(plan.items[0].attachments[0], AttachmentPlan::Failed { .. }));
    assert_eq!(std::fs::read(&outside).unwrap(), b"%PDF-1.4 external");
}

#[test]
fn observed_named_labels_are_mapped_and_legacy_id_adoption_is_explicit() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/paperpile-2026-10-27/tagged-library.json");
    let tagged = import::paperpile::read_input(&path, source()).unwrap();
    assert_eq!(tagged.items.len(), 3);
    assert!(tagged.folders.is_empty());
    assert!(tagged
        .items
        .iter()
        .all(|i| i.tags.len() == 1 && i.tags[0].name == "Fixture label"));
    let ids: std::collections::HashSet<_> = tagged.items.iter().map(|i| &i.tags[0].id).collect();
    assert_eq!(ids.len(), 1);
    let s = Scratch::new();
    let mut input = input(&s, json!([{"_id":"legacy","title":"Source title"}]));
    let existing = old(
        "old-key",
        json!({"type":"article-journal","title":"Locally edited","custom":{"paperpile":{"id":"legacy"}}}),
    );
    assert!(matches!(
        preview(&input, vec![existing.clone()]).items[0].action,
        ItemAction::New
    ));
    input.adopt_legacy_paperpile_ids = true;
    assert!(
        matches!(&preview(&input,vec![existing.clone()]).items[0].action,ItemAction::AddMissing{item_key} if item_key=="old-key")
    );
    let mut deleted = existing;
    deleted.deleted = true;
    assert!(matches!(
        preview(&input, vec![deleted]).items[0].action,
        ItemAction::Blocked { .. }
    ));
}

#[test]
fn crossed_identifiers_and_duplicate_existing_candidates_are_blocked() {
    let s = Scratch::new();
    let i = input(
        &s,
        json!([{"_id":"x","title":"A","doi":"https://doi.org/10.5555/A","pmid":"00123"}]),
    );
    let p = preview(
        &i,
        vec![
            old("A", json!({"DOI":"10.5555/a"})),
            old("B", json!({"PMID":"123"})),
        ],
    );
    assert!(
        matches!(&p.items[0].action, ItemAction::Blocked { candidates, .. } if candidates == &vec!["A".to_string(),"B".to_string()])
    );
    let p = preview(
        &i,
        vec![
            old("A", json!({"DOI":"10.5555/a"})),
            old("B", json!({"DOI":"10.5555/a"})),
        ],
    );
    assert_eq!(p.counts.blocked, 1);
    let p = preview(&i, vec![old("A", json!({"DOI":"10.5555/a","PMID":"999"}))]);
    assert_eq!(p.counts.blocked, 1);
}

#[test]
fn weak_identifiers_never_merge_and_title_author_year_only_propose_review() {
    let s = Scratch::new();
    let i = input(
        &s,
        json!([{"_id":"x","title":"Same title","doi":"N/A","pmid":"unknown","published":{"year":"2026"},"author":[{"last":"Author"}]}]),
    );
    let p = preview(
        &i,
        vec![old(
            "A",
            json!({"DOI":"N/A","PMID":"unknown","ISBN":"same","URL":"https://example.com"}),
        )],
    );
    assert_eq!(p.counts.new_items, 1);
    let p = preview(&i, vec![old("A", i.items[0].csl.clone())]);
    assert_eq!(p.counts.review, 1);
    assert_eq!(p.counts.existing_items, 0);
}

#[test]
fn mapped_objects_are_scoped_and_deleted_or_conflicting_targets_never_recreated() {
    let s = Scratch::new();
    let mut i = input(&s, json!([{"_id":"local-id","title":"No identifiers"}]));
    let mut lib = ExistingLibrary {
        items: vec![old("A", json!({"title":"Edited"}))],
        objects: vec![ExistingObject {
            source_id: i.source.id.clone(),
            object_id: "local-id".into(),
            input_sha256: Some(i.input_sha256.clone()),
            item_key: "A".into(),
        }],
    };
    let p = plan::preview(&i, &lib, &empty_roots(), AttachmentMode::Copy).unwrap();
    assert_eq!(p.counts.existing_items, 1);
    i.input_sha256 = "a".repeat(64); // A different export does not inherit local IDs.
    assert_eq!(
        plan::preview(&i, &lib, &empty_roots(), AttachmentMode::Copy)
            .unwrap()
            .counts
            .new_items,
        1
    );
    i.items[0].stable_id = true;
    lib.objects[0].input_sha256 = None;
    i.source.id = "another-library".into();
    assert_eq!(
        plan::preview(&i, &lib, &empty_roots(), AttachmentMode::Copy)
            .unwrap()
            .counts
            .new_items,
        1
    );
    i.source.id = "chosen-source-1".into();
    lib.items[0].deleted = true;
    assert_eq!(
        plan::preview(&i, &lib, &empty_roots(), AttachmentMode::Copy)
            .unwrap()
            .counts
            .blocked,
        1
    );
    lib.items.clear();
    assert_eq!(
        plan::preview(&i, &lib, &empty_roots(), AttachmentMode::Copy)
            .unwrap()
            .counts
            .blocked,
        1
    );
}

#[test]
fn mapped_identifier_conflict_is_blocked_even_without_other_candidate() {
    let s = Scratch::new();
    let i = input(
        &s,
        json!([{"_id":"x","title":"Changed","doi":"10.5555/new"}]),
    );
    let lib = ExistingLibrary {
        items: vec![old("A", json!({"DOI":"10.5555/old"}))],
        objects: vec![ExistingObject {
            source_id: i.source.id.clone(),
            object_id: "x".into(),
            input_sha256: Some(i.input_sha256.clone()),
            item_key: "A".into(),
        }],
    };
    assert_eq!(
        plan::preview(&i, &lib, &empty_roots(), AttachmentMode::Copy)
            .unwrap()
            .counts
            .blocked,
        1
    );
}

#[test]
fn repeated_input_object_ids_and_strong_identifiers_require_attention() {
    let s = Scratch::new();
    let i = input(&s, json!([{"_id":"x","title":"A"},{"_id":"x","title":"B"}]));
    assert_eq!(preview(&i, vec![]).counts.blocked, 2);
    let i = input(
        &s,
        json!([{"_id":"x","title":"A","doi":"10.5555/a"},{"_id":"y","title":"B","doi":"10.5555/a"}]),
    );
    assert_eq!(preview(&i, vec![]).counts.review, 2);
}

#[test]
fn file_resolution_rejects_escape_and_never_uses_basename() {
    let root = Scratch::new();
    let other = Scratch::new();
    let outside = other.write("file.pdf", b"%PDF-1.4 external");
    root.write("file.pdf", b"%PDF-1.4 local");
    let roots = AttachmentRoots::new(std::slice::from_ref(&root.0)).unwrap();
    assert!(matches!(
        roots.resolve("missing/file.pdf").unwrap(),
        ResolvedPath::Missing
    ));
    assert!(matches!(
        roots.resolve("../file.pdf").unwrap(),
        ResolvedPath::Rejected { .. }
    ));
    assert!(matches!(
        roots.resolve("%2e%2e/file.pdf").unwrap(),
        ResolvedPath::Rejected { .. }
    ));
    assert!(matches!(
        roots.resolve(outside.to_str().unwrap()).unwrap(),
        ResolvedPath::Rejected { .. }
    ));
    assert!(matches!(
        roots.resolve("https://example.com/file.pdf").unwrap(),
        ResolvedPath::Rejected { .. }
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&other.0, root.0.join("escape")).unwrap();
        assert!(matches!(
            roots.resolve("escape/file.pdf").unwrap(),
            ResolvedPath::Rejected { .. }
        ));
    }
}

#[test]
fn exact_layout_collisions_and_multiple_selected_roots_are_ambiguous() {
    let a = Scratch::new();
    let b = Scratch::new();
    a.write("file.pdf", b"%PDF-a");
    b.write("file.pdf", b"%PDF-b");
    let roots = AttachmentRoots::new(&[a.0.clone(), b.0.clone()]).unwrap();
    assert!(
        matches!(roots.resolve("file.pdf").unwrap(),ResolvedPath::Ambiguous { candidates } if candidates.len() == 2)
    );
    a.write("file%20name.pdf", b"%PDF-literal");
    a.write("file name.pdf", b"%PDF-decoded");
    assert!(matches!(
        roots.resolve("file%20name.pdf").unwrap(),
        ResolvedPath::Ambiguous { .. }
    ));
    a.write("All Papers/F/Fixture.pdf", b"%PDF-one");
    a.write("Paperpile files/F/Fixture.pdf", b"%PDF-two");
    assert!(matches!(
        roots
            .resolve_candidates(&["All Papers/F/Fixture.pdf", "Paperpile files/F/Fixture.pdf"])
            .unwrap(),
        ResolvedPath::Ambiguous { .. }
    ));
}

#[test]
fn file_uris_unicode_and_changes_after_preview() {
    use unicode_normalization::UnicodeNormalization;
    let s = Scratch::new();
    let rel: String = "日本語/café.pdf".nfd().collect();
    let path = s.write(&rel, b"%PDF-1.4 original");
    let roots = AttachmentRoots::new(std::slice::from_ref(&s.0)).unwrap();
    assert!(matches!(
        roots.resolve("日本語/café.pdf").unwrap(),
        ResolvedPath::Found { .. }
    ));
    assert!(matches!(
        roots
            .resolve(url::Url::from_file_path(&path).unwrap().as_str())
            .unwrap(),
        ResolvedPath::Found { .. }
    ));
    let fingerprint = FileFingerprint::read(&path).unwrap();
    fingerprint.verify(&path).unwrap();
    std::fs::write(&path, b"%PDF-1.4 modified").unwrap();
    assert!(fingerprint
        .verify(&path)
        .unwrap_err()
        .to_string()
        .contains("変更"));
}

#[test]
#[cfg(unix)]
fn replacing_the_selected_directory_is_reported_before_reading_new_contents() {
    let s = Scratch::new();
    let path = s.write("selected/file.pdf", b"%PDF-original");
    let roots = AttachmentRoots::new(&[s.0.join("selected")]).unwrap();
    std::fs::rename(s.0.join("selected"), s.0.join("original")).unwrap();
    s.write("selected/file.pdf", b"%PDF-replacement");
    let error = roots.inspect_resolved_pdf(&path).unwrap_err().to_string();
    assert!(error.contains("置き換え"), "{error}");
    assert_eq!(
        std::fs::read(s.0.join("original/file.pdf")).unwrap(),
        b"%PDF-original"
    );
}

#[test]
fn attachment_failures_are_reported_without_discarding_bibliography() {
    let s = Scratch::new();
    s.write("broken.pdf", b"not a pdf");
    let i = input(
        &s,
        json!([{"title":"No DOI or PMID","attachments":[
            {"filename":"broken.pdf"},{"filename":"missing.pdf"},{"filename":"image.png","mimeType":"image/png"}
        ]}]),
    );
    let roots = AttachmentRoots::new(std::slice::from_ref(&s.0)).unwrap();
    let p = plan::preview(
        &i,
        &ExistingLibrary::default(),
        &roots,
        AttachmentMode::Copy,
    )
    .unwrap();
    assert_eq!(p.counts.new_items, 1);
    assert_eq!(p.counts.failed_pdfs, 1);
    assert_eq!(p.counts.missing_pdfs, 1);
    assert_eq!(p.counts.unsupported_attachments, 1);
    assert!(matches!(
        p.items[0].attachments[0],
        AttachmentPlan::Failed { .. }
    ));
}

/// created が無い / 数値でない（範囲外）は区別した件数の notices。文献ごとの warnings にはしない
#[test]
fn missing_and_invalid_created_are_counted_separately() {
    let s = Scratch::new();
    let i = input(
        &s,
        json!([{"_id":"a","title":"A"},{"_id":"b","title":"B","created":"yesterday"},
               {"_id":"c","title":"C","created":-1.0},{"_id":"d","title":"D","created":1750000000.0}]),
    );
    assert!(i.items.iter().all(|i| i.warnings.is_empty()));
    assert!(i.items[3].date_added.is_some());
    assert!(i.notices.iter().any(|w| w.starts_with("1 件は元の登録日時（created）が無いため、新しく登録する文献には")), "{:?}", i.notices);
    assert!(i.notices.iter().any(|w| w.starts_with("2 件は元の登録日時（created）が数値でないか範囲外のため")), "{:?}", i.notices);
}

#[test]
fn unreadable_shapes_and_cyclic_folder_graphs_do_not_silently_drop_data() {
    let s = Scratch::new();
    let mut i = input(
        &s,
        json!([{"_id":"x","folders":["unknown-folder"],"labels":["unknown-label"],"notes":[{"html":"example"}]}]),
    );
    assert_eq!(i.items.len(), 1);
    assert_eq!(i.items[0].csl["type"], "document");
    // 題名・folders・notes・labels の 4 件。created が無いことは対応の要らない注意（notices の件数）
    assert_eq!(i.items[0].warnings.len(), 4);
    assert!(i
        .notices
        .iter()
        .any(|w| w.starts_with("1 件は元の登録日時（created）が無い")));
    i.folders = vec![import::ImportFolder {
        id: "a".into(),
        stable_id: false,
        name: "Name/with slash".into(),
        parent_id: Some("a".into()),
        raw: Value::Null,
    }];
    assert!(plan::preview(
        &i,
        &ExistingLibrary::default(),
        &empty_roots(),
        AttachmentMode::Copy
    )
    .unwrap_err()
    .to_string()
    .contains("循環"));
    i.folders[0].parent_id = None;
    i.items[0].folder_ids.push("missing".into());
    assert!(plan::preview(
        &i,
        &ExistingLibrary::default(),
        &empty_roots(),
        AttachmentMode::Copy
    )
    .is_err());
}

#[test]
fn inventory_reads_legacy_schema_without_migration_or_disk_changes() {
    let s = Scratch::new();
    let path = s.0.join("old.sqlite");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE items (key TEXT, csl TEXT, deleted_at TEXT); PRAGMA user_version=1; INSERT INTO items VALUES ('A','{\"title\":\"Keep edits\"}',NULL);").unwrap();
    drop(c);
    let before = std::fs::read(&path).unwrap();
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let library = ExistingLibrary::read(&conn).unwrap();
    assert_eq!(library.items.len(), 1);
    assert_eq!(library.items[0].csl["title"], "Keep edits");
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(conn);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(&s.0).unwrap().count(), 1);
}

/// Explicit paths only: never discovers or opens the user's live OpenRef DB.
/// The exported JSON and PDFs are read without modifying their contents.
#[test]
#[ignore = "requires the user's explicitly supplied 2026-10-27 Paperpile sample"]
fn current_paperpile_real_export_preview() {
    let json_path = PathBuf::from(
        std::env::var_os("OPENREF_TEST_PAPERPILE_JSON").expect("set OPENREF_TEST_PAPERPILE_JSON"),
    );
    let files = PathBuf::from(
        std::env::var_os("OPENREF_TEST_PAPERPILE_FILES").expect("set OPENREF_TEST_PAPERPILE_FILES"),
    );
    let before = FileFingerprint::read(&json_path).unwrap();
    let input = import::paperpile::read_input(&json_path, source()).unwrap();
    let roots = AttachmentRoots::new(&[files]).unwrap();
    let plan = plan::preview(
        &input,
        &ExistingLibrary::default(),
        &roots,
        AttachmentMode::Copy,
    )
    .unwrap();
    assert_eq!(input.items.len(), 3);
    assert_eq!(plan.counts.new_items, 3);
    assert_eq!(plan.counts.available_pdfs, 2);
    assert_eq!(
        plan.counts.missing_pdfs + plan.counts.failed_pdfs + plan.counts.ambiguous_pdfs,
        0
    );
    assert_eq!(plan.counts.copy_bytes_upper_bound, 3_852_802);
    before.verify(&json_path).unwrap();
    for item in &plan.items {
        for att in &item.attachments {
            if let AttachmentPlan::Available {
                path, fingerprint, ..
            } = att
            {
                fingerprint.verify(path).unwrap();
            }
        }
    }
    println!("Real Paperpile preview: 3 records, 2 PDFs, 3852802 bytes; no DB or storage writes");
}
