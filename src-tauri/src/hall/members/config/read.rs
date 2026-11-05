//! Fixed-file, bounded reads. Never follows a declared cwd or file include.
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub(super) const PATHS: [&str; 3] = [
    "agent.yaml",
    ".onehall/agent.yaml",
    ".onehall/agent.ui.yaml",
];
pub(super) const LIMIT: u64 = 64 * 1024;

#[derive(Clone)]
pub(super) struct ReadFile {
    pub path: &'static str,
    pub data: Option<Vec<u8>>,
    pub error: Option<&'static str>,
}

pub(super) fn read_all(root: &Path) -> io::Result<Vec<ReadFile>> {
    let root = Root::open(root)?;
    Ok(read_from(&root))
}

pub(super) fn read_from(root: &Root) -> Vec<ReadFile> {
    PATHS
        .into_iter()
        .map(|path| {
            let result = root.file(path).and_then(|file| bounded(file, LIMIT));
            match result {
                Ok(Ok(data)) => ReadFile {
                    path,
                    data: Some(data),
                    error: None,
                },
                Ok(Err(code)) => ReadFile {
                    path,
                    data: None,
                    error: Some(code),
                },
                Err(error) if error.kind() == io::ErrorKind::NotFound => ReadFile {
                    path,
                    data: None,
                    error: None,
                },
                Err(_) => ReadFile {
                    path,
                    data: None,
                    error: Some("unreadable"),
                },
            }
        })
        .collect()
}

/// Read through the verified directory, never following an include or link.
/// Callers choose a fixed filename and a small limit; errors contain no bytes.
impl Root {
    pub(in crate::hall::members) fn read_fixed(
        &self,
        relative: &str,
        limit: u64,
    ) -> io::Result<Option<Vec<u8>>> {
        match self
            .file(relative)
            .and_then(|file| bounded(file, limit)?.map_err(io::Error::other))
        {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn bounded(file: File, limit: u64) -> io::Result<Result<Vec<u8>, &'static str>> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(Err("not_regular"));
    }
    if metadata.len() > limit {
        return Ok(Err("too_large"));
    }
    let mut data = Vec::new();
    file.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Ok(Err("too_large"));
    }
    Ok(Ok(data))
}

#[cfg(unix)]
pub(in crate::hall::members) struct Root(pub(super) File);

#[cfg(unix)]
pub(super) fn child(parent: &File, name: &std::ffi::OsStr, directory: bool) -> io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let name =
        std::ffi::CString::new(name.as_bytes()).map_err(|_| io::Error::other("Invalid path"))?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: the parent remains owned and open; the checked CString is valid
    // for this call, and every successful descriptor immediately gains an owner.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
impl Root {
    pub(in crate::hall::members) fn open(path: &Path) -> io::Result<Self> {
        use std::path::Component;
        let mut components = path.components();
        if components.next() != Some(Component::RootDir) {
            return Err(io::Error::other("Invalid root"));
        }
        let mut dir = File::open("/")?;
        for part in components {
            let Component::Normal(name) = part else {
                return Err(io::Error::other("Invalid root"));
            };
            dir = child(&dir, name, true)?;
        }
        Ok(Self(dir))
    }

    pub(in crate::hall::members) fn check_directory(&self, relative: &str) -> io::Result<()> {
        let mut dir = self.0.try_clone()?;
        for part in Path::new(relative).components() {
            let std::path::Component::Normal(name) = part else {
                return Err(io::Error::other("Invalid directory"));
            };
            match child(&dir, name, true) {
                Ok(next) => dir = next,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn file(&self, relative: &str) -> io::Result<File> {
        let mut dir = self.0.try_clone()?;
        let mut parts = Path::new(relative).components().peekable();
        while let Some(part) = parts.next() {
            let std::path::Component::Normal(name) = part else {
                return Err(io::Error::other("Invalid file"));
            };
            dir = child(&dir, name, parts.peek().is_some())?;
        }
        Ok(dir)
    }
}

#[cfg(not(unix))]
pub(in crate::hall::members) struct Root(std::path::PathBuf);

#[cfg(not(unix))]
impl Root {
    pub(in crate::hall::members) fn open(path: &Path) -> io::Result<Self> {
        check_ancestors(path)?;
        if !path.is_dir() {
            return Err(io::Error::other("Invalid root"));
        }
        Ok(Self(path.to_owned()))
    }

    pub(in crate::hall::members) fn check_directory(&self, relative: &str) -> io::Result<()> {
        let mut path = self.0.clone();
        for part in Path::new(relative).components() {
            let std::path::Component::Normal(name) = part else {
                return Err(io::Error::other("Invalid directory"));
            };
            path.push(name);
            match check_ancestors(&path) {
                Ok(()) if path.is_dir() => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                _ => return Err(io::Error::other("Invalid directory")),
            }
        }
        Ok(())
    }

    fn file(&self, relative: &str) -> io::Result<File> {
        let path = self.0.join(relative);
        check_ancestors(&path)?;
        crate::commands::folders::open_no_follow(&path)
    }
}

#[cfg(not(unix))]
fn check_ancestors(path: &Path) -> io::Result<()> {
    for entry in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let metadata = std::fs::symlink_metadata(entry)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("Link refused"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(io::Error::other("Reparse point refused"));
            }
        }
    }
    Ok(())
}
