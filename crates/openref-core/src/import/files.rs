//! Attachment inspection shared by migration adapters. Never writes or searches
//! by basename. Only explicitly selected roots are authorized for file reads.
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use unicode_normalization::UnicodeNormalization;

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResolvedPath {
    Found { path: PathBuf },
    Missing,
    Ambiguous { candidates: Vec<PathBuf> },
    Rejected { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileFingerprint {
    pub bytes: u64,
    pub sha256: String,
    pub md5: String,
}

impl FileFingerprint {
    /// Bounded memory, including multi-gigabyte attachments.
    pub fn read(path: &Path) -> Result<Self> {
        let mut file = std::fs::File::open(path)?;
        fingerprint_reader(&mut file)
    }

    pub fn verify(&self, path: &Path) -> Result<()> {
        if Self::read(path)? != *self {
            return Err(CoreError::Conflict(format!(
                "事前確認後に入力が変更されました: {}。もう一度確認してください",
                path.display()
            )));
        }
        Ok(())
    }
}

pub(crate) fn fingerprint_reader(reader: &mut impl Read) -> Result<FileFingerprint> {
    fingerprint_reader_controlled(reader, &AtomicBool::new(false), &mut |_| {})
}

pub(crate) fn fingerprint_reader_controlled(
    reader: &mut impl Read,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<FileFingerprint> {
    let mut hash = Sha256::new();
    let mut md5 = md5::Context::new();
    let mut bytes = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Conflict("入力の確認を中止しました".into()));
        }
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        md5.consume(&buffer[..n]);
        bytes += n as u64;
        progress(bytes);
    }
    Ok(FileFingerprint {
        bytes,
        sha256: format!("{:x}", hash.finalize()),
        md5: format!("{:x}", md5.compute()),
    })
}

/// A root failing canonicalization is an input error rather than an implicit
/// fallback to another location. No default Drive or home-directory discovery.
pub struct AttachmentRoots {
    roots: Vec<PathBuf>,
    aliases: Vec<(PathBuf, PathBuf)>,
    protected_db: std::sync::Mutex<Option<PathBuf>>,
    #[cfg(unix)]
    root_handles: Vec<std::fs::File>,
}

impl AttachmentRoots {
    pub fn new(roots: &[PathBuf]) -> Result<Self> {
        let mut canonical = BTreeSet::new();
        let mut aliases = Vec::new();
        for root in roots {
            let selected = std::path::absolute(root)?;
            let root = root.canonicalize()?;
            if !root.is_dir() {
                return Err(CoreError::BadInput(format!(
                    "添付ルートはフォルダを指定してください: {}",
                    root.display()
                )));
            }
            aliases.push((selected, root.clone()));
            aliases.push((root.clone(), root.clone()));
            canonical.insert(root);
        }
        let roots: Vec<_> = canonical.into_iter().collect();
        #[cfg(unix)]
        let root_handles = roots
            .iter()
            .map(|path| {
                open_beneath(
                    std::fs::File::open("/")?,
                    path.strip_prefix("/").unwrap(),
                    true,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            roots,
            aliases,
            protected_db: std::sync::Mutex::new(None),
            #[cfg(unix)]
            root_handles,
        })
    }

    pub fn protect_database(&self, path: &Path) {
        *self.protected_db.lock().unwrap() = Some(path.to_path_buf());
    }
    fn allowed(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    pub fn resolve_candidates(&self, paths: &[&str]) -> Result<ResolvedPath> {
        let mut found = BTreeSet::new();
        let mut rejection = None;
        for raw in paths {
            match self.resolve(raw)? {
                ResolvedPath::Found { path } => {
                    found.insert(path);
                }
                ResolvedPath::Ambiguous { candidates } => found.extend(candidates),
                ResolvedPath::Rejected { reason } => rejection = Some(reason),
                ResolvedPath::Missing => {}
            }
        }
        Ok(match found.len() {
            0 => rejection
                .map(|reason| ResolvedPath::Rejected { reason })
                .unwrap_or(ResolvedPath::Missing),
            1 => ResolvedPath::Found {
                path: found.into_iter().next().unwrap(),
            },
            _ => ResolvedPath::Ambiguous {
                candidates: found.into_iter().collect(),
            },
        })
    }

    pub fn resolve(&self, raw: &str) -> Result<ResolvedPath> {
        if raw.is_empty() || raw.contains('\0') {
            return Ok(ResolvedPath::Rejected {
                reason: "空または不正な添付パス".into(),
            });
        }
        let mut paths = BTreeSet::new();
        if let Ok(uri) = url::Url::parse(raw) {
            if uri.scheme() != "file" {
                return Ok(ResolvedPath::Rejected {
                    reason: "ローカル添付以外の URI は取得しません".into(),
                });
            }
            let Ok(path) = uri.to_file_path() else {
                return Ok(ResolvedPath::Rejected {
                    reason: "ローカル file URI ではありません".into(),
                });
            };
            paths.insert(path);
        } else {
            paths.insert(PathBuf::from(raw));
            if let Ok(decoded) = percent_encoding::percent_decode_str(raw).decode_utf8() {
                paths.insert(PathBuf::from(decoded.as_ref()));
            }
        }
        // Keep the literal and decoded spellings: if both exist, ask the user.
        let mut found = BTreeSet::new();
        let mut rejected = false;
        for path in paths {
            if path.to_string_lossy().contains('\0')
                || path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
            {
                rejected = true;
                continue;
            }
            if path.is_absolute() {
                // Check the lexical path before touching anything outside a root.
                let Some((alias, root)) = self
                    .aliases
                    .iter()
                    .find(|(alias, _)| path.starts_with(alias))
                else {
                    rejected = true;
                    continue;
                };
                self.resolve_components(
                    root,
                    path.strip_prefix(alias).unwrap(),
                    &mut found,
                    &mut rejected,
                )?;
            } else {
                for root in &self.roots {
                    self.resolve_components(root, &path, &mut found, &mut rejected)?;
                }
            }
        }
        Ok(match found.len() {
            0 if rejected => ResolvedPath::Rejected {
                reason: "選択された添付ルートの外を参照するパスは使用できません".into(),
            },
            0 => ResolvedPath::Missing,
            1 => ResolvedPath::Found {
                path: found.into_iter().next().unwrap(),
            },
            _ => ResolvedPath::Ambiguous {
                candidates: found.into_iter().collect(),
            },
        })
    }

    fn resolve_components(
        &self,
        root: &Path,
        relative: &Path,
        found: &mut BTreeSet<PathBuf>,
        rejected: &mut bool,
    ) -> Result<()> {
        let mut candidates = BTreeSet::from([root.to_path_buf()]);
        for part in relative.components() {
            let Component::Normal(name) = part else {
                continue;
            };
            let Some(name) = name.to_str() else {
                continue;
            };
            let spellings =
                BTreeSet::from([name.to_string(), name.nfc().collect(), name.nfd().collect()]);
            let mut next = BTreeSet::new();
            for parent in &candidates {
                for spelling in &spellings {
                    let path = parent.join(spelling);
                    match path.canonicalize() {
                        Ok(path) if self.allowed(&path) => {
                            next.insert(path);
                        }
                        Ok(_) => *rejected = true,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            candidates = next;
        }
        for path in candidates {
            if path.is_file() {
                found.insert(path);
            }
        }
        Ok(())
    }

    /// Re-resolve immediately before reading so symlinks changed after preview
    /// are checked again. Apply must do the same before copying/linking.
    pub fn inspect_pdf(&self, raw: &str) -> Result<Option<(PathBuf, FileFingerprint)>> {
        let ResolvedPath::Found { path } = self.resolve(raw)? else {
            return Ok(None);
        };
        self.inspect_resolved_pdf(&path).map(Some)
    }

    pub fn inspect_resolved_pdf(&self, path: &Path) -> Result<(PathBuf, FileFingerprint)> {
        self.inspect_controlled(path, &AtomicBool::new(false), &mut |_| {})
    }

    pub(crate) fn inspect_controlled(
        &self,
        path: &Path,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64),
    ) -> Result<(PathBuf, FileFingerprint)> {
        let path = path.canonicalize()?;
        if !self.allowed(&path) {
            return Err(CoreError::BadInput(
                "選択された添付ルートの外は読み取れません".into(),
            ));
        }
        if let Some(db) = self.protected_db.lock().unwrap().as_ref() {
            super::apply::reject_database_file(&path, db)?;
        }
        let mut file = self.open_selected_file(&path)?;
        if !file.metadata()?.is_file() {
            return Err(CoreError::BadInput(
                "添付は通常のファイルである必要があります".into(),
            ));
        }
        let mut header = [0; 5];
        if file.read_exact(&mut header).is_err() || &header != b"%PDF-" {
            return Err(CoreError::BadInput(format!(
                "PDF ヘッダーがありません: {}",
                path.display()
            )));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok((
            path,
            fingerprint_reader_controlled(&mut file, cancel, progress)?,
        ))
    }

    /// Open relative to directory handles captured at explicit root selection.
    /// O_NOFOLLOW on every component closes the canonicalize/open symlink race.
    #[cfg(unix)]
    pub(crate) fn open_selected_file(&self, path: &Path) -> Result<std::fs::File> {
        if let Some(db) = self.protected_db.lock().unwrap().as_ref() {
            super::apply::reject_database_file(path, db)?;
        }
        use std::os::unix::fs::MetadataExt;
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt as MacMetadataExt;
            if std::fs::metadata(path)?.st_flags() & 0x4000_0000 != 0 {
                return Err(CoreError::BadInput(
                    "オンライン専用 PDF は未取得です。先にダウンロードしてから再試行してください"
                        .into(),
                ));
            }
        }
        let (index, root) = self
            .roots
            .iter()
            .enumerate()
            .find(|(_, root)| path.starts_with(root))
            .ok_or_else(|| {
                CoreError::BadInput("選択された添付ルートの外は読み取れません".into())
            })?;
        let selected = self.root_handles[index].metadata()?;
        let current = root.metadata()?;
        if selected.dev() != current.dev() || selected.ino() != current.ino() {
            return Err(CoreError::Conflict(
                "事前確認中に添付ルートが置き換えられました".into(),
            ));
        }
        open_beneath(
            self.root_handles[index].try_clone()?,
            path.strip_prefix(root).unwrap(),
            false,
        )
    }

    #[cfg(not(unix))]
    pub(crate) fn open_selected_file(&self, path: &Path) -> Result<std::fs::File> {
        if let Some(db) = self.protected_db.lock().unwrap().as_ref() {
            super::apply::reject_database_file(path, db)?;
        }
        // Migration file reads on other platforms require an equivalent
        // directory-capability implementation before being enabled.
        Err(CoreError::BadInput(format!(
            "この OS での安全な添付読取は未対応です: {}",
            path.display()
        )))
    }
}

#[cfg(unix)]
fn open_beneath(
    mut parent: std::fs::File,
    relative: &Path,
    directory: bool,
) -> Result<std::fs::File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(CoreError::BadInput("不正な添付パス".into()));
        };
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| CoreError::BadInput("不正な添付パス".into()))?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if directory || components.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: parent owns a live descriptor, name is NUL-terminated,
        // no creation flags are used. A successful fd is owned exactly once.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: fd is new and has not yet been adopted by another owner.
        parent = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(parent)
}
