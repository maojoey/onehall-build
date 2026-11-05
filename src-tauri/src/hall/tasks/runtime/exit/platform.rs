//! Platform mechanism shared by probes, ACP roots and hosted child commands.
#[cfg(not(any(windows, target_os = "macos")))]
use std::sync::Arc;
use std::{io, path::Path};
use tokio::process::{Child, Command};
pub(crate) enum Group {
    #[cfg(not(any(windows, target_os = "macos")))]
    Linux(Arc<super::cgroup::Cgroup>),
    #[cfg(any(windows, target_os = "macos"))]
    Native(super::native::NativeGroup),
}
pub(crate) fn coverage() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux_cgroup_v2"
    } else if cfg!(windows) {
        "windows_job_object"
    } else if cfg!(target_os = "macos") {
        "macos_process_group_best_effort"
    } else {
        "native_observation_only"
    }
}
impl Group {
    pub(crate) fn open(path: &Path, run: &str) -> io::Result<Self> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            if path.file_name().and_then(|s| s.to_str()) != Some(run)
                || uuid::Uuid::parse_str(run).is_err()
            {
                return Err(io::Error::other(
                    "A host-generated run identity is required",
                ));
            }
            Ok(Self::Native(super::native::NativeGroup::new()?))
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            Ok(Self::Linux(Arc::new(super::cgroup::Cgroup::open(
                path, run,
            )?)))
        }
    }
    pub(crate) fn spawn(&self, cmd: &mut Command) -> io::Result<Child> {
        match self {
            #[cfg(not(any(windows, target_os = "macos")))]
            Self::Linux(group) => {
                group.configure(cmd);
                cmd.spawn()
            }
            #[cfg(any(windows, target_os = "macos"))]
            Self::Native(group) => group.spawn(cmd),
        }
    }
    pub(crate) fn cancel(&self) -> io::Result<()> {
        match self {
            #[cfg(not(any(windows, target_os = "macos")))]
            Self::Linux(group) => group.cancel(),
            #[cfg(any(windows, target_os = "macos"))]
            Self::Native(group) => group.cancel(),
        }
    }
    pub(crate) fn populated(&self) -> io::Result<bool> {
        match self {
            #[cfg(not(any(windows, target_os = "macos")))]
            Self::Linux(group) => group.populated(),
            #[cfg(any(windows, target_os = "macos"))]
            Self::Native(group) => group.populated(),
        }
    }
}
