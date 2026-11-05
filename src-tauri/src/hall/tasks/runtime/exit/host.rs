//! Run-domain root inside the server's own cgroup v2 subtree. The service unit
//! must hand that subtree over (systemd `Delegate=yes`). Only empty children
//! named by host-generated UUIDs are created or removed here; no mounts,
//! permissions or controllers are changed.
#[cfg(any(target_os = "linux", test))]
use std::path::Path;
use std::{io, path::PathBuf};
pub(crate) struct RunRoot {
    dir: Option<PathBuf>,
    native_runs: std::sync::Mutex<std::collections::BTreeSet<String>>,
}
fn host_name(name: &str) -> io::Result<&str> {
    uuid::Uuid::parse_str(name)
        .map(|_| name)
        .map_err(|_| io::Error::other("Run domain names are host UUIDs"))
}
impl RunRoot {
    /// `hall-runs` below the server's own cgroup; created on first use.
    #[cfg(target_os = "linux")]
    pub(crate) fn detect() -> io::Result<Self> {
        let own = std::fs::read_to_string("/proc/self/cgroup")?
            .lines()
            .find_map(|l| l.strip_prefix("0::").map(str::to_owned))
            .ok_or_else(|| io::Error::other("No cgroup v2 membership"))?;
        let mount = std::fs::read_to_string("/proc/self/mountinfo")?
            .lines()
            .find_map(|l| {
                let (head, tail) = l.split_once(" - ")?;
                (tail.split(' ').next() == Some("cgroup2"))
                    .then(|| head.split(' ').nth(4).map(PathBuf::from))?
            })
            .ok_or_else(|| io::Error::other("No cgroup v2 mount"))?;
        Self::at(&mount.join(own.trim_start_matches('/')).join("hall-runs"))
    }
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn detect() -> io::Result<Self> {
        // Native run tokens are memory-only. Jobs/process groups are created by
        // Group::open; no Linux directory or certificate is fabricated.
        Ok(Self {
            dir: None,
            native_runs: Default::default(),
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub(crate) fn detect() -> io::Result<Self> {
        Err(io::Error::other(
            "Verified cgroup v2 execution is unavailable on this platform",
        ))
    }
    #[cfg(any(target_os = "linux", test))]
    fn at(dir: &Path) -> io::Result<Self> {
        match std::fs::create_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let dir = dir.canonicalize()?;
        if !dir.join("cgroup.procs").is_file() {
            return Err(io::Error::other("Run root is not a cgroup"));
        }
        Ok(Self {
            dir: Some(dir),
            native_runs: Default::default(),
        })
    }
    pub(crate) fn create(&self, name: &str) -> io::Result<PathBuf> {
        let name = host_name(name)?;
        let Some(dir) = &self.dir else {
            if !self.native_runs.lock().unwrap().insert(name.into()) {
                return Err(io::Error::other("Run domain already reserved"));
            }
            return Ok(PathBuf::from(name));
        };
        let path = dir.join(name);
        std::fs::create_dir(&path)?;
        Ok(path)
    }
    /// Fails harmlessly while the domain still has processes.
    pub(crate) fn remove(&self, name: &str) {
        if let Ok(name) = host_name(name) {
            if let Some(dir) = &self.dir {
                let _ = std::fs::remove_dir(dir.join(name));
            } else {
                self.native_runs.lock().unwrap().remove(name);
            }
        }
    }
    /// Boot cleanup of empty domains left by an earlier process.
    pub(crate) fn sweep(&self) {
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(rows) = std::fs::read_dir(dir) else {
            return;
        };
        for row in rows.flatten() {
            if let Some(name) = row.file_name().to_str() {
                self.remove(name);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::RunRoot;
    #[test]
    fn creates_and_removes_only_empty_uuid_domains() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("hall-runs");
        assert!(RunRoot::at(&base).is_err());
        std::fs::write(base.join("cgroup.procs"), "").unwrap();
        let root = RunRoot::at(&base).unwrap();
        assert!(root.create("../escape").is_err());
        assert!(root.create("not-a-uuid").is_err());
        let run = uuid::Uuid::new_v4().to_string();
        let path = root.create(&run).unwrap();
        assert!(root.create(&run).is_err());
        std::fs::write(path.join("busy"), "").unwrap();
        root.remove(&run);
        assert!(path.exists());
        std::fs::remove_file(path.join("busy")).unwrap();
        let stale = root.create(&uuid::Uuid::new_v4().to_string()).unwrap();
        std::fs::create_dir(base.join("keep")).unwrap();
        root.sweep();
        assert!(!path.exists() && !stale.exists());
        assert!(base.join("keep").exists());
    }
}
