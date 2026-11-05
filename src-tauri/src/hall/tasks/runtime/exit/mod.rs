//! Host-issued scope + actual child ownership. Reports never infer closure
//! from CLI results, cancel acknowledgments, an absent connection or PID zero.
pub(crate) mod cgroup;
pub(crate) mod host;
pub(crate) mod migration;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod native;
#[cfg(windows)]
#[path = "windows.rs"]
mod native;
pub(crate) mod platform;
pub(crate) mod probe;
#[cfg(any(windows, target_os = "macos"))]
mod probe_native;
mod process;
use super::{Binding, ExecutionOwner};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[derive(Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExitObservation {
    pub attempt_id: String,
    pub lease_id: String,
    pub scope_hash: String,
    pub root_birth_hash: Option<String>,
    pub root_reaped: bool,
    pub known_processes: usize,
    pub known_reaped: usize,
    pub pending_spawns: usize,
    pub coverage: String,
    pub complete: bool,
    pub reason: String,
    pub observed_at: DateTime<Utc>,
}
struct Record {
    birth: Option<process::Birth>,
    reaped: bool,
}
#[derive(Default)]
struct State {
    root: Option<String>,
    records: BTreeMap<String, Record>,
    pending: usize,
    root_pending: bool,
    sealed: bool,
    broken: bool,
    process_created: bool,
}
enum Coverage {
    Native,
    Managed(Arc<platform::Group>),
    Linux(Arc<cgroup::Cgroup>),
    #[cfg(test)]
    Fixture(Arc<dyn Fn() -> Option<bool> + Send + Sync>),
}
pub(crate) struct Domain {
    owner: ExecutionOwner,
    lease: String,
    state: Mutex<State>,
    coverage: Coverage,
}
pub(crate) struct Spawn {
    domain: Arc<Domain>,
    root: bool,
}
pub(crate) struct OwnedProcess {
    domain: Arc<Domain>,
    id: String,
}
impl Drop for Spawn {
    fn drop(&mut self) {
        let mut s = self.domain.state.lock().unwrap();
        s.pending -= 1;
        if self.root {
            s.root_pending = false;
        }
    }
}
impl OwnedProcess {
    pub(crate) fn reaped(&self) {
        if let Some(row) = self.domain.state.lock().unwrap().records.get_mut(&self.id) {
            row.reaped = true;
        }
    }
}
impl Domain {
    pub(crate) fn lease_id(&self) -> &str {
        &self.lease
    }
    fn new(owner: ExecutionOwner, coverage: Coverage) -> Arc<Self> {
        Arc::new(Self {
            owner,
            lease: uuid::Uuid::new_v4().to_string(),
            state: Mutex::new(State::default()),
            coverage,
        })
    }
    pub(crate) fn begin(self: &Arc<Self>, root: bool) -> std::io::Result<Spawn> {
        let mut s = self.state.lock().unwrap();
        if s.sealed || (root && (s.root.is_some() || s.root_pending)) {
            return Err(std::io::Error::other(
                "Run is closing or already has a root",
            ));
        }
        s.pending += 1;
        if root {
            s.root_pending = true;
        }
        Ok(Spawn {
            domain: self.clone(),
            root,
        })
    }
    /// Serialize creation with cancellation: no child can start after sealing.
    /// Native Windows assignment/resume and Mac identity capture happen here.
    pub(crate) fn spawn(
        &self,
        cmd: &mut tokio::process::Command,
    ) -> std::io::Result<tokio::process::Child> {
        let mut state = self.state.lock().unwrap();
        if state.sealed {
            return Err(std::io::Error::other("Run is closing"));
        }
        let child = match &self.coverage {
            Coverage::Managed(group) => group.spawn(cmd),
            Coverage::Linux(group) => {
                group.configure(cmd);
                cmd.spawn()
            }
            _ => cmd.spawn(),
        };
        if child.is_ok() {
            state.process_created = true;
        }
        child
    }
    pub(crate) fn cancel(&self) -> std::io::Result<()> {
        self.state.lock().unwrap().sealed = true;
        match &self.coverage {
            Coverage::Managed(group) => group.cancel()?,
            Coverage::Linux(group) => group.cancel()?,
            _ => {}
        }
        Ok(())
    }
    pub(crate) fn started(self: &Arc<Self>, ticket: Spawn, pid: u32) -> OwnedProcess {
        let id = uuid::Uuid::new_v4().to_string();
        let mut s = self.state.lock().unwrap();
        s.process_created = true;
        if !Arc::ptr_eq(self, &ticket.domain) {
            s.broken = true;
        }
        if ticket.root {
            if s.root.is_some() {
                s.broken = true;
            }
            s.root = Some(id.clone());
        }
        s.records.insert(
            id.clone(),
            Record {
                birth: process::birth(pid),
                reaped: false,
            },
        );
        drop(s);
        drop(ticket);
        OwnedProcess {
            domain: self.clone(),
            id,
        }
    }
    pub(crate) fn root_reaped(&self) {
        let mut s = self.state.lock().unwrap();
        s.sealed = true;
        if let Some(id) = s.root.clone() {
            if let Some(row) = s.records.get_mut(&id) {
                row.reaped = true;
            }
        }
    }
    /// Only after sealing: no registered or in-flight spawn ever created a child.
    pub(crate) fn never_started(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.sealed
            && !state.broken
            && !state.process_created
            && state.pending == 0
            && state.records.is_empty()
    }
    pub(crate) fn observation(&self) -> ExitObservation {
        let s = self.state.lock().unwrap();
        let root = s.root.as_ref().and_then(|id| s.records.get(id));
        let root_reaped = root.is_some_and(|r| r.reaped);
        let root_birth_hash = root.and_then(|r| r.birth).map(|b| {
            use sha2::{Digest, Sha256};
            format!(
                "{:x}",
                Sha256::digest(format!(
                    "{}:{}:{}:{}",
                    self.lease, b.pid, b.start, b.fraction
                ))
            )
        });
        let (coverage, empty) = match &self.coverage {
            Coverage::Native => ("native_observation_only", None),
            Coverage::Managed(group) => (platform::coverage(), group.populated().ok().map(|p| !p)),
            Coverage::Linux(g) => (
                "linux_cgroup_v2",
                g.populated().ok().map(|populated| !populated),
            ),
            #[cfg(test)]
            Coverage::Fixture(check) => ("fixture_owned_creation", check()),
        };
        let known_reaped = s.records.values().filter(|r| r.reaped).count();
        let reason = if s.broken {
            "ownership_inconsistent"
        } else if !root_reaped {
            "root_not_reaped"
        } else if root_birth_hash.is_none() {
            "root_birth_unavailable"
        } else if !s.sealed || s.pending > 0 {
            "spawn_not_quiescent"
        } else if known_reaped != s.records.len() {
            "owned_child_not_reaped"
        } else if empty != Some(true) {
            "descendant_coverage_unconfirmed"
        } else {
            "verified_domain_empty"
        };
        ExitObservation {
            attempt_id: self.owner.attempt.clone(),
            lease_id: self.lease.clone(),
            scope_hash: self.owner.digest(),
            root_birth_hash,
            root_reaped,
            known_processes: s.records.len(),
            known_reaped,
            pending_spawns: s.pending,
            coverage: coverage.into(),
            complete: reason == "verified_domain_empty",
            reason: reason.into(),
            observed_at: Utc::now(),
        }
    }
    #[cfg(test)]
    pub(super) fn fixture(
        owner: ExecutionOwner,
        check: Arc<dyn Fn() -> Option<bool> + Send + Sync>,
    ) -> Arc<Self> {
        Self::new(owner, Coverage::Fixture(check))
    }
}
struct Entry {
    domain: Arc<Domain>,
    connection: Option<String>,
}
#[derive(Default)]
pub(crate) struct Book {
    rows: Mutex<BTreeMap<String, Entry>>,
}
pub(crate) type RecoveryDomain = (Arc<Domain>, Option<String>);
impl Book {
    pub(crate) fn prepare(&self, run: &str, owner: ExecutionOwner) -> std::io::Result<Arc<Domain>> {
        let mut rows = self.rows.lock().unwrap();
        if let Some(row) = rows.get(run) {
            if row.domain.owner != owner {
                return Err(std::io::Error::other("Run ownership changed"));
            }
            return Ok(row.domain.clone());
        }
        let domain = Domain::new(owner, Coverage::Native);
        rows.insert(
            run.into(),
            Entry {
                domain: domain.clone(),
                connection: None,
            },
        );
        Ok(domain)
    }
    /// Host-only reviewed configuration. Not exposed by JSON, env or a model tool.
    /// It opens a PREEXISTING kernel domain; it never provisions one.
    pub(crate) fn prepare_linux(
        &self,
        run: &str,
        owner: ExecutionOwner,
        path: &std::path::Path,
    ) -> std::io::Result<()> {
        let group = Arc::new(cgroup::Cgroup::open(path, run)?);
        let mut rows = self.rows.lock().unwrap();
        if rows.contains_key(run) {
            return Err(std::io::Error::other("Run domain already reserved"));
        }
        rows.insert(
            run.into(),
            Entry {
                domain: Domain::new(owner, Coverage::Linux(group)),
                connection: None,
            },
        );
        Ok(())
    }
    pub(crate) fn prepare_group(
        &self,
        run: &str,
        owner: ExecutionOwner,
        group: Arc<platform::Group>,
    ) -> std::io::Result<()> {
        let mut rows = self.rows.lock().unwrap();
        if rows.contains_key(run) {
            return Err(std::io::Error::other("Run domain already reserved"));
        }
        rows.insert(
            run.into(),
            Entry {
                domain: Domain::new(owner, Coverage::Managed(group)),
                connection: None,
            },
        );
        Ok(())
    }
    pub(crate) fn for_spawn(
        &self,
        run: &str,
        connection: &str,
    ) -> std::io::Result<Option<Arc<Domain>>> {
        let mut rows = self.rows.lock().unwrap();
        let Some(row) = rows.get_mut(run) else {
            return Ok(None);
        };
        if row.connection.as_ref().is_some_and(|old| old != connection) {
            return Err(std::io::Error::other(
                "Run already bound to another connection",
            ));
        }
        row.connection = Some(connection.into());
        Ok(Some(row.domain.clone()))
    }
    pub(crate) fn for_recovery(
        &self,
        run: &str,
        owner: &ExecutionOwner,
    ) -> std::io::Result<Option<RecoveryDomain>> {
        let rows = self.rows.lock().unwrap();
        let Some(row) = rows.get(run) else {
            return Ok(None);
        };
        if &row.domain.owner != owner {
            return Err(std::io::Error::other("Run ownership changed"));
        }
        Ok(Some((row.domain.clone(), row.connection.clone())))
    }
    pub(crate) fn for_connection(&self, connection: &str) -> Option<Arc<Domain>> {
        self.rows
            .lock()
            .unwrap()
            .values()
            .find(|row| row.connection.as_deref() == Some(connection))
            .map(|row| row.domain.clone())
    }
    pub(crate) fn bound(&self, b: &Binding) -> Option<Arc<Domain>> {
        let rows = self.rows.lock().unwrap();
        let row = rows.get(&b.run_id)?;
        if row.connection.as_deref() != Some(&b.connection_id)
            || row.domain.owner != b.owner
            || row.domain.lease != b.lease_id
        {
            return None;
        }
        Some(row.domain.clone())
    }
    pub(crate) fn remove(&self, b: &Binding) {
        if self.bound(b).is_some() {
            self.rows.lock().unwrap().remove(&b.run_id);
        }
    }
}
#[cfg(test)]
mod tests;

impl std::fmt::Debug for Domain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedProcessDomain").finish_non_exhaustive()
    }
}
