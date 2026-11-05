//! Best effort only: a child may leave its process group or session. Never
//! turn disappearance of a leader (or a PID scan) into all-descendant proof.
use super::process::{birth, Birth};
use std::{io, sync::Mutex};
use tokio::process::{Child, Command};
#[derive(Default)]
pub(crate) struct NativeGroup {
    leaders: Mutex<Vec<Birth>>,
}
impl NativeGroup {
    pub(crate) fn new() -> io::Result<Self> {
        Ok(Self::default())
    }
    pub(crate) fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.kill_on_drop(true);
        let mut child = command.spawn()?;
        let identity = child.id().and_then(birth);
        match identity {
            Some(identity) => {
                self.leaders.lock().unwrap().push(identity);
                Ok(child)
            }
            None => {
                let _ = child.start_kill();
                Err(io::Error::other("Native child birth identity unavailable"))
            }
        }
    }
    pub(crate) fn cancel(&self) -> io::Result<()> {
        for identity in self.leaders.lock().unwrap().iter() {
            // A stale PID must never authorize a signal to someone else's group.
            if birth(identity.pid) != Some(*identity) {
                continue;
            }
            let pid =
                i32::try_from(identity.pid).map_err(|_| io::Error::other("Invalid owned PID"))?;
            if unsafe { libc::getpgid(pid) } != pid {
                continue;
            }
            if unsafe { libc::kill(-pid, libc::SIGKILL) } != 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(err);
                }
            }
        }
        Ok(())
    }
    pub(crate) fn populated(&self) -> io::Result<bool> {
        Err(io::Error::other(
            "macOS process groups do not contain detached descendants",
        ))
    }
}
impl Drop for NativeGroup {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}
#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
