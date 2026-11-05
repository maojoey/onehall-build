//! Optional, already provisioned per-run Linux domain (see `host` for the
//! directories the server creates). This module never creates groups or alters
//! mounts/permissions/controllers. Not a same-UID hard sandbox.
#[cfg(target_os = "linux")]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::fs::OpenOptionsExt,
};
#[cfg(target_os = "linux")]
use std::{fs::File, io::Read};
use std::{io, path::Path};
pub(crate) struct Cgroup {
    #[cfg(target_os = "linux")]
    dir: File,
}
impl Cgroup {
    #[cfg(target_os = "linux")]
    pub(crate) fn open(path: &Path, run: &str) -> io::Result<Self> {
        if path.file_name().and_then(|s| s.to_str()) != Some(run) || path.canonicalize()? != path {
            return Err(io::Error::other(
                "A canonical dedicated run domain is required",
            ));
        }
        let dir = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)?;
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        if unsafe { libc::fstatfs(dir.as_raw_fd(), fs.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { fs.assume_init() }.f_type != 0x63677270 {
            return Err(io::Error::other("A real cgroup v2 domain is required"));
        }
        let group = Self { dir };
        if group.read("cgroup.type")?.trim() != "domain" || group.populated()? {
            return Err(io::Error::other("Run domain must be empty and unthreaded"));
        }
        // Validate interfaces now; no write occurs before an explicit launch.
        group.file("cgroup.procs", libc::O_WRONLY)?;
        group.file("cgroup.kill", libc::O_WRONLY)?;
        Ok(group)
    }
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn open(_: &Path, _: &str) -> io::Result<Self> {
        Err(io::Error::other(
            "Verified cgroup v2 execution is unavailable on this platform",
        ))
    }
    #[cfg(target_os = "linux")]
    fn file(&self, name: &str, flags: i32) -> io::Result<File> {
        let name = std::ffi::CString::new(name)
            .map_err(|_| io::Error::other("Invalid domain interface"))?;
        let fd = unsafe {
            libc::openat(
                self.dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(target_os = "linux")]
    fn read(&self, name: &str) -> io::Result<String> {
        let mut value = String::new();
        self.file(name, libc::O_RDONLY)?
            .take(4096)
            .read_to_string(&mut value)?;
        Ok(value)
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn populated(&self) -> io::Result<bool> {
        let value = self.read("cgroup.events")?;
        let row = value
            .lines()
            .find_map(|l| l.strip_prefix("populated "))
            .ok_or_else(|| io::Error::other("Missing domain population evidence"))?;
        match row {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(io::Error::other("Invalid domain population evidence")),
        }
    }
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn populated(&self) -> io::Result<bool> {
        Err(io::Error::other("No kernel domain observer"))
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn configure(self: &std::sync::Arc<Self>, cmd: &mut tokio::process::Command) {
        let group = self.clone();
        unsafe {
            cmd.pre_exec(move || {
                // Only async-signal-safe calls between fork/exec. "0" enrolls the
                // current child before untrusted code can fork its descendants.
                let fd = libc::openat(
                    group.dir.as_raw_fd(),
                    c"cgroup.procs".as_ptr(),
                    libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                );
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let result = libc::write(fd, c"0\n".as_ptr().cast(), 2);
                let error = io::Error::last_os_error();
                libc::close(fd);
                if result != 2 {
                    return Err(error);
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn configure(self: &std::sync::Arc<Self>, _: &mut tokio::process::Command) {}
    #[cfg(target_os = "linux")]
    pub(crate) fn cancel(&self) -> io::Result<()> {
        use std::io::Write;
        self.file("cgroup.kill", libc::O_WRONLY)?.write_all(b"1\n")
    }
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn cancel(&self) -> io::Result<()> {
        Err(io::Error::other("No owned kernel domain"))
    }
}
