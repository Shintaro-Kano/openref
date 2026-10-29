//! Short per-item transactions, explicit managed storage, and durable import
//! provenance. Copying/hashing and progress callbacks run outside DB locks.
use super::{
    files::{AttachmentRoots, FileFingerprint},
    inventory,
    plan::{self, AttachmentPlan, ImportPlan, ItemAction},
    AttachmentMode, ImportInput, ImportItem,
};
use crate::{CoreError, Db, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use super::files::digest;

fn storage_path(db_path: &Path) -> Result<PathBuf> {
    let absolute = if db_path.try_exists()? {
        db_path.canonicalize()?
    } else {
        std::path::absolute(db_path)?
    };
    let mut parent = absolute
        .parent()
        .ok_or_else(|| CoreError::BadInput("DB の親フォルダがありません".into()))?
        .to_path_buf();
    let mut suffix = Vec::new();
    while !parent.try_exists()? {
        suffix.push(
            parent
                .file_name()
                .ok_or_else(|| CoreError::BadInput("DB パスが不正です".into()))?
                .to_os_string(),
        );
        parent.pop();
    }
    parent = parent.canonicalize()?;
    for part in suffix.into_iter().rev() {
        parent.push(part);
    }
    Ok(parent.join("storage"))
}

pub fn prepare(
    db_path: &Path,
    input: &ImportInput,
    roots: &AttachmentRoots,
    mode: AttachmentMode,
) -> Result<ImportPlan> {
    prepare_controlled(db_path, input, roots, mode, &AtomicBool::new(false), |_| {})
}

pub fn prepare_controlled(
    db_path: &Path,
    input: &ImportInput,
    roots: &AttachmentRoots,
    mode: AttachmentMode,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> Result<ImportPlan> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Conflict("事前確認を中止しました".into()));
    }
    progress(&Progress {
        phase: "database",
        completed: 0,
        total: input.items.len(),
        bytes: 0,
    });
    roots.protect_database(db_path);
    let (library, saved, prior) = inventory::read_import(db_path, input)?;
    let mut plan = plan::preview_controlled(input, &library, roots, mode, cancel, progress)?;
    let exact: std::collections::HashSet<_> = library
        .objects
        .iter()
        .filter(|o| {
            o.source_id == input.source.id
                && (o.input_sha256.is_none()
                    || o.input_sha256.as_deref() == Some(&input.input_sha256))
        })
        .map(|o| o.object_id.as_str())
        .collect();
    let live: std::collections::HashSet<_> = library
        .items
        .iter()
        .filter(|i| !i.deleted)
        .map(|i| i.key.as_str())
        .collect();
    for (item, planned) in input.items.iter().zip(&mut plan.items) {
        if let ItemAction::AddMissing { item_key } = &planned.action {
            if library
                .objects
                .iter()
                .any(|o| o.item_key == *item_key && o.source_id != input.source.id)
            {
                planned.warnings.push("別の移行元が取り込んだ文献への追加です。同じライブラリの再書出しなら以前の移行元を選んでください。別移行元のノート・所属は別のデータとして扱われます".into());
            }
        }
        if !exact.contains(item.id.as_str())
            && matches!(
                planned.action,
                ItemAction::New | ItemAction::AddMissing { .. } | ItemAction::Review { .. }
            )
        {
            if let Some(action) = prior.action(item, |key| Ok(live.contains(key)))? {
                match planned.action {
                    ItemAction::New => plan.counts.new_items -= 1,
                    ItemAction::Review { .. } => plan.counts.review -= 1,
                    _ => plan.counts.existing_items -= 1,
                }
                match action {
                    ItemAction::Blocked { .. } => plan.counts.blocked += 1,
                    _ => plan.counts.review += 1,
                }
                planned.allow_new = false;
                planned.action = action;
            }
        }
    }
    if input
        .folders
        .iter()
        .any(|f| prior.folder_needs_review(f) && prior.target("folder", &f.id, &f.name).is_none())
    {
        plan.warnings
            .push("別の書出しのフォルダ対応候補は自動作成せず保留します".into());
    }
    super::decisions::describe(&mut plan, &library);
    plan.managed_storage = Some(storage_path(db_path)?);
    reuse_managed(input, &mut plan, saved, cancel)?;
    Ok(plan)
}

fn reuse_managed(
    input: &ImportInput,
    plan: &mut ImportPlan,
    saved: Vec<inventory::ManagedAttachment>,
    cancel: &AtomicBool,
) -> Result<()> {
    let storage = plan.managed_storage.as_ref().unwrap();
    if !std::fs::symlink_metadata(storage).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        return Ok(());
    }
    let roots = AttachmentRoots::new(std::slice::from_ref(storage))?;
    let saved: HashMap<_, _> = saved
        .into_iter()
        .map(|a| ((a.scope.clone(), a.object_id.clone()), a))
        .collect();
    for (source, planned) in input.items.iter().zip(&mut plan.items) {
        let ItemAction::AddMissing { item_key } = &planned.action else {
            continue;
        };
        for (att, decision) in source.attachments.iter().zip(&mut planned.attachments) {
            if cancel.load(Ordering::Relaxed) {
                return Err(CoreError::Conflict("事前確認を中止しました".into()));
            }
            if !matches!(
                decision,
                AttachmentPlan::Missing { .. } | AttachmentPlan::Available { .. }
            ) {
                continue;
            }
            let key = (
                scope(input, source.stable_id && att.stable_id).to_string(),
                child_id(&source.id, &att.id),
            );
            let Some(saved) = saved.get(&key).filter(|a| &a.item_key == item_key) else {
                continue;
            };
            let inspected = roots.inspect_controlled(&saved.path, cancel, &mut |_| {});
            let Ok((path, fingerprint)) = inspected else {
                continue;
            };
            let source_file = match decision {
                AttachmentPlan::Available {
                    path, fingerprint, ..
                } => Some((path.clone(), fingerprint.clone())),
                _ => None,
            };
            match decision {
                AttachmentPlan::Available {
                    fingerprint: incoming,
                    ..
                } if incoming != &fingerprint => continue,
                AttachmentPlan::Available {
                    fingerprint: incoming,
                    ..
                } => {
                    if plan.attachment_mode == AttachmentMode::Copy {
                        plan.counts.copy_bytes_upper_bound -= incoming.bytes;
                    }
                }
                AttachmentPlan::Missing { .. } => {
                    plan.counts.missing_pdfs -= 1;
                    plan.counts.available_pdfs += 1;
                }
                _ => unreachable!(),
            }
            plan.counts.existing_pdfs += 1;
            *decision = AttachmentPlan::Existing {
                id: att.id.clone(),
                target_key: saved.target_key.clone(),
                path,
                fingerprint,
                source_file,
            };
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Conflict("事前確認を中止しました".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentResult {
    pub id: String,
    pub status: String,
    pub target_key: Option<String>,
    pub detail: Option<String>,
    pub fingerprint: Option<FileFingerprint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemResult {
    pub object_id: String,
    pub status: String,
    pub item_key: Option<String>,
    pub attachments: Vec<AttachmentResult>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub run_id: String,
    pub source_id: String,
    pub state: String,
    pub backup_path: Option<PathBuf>,
    pub total: usize,
    pub results: Vec<ItemResult>,
    pub warnings: Vec<String>,
    pub complete: bool,
    pub fulltext_pending: usize,
}

pub use super::ImportProgress as Progress;

/// OS advisory lock is released on process death; the persistent lock file must
/// not be unlinked (a new inode would allow a concurrent second lock).
pub struct JobLock {
    _file: std::fs::File,
}
impl Drop for JobLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // close alone leaves flock held by an unrelated concurrent fork
            // until that child execs/exits. The job ends here, explicitly unlock.
            unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN); }
        }
    }
}
impl JobLock {
    pub fn acquire(db_path: &Path) -> Result<Self> {
        let absolute = if db_path.try_exists()? {
            db_path.canonicalize()?
        } else {
            std::path::absolute(db_path)?
        };
        let parent = absolute.parent().unwrap();
        std::fs::create_dir_all(parent)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(parent.join(".openref-import.lock"))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: file owns a live fd; flock does not transfer ownership.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(CoreError::Conflict(
                    "この DB では別のライブラリ移行が実行中です".into(),
                ));
            }
        }
        #[cfg(not(unix))]
        {
            return Err(CoreError::BadInput(
                "この OS の移行ジョブロックは未対応です".into(),
            ));
        }
        #[allow(unreachable_code)]
        Ok(Self { _file: file })
    }
}

fn scope(input: &ImportInput, stable: bool) -> &str {
    if stable {
        ""
    } else {
        &input.input_sha256
    }
}
fn child_id(item: &str, child: &str) -> String {
    json!([item, child]).to_string()
}

fn object(
    conn: &Connection,
    input: &ImportInput,
    kind: &str,
    scope: &str,
    id: &str,
) -> Result<Option<(String, String)>> {
    Ok(conn.query_row("SELECT target_key,content_sha256 FROM import_objects WHERE source_id=?1 AND kind=?2 AND scope=?3 AND object_id=?4", params![input.source.id,kind,scope,id], |r|Ok((r.get(0)?,r.get(1)?))).optional()?)
}

fn remember(
    conn: &Connection,
    input: &ImportInput,
    kind: &str,
    scope: &str,
    id: &str,
    key: &str,
    raw: &Value,
    hash: &str,
) -> Result<()> {
    conn.execute("INSERT INTO import_objects(source_id,kind,scope,object_id,target_key,raw_json,content_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(source_id,kind,scope,object_id) DO UPDATE SET target_key=excluded.target_key,raw_json=excluded.raw_json,content_sha256=excluded.content_sha256",params![input.source.id,kind,scope,id,key,raw.to_string(),hash])?;
    Ok(())
}

/// Re-evaluate all strong matches, tombstones and weak candidates while the
/// caller holds IMMEDIATE. The transaction never reads attachment file bytes.
fn match_item(conn: &Connection, input: &ImportInput, item: &ImportItem) -> Result<ItemAction> {
    let mapped = object(conn, input, "item", scope(input, item.stable_id), &item.id)?.map(|v| v.0);
    let doi = plan::doi(&item.csl);
    let pmid = plan::pmid(&item.csl);
    let mut candidates = plan::identifier_candidates(conn, &item.csl, mapped.as_deref())?;
    if input.adopt_legacy_paperpile_ids && input.source.format == "paperpile" {
        if let Some(id) = item
            .csl
            .pointer("/custom/paperpile/id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            let mut stmt=conn.prepare_cached("SELECT key,csl,deleted_at IS NOT NULL FROM items WHERE json_extract(csl,'$.custom.paperpile.id')=?1")?;
            for row in stmt.query_map([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })? {
                let (key, csl, deleted) = row?;
                if !candidates.iter().any(|c| c.0 == key) {
                    candidates.push((key, serde_json::from_str(&csl)?, deleted));
                }
            }
        }
    }
    if let Some(key) = &mapped {
        if !candidates.iter().any(|c| &c.0 == key) {
            return Ok(ItemAction::Blocked {
                candidates: vec![key.clone()],
                reason: "以前の移行先文献が削除されています".into(),
            });
        }
    }
    if candidates.len() > 1 {
        return Ok(ItemAction::Blocked {
            candidates: candidates.iter().map(|c| c.0.clone()).collect(),
            reason: "書込直前の DOI・PMID・元 ID の照合で複数の文献が見つかりました".into(),
        });
    }
    if let Some((key, csl, deleted)) = candidates.first() {
        let conflict = [(doi, plan::doi(csl)), (pmid, plan::pmid(csl))]
            .into_iter()
            .any(|(a, b)| matches!((a,b),(Some(a),Some(b)) if a!=b));
        return Ok(if *deleted || conflict {
            ItemAction::Blocked {
                candidates: vec![key.clone()],
                reason: "書込直前に識別子の矛盾またはゴミ箱の文献を検出しました".into(),
            }
        } else {
            ItemAction::AddMissing {
                item_key: key.clone(),
            }
        });
    }
    if let Some(description) = plan::descriptive(&item.csl) {
        let mut stmt=conn.prepare_cached("SELECT i.key,i.csl FROM import_match_keys m JOIN items i ON i.id=m.item_id WHERE m.valid=1 AND m.fingerprint=?1 UNION ALL SELECT i.key,i.csl FROM import_match_keys m INDEXED BY idx_import_match_pending CROSS JOIN items i ON i.id=m.item_id WHERE m.valid=0 AND i.year=?2")?;
        let mut weak = Vec::new();
        for row in stmt.query_map(
            params![
                digest(description.as_bytes()),
                crate::model::year(&item.csl)
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )? {
            let (key, csl) = row?;
            if plan::descriptive(&serde_json::from_str(&csl)?) == Some(description.clone()) {
                weak.push(key);
            }
        }
        if !weak.is_empty() {
            return Ok(ItemAction::Review {
                candidates: weak,
                reason: "書込直前に題名・著者・年の一致候補を検出しました".into(),
            });
        }
    }
    Ok(ItemAction::New)
}

fn union_keys(target: &mut Value, incoming: &Value) {
    if let Some(fields) = incoming.as_object() {
        if !target.is_object() {
            *target = json!({});
        }
        for (key, value) in fields {
            union_keys(
                target
                    .as_object_mut()
                    .unwrap()
                    .entry(key.clone())
                    .or_insert(Value::Null),
                value,
            );
        }
    } else {
        *target = json!(true);
    }
}
// Only pre-key-history imports need their full saved inputs. Parse each distinct
// export once, outside the per-item writer transactions. Once a bibliography
// row exists, it already contains the accumulated legacy keys for that target.
fn legacy_bibliography(conn: &Connection) -> Result<HashMap<String, Value>> {
    let mut objects: HashMap<(String, String, String), String> = HashMap::new();
    let mut stmt = conn.prepare("SELECT source_id,scope,object_id,target_key FROM import_objects o WHERE kind='item' AND NOT EXISTS(SELECT 1 FROM import_objects b WHERE b.kind='bibliography' AND b.target_key=o.target_key)")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })? {
        let (source, scope, id, key) = row?;
        objects.insert((source, scope, id), key);
    }
    let mut result = HashMap::new();
    if objects.is_empty() {
        return Ok(result);
    }
    let mut seen = std::collections::HashSet::new();
    let mut runs = conn.prepare("SELECT source_id,input_sha256,input_json FROM import_runs")?;
    for row in runs.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (source, sha, raw) = row?;
        if !seen.insert((source.clone(), sha.clone())) {
            continue;
        }
        let prior: ImportInput = serde_json::from_str(&raw)?;
        for item in prior.items {
            for scope in [&sha, ""] {
                if let Some(key) =
                    objects.get(&(source.clone(), scope.to_string(), item.id.clone()))
                {
                    union_keys(
                        result.entry(key.clone()).or_insert_with(|| json!({})),
                        &item.csl,
                    );
                }
            }
        }
    }
    Ok(result)
}

fn applied_bibliography(
    conn: &Connection,
    legacy: &HashMap<String, Value>,
    key: &str,
) -> Result<Value> {
    let mut result = legacy.get(key).cloned().unwrap_or_else(|| json!({}));
    let mut stmt = conn.prepare(
        "SELECT raw_json FROM import_objects WHERE kind='bibliography' AND target_key=?1",
    )?;
    for raw in stmt.query_map([key], |r| r.get::<_, String>(0))? {
        union_keys(&mut result, &serde_json::from_str::<Value>(&raw?)?);
    }
    Ok(result)
}

fn fill_missing(existing: &mut Value, incoming: &Value, applied: &Value) -> bool {
    let (Some(old), Some(new)) = (existing.as_object_mut(), incoming.as_object()) else {
        return false;
    };
    let mut changed = false;
    for (key, value) in new {
        let prior = applied.get(key).unwrap_or(&Value::Null);
        if !prior.is_null()
            && old
                .get(key)
                .is_none_or(|v| v.is_null() || v == "" || v == &json!([]) || v == &json!({}))
        {
            continue;
        }
        match old.get_mut(key) {
            Some(v) if v.is_null() || v == "" || v == &json!([]) || v == &json!({}) => {
                if v != value {
                    *v = value.clone();
                    changed = true;
                }
            }
            Some(v)
                if v.is_object()
                    && value.is_object()
                    && !matches!(
                        key.as_str(),
                        "issued" | "accessed" | "event-date" | "original-date" | "submitted"
                    ) =>
            {
                changed |= fill_missing(v, value, prior)
            }
            None => {
                old.insert(key.clone(), value.clone());
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

struct PreparedPdf {
    id: String,
    key: String,
    path: PathBuf,
    sha256: String,
    md5: String,
    copied: bool,
    bytes: u64,
    parent: Option<super::storage::Dir>,
    identity: Option<(String, String)>,
}

fn copy_pdf(
    db: &Db,
    run: &str,
    roots: &AttachmentRoots,
    storage: &Path,
    id: &str,
    path: &Path,
    expected: &FileFingerprint,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(&Progress),
    completed: usize,
    total: usize,
) -> Result<PreparedPdf> {
    let key = crate::keys::new_key();
    let filename = path
        .file_name()
        .ok_or_else(|| CoreError::BadInput("PDF のファイル名がありません".into()))?;
    let dest = storage.join(&key).join(filename);
    let staging = storage
        .join(".imports")
        .join(run)
        .join(&key)
        .join("file.part");
    db.conn.execute("INSERT INTO import_files(run_id,key,path,staging_path,sha256,state) VALUES(?1,?2,?3,?4,?5,'planned')",params![run,key,dest.to_string_lossy(),staging.to_string_lossy(),expected.sha256])?;
    let storage_dir = super::storage::Dir::open(storage.parent().unwrap())?
        .child(storage.file_name().unwrap(), true)?;
    let imports_dir = storage_dir.child(std::ffi::OsStr::new(".imports"), true)?;
    let run_dir = imports_dir.child(std::ffi::OsStr::new(run), true)?;
    let stage_dir = run_dir.child(std::ffi::OsStr::new(&key), true)?;
    db.conn.execute(
        "UPDATE import_files SET owns_staging=1 WHERE key=?1",
        [&key],
    )?;
    let mut src = roots.open_selected_file(path)?;
    let mut dst = stage_dir.open_file(std::ffi::OsStr::new("file.part"), true)?;
    let file_identity = super::storage::identity(&dst.metadata()?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = dst.metadata()?;
        db.conn.execute(
            "UPDATE import_files SET file_device=?1,file_inode=?2 WHERE key=?3",
            params![m.dev().to_string(), m.ino().to_string(), key],
        )?;
    }
    let mut sha = Sha256::new();
    let mut md5 = md5::Context::new();
    let mut bytes = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Conflict("移行が中止されました".into()));
        }
        let n = src.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buffer[..n])?;
        sha.update(&buffer[..n]);
        md5.consume(&buffer[..n]);
        bytes += n as u64;
        progress(&Progress {
            phase: "copy",
            completed,
            total,
            bytes,
        });
    }
    let sha256 = format!("{:x}", sha.finalize());
    if bytes != expected.bytes || sha256 != expected.sha256 {
        return Err(CoreError::Conflict(
            "事前確認後に PDF が変更されました。コピーを採用しません".into(),
        ));
    }
    dst.sync_all()?;
    storage_dir.verify()?;
    stage_dir.verify()?;
    // Hash the descriptor we created, not a path that could have been replaced.
    use std::io::{Seek, SeekFrom};
    dst.seek(SeekFrom::Start(0))?;
    if super::files::fingerprint_reader(&mut dst)? != *expected {
        return Err(CoreError::Conflict(
            "コピー先 PDF のハッシュが一致しません".into(),
        ));
    }
    let current = stage_dir.open_file(std::ffi::OsStr::new("file.part"), false)?;
    if super::storage::identity(&current.metadata()?) != file_identity {
        return Err(CoreError::Conflict(
            "staging ファイルが変更されました".into(),
        ));
    }
    let dest_dir = storage_dir.child(std::ffi::OsStr::new(&key), true)?;
    dest_dir.publish(&stage_dir, std::ffi::OsStr::new("file.part"), filename)?;
    if let Some((device, inode)) = &file_identity {
        stage_dir.remove_owned(std::ffi::OsStr::new("file.part"), device, inode)?;
    }
    let _ = run_dir.remove_empty(std::ffi::OsStr::new(&key));
    dest_dir.verify()?;
    db.conn
        .execute("UPDATE import_files SET state='ready' WHERE key=?1", [&key])?;
    Ok(PreparedPdf {
        id: id.into(),
        key,
        path: dest,
        sha256,
        md5: format!("{:x}", md5.compute()),
        copied: true,
        bytes,
        parent: Some(dest_dir),
        identity: file_identity,
    })
}

/// Only exact, generated ledger paths are eligible. Never traverse a symlinked
/// parent or delete a path currently referenced by any attachment.
fn cleanup(db: &mut Db, storage: &Path) -> Result<Vec<String>> {
    struct Entry {
        key: String,
        run: String,
        path: PathBuf,
        staging: PathBuf,
        owns_staging: bool,
        device: Option<String>,
        inode: Option<String>,
    }
    let mut warnings = Vec::new();
    let rows:Vec<Entry>=db.conn.prepare("SELECT key,run_id,path,staging_path,owns_staging,file_device,file_inode FROM import_files WHERE state IN ('planned','ready')")?
        .query_map([],|r|Ok(Entry{key:r.get(0)?,run:r.get(1)?,path:PathBuf::from(r.get::<_,String>(2)?),staging:PathBuf::from(r.get::<_,String>(3)?),owns_staging:r.get(4)?,device:r.get(5)?,inode:r.get(6)?}))?.collect::<std::result::Result<_,_>>()?;
    for row in rows {
        let safe_id = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric());
        if !safe_id(&row.key)
            || !safe_id(&row.run)
            || row.path.parent() != Some(storage.join(&row.key).as_path())
            || row.path.file_name().is_none()
            || row.staging
                != storage
                    .join(".imports")
                    .join(&row.run)
                    .join(&row.key)
                    .join("file.part")
        {
            warnings.push(format!("清掃対象パスを検証できません: {}", row.key));
            continue;
        }
        // Resolve potentially remote attachment paths without a SQLite writer
        // lock. Recheck the complete path set under IMMEDIATE before unlinking.
        let read_paths = |conn: &Connection| -> Result<Vec<String>> {
            Ok(conn.prepare("SELECT DISTINCT path FROM attachments ORDER BY path")?
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?)
        };
        let paths = read_paths(&db.conn)?;
        let mut identities = std::collections::HashSet::new();
        let mut uncertain = false;
        for path in &paths {
            match std::fs::metadata(path) {
                Ok(meta) => {
                    if let Some(identity) = super::storage::identity(&meta) {
                        identities.insert(identity);
                    } else {
                        uncertain = true;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => uncertain = true,
            }
        }
        let tx = db.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if uncertain || read_paths(&tx)? != paths {
            warnings.push(format!("添付参照を確定できないため未確定コピーを保持しました: {}", row.key));
            continue;
        }
        let referenced: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM attachments WHERE path=?1 OR path=?2)",
            params![row.path.to_string_lossy(), row.staging.to_string_lossy()],
            |r| r.get(0),
        )?;
        if referenced {
            tx.execute(
                "UPDATE import_files SET state='attached' WHERE key=?1",
                [&row.key],
            )?;
            tx.commit()?;
            continue;
        }
        let mut clean = true;
        for (staging, file) in [(false, &row.path), (true, &row.staging)] {
            if staging && !row.owns_staging {
                continue;
            }
            let checked = (|| -> Result<()> {
                crate::db::reject_connection_file(&tx, file)?;
                let parent = super::storage::Dir::open(file.parent().unwrap())?;
                let (Some(device), Some(inode)) = (&row.device, &row.inode) else {
                    return Err(CoreError::Conflict(
                        "所有を証明できないファイルは保持します".into(),
                    ));
                };
                if parent.file_identity(file.file_name().unwrap())?
                    != Some((device.clone(), inode.clone()))
                {
                    return Err(CoreError::Conflict(
                        "この実行が作成したファイルではないため保持しました".into(),
                    ));
                }
                if identities.contains(&(device.clone(), inode.clone())) {
                    return Err(CoreError::Conflict(
                        "別の添付が参照中のファイルを保持しました".into(),
                    ));
                }
                parent.remove_owned(file.file_name().unwrap(), device, inode)?;
                if let Ok(grand) =
                    super::storage::Dir::open(file.parent().unwrap().parent().unwrap())
                {
                    let _ = grand.remove_empty(file.parent().unwrap().file_name().unwrap());
                }
                Ok(())
            })();
            if let Err(e) = checked {
                if !matches!(&e,CoreError::Io(io) if io.kind()==std::io::ErrorKind::NotFound) {
                    clean = false;
                    warnings.push(format!("未確定コピーを保持: {}: {e}", row.key));
                }
            }
        }
        if clean {
            tx.execute(
                "UPDATE import_files SET state='cleaned' WHERE key=?1",
                [&row.key],
            )?;
        }
        tx.commit()?;
    }
    Ok(warnings)
}

pub use crate::db::reject_database_file;

/// Current rows are hashed before acquiring the write transaction. SQLite row
/// key/path are rechecked in the transaction before using these fingerprints.
#[derive(Clone)]
struct AttachmentHash {
    path: String,
    sha256: Option<String>,
    missing: bool,
}
type AttachmentHashes = HashMap<String, AttachmentHash>;
fn attachment_hashes(
    db: &Db,
    _roots: &AttachmentRoots,
    storage: &Path,
    key: Option<&str>,
    db_path: &Path,
    cancel: &AtomicBool,
) -> Result<AttachmentHashes> {
    let Some(key) = key else {
        return Ok(HashMap::new());
    };
    let managed = if std::fs::symlink_metadata(storage)
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        Some(AttachmentRoots::new(&[storage.to_path_buf()])?)
    } else {
        None
    };
    let mut hashes = HashMap::new();
    let rows: Vec<(String, String, String)> = {
        let mut stmt=db.conn.prepare("SELECT a.key,a.path,a.mode FROM attachments a JOIN items i ON i.id=a.item_id WHERE i.key=?1")?;
        let rows = stmt
            .query_map([key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        rows
    };
    for (key, path, mode) in rows {
        let missing =
            matches!(std::fs::metadata(&path),Err(e) if e.kind()==std::io::ErrorKind::NotFound);
        reject_database_file(Path::new(&path), db_path)?;
        let inspected = if mode == "managed" {
            managed.as_ref().map(|r| r.inspect_pdf(&path))
        } else {
            Some((|| {
                reject_database_file(Path::new(&path), db_path)?;
                let path = Path::new(&path);
                let owned = AttachmentRoots::new(&[path
                    .parent()
                    .ok_or_else(|| CoreError::BadInput("添付パスに親がありません".into()))?
                    .to_path_buf()])?;
                owned
                    .inspect_controlled(path, cancel, &mut |_| {})
                    .map(Some)
            })())
        };
        hashes.insert(
            key,
            AttachmentHash {
                path,
                sha256: match inspected {
                    Some(Ok(Some((_, f)))) => Some(f.sha256),
                    _ => None,
                },
                missing,
            },
        );
    }
    Ok(hashes)
}

fn apply_pdfs(
    conn: &Connection,
    input: &ImportInput,
    item: &ImportItem,
    item_id: i64,
    pdfs: &[PreparedPdf],
    hashes: &AttachmentHashes,
    results: &mut Vec<AttachmentResult>,
) -> Result<()> {
    let mut hashes = hashes.clone();
    for pdf in pdfs {
        if let Some(parent) = &pdf.parent {
            parent.verify()?;
            let file = parent.open_file(pdf.path.file_name().unwrap(), false)?;
            if super::storage::identity(&file.metadata()?) != pdf.identity {
                return Err(CoreError::Conflict(
                    "登録前にコピー先 PDF が変更されました".into(),
                ));
            }
        }
        let object_id = child_id(&item.id, &pdf.id);
        let attachment_scope = scope(
            input,
            item.stable_id
                && item
                    .attachments
                    .iter()
                    .find(|a| a.id == pdf.id)
                    .is_some_and(|a| a.stable_id),
        );
        let mapped = object(conn, input, "attachment", attachment_scope, &object_id)?;
        let rows: Vec<(String, String, String)> = conn
            .prepare("SELECT key,path,mode FROM attachments WHERE item_id=?1")?
            .query_map([item_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        if mapped
            .as_ref()
            .is_some_and(|(key, _)| !rows.iter().any(|r| &r.0 == key))
        {
            results.push(AttachmentResult {
                id: pdf.id.clone(),
                status: "blocked".into(),
                fingerprint: None,
                target_key: mapped.map(|v| v.0),
                detail: Some("以前の添付が削除されています。自動再作成しません".into()),
            });
            continue;
        }
        let same = rows.iter().find(|(key, path, _)| {
            hashes
                .get(key)
                .is_some_and(|h| &h.path == path && h.sha256.as_deref() == Some(&pdf.sha256))
        });
        let repair = mapped.as_ref().and_then(|(key, hash)| {
            rows.iter().find(|(k, _, _)| {
                k == key && hash == &pdf.sha256 && hashes.get(k).is_some_and(|h| h.missing)
            })
        });
        if same.is_none()
            && repair.is_none()
            && rows.iter().any(|(key, path, _)| {
                hashes
                    .get(key)
                    .is_none_or(|h| h.sha256.is_none() || &h.path != path)
            })
        {
            results.push(AttachmentResult {
                id: pdf.id.clone(),
                status: "retry".into(),
                fingerprint: None,
                target_key: None,
                detail: Some(
                    "既存添付の内容を照合できません。重複を避けるため追加を保留します".into(),
                ),
            });
            continue;
        }
        let (target, status) = if let Some((key, _, mode)) = same.or(repair) {
            if (pdf.copied && mode == "linked") || repair.is_some() {
                conn.execute(
                    "UPDATE attachments SET mode=?1,path=?2,md5=?3 WHERE key=?4",
                    params![
                        if pdf.copied { "managed" } else { "linked" },
                        pdf.path.to_string_lossy(),
                        pdf.md5,
                        key
                    ],
                )?;
                (key.clone(), "relocated")
            } else {
                (key.clone(), "existing")
            }
        } else {
            conn.execute("INSERT INTO attachments(key,item_id,kind,mode,path,md5) VALUES(?1,?2,'pdf',?3,?4,?5)",params![pdf.key,item_id,if pdf.copied{"managed"}else{"linked"},pdf.path.to_string_lossy(),pdf.md5])?;
            (
                pdf.key.clone(),
                if pdf.copied { "copied" } else { "linked" },
            )
        };
        if pdf.copied && status != "existing" {
            conn.execute(
                "UPDATE import_files SET state='attached' WHERE key=?1",
                [&pdf.key],
            )?;
        }
        let raw = item
            .attachments
            .iter()
            .find(|a| a.id == pdf.id)
            .map(|a| a.raw.clone())
            .unwrap_or(Value::Null);
        remember(
            conn,
            input,
            "attachment",
            attachment_scope,
            &object_id,
            &target,
            &raw,
            &pdf.sha256,
        )?;
        conn.execute(
            "UPDATE attachments SET md5=?1 WHERE key=?2",
            params![pdf.md5, target],
        )?;
        let stored_path: String = conn.query_row(
            "SELECT path FROM attachments WHERE key=?1",
            [&target],
            |r| r.get(0),
        )?;
        hashes.insert(
            target.clone(),
            AttachmentHash {
                path: stored_path,
                sha256: Some(pdf.sha256.clone()),
                missing: false,
            },
        );
        results.push(AttachmentResult {
            id: pdf.id.clone(),
            status: status.into(),
            fingerprint: Some(FileFingerprint {
                bytes: pdf.bytes,
                sha256: pdf.sha256.clone(),
                md5: pdf.md5.clone(),
            }),
            target_key: Some(target),
            detail: Some("全文索引は独立した再試行対象です".into()),
        });
    }
    Ok(())
}

fn verify_source(
    roots: &AttachmentRoots,
    source: &super::ImportAttachment,
    id: &str,
    expected: Option<(&Path, &FileFingerprint)>,
    selected: bool,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<()> {
    let paths: Vec<_> = std::iter::once(source.path.as_str())
        .chain(source.alternative_paths.iter().map(String::as_str))
        .collect();
    let resolved = roots.resolve_candidates(&paths)?;
    let valid = match expected {
        Some((path, fingerprint)) => {
            (resolved
                == (super::files::ResolvedPath::Found {
                    path: path.to_path_buf(),
                })
                || (selected
                    && matches!(&resolved, super::files::ResolvedPath::Ambiguous { candidates } if candidates.contains(&path.to_path_buf()))))
                && roots.inspect_controlled(path, cancel, progress)?.1 == *fingerprint
        }
        None => matches!(resolved, super::files::ResolvedPath::Missing),
    };
    if id != source.id || !valid {
        return Err(CoreError::Conflict(
            "事前確認後に添付が変更されました。もう一度確認してください".into(),
        ));
    }
    Ok(())
}

fn apply_existing(
    conn: &Connection,
    input: &ImportInput,
    item: &ImportItem,
    item_id: i64,
    existing: &[&AttachmentPlan],
    results: &mut Vec<AttachmentResult>,
) -> Result<()> {
    for planned in existing {
        let AttachmentPlan::Existing {
            id,
            target_key,
            path,
            fingerprint,
            ..
        } = planned
        else {
            unreachable!();
        };
        let att = item
            .attachments
            .iter()
            .find(|a| &a.id == id)
            .ok_or_else(|| CoreError::BadInput("添付 ID が一致しません".into()))?;
        let mapped = object(
            conn,
            input,
            "attachment",
            scope(input, item.stable_id && att.stable_id),
            &child_id(&item.id, id),
        )?;
        let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM attachments WHERE key=?1 AND item_id=?2 AND mode='managed' AND path=?3)",params![target_key,item_id,path.to_string_lossy()],|r|r.get(0))?;
        if !valid || mapped.as_ref().is_none_or(|v| &v.0 != target_key) {
            results.push(AttachmentResult {
                id: id.clone(),
                status: "blocked".into(),
                target_key: None,
                fingerprint: None,
                detail: Some("事前確認した既存添付が変更または削除されました".into()),
            });
            continue;
        }
        conn.execute(
            "UPDATE attachments SET md5=?1 WHERE key=?2",
            params![fingerprint.md5, target_key],
        )?;
        results.push(AttachmentResult {
            id: id.clone(),
            status: "existing".into(),
            target_key: Some(target_key.clone()),
            fingerprint: Some(fingerprint.clone()),
            detail: Some("OpenRef に保存済みの PDF を再利用しました".into()),
        });
    }
    Ok(())
}

/// Owns a dedicated connection. No AppState mutex is needed by GUI callers.
pub fn execute(
    db_path: &Path,
    input_path: &Path,
    input: &ImportInput,
    approved: &ImportPlan,
    roots: &AttachmentRoots,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> Result<ImportReport> {
    roots.protect_database(db_path);
    reject_database_file(input_path, db_path)?;
    plan::validate(input)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Conflict("移行開始前に中止されました".into()));
    }
    if approved.items.len() != input.items.len()
        || approved
            .items
            .iter()
            .zip(&input.items)
            .any(|(p, i)| p.object_id != i.id)
    {
        return Err(CoreError::Conflict(
            "事前確認の文献一覧が入力と一致しません".into(),
        ));
    }
    if approved.input_model_sha256 != digest(&serde_json::to_vec(input)?)
        || approved.input_sha256 != FileFingerprint::read(input_path)?.sha256
        || approved.source_id != input.source.id
    {
        return Err(CoreError::Conflict(
            "事前確認後に入力が変更されました。もう一度確認してください".into(),
        ));
    }
    let storage = storage_path(db_path)?;
    if approved.managed_storage.as_ref() != Some(&storage) {
        return Err(CoreError::Conflict(
            "事前確認した保存先が一致しません".into(),
        ));
    }
    let has_existing = approved.items.iter().any(|i| {
        i.attachments
            .iter()
            .any(|a| matches!(a, AttachmentPlan::Existing { .. }))
    });
    let managed = if has_existing {
        if !std::fs::symlink_metadata(&storage)
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            return Err(CoreError::Conflict(
                "事前確認した保存先が変更されました".into(),
            ));
        }
        Some(AttachmentRoots::new(std::slice::from_ref(&storage))?)
    } else {
        None
    };
    for (ordinal, (item, normalized)) in approved.items.iter().zip(&input.items).enumerate() {
        if item.attachments.len() != normalized.attachments.len() {
            return Err(CoreError::Conflict(
                "事前確認した添付一覧が一致しません".into(),
            ));
        }
        for (att, source) in item.attachments.iter().zip(&normalized.attachments) {
            if cancel.load(Ordering::Relaxed) {
                return Err(CoreError::Conflict("移行開始前に中止されました".into()));
            }
            let mut notify = |bytes| {
                progress(&Progress {
                    phase: "verify",
                    completed: ordinal,
                    total: input.items.len(),
                    bytes,
                })
            };
            match att {
                AttachmentPlan::Available {
                    id,
                    path,
                    fingerprint,
                } => verify_source(
                    roots,
                    source,
                    id,
                    Some((path, fingerprint)),
                    item.selected_files.contains(id),
                    cancel,
                    &mut notify,
                )?,
                AttachmentPlan::Existing {
                    id,
                    path,
                    fingerprint,
                    source_file,
                    ..
                } => {
                    verify_source(
                        roots,
                        source,
                        id,
                        source_file.as_ref().map(|(p, f)| (p.as_path(), f)),
                        false,
                        cancel,
                        &mut notify,
                    )?;
                    if managed
                        .as_ref()
                        .unwrap()
                        .inspect_controlled(path, cancel, &mut notify)?
                        .1
                        != *fingerprint
                    {
                        return Err(CoreError::Conflict(
                            "保存済み PDF が事前確認後に変更されました".into(),
                        ));
                    }
                }
                AttachmentPlan::Missing { id, .. } => {
                    verify_source(roots, source, id, None, false, cancel, &mut notify)?
                }
                _ => {}
            }
        }
    }
    let _lock = JobLock::acquire(db_path)?;
    let resolved = if db_path.try_exists()? {
        db_path.canonicalize()?
    } else {
        storage.parent().unwrap().join(db_path.file_name().unwrap())
    };
    let mut db = Db::open(&resolved)?;
    let legacy_keys = legacy_bibliography(&db.conn)?;
    let run = format!("{}{}", crate::keys::new_key(), crate::keys::new_key());
    let backup = crate::backup::snapshot(&db, &format!("pre-import-{run}-"), 1)?;
    db.conn.execute("UPDATE import_runs SET state='interrupted',date_finished=datetime('now') WHERE state='running'",[])?;
    let mut warnings = cleanup(&mut db, &storage)?;
    let existing_format: Option<String> = db
        .conn
        .query_row(
            "SELECT format FROM import_sources WHERE id=?1",
            [&input.source.id],
            |r| r.get(0),
        )
        .optional()?;
    if existing_format
        .as_deref()
        .is_some_and(|f| f != input.source.format)
    {
        return Err(CoreError::BadInput(
            "選択した移行元の形式が一致しません".into(),
        ));
    }
    db.conn.execute(
        "INSERT OR IGNORE INTO import_sources(id,format) VALUES(?1,?2)",
        params![input.source.id, input.source.format],
    )?;
    db.conn.execute("INSERT INTO import_runs(id,source_id,input_sha256,adapter_version,attachment_mode,state,input_json,backup_path) VALUES(?1,?2,?3,?4,?5,'running',?6,?7)",params![run,input.source.id,input.input_sha256,input.adapter_version,if approved.attachment_mode==AttachmentMode::Copy{"copy"}else{"link"},serde_json::to_string(input)?,backup.as_ref().map(|p|p.to_string_lossy().into_owned())])?;
    warnings.extend(input.warnings.clone());
    let mut report = ImportReport {
        run_id: run.clone(),
        source_id: input.source.id.clone(),
        state: "running".into(),
        backup_path: backup,
        total: input.items.len(),
        results: Vec::new(),
        warnings,
        complete: false,
        fulltext_pending: 0,
    };
    let prior = super::provenance::CrossExport::read(&db.conn, input)?;
    let imported_before: std::collections::HashSet<String> = db
        .conn
        .prepare("SELECT target_key FROM import_objects WHERE source_id=?1 AND kind='item'")?
        .query_map([&input.source.id], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let work = (|| -> Result<()> {
        let folders = prepare_folders(&mut db, input, &prior, cancel, &mut report.warnings)?;
        for (ordinal, (item, planned)) in input.items.iter().zip(&approved.items).enumerate() {
            progress(&Progress {
                phase: "item",
                completed: ordinal,
                total: input.items.len(),
                bytes: 0,
            });
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut result = ItemResult {
                object_id: item.id.clone(),
                status: "pending".into(),
                item_key: None,
                attachments: Vec::new(),
                warnings: planned.warnings.clone(),
            };
            let before = planned
                .selection
                .as_ref()
                .map(|s| ItemAction::AddMissing {
                    item_key: s.item_key.clone(),
                })
                .unwrap_or(match_item(&db.conn, input, item)?);
            let hashes = attachment_hashes(
                &db,
                roots,
                &storage,
                if let ItemAction::AddMissing { item_key } = &before {
                    Some(item_key)
                } else {
                    None
                },
                &resolved,
                cancel,
            )?;
            let mut pdfs = Vec::new();
            let mut reuse = Vec::new();
            if matches!(
                planned.action,
                ItemAction::New | ItemAction::AddMissing { .. }
            ) {
                for att in &planned.attachments {
                    match att {
                        AttachmentPlan::Existing {
                            id,
                            path,
                            fingerprint,
                            source_file,
                            ..
                        } => {
                            let source = item.attachments.iter().find(|a| &a.id == id).unwrap();
                            let inspected = (|| -> Result<()> {
                                verify_source(
                                    roots,
                                    source,
                                    id,
                                    source_file.as_ref().map(|(p, f)| (p.as_path(), f)),
                                    false,
                                    cancel,
                                    &mut |_| {},
                                )?;
                                if managed
                                    .as_ref()
                                    .unwrap()
                                    .inspect_controlled(path, cancel, &mut |_| {})?
                                    .1
                                    != *fingerprint
                                {
                                    return Err(CoreError::Conflict(
                                        "保存済み PDF が変更されました".into(),
                                    ));
                                }
                                Ok(())
                            })();
                            match inspected {
                                Ok(()) => reuse.push(att),
                                Err(e) => result.attachments.push(AttachmentResult {
                                    id: id.clone(),
                                    status: "failed".into(),
                                    target_key: None,
                                    fingerprint: None,
                                    detail: Some(e.to_string()),
                                }),
                            }
                        }
                        AttachmentPlan::Available {
                            id,
                            path,
                            fingerprint,
                        } => {
                            let prepared = if approved.attachment_mode == AttachmentMode::Copy {
                                copy_pdf(
                                    &db,
                                    &run,
                                    roots,
                                    &storage,
                                    id,
                                    path,
                                    fingerprint,
                                    cancel,
                                    &mut progress,
                                    ordinal,
                                    input.items.len(),
                                )
                            } else {
                                (|| -> Result<PreparedPdf> {
                                    let (_, fresh) = roots.inspect_resolved_pdf(path)?;
                                    if &fresh != fingerprint {
                                        Err(CoreError::Conflict(
                                            "事前確認後に PDF が変更されました".into(),
                                        ))
                                    } else {
                                        let mut reader = roots.open_selected_file(path)?;
                                        let mut md5 = md5::Context::new();
                                        let mut sha = Sha256::new();
                                        let mut bytes = 0;
                                        let mut b = [0; 64 * 1024];
                                        loop {
                                            if cancel.load(Ordering::Relaxed) {
                                                return Err(CoreError::Conflict(
                                                    "移行が中止されました".into(),
                                                ));
                                            }
                                            let n = reader.read(&mut b)?;
                                            if n == 0 {
                                                break;
                                            }
                                            md5.consume(&b[..n]);
                                            sha.update(&b[..n]);
                                            bytes += n as u64;
                                        }
                                        if format!("{:x}", sha.finalize()) != fingerprint.sha256
                                            || bytes != fingerprint.bytes
                                        {
                                            return Err(CoreError::Conflict(
                                                "リンク元 PDF が変更されました".into(),
                                            ));
                                        }
                                        Ok(PreparedPdf {
                                            id: id.clone(),
                                            key: crate::keys::new_key(),
                                            path: path.clone(),
                                            sha256: fingerprint.sha256.clone(),
                                            md5: format!("{:x}", md5.compute()),
                                            copied: false,
                                            bytes,
                                            parent: None,
                                            identity: None,
                                        })
                                    }
                                })()
                            };
                            match prepared {
                                Ok(pdf) => pdfs.push(pdf),
                                Err(e) => result.attachments.push(AttachmentResult {
                                    id: id.clone(),
                                    fingerprint: None,
                                    status: "failed".into(),
                                    target_key: None,
                                    detail: Some(e.to_string()),
                                }),
                            }
                            if cancel.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                        other => {
                            result.attachments.push(AttachmentResult {
                                fingerprint: None,
                                id: match other {
                                    AttachmentPlan::Missing { id, .. }
                                    | AttachmentPlan::Ambiguous { id, .. }
                                    | AttachmentPlan::Unsupported { id, .. }
                                    | AttachmentPlan::Failed { id, .. } => id.clone(),
                                    _ => unreachable!(),
                                },
                                status: "unresolved".into(),
                                target_key: None,
                                detail: Some(serde_json::to_string(other)?),
                            });
                        }
                    }
                }
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let applied = (|| -> Result<()> {
                let tx = db
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let mut action = if matches!(
                    planned.action,
                    ItemAction::New | ItemAction::AddMissing { .. }
                ) {
                    match_item(&tx, input, item)?
                } else {
                    planned.action.clone()
                };
                if matches!(
                    action,
                    ItemAction::New | ItemAction::AddMissing { .. } | ItemAction::Review { .. }
                ) && object(&tx, input, "item", scope(input, item.stable_id), &item.id)?
                    .is_none()
                {
                    if let Some(held) = prior.action(item, |key| Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM items WHERE key=?1 AND deleted_at IS NULL)",[key],|r|r.get(0))?))? {
                        action=held;
                    }
                }
                if planned.register_new && matches!(action, ItemAction::Review { .. })
                    && plan::identifier_candidates(&tx, &item.csl, None)?.is_empty()
                    && prior.action(item, |key| Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM items WHERE key=?1 AND deleted_at IS NULL)", [key], |r| r.get(0))?))?.is_none()
                {
                    action = ItemAction::New;
                }
                if let Some(selection) = &planned.selection {
                    action = super::decisions::recheck(&tx, item, &action, selection)?;
                }
                if let ItemAction::AddMissing { item_key: expected } = &planned.action {
                    if !matches!(&action,ItemAction::AddMissing{item_key} if item_key==expected)
                        && !matches!(&action, ItemAction::Blocked { .. })
                    {
                        action = ItemAction::Blocked {
                            candidates: vec![expected.clone()],
                            reason:
                                "事前確認した既存文献が変更または削除されました。自動再作成しません"
                                    .into(),
                        };
                    }
                }
                match action {
                    ItemAction::New | ItemAction::AddMissing { .. } => {
                        let (key, id) = if let ItemAction::AddMissing { item_key } = action {
                            let (id, csl): (i64, String) = tx.query_row(
                                "SELECT id,csl FROM items WHERE key=?1",
                                [&item_key],
                                |r| Ok((r.get(0)?, r.get(1)?)),
                            )?;
                            let mut existing: Value = serde_json::from_str(&csl)?;
                            let previous_key =
                                crate::bibtex::explicit_citekey(&existing).map(str::to_string);
                            let mut incoming = item.csl.clone();
                            if previous_key.is_some() {
                                // Citation keys are one semantic field, even across formats.
                                incoming.as_object_mut().unwrap().remove("citation-key");
                                if let Some(pp) = incoming
                                    .pointer_mut("/custom/paperpile")
                                    .and_then(Value::as_object_mut)
                                {
                                    pp.remove("citekey");
                                }
                            }
                            let applied = applied_bibliography(&tx, &legacy_keys, &item_key)?;
                            if fill_missing(&mut existing, &incoming, &applied) {
                                tx.execute("UPDATE items SET csl=?1,date_modified=datetime('now') WHERE id=?2",params![existing.to_string(),id])?;
                                crate::ingest::reindex_item(&tx, id, &existing)?;
                            }
                            result.status = "merged".into();
                            (item_key, id)
                        } else {
                            let key = crate::keys::new_key();
                            tx.execute("INSERT INTO items(key,csl,date_added,date_modified) VALUES(?1,?2,COALESCE(?3,datetime('now')),COALESCE(?3,datetime('now')))",params![key,item.csl.to_string(),item.date_added])?;
                            let id = tx.last_insert_rowid();
                            crate::ingest::index_item(&tx, id, &item.csl)?;
                            result.status = "inserted".into();
                            (key, id)
                        };
                        let previously_imported = imported_before.contains(&key);
                        let mut applied = applied_bibliography(&tx, &legacy_keys, &key)?;
                        union_keys(&mut applied, &item.csl);
                        remember(&tx, input, "bibliography", "", &key, &key, &applied, "")?;
                        remember(
                            &tx,
                            input,
                            "item",
                            scope(input, item.stable_id),
                            &item.id,
                            &key,
                            &item.raw,
                            &digest(item.csl.to_string().as_bytes()),
                        )?;
                        apply_pdfs(
                            &tx,
                            input,
                            item,
                            id,
                            &pdfs,
                            &hashes,
                            &mut result.attachments,
                        )?;
                        apply_existing(&tx, input, item, id, &reuse, &mut result.attachments)?;
                        let held_metadata = planned.selection.is_some() && previously_imported
                            || object(
                                &tx,
                                input,
                                "metadata_hold",
                                scope(input, item.stable_id),
                                &item.id,
                            )?
                            .is_some();
                        if held_metadata {
                            remember(
                                &tx,
                                input,
                                "metadata_hold",
                                scope(input, item.stable_id),
                                &item.id,
                                &key,
                                &item.raw,
                                "",
                            )?;
                            result.warnings.push("確認した文献へ書誌の不足分とPDFを追加しました。過去の編集・削除を保護するため、この入力のノート・タグ・所属は保留しています".into());
                        } else {
                            apply_metadata(
                                &tx,
                                input,
                                item,
                                id,
                                &key,
                                previously_imported,
                                &prior,
                                &folders,
                                &mut result.warnings,
                            )?;
                        }
                        if let Some(selection) = &planned.selection {
                            result.warnings.push(format!(
                                "利用者が確認した対応: {}",
                                serde_json::to_string(selection)?
                            ));
                        }
                        for att in &planned.attachments {
                            if let AttachmentPlan::Available { id, path, .. } = att {
                                if planned.selected_files.contains(id) {
                                    result.warnings.push(format!(
                                        "利用者が選択したPDF: {id} → {}",
                                        path.display()
                                    ));
                                }
                            }
                        }
                        result.item_key = Some(key);
                    }
                    ItemAction::Blocked { reason, .. } => {
                        result.status = "blocked".into();
                        result.warnings.push(reason);
                    }
                    ItemAction::Review { reason, .. } => {
                        result.status = "review".into();
                        result.warnings.push(reason);
                    }
                }
                tx.execute(
                    "INSERT INTO import_results(run_id,ordinal,result_json) VALUES(?1,?2,?3)",
                    params![run, ordinal as i64, serde_json::to_string(&result)?],
                )?;
                tx.commit()?;
                Ok(())
            })();
            if let Err(e) = applied {
                result.status = "failed".into();
                result.item_key = None;
                result.warnings.push(e.to_string());
                for att in &mut result.attachments {
                    if att.target_key.is_some() {
                        att.target_key = None;
                        att.status = "not_applied".into();
                    }
                }
                db.conn.execute("INSERT OR REPLACE INTO import_results(run_id,ordinal,result_json) VALUES(?1,?2,?3)",params![run,ordinal as i64,serde_json::to_string(&result)?])?;
            }
            report.results.push(result);
            progress(&Progress {
                phase: "committed",
                completed: ordinal + 1,
                total: input.items.len(),
                bytes: 0,
            });
        }
        Ok(())
    })();
    report.state = if let Err(e) = work {
        report.warnings.push(e.to_string());
        "failed"
    } else if cancel.load(Ordering::Relaxed) {
        "cancelled"
    } else {
        "finished"
    }
    .into();
    match cleanup(&mut db, &storage) {
        Ok(w) => report.warnings.extend(w),
        Err(e) => report
            .warnings
            .push(format!("未確定コピーの清掃を後で再試行します: {e}")),
    }
    let targets: std::collections::HashSet<_> = report
        .results
        .iter()
        .flat_map(|r| r.attachments.iter().filter_map(|a| a.target_key.as_deref()))
        .collect();
    for key in targets {
        let pending:bool=db.conn.query_row("SELECT EXISTS(SELECT 1 FROM attachments a LEFT JOIN fulltext f ON f.attachment_id=a.id WHERE a.key=?1 AND (f.status IS NULL OR f.status IN ('error','partial') OR f.md5 IS NOT a.md5))",[key],|r|r.get(0))?;
        report.fulltext_pending += usize::from(pending);
    }
    report.complete = report_complete(&report);
    db.conn.execute(
        "UPDATE import_runs SET state=?1,report_json=?2,date_finished=datetime('now') WHERE id=?3",
        params![report.state, serde_json::to_string(&report)?, run],
    )?;
    Ok(report)
}

fn report_complete(report: &ImportReport) -> bool {
    report.state == "finished"
        && report.results.len() == report.total
        && report.fulltext_pending == 0
        && report.warnings.is_empty()
        && report.results.iter().all(|r| {
            r.warnings.is_empty()
                && matches!(r.status.as_str(), "inserted" | "merged")
                && r.attachments.iter().all(|a| {
                    matches!(
                        a.status.as_str(),
                        "copied" | "linked" | "relocated" | "existing"
                    )
                })
        })
}

struct FolderMap {
    root: Option<String>,
    nodes: HashMap<String, Option<String>>,
}

fn mapped_folder(
    conn: &Connection,
    input: &ImportInput,
    kind: &str,
    id: &str,
    stable: bool,
    name: &str,
    parent: Option<&str>,
    raw: &Value,
    warnings: &mut Vec<String>,
) -> Result<Option<String>> {
    let scope = scope(input, stable);
    if let Some((key, _)) = object(conn, input, kind, scope, id)? {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM collections WHERE key=?1 AND deleted_at IS NULL",
                [&key],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_none() {
            warnings.push(format!("削除済みフォルダを自動復元しません: {name}"));
        }
        return Ok(existing.map(|_| key));
    }
    let parent = match parent {
        Some(key) => {
            let id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM collections WHERE key=?1 AND deleted_at IS NULL",
                    [key],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(id) = id else {
                warnings.push(format!("親フォルダが削除されているため作成を保留: {name}"));
                return Ok(None);
            };
            Some(id)
        }
        None => None,
    };
    let base = name.trim().replace('/', "∕");
    let base = if base.is_empty() {
        "Untitled".to_string()
    } else {
        base
    };
    let mut display = base.clone();
    let suffix = &digest(json!([input.source.id, scope, id]).to_string().as_bytes())[..8];
    let mut n = 0;
    loop {
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM collections WHERE parent_id IS ?1 AND name=?2 COLLATE NOCASE)",params![parent,display],|r|r.get(0))?;
        if !exists {
            break;
        }
        n += 1;
        display = if n == 1 {
            format!("{base} ({suffix})")
        } else {
            format!("{base} ({suffix}-{n})")
        };
    }
    if display != name {
        warnings.push(format!("フォルダ表示名を変更: {name} → {display}"));
    }
    let key = crate::keys::new_key();
    conn.execute(
        "INSERT INTO collections(key,name,parent_id) VALUES(?1,?2,?3)",
        params![key, display, parent],
    )?;
    remember(conn, input, kind, scope, id, &key, raw, "")?;
    Ok(Some(key))
}

fn prepare_folders(
    db: &mut Db,
    input: &ImportInput,
    prior: &super::provenance::CrossExport,
    cancel: &AtomicBool,
    warnings: &mut Vec<String>,
) -> Result<FolderMap> {
    let root = {
        let tx = db
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root = mapped_folder(
            &tx,
            input,
            "source_root",
            "root",
            true,
            if input.source.format == "paperpile" {
                "Paperpile"
            } else {
                "Zotero"
            },
            None,
            &json!({"generated_source_root":true}),
            warnings,
        )?;
        tx.commit()?;
        root
    };
    let nodes: HashMap<_, _> = input.folders.iter().map(|f| (f.id.as_str(), f)).collect();
    let mut folders = HashMap::new();
    for folder in &input.folders {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut chain = Vec::new();
        let mut node = folder;
        while !folders.contains_key(&node.id) {
            chain.push(node);
            match node.parent_id.as_deref() {
                Some(parent) => node = nodes[parent],
                None => break,
            }
        }
        for node in chain.into_iter().rev() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(FolderMap {
                    root,
                    nodes: folders,
                });
            }
            let parent = node
                .parent_id
                .as_ref()
                .and_then(|p| folders.get(p).cloned())
                .unwrap_or_else(|| root.clone());
            if object(
                &db.conn,
                input,
                "folder",
                scope(input, node.stable_id),
                &node.id,
            )?
            .is_none()
            {
                if let Some(key) = prior.target("folder", &node.id, &node.name) {
                    remember(
                        &db.conn,
                        input,
                        "folder",
                        scope(input, node.stable_id),
                        &node.id,
                        key,
                        &node.raw,
                        &digest(node.name.as_bytes()),
                    )?;
                }
            }
            let pending = prior.folder_needs_review(node)
                && object(
                    &db.conn,
                    input,
                    "folder",
                    scope(input, node.stable_id),
                    &node.id,
                )?
                .is_none();
            let target = if pending {
                warnings.push(format!("別の書出しのフォルダ対応候補を保留: {}", node.name));
                None
            } else if let Some(parent) = parent {
                let tx = db
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let target = mapped_folder(
                    &tx,
                    input,
                    "folder",
                    &node.id,
                    node.stable_id,
                    &node.name,
                    Some(&parent),
                    &node.raw,
                    warnings,
                )?;
                tx.commit()?;
                target
            } else {
                None
            };
            folders.insert(node.id.clone(), target);
        }
    }
    Ok(FolderMap {
        root,
        nodes: folders,
    })
}

fn apply_metadata(
    conn: &Connection,
    input: &ImportInput,
    item: &ImportItem,
    item_id: i64,
    item_key: &str,
    previously_imported: bool,
    prior: &super::provenance::CrossExport,
    folders: &FolderMap,
    warnings: &mut Vec<String>,
) -> Result<()> {
    for (id, folder) in std::iter::once(("移行元の親フォルダ", folders.root.as_deref())).chain(
        item.folder_ids.iter().map(|id| {
            (
                id.as_str(),
                folders.nodes.get(id).and_then(|k| k.as_deref()),
            )
        }),
    ) {
        if let Some(folder) = folder {
            apply_membership(
                conn,
                input,
                item_id,
                item_key,
                "folder",
                folder,
                previously_imported,
                warnings,
            )?;
        } else {
            warnings.push(format!(
                "所属先フォルダが削除済み、または対応未確定のため追加を保留: {id}"
            ));
        }
    }
    for tag in &item.tags {
        if object(conn, input, "tag", scope(input, tag.stable_id), &tag.id)?.is_none() {
            if let Some(key) = prior.target("tag", &tag.id, &tag.name) {
                remember(
                    conn,
                    input,
                    "tag",
                    scope(input, tag.stable_id),
                    &tag.id,
                    key,
                    &tag.raw,
                    &digest(tag.name.as_bytes()),
                )?;
            }
        }
        if prior.tag_needs_review(tag)
            && object(conn, input, "tag", scope(input, tag.stable_id), &tag.id)?.is_none()
        {
            warnings.push(format!("別の書出しのタグ対応候補を保留: {}", tag.name));
            continue;
        }
        apply_tag(
            conn,
            input,
            item_id,
            item_key,
            previously_imported,
            tag,
            warnings,
        )?;
    }
    for note in &item.notes {
        let id = child_id(&item.id, &note.id);
        let scope = scope(input, item.stable_id && note.stable_id);
        let hash = digest(note.content_md.as_bytes());
        if let Some((key, old_hash)) = object(conn, input, "note", scope, &id)? {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM notes WHERE key=?1 AND item_id=?2)",
                params![key, item_id],
                |r| r.get(0),
            )?;
            if !exists {
                warnings.push(format!("削除されたノートを再作成しません: {}", note.id));
            } else if old_hash != hash {
                warnings.push(format!("元ノートの変更を差分候補として実行履歴に保持しました。編集済みノートは上書きしません: {}",note.id));
            }
            continue;
        }
        let key = crate::keys::new_key();
        conn.execute("INSERT INTO notes(key,item_id,author,content_md,date_added,date_modified) VALUES(?1,?2,'user',?3,COALESCE(datetime(?4),datetime('now')),COALESCE(datetime(?5),datetime(?4),datetime('now')))",params![key,item_id,note.content_md,note.date_added,note.date_modified])?;
        remember(conn, input, "note", scope, &id, &key, &note.raw, &hash)?;
    }
    if let Some(citekey) = crate::bibtex::explicit_citekey(&item.csl) {
        let stored: String =
            conn.query_row("SELECT csl FROM items WHERE id=?1", [item_id], |r| r.get(0))?;
        let stored: Value = serde_json::from_str(&stored)?;
        if crate::bibtex::explicit_citekey(&stored) != Some(citekey) {
            warnings.push(format!(
                "既存の引用キーを保持し、元の引用キーは原情報に保存しました: {citekey}"
            ));
        }
        let collision:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM items WHERE id!=?1 AND (json_extract(csl,'$.custom.paperpile.citekey')=?2 OR json_extract(csl,'$.\"citation-key\"')=?2))",params![item_id,citekey],|r|r.get(0))?;
        if collision {
            warnings.push(format!(
                "引用キーが衝突しています。既存キーは変更していません: {citekey}"
            ));
        }
    }
    if item.date_added.is_none() {
        warnings.push("元の登録日時を取得できないため、OpenRef の取込日時を使用しました".into());
    }
    Ok(())
}

fn apply_tag(
    conn: &Connection,
    input: &ImportInput,
    item_id: i64,
    item_key: &str,
    previously_imported: bool,
    tag: &super::ImportTag,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let name = tag.name.trim();
    if name.is_empty() {
        warnings.push("空のタグ名は元レコードに保持しました".into());
        return Ok(());
    }
    let tag_scope = scope(input, tag.stable_id);
    let id = if let Some((key, old_hash)) = object(conn, input, "tag", tag_scope, &tag.id)? {
        let target: Option<i64> = conn
            .query_row("SELECT id FROM tags WHERE import_key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        if target.is_none() {
            warnings.push(format!("削除されたタグを再作成しません: {name}"));
        } else if old_hash != digest(tag.name.as_bytes()) {
            warnings.push(format!(
                "元のタグ名が変更されています。OpenRef 側の名前を保持しました: {name}"
            ));
        }
        target
    } else {
        conn.execute(
            "INSERT OR IGNORE INTO tags(name,color) VALUES(?1,?2)",
            params![name, tag.color],
        )?;
        let (id, key): (i64, Option<String>) = conn.query_row(
            "SELECT id,import_key FROM tags WHERE name=?1 COLLATE NOCASE",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let key = match key {
            Some(key) => key,
            None => {
                let key = format!("{}{}", crate::keys::new_key(), crate::keys::new_key());
                conn.execute(
                    "UPDATE tags SET import_key=?1 WHERE id=?2",
                    params![key, id],
                )?;
                key
            }
        };
        remember(
            conn,
            input,
            "tag",
            tag_scope,
            &tag.id,
            &key,
            &tag.raw,
            &digest(tag.name.as_bytes()),
        )?;
        Some(id)
    };
    if let Some(id) = id {
        let key: String = conn.query_row("SELECT import_key FROM tags WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
        apply_membership(
            conn,
            input,
            item_id,
            item_key,
            "tag",
            &key,
            previously_imported,
            warnings,
        )?;
    }
    Ok(())
}

// Record the relationship, not just its endpoints. An absent previously applied
// relationship means the user removed it; PDF retry must not restore it.
fn apply_membership(
    conn: &Connection,
    input: &ImportInput,
    item_id: i64,
    item_key: &str,
    kind: &str,
    target: &str,
    previously_imported: bool,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let object_id = json!([item_key, kind, target]).to_string();
    let prior = object(conn, input, "membership", "", &object_id)?.is_some();
    let (lookup, present, insert) = if kind == "folder" {
        (
            "SELECT id FROM collections WHERE key=?1 AND deleted_at IS NULL",
            "SELECT EXISTS(SELECT 1 FROM collection_items WHERE item_id=?1 AND collection_id=?2)",
            "INSERT OR IGNORE INTO collection_items(item_id,collection_id) VALUES(?1,?2)",
        )
    } else {
        (
            "SELECT id FROM tags WHERE import_key=?1",
            "SELECT EXISTS(SELECT 1 FROM item_tags WHERE item_id=?1 AND tag_id=?2)",
            "INSERT OR IGNORE INTO item_tags(item_id,tag_id) VALUES(?1,?2)",
        )
    };
    let id: Option<i64> = conn.query_row(lookup, [target], |r| r.get(0)).optional()?;
    let Some(id) = id else {
        warnings.push(format!("削除された所属先への追加を保留: {kind} {target}"));
        return Ok(());
    };
    let exists: bool = conn.query_row(present, params![item_id, id], |r| r.get(0))?;
    if !exists && (prior || previously_imported) {
        warnings.push(format!(
            "解除済み、または以前の所属を確認できないため自動復元しません: {kind} {target}"
        ));
    } else if !exists {
        conn.execute(insert, params![item_id, id])?;
    }
    remember(
        conn,
        input,
        "membership",
        "",
        &object_id,
        target,
        &json!({"item_key":item_key,"kind":kind}),
        "",
    )?;
    Ok(())
}

/// Independent, cancellable full-text work for the PDFs of one committed run.
/// Extraction runs without a transaction or the desktop's shared DB mutex.
pub fn index_report(
    db_path: &Path,
    report: &mut ImportReport,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> Result<()> {
    use crate::fulltext::{self, IndexOutcome};
    let _lock = JobLock::acquire(db_path)?;
    let db = Db::open(db_path)?;
    let keys: std::collections::BTreeSet<_> = report
        .results
        .iter()
        .flat_map(|r| r.attachments.iter().filter_map(|a| a.target_key.clone()))
        .collect();
    for (ordinal, key) in keys.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        progress(&Progress {
            phase: "fulltext",
            completed: ordinal,
            total: keys.len(),
            bytes: 0,
        });
        let row:Option<(i64,bool)>=db.conn.query_row("SELECT a.id, f.attachment_id IS NULL OR f.status IN ('error','partial') OR f.md5 IS NOT a.md5 FROM attachments a LEFT JOIN fulltext f ON f.attachment_id=a.id WHERE a.key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((attachment, pending)) = row else {
            continue;
        };
        if !pending {
            continue;
        }
        let reason = match fulltext::index_attachment(&db.conn, attachment) {
            Ok(IndexOutcome::Failed(reason) | IndexOutcome::Skipped(reason)) => Some(reason),
            Err(e) => Some(e.to_string()),
            _ => None,
        };
        if let Some(reason) = reason {
            let warning = format!("全文索引 {key}: {reason}");
            if !report.warnings.contains(&warning) {
                report.warnings.push(warning);
            }
        }
    }
    report.fulltext_pending = 0;
    for key in &keys {
        let unresolved:bool=db.conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM attachments a JOIN fulltext f ON f.attachment_id=a.id WHERE a.key=?1 AND f.status NOT IN ('error','partial') AND f.md5 IS a.md5)",[key],|r|r.get(0))?;
        report.fulltext_pending += usize::from(unresolved);
    }
    progress(&Progress {
        phase: "fulltext",
        completed: keys.len() - report.fulltext_pending,
        total: keys.len(),
        bytes: 0,
    });
    report.complete = report_complete(report);
    db.conn.execute(
        "UPDATE import_runs SET report_json=?1 WHERE id=?2",
        params![serde_json::to_string(report)?, report.run_id],
    )?;
    Ok(())
}

/// Resolve an omitted source consistently across CLI and desktop without reading inputs.
pub fn default_source(db_path: &Path, format: &str) -> Result<String> {
    let sources = inventory::with_snapshot(db_path, |conn| {
        let has: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='import_sources')",
            [],
            |r| r.get(0),
        )?;
        if !has {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare("SELECT id FROM import_sources WHERE format=?1 ORDER BY id")?;
        let rows = stmt
            .query_map([format], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;
    match sources.unwrap_or_default().as_slice() {
        [] => Ok(format!("{format}-default")),
        [source] => Ok(source.clone()),
        _ => Err(CoreError::BadInput("同じ形式の移行元が複数あります。以前の移行元を選択するか --source-id を指定してください".into())),
    }
}

pub fn history(db_path: &Path) -> Result<Vec<Value>> {
    inventory::with_snapshot(db_path, |conn| {
        let has:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='import_runs')",[],|r|r.get(0))?;
        if !has{return Ok(Vec::new());}
        let mut stmt=conn.prepare("SELECT json_object('id',id,'format',(SELECT format FROM import_sources WHERE import_sources.id=import_runs.source_id),'source_id',source_id,'input_sha256',input_sha256,'state',state,'attachment_mode',attachment_mode,'date_added',date_added,'backup_path',backup_path,'total',json_array_length(input_json,'$.items'),'report',json(report_json)) FROM import_runs ORDER BY date_added DESC,id DESC")?;
        let rows=stmt.query_map([],|r|r.get::<_,String>(0))?;
        let mut history=Vec::new();
        for row in rows {
            let mut entry:Value=serde_json::from_str(&row?)?;
            if entry["report"].is_null() {
                let mut results=conn.prepare("SELECT result_json FROM import_results WHERE run_id=?1 ORDER BY ordinal")?;
                let committed=results.query_map([entry["id"].as_str().unwrap()],|r|r.get::<_,String>(0))?
                    .map(|r|Ok(serde_json::from_str::<Value>(&r?)?)).collect::<Result<Vec<_>>>()?;
                entry["report"]=json!({"run_id":entry["id"],"state":entry["state"],"results":committed,"complete":false,"warnings":["実行は完了していません。保存済みの文献単位の結果を表示しています"]});
            }
            let mut pending = std::collections::BTreeSet::new();
            if let Some(results) = entry["report"]["results"].as_array() {
                for result in results {
                    for att in result["attachments"].as_array().into_iter().flatten() {
                        if let Some(key) = att["target_key"].as_str() {
                            let needs_index: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM attachments a LEFT JOIN fulltext f ON f.attachment_id=a.id WHERE a.key=?1 AND (f.status IS NULL OR f.status IN ('error','partial') OR f.md5 IS NOT a.md5))", [key], |r|r.get(0))?;
                            if needs_index { pending.insert(key.to_string()); }
                        }
                    }
                }
            }
            entry["report"]["fulltext_pending"] = json!(pending.len());
            if !pending.is_empty() { entry["report"]["complete"] = json!(false); }
            history.push(entry);
        }
        Ok(history)
    }).map(Option::unwrap_or_default)
}
