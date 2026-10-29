//! Inspect a database without letting SQLite touch the original or its sidecars.
//! Copy main+WAL into a private temporary directory, verify both source hashes
//! again, and let SQLite recover/read only the copy. Busy copies are retried.
use super::{files::FileFingerprint, plan::ExistingLibrary};
use crate::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "openref-import-snapshot-{}-{}",
            std::process::id(),
            crate::keys::new_key()
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(suffix);
    p.into()
}

fn fingerprint_if_present(path: &Path) -> Result<Option<FileFingerprint>> {
    match FileFingerprint::read(path) {
        Ok(f) => Ok(Some(f)),
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

// On POSIX, closing ANY fd for an inode releases the process's record locks
// for that inode. The desktop may already have SQLite's live connection open.
// Never open the original DB here, even just to hash it. A separate process
// owns every source read; only the private copies are read in this process.
fn private_copy(source: &Path, dest: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let copied = std::process::Command::new("/bin/cp")
            .arg("--")
            .arg(source)
            .arg(dest)
            .output()?;
        if !copied.status.success() {
            let kind = if !source.try_exists()? {
                std::io::ErrorKind::NotFound
            } else {
                std::io::ErrorKind::Other
            };
            return Err(std::io::Error::new(
                kind,
                format!(
                    "DBの私用コピーに失敗: {}",
                    String::from_utf8_lossy(&copied.stderr).trim()
                ),
            )
            .into());
        }
    }
    #[cfg(not(unix))]
    std::fs::copy(source, dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn source_fingerprint(path: &Path, check: &Path) -> Result<Option<FileFingerprint>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    match private_copy(path, check) {
        Ok(()) => FileFingerprint::read(check).map(Some),
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// No source SQLite connection, locks, migration, or journal-mode changes.
/// The clone may contain transient SQLite sidecars; Scratch removes all of them.
/// Preview is advisory: execute always re-matches in a live write transaction.
pub fn with_snapshot<T>(path: &Path, read: impl Fn(&Connection) -> Result<T>) -> Result<Option<T>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    let scratch = Scratch::new()?;
    let cloned = scratch.0.join("snapshot.sqlite");
    let check = scratch.0.join("fingerprint-copy");
    let wal = sidecar(path, "-wal");
    let journal = sidecar(path, "-journal");
    for _ in 0..3 {
        // A rollback journal could contain pages needed to recover the main DB.
        // Do not attempt an unlocked copy of a rollback transaction in progress.
        if journal.try_exists()? {
            continue;
        }
        let main_before = source_fingerprint(path, &check)?;
        let wal_before = source_fingerprint(&wal, &check)?;
        if main_before.is_none() {
            continue;
        }
        private_copy(path, &cloned)?;
        let clone_wal = sidecar(&cloned, "-wal");
        if wal_before.is_some() {
            if let Err(e) = private_copy(&wal, &clone_wal) {
                if matches!(&e,CoreError::Io(io) if io.kind()==std::io::ErrorKind::NotFound) {
                    continue;
                }
                return Err(e);
            }
        } else if clone_wal.try_exists()? {
            std::fs::remove_file(&clone_wal)?;
        }
        let main_copy = Some(FileFingerprint::read(&cloned)?);
        let wal_copy = fingerprint_if_present(&clone_wal)?;
        if main_before != main_copy
            || wal_before != wal_copy
            || main_before != source_fingerprint(path, &check)?
            || wal_before != source_fingerprint(&wal, &check)?
            || journal.try_exists()?
        {
            continue;
        }
        let conn =
            Connection::open_with_flags(&cloned, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let integrity: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(CoreError::Parse(format!(
                "DB の一時コピーを検証できません: {integrity}"
            )));
        }
        return read(&conn).map(Some);
    }
    Err(CoreError::Conflict(
        "DB が更新中のため事前確認用コピーを確定できませんでした。もう一度確認してください".into(),
    ))
}

pub fn read(path: &Path) -> Result<ExistingLibrary> {
    with_snapshot(path, |conn| {
        let has_items: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='items' AND type='table')", [], |r|r.get(0))?;
        if has_items { ExistingLibrary::read(conn) } else {
            let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |r|r.get(0))?;
            if tables == 0 { Ok(ExistingLibrary::default()) } else { Err(CoreError::Parse("OpenRef の DB ではありません".into())) }
        }
    }).map(Option::unwrap_or_default)
}

pub(crate) struct ManagedAttachment {
    pub scope: String,
    pub object_id: String,
    pub target_key: String,
    pub item_key: String,
    pub path: PathBuf,
}

pub(crate) fn read_import(
    path: &Path,
    input: &super::ImportInput,
) -> Result<(
    ExistingLibrary,
    Vec<ManagedAttachment>,
    super::provenance::CrossExport,
)> {
    with_snapshot(path,|conn| {
        let version:i64=conn.query_row("PRAGMA user_version",[],|r|r.get(0))?;
        if version>crate::db::SCHEMA_VERSION{return Err(CoreError::BadInput("この DB は新しい OpenRef の形式です".into()));}
        let has_items:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='items' AND type='table')",[],|r|r.get(0))?;
        if !has_items {
            let tables:i64=conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0))?;
            if tables>0{return Err(CoreError::Parse("OpenRef の DB ではありません".into()));}
        }
        let library=if has_items{ExistingLibrary::read(conn)?}else{ExistingLibrary::default()};
        let has_objects:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='import_objects' AND type='table')",[],|r|r.get(0))?;
        if !has_objects{return Ok((library,Vec::new(),super::provenance::CrossExport::default()));}
        let format:Option<String>=conn.query_row("SELECT format FROM import_sources WHERE id=?1",[&input.source.id],|r|r.get(0)).optional()?;
        if format.as_deref().is_some_and(|f|f!=input.source.format){return Err(CoreError::BadInput("選択した移行元の形式が一致しません".into()));}
        let mut stmt=conn.prepare("SELECT o.scope,o.object_id,o.target_key,i.key,a.path FROM import_objects o JOIN attachments a ON a.key=o.target_key JOIN items i ON i.id=a.item_id WHERE o.source_id=?1 AND o.kind='attachment' AND o.scope IN ('',?2) AND a.mode='managed'")?;
        let rows=stmt.query_map(rusqlite::params![input.source.id,input.input_sha256],|r|Ok(ManagedAttachment{scope:r.get(0)?,object_id:r.get(1)?,target_key:r.get(2)?,item_key:r.get(3)?,path:PathBuf::from(r.get::<_,String>(4)?)}))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok((library,rows,super::provenance::CrossExport::read(conn,input)?))
    }).map(Option::unwrap_or_default)
}
