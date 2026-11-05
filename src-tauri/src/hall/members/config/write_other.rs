//! Path-based fallback, with ancestor/reparse checks and held identity handles
//! on Windows. Same-user directory races are not an OS sandbox guarantee.
use super::super::read;
use super::{conflict, failed};
use crate::app_error::AppCommandError;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) struct Writer {
    path: PathBuf,
    directory: PathBuf,
    lock: File,
    root_identity: Identity,
    directory_identity: Identity,
    temporary: Option<tempfile::NamedTempFile>,
}

#[cfg(windows)]
type Identity = (u32, u32, u32);
#[cfg(windows)]
fn identity(path: &Path, directory: bool) -> Result<Identity, AppCommandError> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT,
    };
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        )
        .open(path)
        .map_err(|_| failed())?;
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: a live owned handle and sufficient output storage; assume_init
    // occurs only after Win32 reports a successful complete write.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(failed());
    }
    let info = unsafe { info.assume_init() };
    if info.dwFileAttributes & 0x400 != 0 {
        return Err(failed());
    }
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

#[cfg(not(windows))]
type Identity = PathBuf;
#[cfg(not(windows))]
fn identity(path: &Path, _directory: bool) -> Result<Identity, AppCommandError> {
    path.canonicalize().map_err(|_| failed())
}

impl Writer {
    pub(super) fn open(_root: &read::Root, path: &Path) -> Result<Self, AppCommandError> {
        let directory = path.join(".onehall");
        match std::fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(failed()),
        }
        read::Root::open(&directory).map_err(|_| failed())?;
        let lock_path = directory.join(".agent.ui.lock");
        if let Ok(metadata) = std::fs::symlink_metadata(&lock_path) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(failed());
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let lock = options.open(&lock_path).map_err(|_| failed())?;
        if !lock.metadata().map_err(|_| failed())?.is_file() {
            return Err(failed());
        }
        identity(&lock_path, false)?;
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => conflict(),
            std::fs::TryLockError::Error(_) => failed(),
        })?;
        Ok(Self {
            path: path.to_owned(),
            root_identity: identity(path, true)?,
            directory_identity: identity(&directory, true)?,
            directory,
            lock,
            temporary: None,
        })
    }

    pub(super) fn stage(&mut self, bytes: &[u8]) -> Result<(), AppCommandError> {
        self.temporary = Some(
            tempfile::Builder::new()
                .prefix(".agent.ui-")
                .suffix(".tmp")
                .tempfile_in(&self.directory)
                .map_err(|_| failed())?,
        );
        let file = self.temporary.as_mut().ok_or_else(failed)?.as_file_mut();
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| failed())
    }

    pub(super) fn validate(&self, _root: &read::Root, _path: &Path) -> Result<(), AppCommandError> {
        read::Root::open(&self.directory).map_err(|_| conflict())?;
        if identity(&self.path, true)? != self.root_identity
            || identity(&self.directory, true)? != self.directory_identity
        {
            return Err(conflict());
        }
        // Keep the advisory lock owned through the atomic replacement.
        self.lock.metadata().map_err(|_| failed())?;
        Ok(())
    }

    pub(super) fn commit(&mut self) -> Result<(), AppCommandError> {
        let temporary = self.temporary.take().ok_or_else(failed)?;
        match temporary.persist(self.directory.join("agent.ui.yaml")) {
            Ok(_) => Ok(()),
            Err(error) => {
                self.temporary = Some(error.file);
                Err(failed())
            }
        }
    }
}
