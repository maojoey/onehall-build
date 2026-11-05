//! Handle-anchored filesystem operations. Never reopen an untrusted absolute
//! pathname after canonicalization. Windows holds ancestor handles without
//! write/delete sharing; Unix walks with openat/O_NOFOLLOW. Private bytes are published
//! before their database grants, so crash leftovers never confer read access.
use super::{manifest::validate_name, types::*};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    identity: [u64; 2],
    size: u64,
    links: u64,
    modified: i128,
    changed: i128,
    mode: u32,
}
#[cfg(unix)]
fn stamp(file: &File, regular: bool) -> Result<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    if (regular && (!m.is_file() || m.nlink() != 1)) || (!regular && !m.is_dir()) {
        return Err(ExchangeError::Unstable);
    }
    Ok(Stamp {
        identity: [m.dev(), m.ino()],
        size: m.len(),
        links: m.nlink(),
        modified: m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128,
        changed: m.ctime() as i128 * 1_000_000_000 + m.ctime_nsec() as i128,
        mode: m.mode(),
    })
}
#[cfg(windows)]
fn stamp(file: &File, regular: bool) -> Result<Stamp> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
    };
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let mut basic = std::mem::MaybeUninit::<FILE_BASIC_INFO>::uninit();
    // SAFETY: owned live file handle and correctly-sized output structures.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                basic.as_mut_ptr().cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
    {
        return Err(ExchangeError::Storage);
    }
    let info = unsafe { info.assume_init() };
    let basic = unsafe { basic.assume_init() };
    let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DEVICE) != 0
        || regular == directory
        || (regular && info.nNumberOfLinks != 1)
    {
        return Err(ExchangeError::Unstable);
    }
    Ok(Stamp {
        identity: [
            info.dwVolumeSerialNumber as u64,
            ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        ],
        size: ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64,
        links: info.nNumberOfLinks as u64,
        modified: basic.LastWriteTime as i128,
        changed: basic.ChangeTime as i128,
        mode: basic.FileAttributes,
    })
}

pub(super) struct Directory {
    file: File,
    path: PathBuf,
    parent: Option<Arc<Directory>>,
    name: String,
}
impl Directory {
    pub(super) fn absolute(path: &Path) -> Result<Arc<Self>> {
        let mut parts = path.components();
        #[cfg(unix)]
        let (file, base) = {
            use std::os::unix::fs::OpenOptionsExt;
            if parts.next() != Some(Component::RootDir) {
                return Err(ExchangeError::Invalid);
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open("/")?;
            (file, PathBuf::from("/"))
        };
        #[cfg(windows)]
        let (file, base) = {
            let Some(Component::Prefix(prefix)) = parts.next() else {
                return Err(ExchangeError::Invalid);
            };
            if !matches!(prefix.kind(), std::path::Prefix::Disk(_))
                || parts.next() != Some(Component::RootDir)
            {
                return Err(ExchangeError::Invalid);
            }
            let base = PathBuf::from(format!(
                "{}\\",
                prefix.as_os_str().to_str().ok_or(ExchangeError::Invalid)?
            ));
            (open_windows(&base, true, false)?, base)
        };
        let mut dir = Arc::new(Self {
            file,
            path: base,
            parent: None,
            name: String::new(),
        });
        for part in parts {
            let Component::Normal(name) = part else {
                return Err(ExchangeError::Invalid);
            };
            dir = dir.child(name.to_str().ok_or(ExchangeError::Invalid)?)?;
        }
        Ok(dir)
    }
    fn child(self: &Arc<Self>, name: &str) -> Result<Arc<Self>> {
        validate_name(name)?;
        let file = self.open(name, true, false)?;
        stamp(&file, false)?;
        Ok(Arc::new(Self {
            file,
            path: self.path.join(name),
            parent: Some(self.clone()),
            name: name.into(),
        }))
    }
    pub(super) fn ensure(self: &Arc<Self>, name: &str) -> Result<Arc<Self>> {
        validate_name(name)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = std::ffi::CString::new(name).map_err(|_| ExchangeError::Invalid)?;
            // SAFETY: this directory descriptor is held through the syscall.
            if unsafe { libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error.into());
                }
            }
        }
        #[cfg(windows)]
        if let Err(e) = std::fs::create_dir(self.path.join(name)) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(e.into());
            }
        }
        self.child(name)
    }
    fn open(&self, name: &str, directory: bool, create: bool) -> Result<File> {
        validate_name(name)?;
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = std::ffi::CString::new(name).map_err(|_| ExchangeError::Invalid)?;
            let flags = libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | if directory {
                    libc::O_RDONLY | libc::O_DIRECTORY
                } else if create {
                    libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
                } else {
                    libc::O_RDONLY
                };
            // SAFETY: anchored parent, valid name, ownership taken exactly once.
            let fd = unsafe { libc::openat(self.file.as_raw_fd(), name.as_ptr(), flags, 0o600) };
            if fd < 0 {
                return Err(ExchangeError::Unstable);
            }
            Ok(unsafe { File::from_raw_fd(fd) })
        }
        #[cfg(windows)]
        {
            open_windows(&self.path.join(name), directory, create)
        }
    }
    pub(super) fn validate(&self) -> Result<()> {
        if let Some(parent) = &self.parent {
            parent.validate()?;
            let current = parent.open(&self.name, true, false)?;
            if stamp(&current, false)?.identity != stamp(&self.file, false)?.identity {
                return Err(ExchangeError::Unstable);
            }
        }
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        #[cfg(unix)]
        self.file.sync_all()?;
        Ok(())
    }
    fn unlink(&self, name: &str) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if let Ok(name) = std::ffi::CString::new(name) {
                unsafe {
                    libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0);
                }
            }
        }
        #[cfg(windows)]
        {
            let _ = std::fs::remove_file(self.path.join(name));
        }
    }
    fn publish(&self, name: &str, destination: &Directory, new_name: &str) -> Result<()> {
        self.validate()?;
        destination.validate()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let old = std::ffi::CString::new(name).map_err(|_| ExchangeError::Invalid)?;
            let new = std::ffi::CString::new(new_name).map_err(|_| ExchangeError::Invalid)?;
            // linkat never replaces a preexisting name; only private temp files
            // use this operation. The final object must have one link again.
            if unsafe {
                libc::linkat(
                    self.file.as_raw_fd(),
                    old.as_ptr(),
                    destination.file.as_raw_fd(),
                    new.as_ptr(),
                    0,
                )
            } < 0
            {
                return Err(ExchangeError::Storage);
            }
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), old.as_ptr(), 0) } < 0 {
                destination.unlink(new_name);
                return Err(ExchangeError::Storage);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
            let old: Vec<u16> = self
                .path
                .join(name)
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            let new: Vec<u16> = destination
                .path
                .join(new_name)
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            // No MOVEFILE_REPLACE_EXISTING. All ancestors stay pinned.
            if unsafe { MoveFileExW(old.as_ptr(), new.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
                return Err(ExchangeError::Storage);
            }
        }
        destination.sync()?;
        self.sync()?;
        Ok(())
    }
}
#[cfg(windows)]
fn open_windows(path: &Path, directory: bool, create: bool) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create_new(create)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        )
        .open(path)
        .map_err(|_| ExchangeError::Unstable)?;
    stamp(&file, !directory)?;
    Ok(file)
}

pub(super) struct Temporary {
    directory: Arc<Directory>,
    name: String,
    pub(super) file: Option<File>,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        self.file.take();
        self.directory.unlink(&self.name);
    }
}
impl Temporary {
    pub(super) fn publish(mut self, blobs: &Arc<Directory>, size: u64) -> Result<String> {
        let file = self.file.take().ok_or(ExchangeError::Storage)?;
        let before = stamp(&file, true)?;
        if before.size != size {
            return Err(ExchangeError::Unstable);
        }
        file.sync_all()?;
        readonly(&file)?;
        drop(file); // Windows must release the no-delete file handle to rename.
        let blob = id();
        self.directory.publish(&self.name, blobs, &blob)?;
        let published = blobs.open(&blob, false, false)?;
        let after = stamp(&published, true)?;
        if after.identity != before.identity
            || after.size != size
            || !published.metadata()?.permissions().readonly()
        {
            return Err(ExchangeError::Unstable);
        }
        Ok(blob)
    }
}
fn readonly(file: &File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o400))?;
    }
    #[cfg(windows)]
    {
        let mut p = file.metadata()?.permissions();
        p.set_readonly(true);
        file.set_permissions(p)?;
    }
    Ok(())
}
pub(super) struct Source {
    directory: Arc<Directory>,
    name: String,
    file: File,
    before: Stamp,
}
impl Source {
    pub(super) fn open(directory: Arc<Directory>, name: &str) -> Result<Self> {
        directory.validate()?;
        let file = directory.open(name, false, false)?;
        let before = stamp(&file, true)?;
        Ok(Self {
            directory,
            name: name.into(),
            file,
            before,
        })
    }
    pub(super) fn name(&self) -> &str {
        &self.name
    }
    pub(super) fn len(&self) -> u64 {
        self.before.size
    }
    /// The handle opened through the anchored walk, for streaming to a host
    /// that verifies size and hash itself.
    #[cfg_attr(not(feature = "tauri-runtime"), allow(dead_code))]
    pub(super) fn into_file(self) -> File {
        self.file
    }
    fn unchanged(&self) -> Result<()> {
        self.directory.validate()?;
        let current = self.directory.open(&self.name, false, false)?;
        if stamp(&self.file, true)? != self.before || stamp(&current, true)? != self.before {
            return Err(ExchangeError::Unstable);
        }
        Ok(())
    }
    pub(super) fn copy<W: Write>(
        &mut self,
        destination: &mut W,
        limit: u64,
    ) -> Result<(u64, String)> {
        let result = copy_bounded(&mut self.file, destination, limit)?;
        if result.0 != self.before.size {
            return Err(ExchangeError::Unstable);
        }
        self.unchanged()?;
        Ok(result)
    }
}
pub(super) fn copy_bounded<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    limit: u64,
) -> Result<(u64, String)> {
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    let mut hash = Sha256::new();
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size = size.checked_add(n as u64).ok_or(ExchangeError::Quota)?;
        if size > limit || size > MAX_FILE_BYTES {
            return Err(ExchangeError::Quota);
        }
        hash.update(&buffer[..n]);
        writer.write_all(&buffer[..n])?;
    }
    Ok((size, format!("{:x}", hash.finalize())))
}
pub(super) struct Layout {
    root: Arc<Directory>,
    pub(super) blobs: Arc<Directory>,
    temporary: Arc<Directory>,
    attempts: Arc<Directory>,
    #[cfg(windows)]
    _privacy: crate::cli_run::discovery::PreparedDir,
}
impl Layout {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let parent = Directory::absolute(path.parent().ok_or(ExchangeError::Invalid)?)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(ExchangeError::Invalid)?;
        let root = parent.ensure(name)?;
        #[cfg(unix)]
        verify_private(&root.file)?;
        #[cfg(windows)]
        let privacy =
            crate::cli_run::discovery::prepare_dir(path).map_err(|_| ExchangeError::Storage)?;
        root.validate()?;
        let blobs = root.ensure("blobs")?;
        let temporary = root.ensure("temporary")?;
        let attempts = root.ensure("attempts")?;
        Ok(Self {
            root,
            blobs,
            temporary,
            attempts,
            #[cfg(windows)]
            _privacy: privacy,
        })
    }
    pub(super) fn temporary(&self) -> Result<Temporary> {
        self.root.validate()?;
        let name = id();
        let file = self.temporary.open(&name, false, true)?;
        Ok(Temporary {
            directory: self.temporary.clone(),
            name,
            file: Some(file),
        })
    }
    pub(super) fn blob(&self, blob: &str, size: u64, hash: &str) -> Result<File> {
        uuid(blob)?;
        let mut source = Source::open(self.blobs.clone(), blob)?;
        if !source.file.metadata()?.permissions().readonly() || source.len() != size {
            return Err(ExchangeError::Unstable);
        }
        let (actual, digest) = copy_bounded(&mut source.file, &mut std::io::sink(), size)?;
        if actual != size || digest != hash {
            return Err(ExchangeError::Unstable);
        }
        source.unchanged()?;
        source.file.seek(SeekFrom::Start(0))?;
        Ok(source.file)
    }
    pub(super) fn existing_paths(
        &self,
        nonce: &str,
        manifest: &ExchangeManifest,
    ) -> Result<super::store::MaterializedAttempt> {
        uuid(nonce)?;
        let attempt = self.attempts.child(nonce)?;
        let mut inputs = std::collections::BTreeMap::new();
        for object in &manifest.inputs {
            let directory = attempt.child("inputs")?.child(&object.object_id)?;
            let mut source = Source::open(directory.clone(), &object.display_name)?;
            if !source.file.metadata()?.permissions().readonly() {
                return Err(ExchangeError::Unstable);
            }
            let (size, hash) = source.copy(&mut std::io::sink(), object.size)?;
            if size != object.size || hash != object.sha256 {
                return Err(ExchangeError::Unstable);
            }
            inputs.insert(
                object.object_id.clone(),
                directory.path.join(&object.display_name),
            );
        }
        let mut outputs = std::collections::BTreeMap::new();
        for output in &manifest.outputs {
            let directory = attempt.child("outputs")?.child(&output.slot_id)?;
            outputs.insert(
                output.slot_id.clone(),
                directory.path.join(&output.display_name),
            );
        }
        Ok(super::store::MaterializedAttempt { inputs, outputs })
    }
    pub(super) fn attempt_directory(&self, nonce: &str) -> Result<Arc<Directory>> {
        uuid(nonce)?;
        self.attempts.ensure(nonce)
    }
    pub(super) fn input_copy(
        &self,
        attempt: &Arc<Directory>,
        object: &ObjectRef,
        blob: &str,
    ) -> Result<PathBuf> {
        let directory = attempt.ensure("inputs")?.ensure(&object.object_id)?;
        let mut target = directory.open(&object.display_name, false, true)?;
        let mut source = Source::open(self.blobs.clone(), blob)?;
        let (size, hash) = source.copy(&mut target, object.size)?;
        if size != object.size || hash != object.sha256 {
            return Err(ExchangeError::Unstable);
        }
        target.sync_all()?;
        readonly(&target)?;
        directory.sync()?;
        Ok(directory.path.join(&object.display_name))
    }
    pub(super) fn output_directory(&self, nonce: &str, slot: &str) -> Result<Arc<Directory>> {
        uuid(nonce)?;
        uuid(slot)?;
        self.attempts.child(nonce)?.child("outputs")?.child(slot)
    }
    pub(super) fn prepare_outputs(
        &self,
        attempt: &Arc<Directory>,
        outputs: &[OutputSlot],
    ) -> Result<std::collections::BTreeMap<String, PathBuf>> {
        let root = attempt.ensure("outputs")?;
        let mut paths = std::collections::BTreeMap::new();
        for slot in outputs {
            let directory = root.ensure(&slot.slot_id)?;
            paths.insert(
                slot.slot_id.clone(),
                directory.path.join(&slot.display_name),
            );
        }
        Ok(paths)
    }
}

#[cfg(unix)]
fn verify_private(file: &File) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(ExchangeError::Forbidden);
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        extern "C" {
            fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut libc::c_void;
            fn acl_get_entry(
                acl: *mut libc::c_void,
                entry_id: libc::c_int,
                entry: *mut *mut libc::c_void,
            ) -> libc::c_int;
            fn acl_free(object: *mut libc::c_void) -> libc::c_int;
        }
        // Same ACL policy as cli_run/discovery: macOS ACLs can grant access
        // independently of mode bits. Query this held FD, never a pathname.
        unsafe {
            let acl = acl_get_fd_np(file.as_raw_fd(), 0x100);
            if acl.is_null() {
                let e = std::io::Error::last_os_error();
                if !matches!(
                    e.raw_os_error(),
                    Some(libc::ENOENT) | Some(libc::ENOTSUP) | Some(libc::EOPNOTSUPP)
                ) {
                    return Err(ExchangeError::Storage);
                }
            } else {
                let mut entry = std::ptr::null_mut();
                let any = acl_get_entry(acl, 0, &mut entry) == 0;
                acl_free(acl);
                if any {
                    return Err(ExchangeError::Forbidden);
                }
            }
        }
    }
    Ok(())
}
