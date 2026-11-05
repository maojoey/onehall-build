use super::super::read;
use super::{conflict, failed};
use crate::app_error::AppCommandError;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub(super) struct Writer {
    root: File,
    directory: File,
    lock: File,
    temporary: Option<(CString, File)>,
}

fn same(a: &File, b: &File) -> bool {
    match (a.metadata(), b.metadata()) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

fn create(directory: &File, name: &CString, exclusive: bool) -> Result<File, AppCommandError> {
    let flags = libc::O_RDWR
        | libc::O_CREAT
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if exclusive { libc::O_EXCL } else { 0 };
    // SAFETY: the directory and CString remain live; each successful fd is
    // immediately owned. Neither a symlink nor an existing temp is followed.
    // mode_t is u16 on macOS, so pass the C-promoted integer to varargs.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags,
            0o600 as libc::c_int,
        )
    };
    if fd < 0 {
        return Err(failed());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|_| failed())?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(failed());
    }
    Ok(file)
}

impl Writer {
    pub(super) fn open(root: &read::Root, _path: &Path) -> Result<Self, AppCommandError> {
        let name = CString::new(".onehall").expect("fixed name");
        // SAFETY: a fixed relative name under the pinned root directory.
        if unsafe { libc::mkdirat(root.0.as_raw_fd(), name.as_ptr(), 0o700) } != 0
            && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(failed());
        }
        let directory = read::child(&root.0, OsStr::new(".onehall"), true).map_err(|_| failed())?;
        let lock = create(
            &directory,
            &CString::new(".agent.ui.lock").expect("fixed name"),
            false,
        )?;
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => conflict(),
            std::fs::TryLockError::Error(_) => failed(),
        })?;
        Ok(Self {
            root: root.0.try_clone().map_err(|_| failed())?,
            directory,
            lock,
            temporary: None,
        })
    }

    pub(super) fn stage(&mut self, bytes: &[u8]) -> Result<(), AppCommandError> {
        let name = CString::new(format!(".agent.ui-{}.tmp", uuid::Uuid::new_v4().simple()))
            .expect("generated name");
        let file = create(&self.directory, &name, true)?;
        self.temporary = Some((name, file));
        let file = &mut self.temporary.as_mut().expect("staged file").1;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| failed())
    }

    pub(super) fn validate(&self, root: &read::Root, path: &Path) -> Result<(), AppCommandError> {
        let fresh = read::Root::open(path).map_err(|_| conflict())?;
        let directory =
            read::child(&root.0, OsStr::new(".onehall"), true).map_err(|_| conflict())?;
        let lock =
            read::child(&directory, OsStr::new(".agent.ui.lock"), false).map_err(|_| conflict())?;
        let (name, file) = self.temporary.as_ref().ok_or_else(failed)?;
        let temporary = read::child(&directory, OsStr::from_bytes(name.as_bytes()), false)
            .map_err(|_| conflict())?;
        if !same(&fresh.0, &self.root)
            || !same(&directory, &self.directory)
            || !same(&lock, &self.lock)
            || !same(&temporary, file)
        {
            return Err(conflict());
        }
        Ok(())
    }

    pub(super) fn commit(&mut self) -> Result<(), AppCommandError> {
        let name = &self.temporary.as_ref().ok_or_else(failed)?.0;
        let destination = CString::new("agent.ui.yaml").expect("fixed name");
        // SAFETY: both names are fixed/generated relative basenames under the
        // same held directory. renameat atomically replaces the entry itself.
        if unsafe {
            libc::renameat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                self.directory.as_raw_fd(),
                destination.as_ptr(),
            )
        } != 0
        {
            return Err(failed());
        }
        self.temporary = None;
        self.directory
            .sync_all()
            .and_then(|_| self.root.sync_all())
            .map_err(|_| failed())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let Some((name, file)) = &self.temporary {
            if read::child(&self.directory, OsStr::from_bytes(name.as_bytes()), false)
                .is_ok_and(|current| same(&current, file))
            {
                // SAFETY: remove only this operation's still-owned temp inode.
                unsafe {
                    libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0);
                }
            }
        }
    }
}
