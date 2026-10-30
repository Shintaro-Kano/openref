//! Read-only, source-independent migration planning. No DB open/migration,
//! network, backup, storage creation, or writes are performed here.
use super::{
    files::{AttachmentRoots, FileFingerprint, ResolvedPath},
    AttachmentMode, ImportInput,
};
use crate::{model, CoreError, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct ExistingItem {
    pub key: String,
    pub csl: Value,
    pub deleted: bool,
}

/// A verified source-object mapping loaded by the caller. An absent target is
/// intentionally retained as a tombstone; it must never silently create an item.
#[derive(Debug, Clone)]
pub struct ExistingObject {
    pub source_id: String,
    pub object_id: String,
    /// None for verified stable IDs; Some(digest) for export-local IDs.
    pub input_sha256: Option<String>,
    pub item_key: String,
}

#[derive(Debug, Default)]
pub struct ExistingLibrary {
    pub items: Vec<ExistingItem>,
    pub objects: Vec<ExistingObject>,
}

impl ExistingLibrary {
    /// Read an already-open connection in one read transaction. This works on
    /// pre-migration schemas and does not call Db::open. For a nonexistent DB,
    /// callers use Default rather than creating a DB merely to preview it.
    pub fn read(conn: &Connection) -> Result<Self> {
        let tx = conn.unchecked_transaction()?;
        let items = {
            let mut stmt = tx.prepare("SELECT key, csl, deleted_at IS NOT NULL FROM items")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?;
            let mut items = Vec::new();
            for row in rows {
                let (key, csl, deleted) = row?;
                items.push(ExistingItem {
                    key,
                    csl: serde_json::from_str(&csl)?,
                    deleted,
                });
            }
            items
        };
        let mut objects = Vec::new();
        let has_objects: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='import_objects')", [], |r| r.get(0))?;
        if has_objects {
            let mut stmt = tx.prepare("SELECT source_id, object_id, scope, target_key FROM import_objects WHERE kind='item'")?;
            objects = stmt
                .query_map([], |r| {
                    let scope: String = r.get(2)?;
                    Ok(ExistingObject {
                        source_id: r.get(0)?,
                        object_id: r.get(1)?,
                        input_sha256: (!scope.is_empty()).then_some(scope),
                        item_key: r.get(3)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
        }
        tx.rollback()?;
        Ok(Self { items, objects })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ItemAction {
    New,
    AddMissing {
        item_key: String,
    },
    Review {
        candidates: Vec<String>,
        reason: String,
    },
    Blocked {
        candidates: Vec<String>,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AttachmentPlan {
    Existing {
        id: String,
        target_key: String,
        path: PathBuf,
        fingerprint: FileFingerprint,
        source_file: Option<(PathBuf, FileFingerprint)>,
    },
    Available {
        id: String,
        path: PathBuf,
        fingerprint: FileFingerprint,
    },
    Missing {
        id: String,
        path: String,
    },
    Ambiguous {
        id: String,
        candidates: Vec<PathBuf>,
    },
    Unsupported {
        id: String,
        path: String,
        reason: String,
    },
    Failed {
        id: String,
        path: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedItem {
    pub object_id: String,
    #[serde(default)]
    pub title: String,
    pub input_sha256: Option<String>,
    pub action: ItemAction,
    #[serde(default)]
    pub candidate_details: Vec<super::decisions::Candidate>,
    #[serde(default)]
    pub allow_new: bool,
    #[serde(default)]
    pub register_new: bool,
    #[serde(default)]
    pub selection: Option<super::decisions::Selection>,
    #[serde(default)]
    pub selected_files: Vec<String>,
    #[serde(default)]
    pub file_options: Vec<super::decisions::FileChoice>,
    pub attachments: Vec<AttachmentPlan>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanCounts {
    pub new_items: usize,
    pub existing_items: usize,
    pub review: usize,
    pub blocked: usize,
    pub folders: usize,
    pub notes: usize,
    pub tags: usize,
    pub available_pdfs: usize,
    pub existing_pdfs: usize,
    pub missing_pdfs: usize,
    pub ambiguous_pdfs: usize,
    pub unsupported_attachments: usize,
    pub failed_pdfs: usize,
    /// Upper bound before per-item attachment deduplication in apply.
    pub copy_bytes_upper_bound: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPlan {
    pub source_id: String,
    pub input_sha256: String,
    pub adapter_version: String,
    pub input_model_sha256: String,
    pub managed_storage: Option<PathBuf>,
    pub attachment_mode: AttachmentMode,
    pub items: Vec<PlannedItem>,
    pub counts: PlanCounts,
    pub warnings: Vec<String>,
    /// 対応の要らない注意（ImportInput.notices）
    #[serde(default)]
    pub notices: Vec<String>,
}

#[derive(Default)]
struct IdentityIndex {
    doi: HashMap<String, BTreeSet<String>>,
    pmid: HashMap<String, BTreeSet<String>>,
    descriptive: HashMap<String, BTreeSet<String>>,
}

pub(crate) fn doi(csl: &Value) -> Option<String> {
    let d = model::normalize_doi(model::s(csl, "DOI")?);
    let (prefix, suffix) = d.split_once('/')?;
    let registrant = prefix.strip_prefix("10.")?;
    let valid = registrant.len() >= 4
        && registrant.bytes().all(|c| c.is_ascii_digit())
        && !suffix.is_empty()
        && !d.chars().any(char::is_whitespace);
    valid.then_some(d)
}

pub(crate) fn pmid(csl: &Value) -> Option<String> {
    if let Some(number) = csl.get("PMID").and_then(Value::as_u64) {
        return (number > 0).then(|| number.to_string());
    }
    let raw = model::s(csl, "PMID")?.trim();
    // IDs are strings; do not silently match invalid values or overflow integers.
    if raw.is_empty() || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let normalized = raw.trim_start_matches('0');
    (!normalized.is_empty()).then(|| normalized.to_string())
}

/// Native SQLite expressions keep old binaries able to maintain these indexes.
/// Unicode identifiers/whitespace use the small unusual-identifiers index and
/// the same Rust normalizer as preview; SQLite's lower() is ASCII-only.
pub(crate) fn identifier_sql() -> (String, String) {
    let whitespace = "char(9)||char(10)||char(11)||char(12)||char(13)||char(32)";
    let raw = format!("lower(trim(doi,{whitespace}))");
    let mut expression = "CASE".to_string();
    for prefix in model::DOI_PREFIXES {
        expression.push_str(&format!(
            " WHEN {raw} LIKE '{prefix}%' THEN trim(substr({raw},{}),{whitespace})",
            prefix.len() + 1
        ));
    }
    expression.push_str(&format!(" ELSE {raw} END"));
    (expression, format!("ltrim(trim(pmid,{whitespace}),'0')"))
}

pub(crate) fn create_identity_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_import_legacy_paperpile ON items(json_extract(csl,'$.custom.paperpile.id'));")?;
    let (doi, pmid) = identifier_sql();
    conn.execute_batch(&format!("CREATE INDEX idx_import_doi ON items({doi}) WHERE doi IS NOT NULL; CREATE INDEX idx_import_pmid ON items({pmid}) WHERE pmid IS NOT NULL; CREATE INDEX idx_import_unusual_ids ON items(key) WHERE doi GLOB '*[^ -~]*' OR pmid GLOB '*[^ -~]*';"))?;
    conn.execute_batch("CREATE TABLE import_match_keys(item_id INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,fingerprint TEXT,valid INTEGER NOT NULL DEFAULT 0); CREATE INDEX idx_import_match_keys ON import_match_keys(fingerprint) WHERE valid=1; CREATE INDEX idx_import_match_pending ON import_match_keys(item_id) WHERE valid=0; CREATE TRIGGER import_match_insert AFTER INSERT ON items BEGIN INSERT INTO import_match_keys(item_id,valid) VALUES(new.id,0) ON CONFLICT(item_id) DO UPDATE SET valid=0,fingerprint=NULL; END; CREATE TRIGGER import_match_invalidate AFTER UPDATE OF csl ON items BEGIN INSERT INTO import_match_keys(item_id,valid) VALUES(new.id,0) ON CONFLICT(item_id) DO UPDATE SET valid=0,fingerprint=NULL; END;")?;
    let rows: Vec<(i64, String)> = conn
        .prepare("SELECT id,csl FROM items")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for (id, csl) in rows {
        index_descriptive(conn, id, &serde_json::from_str(&csl)?)?;
    }
    Ok(())
}

pub(crate) fn index_descriptive(conn: &Connection, id: i64, csl: &Value) -> rusqlite::Result<()> {
    // During v2/v6 backfill the derived table does not exist yet.
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='import_match_keys')",
        [],
        |r| r.get(0),
    )?;
    if exists {
        let fingerprint = descriptive(csl).map(|s| super::files::digest(s.as_bytes()));
        conn.execute("INSERT INTO import_match_keys(item_id,fingerprint,valid) VALUES(?1,?2,1) ON CONFLICT(item_id) DO UPDATE SET fingerprint=excluded.fingerprint,valid=1",rusqlite::params![id,fingerprint])?;
    }
    Ok(())
}

pub(crate) fn descriptive(csl: &Value) -> Option<String> {
    let title = model::journal_norm(model::s(csl, "title")?);
    let authors = model::authors(csl);
    let year = model::year(csl)?;
    if title.is_empty() || authors.is_empty() {
        return None;
    }
    Some(
        serde_json::to_string(&(
            title,
            authors
                .iter()
                .map(|s| model::journal_norm(s))
                .collect::<Vec<_>>(),
            year,
        ))
        .unwrap(),
    )
}

impl IdentityIndex {
    fn add(&mut self, key: &str, csl: &Value) {
        for (index, value) in [
            (&mut self.doi, doi(csl)),
            (&mut self.pmid, pmid(csl)),
            (&mut self.descriptive, descriptive(csl)),
        ] {
            if let Some(value) = value {
                index.entry(value).or_default().insert(key.to_string());
            }
        }
    }

    fn strong(&self, csl: &Value) -> BTreeSet<String> {
        let mut candidates = BTreeSet::new();
        for (index, value) in [(&self.doi, doi(csl)), (&self.pmid, pmid(csl))] {
            if let Some(found) = value.and_then(|v| index.get(&v)) {
                candidates.extend(found.iter().cloned());
            }
        }
        candidates
    }
}

pub(crate) fn conflicts(a: &Value, b: &Value) -> bool {
    [(doi(a), doi(b)), (pmid(a), pmid(b))]
        .into_iter()
        .any(|(a, b)| matches!((a, b), (Some(a), Some(b)) if a != b))
}

pub(crate) fn validate(input: &ImportInput) -> Result<()> {
    if input.source.id.trim().is_empty()
        || input.source.format.trim().is_empty()
        || input.adapter_version.trim().is_empty()
    {
        return Err(CoreError::BadInput(
            "移行元 ID・形式・変換器版が必要です".into(),
        ));
    }
    if input.input_sha256.len() != 64 || !input.input_sha256.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(CoreError::BadInput("入力の SHA-256 が必要です".into()));
    }
    let mut folders = HashMap::new();
    for folder in &input.folders {
        if folder.id.is_empty() || folders.insert(folder.id.as_str(), folder).is_some() {
            return Err(CoreError::BadInput(
                "フォルダ ID が空または重複しています".into(),
            ));
        }
    }
    // Walk each graph edge once, including large/deep exports, without recursion.
    let mut checked = HashSet::new();
    for folder in &input.folders {
        let mut visiting = HashSet::new();
        let mut next = Some(folder.id.as_str());
        while let Some(id) = next {
            if checked.contains(id) {
                break;
            }
            if !visiting.insert(id) {
                return Err(CoreError::BadInput(format!(
                    "フォルダの親子関係が循環しています: {id}"
                )));
            }
            let node = folders
                .get(id)
                .ok_or_else(|| CoreError::BadInput(format!("参照先フォルダがありません: {id}")))?;
            next = node.parent_id.as_deref();
        }
        checked.extend(visiting);
    }
    let mut label_names = HashMap::new();
    for item in &input.items {
        if item.id.is_empty() || !item.csl.is_object() {
            return Err(CoreError::BadInput(
                "文献 ID が空、または CSL がオブジェクトではありません".into(),
            ));
        }
        for ids in [
            item.tags.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            item.notes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            item.attachments.iter().map(|a| a.id.as_str()).collect(),
        ] {
            let mut seen = HashSet::new();
            if ids.iter().any(|id| id.is_empty() || !seen.insert(*id)) {
                return Err(CoreError::BadInput(format!(
                    "ノートまたは添付の ID が空または重複しています: {}",
                    item.id
                )));
            }
        }
        for tag in &item.tags {
            if label_names
                .insert((&tag.id, tag.stable_id), &tag.name)
                .is_some_and(|name| name != &tag.name)
            {
                return Err(CoreError::BadInput(format!(
                    "同じタグ ID に複数の名前があります: {}",
                    tag.id
                )));
            }
        }
        for id in &item.folder_ids {
            if !folders.contains_key(id.as_str()) {
                return Err(CoreError::BadInput(format!(
                    "所属先フォルダがありません: {id}"
                )));
            }
        }
    }
    Ok(())
}

/// Preview is a pure application-layer read: its database inventory is supplied
/// explicitly, so there is no path that accidentally opens/creates/migrates a DB.
/// It is advisory; apply must verify fingerprints and re-match inside a short
/// IMMEDIATE transaction before creating or updating any objects.
pub fn preview(
    input: &ImportInput,
    library: &ExistingLibrary,
    roots: &AttachmentRoots,
    mode: AttachmentMode,
) -> Result<ImportPlan> {
    preview_controlled(input, library, roots, mode, &AtomicBool::new(false), |_| {})
}

pub fn preview_controlled(
    input: &ImportInput,
    library: &ExistingLibrary,
    roots: &AttachmentRoots,
    mode: AttachmentMode,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&super::ImportProgress),
) -> Result<ImportPlan> {
    validate(input)?;
    let existing: HashMap<_, _> = library.items.iter().map(|i| (i.key.clone(), i)).collect();
    let mut index = IdentityIndex::default();
    for item in &library.items {
        index.add(&item.key, &item.csl);
    }
    let mut incoming = IdentityIndex::default();
    let mut legacy: HashMap<&str, BTreeSet<String>> = HashMap::new();
    if input.adopt_legacy_paperpile_ids {
        if input.source.format != "paperpile" {
            return Err(CoreError::BadInput(
                "旧 Paperpile ID の照合は Paperpile 入力だけに使用できます".into(),
            ));
        }
        for item in &library.items {
            if let Some(id) = item
                .csl
                .pointer("/custom/paperpile/id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                legacy.entry(id).or_default().insert(item.key.clone());
            }
        }
    }
    let mut id_counts = HashMap::new();
    for item in &input.items {
        incoming.add(&item.id, &item.csl);
        *id_counts.entry(&item.id).or_insert(0) += 1;
    }
    // Source identity is independent of export filename and item identifiers.
    let mut mappings: HashMap<(&str, Option<&str>), BTreeSet<String>> = HashMap::new();
    for obj in &library.objects {
        if obj.source_id == input.source.id {
            mappings
                .entry((&obj.object_id, obj.input_sha256.as_deref()))
                .or_default()
                .insert(obj.item_key.clone());
        }
    }
    let mut counts = PlanCounts {
        folders: input.folders.len(),
        ..Default::default()
    };
    let mut items = Vec::with_capacity(input.items.len());
    for (ordinal, item) in input.items.iter().enumerate() {
        progress(&super::ImportProgress {
            phase: "preview",
            completed: ordinal,
            total: input.items.len(),
            bytes: 0,
        });
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Conflict("事前確認を中止しました".into()));
        }
        let scope = (!item.stable_id).then_some(input.input_sha256.as_str());
        let mut mapped = mappings
            .get(&(item.id.as_str(), scope))
            .cloned()
            .unwrap_or_default();
        if input.adopt_legacy_paperpile_ids {
            if let Some(id) = item
                .csl
                .pointer("/custom/paperpile/id")
                .and_then(Value::as_str)
            {
                if let Some(keys) = legacy.get(id) {
                    mapped.extend(keys.clone());
                }
            }
        }
        let strong = index.strong(&item.csl);
        let candidates: BTreeSet<_> = mapped.union(&strong).cloned().collect();
        let mut overlap = incoming.strong(&item.csl);
        if let Some(found) = descriptive(&item.csl).and_then(|d| incoming.descriptive.get(&d)) {
            overlap.extend(found.iter().cloned());
        }
        let input_overlap: Vec<_> = overlap.into_iter().filter(|id| id != &item.id).collect();
        let no_existing_identity = candidates.is_empty();
        let action = if id_counts[&item.id] > 1 {
            ItemAction::Blocked {
                candidates: Vec::new(),
                reason: "入力内のオブジェクト ID が重複しています".into(),
            }
        } else if candidates.len() > 1 {
            ItemAction::Blocked {
                candidates: candidates.into_iter().collect(),
                reason: "元 ID・DOI・PMID が複数の文献を指しています".into(),
            }
        } else if let Some(key) = candidates.iter().next() {
            match existing.get(key) {
                None => ItemAction::Blocked {
                    candidates: vec![key.clone()],
                    reason: "以前取り込んだ文献が削除されています。再作成には明示操作が必要です"
                        .into(),
                },
                Some(old) if old.deleted => ItemAction::Blocked {
                    candidates: vec![key.clone()],
                    reason: "既存文献はゴミ箱にあります。自動復元しません".into(),
                },
                Some(old) if conflicts(&item.csl, &old.csl) => ItemAction::Blocked {
                    candidates: vec![key.clone()],
                    reason: "既存文献と DOI または PMID が矛盾しています".into(),
                },
                Some(_) => ItemAction::AddMissing {
                    item_key: key.clone(),
                },
            }
        } else if !input_overlap.is_empty() {
            ItemAction::Review {
                candidates: input_overlap,
                reason: "入力内で識別子または題名・著者・年を共有する文献があります".into(),
            }
        } else if let Some(found) = descriptive(&item.csl).and_then(|v| index.descriptive.get(&v)) {
            ItemAction::Review {
                candidates: found.iter().cloned().collect(),
                reason: "題名・著者・年が一致する候補です。自動統合しません".into(),
            }
        } else {
            ItemAction::New
        };
        match &action {
            ItemAction::New => counts.new_items += 1,
            ItemAction::AddMissing { .. } => counts.existing_items += 1,
            ItemAction::Review { .. } => counts.review += 1,
            ItemAction::Blocked { .. } => counts.blocked += 1,
        }
        counts.notes += item.notes.len();
        counts.tags += item.tags.len();
        let mut attachments = Vec::new();
        for att in &item.attachments {
            let pdf = att
                .media_type
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("application/pdf"))
                .unwrap_or_else(|| att.path.to_lowercase().ends_with(".pdf"));
            let planned = if !pdf {
                counts.unsupported_attachments += 1;
                AttachmentPlan::Unsupported {
                    id: att.id.clone(),
                    path: att.path.clone(),
                    reason: "PDF 以外の添付は今回の移行対象外です".into(),
                }
            } else {
                let paths: Vec<_> = std::iter::once(att.path.as_str())
                    .chain(att.alternative_paths.iter().map(String::as_str))
                    .collect();
                match roots.resolve_candidates(&paths) {
                    Ok(ResolvedPath::Missing) => {
                        counts.missing_pdfs += 1;
                        AttachmentPlan::Missing {
                            id: att.id.clone(),
                            path: att.path.clone(),
                        }
                    }
                    Ok(ResolvedPath::Ambiguous { candidates }) => {
                        counts.ambiguous_pdfs += 1;
                        AttachmentPlan::Ambiguous {
                            id: att.id.clone(),
                            candidates,
                        }
                    }
                    resolved => {
                        let inspection = match resolved {
                            Ok(ResolvedPath::Rejected { reason }) => {
                                Err(CoreError::BadInput(reason))
                            }
                            Err(e) => Err(e),
                            Ok(ResolvedPath::Found { path }) => {
                                roots.inspect_controlled(&path, cancel, &mut |bytes| {
                                    progress(&super::ImportProgress {
                                        phase: "preview",
                                        completed: ordinal,
                                        total: input.items.len(),
                                        bytes,
                                    })
                                })
                            }
                            _ => unreachable!("missing and ambiguous handled above"),
                        };
                        match inspection {
                            Ok((path, fingerprint)) => {
                                counts.available_pdfs += 1;
                                if mode == AttachmentMode::Copy {
                                    counts.copy_bytes_upper_bound += fingerprint.bytes;
                                }
                                AttachmentPlan::Available {
                                    id: att.id.clone(),
                                    path,
                                    fingerprint,
                                }
                            }
                            Err(e) => {
                                counts.failed_pdfs += 1;
                                AttachmentPlan::Failed {
                                    id: att.id.clone(),
                                    path: att.path.clone(),
                                    reason: e.to_string(),
                                }
                            }
                        }
                    }
                }
            };
            attachments.push(planned);
        }
        items.push(PlannedItem {
            title: crate::model::s(&item.csl, "title").unwrap_or("").into(),
            object_id: item.id.clone(),
            input_sha256: scope.map(str::to_string),
            allow_new: matches!(&action, ItemAction::Review { .. }) && no_existing_identity,
            register_new: false,
            action,
            candidate_details: Vec::new(),
            selection: None,
            selected_files: Vec::new(),
            file_options: Vec::new(),
            attachments,
            warnings: item.warnings.clone(),
        });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Conflict("事前確認を中止しました".into()));
    }
    Ok(ImportPlan {
        source_id: input.source.id.clone(),
        input_sha256: input.input_sha256.clone(),
        adapter_version: input.adapter_version.clone(),
        input_model_sha256: super::files::digest(&serde_json::to_vec(input)?),
        managed_storage: None,
        attachment_mode: mode,
        items,
        counts,
        warnings: input.warnings.clone(),
        notices: input.notices.clone(),
    })
}

/// Shared by migration and automatic ingestion, under their writer transaction.
pub(crate) fn identifier_candidates(
    conn: &Connection,
    csl: &Value,
    mapped: Option<&str>,
) -> Result<Vec<(String, Value, bool)>> {
    let doi = self::doi(csl);
    let pmid = self::pmid(csl);
    if mapped.is_none() && doi.is_none() && pmid.is_none() {
        return Ok(Vec::new());
    }
    let (doi_sql, pmid_sql) = identifier_sql();
    let sql=format!("SELECT key,csl,deleted_at IS NOT NULL FROM items WHERE key=?1 OR (doi IS NOT NULL AND {doi_sql}=?2) OR (pmid IS NOT NULL AND {pmid_sql}=?3)");
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut candidates: Vec<(String, Value, bool)> = stmt
        .query_map(rusqlite::params![mapped, doi, pmid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?
        .map(|r| {
            let (key, csl, deleted) = r?;
            Ok((key, serde_json::from_str(&csl)?, deleted))
        })
        .collect::<Result<_>>()?;
    let mut unusual=conn.prepare_cached("SELECT key,csl,deleted_at IS NOT NULL FROM items WHERE doi GLOB '*[^ -~]*' OR pmid GLOB '*[^ -~]*'")?;
    for row in unusual.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, bool>(2)?,
        ))
    })? {
        let (key, csl, deleted) = row?;
        let csl: Value = serde_json::from_str(&csl)?;
        if !candidates.iter().any(|c| c.0 == key)
            && ((doi.is_some() && doi == self::doi(&csl))
                || (pmid.is_some() && pmid == self::pmid(&csl)))
        {
            candidates.push((key, csl, deleted));
        }
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SQL の索引の式（items.doi = 生成列で小文字済み）と Rust の正規化が同じ値を出す
    #[test]
    fn identifier_sql_matches_rust_normalizer() {
        let conn = Connection::open_in_memory().unwrap();
        let (doi_sql, pmid_sql) = identifier_sql();
        for raw in [
            "10.1148/rg.230216",
            " 10.1148/RG.230216 ",
            "https://doi.org/10.1148/rg.230216",
            "HTTP://DOI.ORG/10.1148/rg.230216",
            "https://dx.doi.org/ 10.1148/rg.230216",
            "http://dx.doi.org/10.1148/rg.230216",
            "doi: 10.1148/rg.230216",
            "https://doi.org/doi:10.1/x",
            "https://www.doi.org/10.1/x",
        ] {
            let sql: String = conn
                .query_row(&format!("SELECT {doi_sql} FROM (SELECT lower(?1) AS doi)"), [raw], |r| r.get(0))
                .unwrap();
            assert_eq!(sql, model::normalize_doi(raw), "{raw:?}");
        }
        for (raw, want) in [("00123", "123"), (" 123 ", "123")] {
            let sql: String = conn
                .query_row(&format!("SELECT {pmid_sql} FROM (SELECT ?1 AS pmid)"), [raw], |r| r.get(0))
                .unwrap();
            assert_eq!(sql, want);
            assert_eq!(pmid(&serde_json::json!({ "PMID": raw })).as_deref(), Some(want));
        }
    }
}
