//! The one preparation path shared by the CLI and the desktop migration:
//! read the export, decide the source, add the attachment roots, plan
//! (read-only) and apply saved decisions. Entry points only map their own
//! arguments onto [`Request`].
use super::{
    apply, decisions, files::AttachmentRoots, plan::ImportPlan, AttachmentMode, ImportInput,
    ImportProgress, ImportSource,
};
use crate::{CoreError, Result};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// `source_id` value that asks for a new, generated source.
const NEW_SOURCE: &str = "@new";

/// Read an export with the adapter for `source.format`.
pub fn read_input(path: &Path, source: ImportSource) -> Result<ImportInput> {
    match source.format.as_str() {
        "paperpile" => super::paperpile::read_input(path, source),
        "zotero" => super::zotero_rdf::read_input(path, source),
        other => Err(CoreError::BadInput(format!(
            "対応していない移行元の形式です: {other}"
        ))),
    }
}

/// Attachment roots for an export: the selected folders, plus the RDF's own
/// folder for Zotero (its `files/` paths are relative to the RDF).
fn default_roots(format: &str, path: &Path, extra: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut roots = extra.to_vec();
    if format == "zotero" {
        roots.push(
            std::fs::canonicalize(path)?
                .parent()
                .ok_or_else(|| CoreError::BadInput("RDF の親フォルダがありません".into()))?
                .into(),
        );
    }
    Ok(roots)
}

/// The source an export is added to. Blank means "not chosen" (reuse the single
/// existing source of this format); [`NEW_SOURCE`] makes a new one.
fn resolve_source_id(db_path: &Path, format: &str, requested: Option<&str>) -> Result<String> {
    match requested.filter(|s| !s.trim().is_empty()) {
        Some(NEW_SOURCE) => Ok(format!("{format}-{}", crate::keys::new_key())),
        Some(id) => Ok(id.to_string()),
        None => apply::default_source(db_path, format),
    }
}

pub struct Request<'a> {
    pub format: &'a str,
    pub path: &'a Path,
    /// Folders chosen by the user only. For Zotero, `prepare_request` itself
    /// adds the RDF's folder; callers do not.
    pub roots: &'a [PathBuf],
    pub mode: AttachmentMode,
    pub source_id: Option<&'a str>,
    /// Paperpile only: confirmed legacy `custom.paperpile.id` values.
    pub adopt_legacy_ids: bool,
    pub decisions: Option<&'a decisions::Decisions>,
}

pub struct Prepared {
    pub input: ImportInput,
    pub roots: AttachmentRoots,
    pub plan: ImportPlan,
}

pub fn prepare_request(
    db_path: &Path,
    req: &Request,
    cancel: &AtomicBool,
    progress: impl FnMut(&ImportProgress),
) -> Result<Prepared> {
    apply::reject_database_file(req.path, db_path)?;
    let source = ImportSource {
        id: "pending".into(),
        format: req.format.into(),
    };
    let mut input = read_input(req.path, source)?;
    input.source.id = resolve_source_id(db_path, req.format, req.source_id)?;
    input.adopt_legacy_paperpile_ids = req.format == "paperpile" && req.adopt_legacy_ids;
    let roots = AttachmentRoots::new(&default_roots(req.format, req.path, req.roots)?)?;
    let mut plan = apply::prepare_controlled(db_path, &input, &roots, req.mode, cancel, progress)?;
    if let Some(choices) = req.decisions {
        decisions::resolve(db_path, &input, &roots, &mut plan, choices, cancel)?;
    }
    Ok(Prepared { input, roots, plan })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("openref-prepare-{name}-{}", crate::keys::new_key()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn source_id_new_blank_and_explicit() {
        let dir = scratch("source");
        let db = dir.join("none.sqlite");
        let new = resolve_source_id(&db, "zotero", Some(NEW_SOURCE)).unwrap();
        assert!(
            new.starts_with("zotero-") && new.len() == "zotero-".len() + 8,
            "{new}"
        );
        assert_ne!(
            new,
            resolve_source_id(&db, "zotero", Some(NEW_SOURCE)).unwrap()
        );
        for blank in [None, Some(""), Some("  \t")] {
            assert_eq!(
                resolve_source_id(&db, "paperpile", blank).unwrap(),
                "paperpile-default",
                "{blank:?}"
            );
        }
        assert_eq!(
            resolve_source_id(&db, "paperpile", Some(" kept as given ")).unwrap(),
            " kept as given "
        );
        assert!(!db.exists(), "resolving a source never creates the library");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_input_dispatches_by_format_and_rejects_unknown() {
        let dir = scratch("format");
        let json = dir.join("export.json");
        std::fs::write(&json, r#"[{"_id":"one","title":"Format fixture"}]"#).unwrap();
        let rdf = dir.join("export.rdf");
        std::fs::write(&rdf, r##"<r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:b="http://purl.org/net/biblio#" xmlns:d="http://purl.org/dc/elements/1.1/" xmlns:z="http://www.zotero.org/namespaces/export#"><b:Article r:about="#1"><z:itemType>journalArticle</z:itemType><d:title>RDF fixture</d:title></b:Article></r:RDF>"##).unwrap();
        let source = |format: &str| ImportSource {
            id: "pending".into(),
            format: format.into(),
        };
        assert_eq!(
            read_input(&json, source("paperpile")).unwrap().items.len(),
            1
        );
        assert_eq!(read_input(&rdf, source("zotero")).unwrap().items.len(), 1);
        // Each adapter refuses the other's export (no silent fallback).
        assert!(read_input(&json, source("zotero")).is_err());
        assert!(read_input(&rdf, source("paperpile")).is_err());
        let err = read_input(&json, source("endnote"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("対応していない移行元の形式です: endnote"),
            "{err}"
        );
        assert_eq!(
            default_roots("zotero", &rdf, &[]).unwrap(),
            vec![dir.canonicalize().unwrap()]
        );
        assert!(default_roots("paperpile", &json, &[]).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prepare_request_is_read_only_and_ignores_adopt_for_zotero() {
        let dir = scratch("request");
        let rdf = dir.join("export.rdf");
        std::fs::write(&rdf, r##"<r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:b="http://purl.org/net/biblio#" xmlns:d="http://purl.org/dc/elements/1.1/" xmlns:z="http://www.zotero.org/namespaces/export#"><b:Article r:about="#1"><z:itemType>journalArticle</z:itemType><d:title>RDF fixture</d:title></b:Article></r:RDF>"##).unwrap();
        let db = dir.join("app/openref.sqlite");
        let prepared = prepare_request(
            &db,
            &Request {
                format: "zotero",
                path: &rdf,
                roots: &[],
                mode: AttachmentMode::Copy,
                source_id: Some(NEW_SOURCE),
                adopt_legacy_ids: true,
                decisions: None,
            },
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert!(prepared.input.source.id.starts_with("zotero-"));
        assert_ne!(prepared.input.source.id, NEW_SOURCE);
        assert!(!prepared.input.adopt_legacy_paperpile_ids);
        assert_eq!(prepared.plan.counts.new_items, 1);
        assert!(!db.parent().unwrap().exists(), "preview stays read-only");
        // The library itself is never an import input.
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        crate::Db::open(&db).unwrap();
        let err = prepare_request(
            &db,
            &Request {
                format: "paperpile",
                path: &db,
                roots: &[],
                mode: AttachmentMode::Link,
                source_id: None,
                adopt_legacy_ids: false,
                decisions: None,
            },
            &AtomicBool::new(false),
            |_| {},
        );
        assert!(err.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
