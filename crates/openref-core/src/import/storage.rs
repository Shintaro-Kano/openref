//! Filesystem operations anchored to directory descriptors, never a re-resolved
//! destination path. The path is used only to detect renamed/replaced ancestors.
use crate::{CoreError, Result};
#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::MetadataExt},
};
use std::{
    ffi::OsStr,
    fs::File,
    path::{Component, Path, PathBuf},
};

pub(crate) struct Dir {
    file: File,
    path: PathBuf,
}
fn changed() -> CoreError {
    CoreError::Conflict("移行の保存先が変更されました。コピーを採用せず保留します".into())
}
#[cfg(unix)]
fn name(value: &OsStr) -> Result<std::ffi::CString> {
    if Path::new(value).components().count() != 1
        || !matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(changed());
    }
    std::ffi::CString::new(value.as_bytes()).map_err(|_| changed())
}
pub(crate) fn identity(m: &std::fs::Metadata) -> Option<(String, String)> {
    #[cfg(unix)]
    {
        Some((m.dev().to_string(), m.ino().to_string()))
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        None
    }
}
impl Dir {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(changed());
        }
        let mut dir = Self {
            file: File::open("/")?,
            path: PathBuf::from("/"),
        };
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(n) => dir = dir.child(n, false)?,
                _ => return Err(changed()),
            }
        }
        Ok(dir)
    }
    pub fn child(&self, value: &OsStr, create: bool) -> Result<Self> {
        #[cfg(unix)]
        {
            let n = name(value)?;
            if create {
                // SAFETY: the directory descriptor is live and n is NUL terminated.
                let rc = unsafe { libc::mkdirat(self.file.as_raw_fd(), n.as_ptr(), 0o700) };
                if rc != 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                self.file.sync_all()?;
            }
            // SAFETY: openat returns a new owned fd, O_NOFOLLOW rejects symlinks.
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    n.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Self {
                file: unsafe { File::from_raw_fd(fd) },
                path: self.path.join(value),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (value, create);
            Err(changed())
        }
    }
    pub fn verify(&self) -> Result<()> {
        let current = Self::open(&self.path).map_err(|_| changed())?;
        if identity(&current.file.metadata()?) != identity(&self.file.metadata()?) {
            return Err(changed());
        }
        Ok(())
    }
    pub fn open_file(&self, value: &OsStr, create: bool) -> Result<File> {
        #[cfg(unix)]
        {
            let n = name(value)?;
            let flags = libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | if create {
                    libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
                } else {
                    libc::O_RDONLY
                };
            // SAFETY: arguments are valid, successful fd is adopted exactly once.
            let fd = unsafe { libc::openat(self.file.as_raw_fd(), n.as_ptr(), flags, 0o600) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let file = unsafe { File::from_raw_fd(fd) };
            if !file.metadata()?.is_file() {
                return Err(changed());
            }
            Ok(file)
        }
        #[cfg(not(unix))]
        {
            let _ = (value, create);
            Err(changed())
        }
    }
    pub fn publish(&self, from: &Dir, source: &OsStr, dest: &OsStr) -> Result<()> {
        self.verify()?;
        from.verify()?;
        #[cfg(unix)]
        {
            let a = name(source)?;
            let b = name(dest)?;
            // SAFETY: both fds are live directories; flags=0 never follows a source symlink.
            if unsafe {
                libc::linkat(
                    from.file.as_raw_fd(),
                    a.as_ptr(),
                    self.file.as_raw_fd(),
                    b.as_ptr(),
                    0,
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            self.file.sync_all()?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (from, source, dest);
            Err(changed())
        }
    }
    /// Inspect without opening the file: even closing a rejected DB alias fd
    /// could release this process's SQLite record locks.
    pub fn file_identity(&self, value: &OsStr) -> Result<Option<(String, String)>> {
        #[cfg(unix)]
        {
            let n = name(value)?;
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: live directory fd, valid name, initialized only on success.
            if unsafe { libc::fstatat(self.file.as_raw_fd(), n.as_ptr(), stat.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let stat = unsafe { stat.assume_init() };
            if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
                return Err(changed());
            }
            Ok(Some((stat.st_dev.to_string(), stat.st_ino.to_string())))
        }
        #[cfg(not(unix))]
        { let _ = value; Err(changed()) }
    }
    pub fn remove_owned(&self, value: &OsStr, device: &str, inode: &str) -> Result<()> {
        self.verify()?;
        if self.file_identity(value)? != Some((device.into(), inode.into())) {
            return Err(CoreError::Conflict(
                "この実行が作成したファイルではないため保持しました".into(),
            ));
        }
        #[cfg(unix)]
        {
            let n = name(value)?;
            // SAFETY: unlink is relative to the validated, held parent directory.
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), n.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            self.file.sync_all()?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(changed())
        }
    }
    pub fn remove_empty(&self, value: &OsStr) -> Result<()> {
        self.verify()?;
        #[cfg(unix)]
        {
            let n = name(value)?;
            // SAFETY: AT_REMOVEDIR never recursively removes contents or follows symlinks.
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), n.as_ptr(), libc::AT_REMOVEDIR) } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = value;
            Err(changed())
        }
    }
}
