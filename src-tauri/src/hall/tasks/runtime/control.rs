//! One boot-local owner for claims/bindings, used by authenticated UI controls.
//! No JSON identity, resume-by-PID, automatic replay or idle model wake-up.
//! A dispatch needs the member switch, a current human review and a run domain
//! the host creates under its own delegated cgroup after a passing probe.
use super::{
    admission::{AdmissionStatus, Gate, LaunchScope},
    exit::{
        host::RunRoot,
        platform::Group,
        probe::{self, NativeCertificate, ProbeReport},
    },
    Binding, Coordinator,
};
use crate::{
    acp::manager::ConnectionManager,
    app_error::AppCommandError,
    cli_run::CliRunService,
    hall::tasks::{
        get_task,
        queue::{Claim, QueueHost},
        review, TaskDetail,
    },
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, Notify};
struct Active {
    claim: Claim,
    binding: Binding,
    queued_revision: i64,
    paused: bool,
    terminal: bool,
    cancel_revision: Option<i64>,
    resolution_revision: Option<i64>,
}
struct State {
    active: BTreeMap<String, Active>,
    certificate: Option<NativeCertificate>,
    /// Last probe result; None until the boot probe has run.
    probed: Option<bool>,
}
pub struct Controller {
    #[cfg(test)]
    fixture: bool,
    co: Arc<Coordinator>,
    gate: Arc<Gate>,
    service: Arc<CliRunService>,
    manager: ConnectionManager,
    state: Mutex<State>,
    notify: Arc<Notify>,
    root: Option<Arc<RunRoot>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    #[serde(flatten)]
    pub admission: AdmissionStatus,
    pub platform: String,
    pub native_probe_supported: bool,
    pub coverage: String,
    pub complete_descendant_coverage: bool,
    pub transport_ready: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub conversation_id: i32,
    pub connection_id: Option<String>,
    pub agent_type: String,
}
/// How many team runs may be live at once (different members only; one
/// member always works serially). `ONEHALL_TEAM_CONCURRENCY`, 1-8, default 2.
fn team_concurrency() -> u16 {
    concurrency_from(std::env::var("ONEHALL_TEAM_CONCURRENCY").ok().as_deref())
}
fn concurrency_from(value: Option<&str>) -> u16 {
    value
        .and_then(|v| v.trim().parse().ok())
        .filter(|n| (1..=8).contains(n))
        .unwrap_or(2)
}
fn denied() -> AppCommandError {
    AppCommandError::permission_denied(
        "No verified run domain or current reviewed task scope is \
    available",
    )
}
fn conflict() -> AppCommandError {
    crate::hall::tasks::store::conflict()
}
impl Controller {
    pub fn closed(
        db: sea_orm::DatabaseConnection,
        service: Arc<CliRunService>,
        manager: ConnectionManager,
    ) -> Arc<Self> {
        let gate = Arc::new(Gate::default());
        let port = Arc::new(super::port::CliPort::controlled(
            service.clone(),
            manager.clone_ref(),
            gate.clone(),
            db.clone(),
        ));
        let host = QueueHost::new(db, team_concurrency()).expect("bounded team capacity");
        Arc::new(Self {
            #[cfg(test)]
            fixture: false,
            co: Arc::new(Coordinator::controlled(host, port, gate.clone())),
            gate,
            service,
            manager,
            state: Mutex::new(State {
                active: BTreeMap::new(),
                certificate: None,
                probed: None,
            }),
            notify: Arc::new(Notify::new()),
            root: Self::run_root(),
        })
    }
    /// Host startup attaches the exchange once; without it tasks stay plain.
    pub fn attach_exchange(&self, exchange: crate::hall::exchange::Exchange) {
        self.co.attach_exchange(exchange);
    }
    fn run_root() -> Option<Arc<RunRoot>> {
        // Unit tests must never create domains in the host's own cgroup.
        if cfg!(test) {
            return None;
        }
        match RunRoot::detect() {
            Ok(root) => Some(Arc::new(root)),
            Err(e) => {
                tracing::warn!("[hall] team run domains unavailable: {e}");
                None
            }
        }
    }
    fn transport_ready(&self) -> bool {
        self.manager
            .delegation_snapshot()
            .is_some_and(|i| i.socket_path.exists())
            && crate::acp::connection::locate_codeg_mcp_binary().is_some()
    }
    fn admitted(&self, task: &str) -> bool {
        if self.gate.allows(task) {
            return true;
        }
        #[cfg(test)]
        {
            if self.fixture {
                return true;
            }
        }
        false
    }
    pub fn queue(&self) -> QueueHost {
        self.co.host.clone()
    }
    pub async fn readiness(&self) -> Readiness {
        let s = self.state.lock().await;
        let transport_ready = self.transport_ready();
        let native_verified = s.probed == Some(true);
        let run_root = self.root.is_some();
        let reason = if !run_root {
            "run_root_unavailable"
        } else if s.probed.is_none() {
            "native_probe_pending"
        } else if !native_verified {
            "native_probe_failed"
        } else if !transport_ready {
            "transport_unavailable"
        } else {
            "ready"
        };
        Readiness {
            admission: AdmissionStatus {
                enabled: reason == "ready",
                native_verified,
                run_root,
                reason: reason.into(),
            },
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            native_probe_supported: cfg!(any(target_os = "linux", target_os = "macos", windows)),
            coverage: super::exit::platform::coverage().into(),
            complete_descendant_coverage: s
                .certificate
                .as_ref()
                .is_some_and(|p| p.valid() && p.report().complete),
            transport_ready,
        }
    }
    /// Re-runs the host probe in a fresh host-created domain.
    pub async fn probe(&self) -> Result<ProbeReport, AppCommandError> {
        let mut s = self.state.lock().await;
        s.certificate = None;
        self.certify(&mut s).await
    }
    /// Certificates last 15 minutes; an expired one is renewed before use.
    async fn certify(&self, s: &mut State) -> Result<ProbeReport, AppCommandError> {
        if let Some(proof) = s.certificate.as_ref().filter(|p| p.valid()) {
            return Ok(proof.report());
        }
        s.certificate = None;
        let root = self.root.as_ref().ok_or_else(denied)?;
        let nonce = uuid::Uuid::new_v4().to_string();
        let result = match root.create(&nonce) {
            Ok(path) => probe::verify(&path, &nonce).await,
            Err(_) => Err(denied()),
        };
        root.remove(&nonce);
        s.probed = Some(result.is_ok());
        let proof = result?;
        let report = proof.report();
        s.certificate = Some(proof);
        Ok(report)
    }
    /// Removes a finished run's domain; harmless if processes remain.
    fn release(&self, run: &str) {
        if let Some(root) = &self.root {
            root.remove(run);
        }
    }
    async fn member_allowed(&self, member: &str) -> Result<bool, AppCommandError> {
        let row = self
            .co
            .database()
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT enabled FROM hall_member WHERE member_id=?",
                vec![member.to_owned().into()],
            ))
            .await
            .map_err(crate::hall::tasks::store::storage_error)?;
        Ok(match row {
            Some(r) => r
                .try_get::<bool>("", "enabled")
                .map_err(crate::hall::tasks::store::storage_error)?,
            None => false,
        })
    }
    /// Mints the one launch scope for this task in a fresh run domain.
    async fn admit(&self, s: &mut State, task: &str) -> Result<(), AppCommandError> {
        if !self.transport_ready() {
            return Err(AppCommandError::dependency_missing(
                "Team companion transport is unavailable",
            ));
        }
        self.certify(s).await?;
        let root = self.root.as_ref().ok_or_else(denied)?;
        let plan = review::approved_plan(self.co.database(), task).await?;
        let pm = crate::hall::team::mailbox::role(self.co.database()).await?;
        let run = self.service.reserve_team().map_err(|_| denied())?.run_id;
        let path = root.create(&run).map_err(|_| denied())?;
        let group = match Group::open(&path, &run) {
            Ok(group) => Arc::new(group),
            Err(_) => {
                root.remove(&run);
                return Err(denied());
            }
        };
        self.manager.hall_registry().configure_pm(pm.member_id);
        self.gate.admit(
            task.into(),
            LaunchScope {
                hash: plan.plan_hash,
                run,
                group,
            },
        )
    }
    fn withdraw(&self, task: &str) {
        if let Some(scope) = self.gate.withdraw(task) {
            drop(scope.group);
            self.release(&scope.run);
        }
    }
    pub async fn dispatch(
        &self,
        task: String,
        revision: i64,
    ) -> Result<TaskDetail, AppCommandError> {
        let mut s = self.state.lock().await;
        if let Some(a) = s.active.get(&task) {
            if a.queued_revision != revision {
                return Err(conflict());
            }
            return get_task(self.co.database(), task).await;
        }
        let t = get_task(self.co.database(), task.clone()).await?;
        if t.task.revision != revision || t.task.status != "queued" {
            return Err(conflict());
        }
        if !self.member_allowed(&t.task.member_id).await? {
            return Err(AppCommandError::permission_denied(
                "Dispatch is switched off for this member",
            ));
        }
        if !self.admitted(&task) {
            self.admit(&mut s, &task).await?;
        }
        let dispatched = self.co.dispatch_task(&task).await;
        // A scope the port never consumed still owns an empty run domain.
        self.withdraw(&task);
        let (claim, binding) = dispatched?.ok_or_else(conflict)?;
        s.active.insert(
            task.clone(),
            Active {
                claim,
                binding,
                queued_revision: revision,
                paused: false,
                terminal: false,
                cancel_revision: None,
                resolution_revision: None,
            },
        );
        self.notify.notify_one();
        drop(s);
        get_task(self.co.database(), task).await
    }
    pub async fn cancel(&self, task: String, revision: i64) -> Result<TaskDetail, AppCommandError> {
        let mut s = self.state.lock().await;
        let a = s.active.get_mut(&task).ok_or_else(denied)?;
        if a.cancel_revision != Some(revision) {
            self.co
                .cancel_at(&a.claim, &a.binding, Some(revision))
                .await?;
            a.cancel_revision = Some(revision);
        }
        self.notify.notify_one();
        drop(s);
        get_task(self.co.database(), task).await
    }
    async fn pump(&self, task: &str, a: &mut Active) -> Result<(), AppCommandError> {
        if a.terminal || a.paused {
            return Ok(());
        }
        if self.co.poll_once(&a.claim, &a.binding).await.is_err() {
            a.paused = true;
            return Ok(());
        }
        let t = get_task(self.co.database(), task.into()).await?;
        let stage = t.runtimes.last().map(|r| r.stage.as_str());
        if matches!(stage, Some("finishing" | "cancel_requested")) {
            let _ = self.co.confirm_exit(&a.claim, &a.binding).await?;
        }
        let t = get_task(self.co.database(), task.into()).await?;
        a.terminal = t.runtimes.last().is_some_and(|r| r.stage == "terminal");
        a.paused = t.runtimes.last().is_some_and(|r| r.stage == "needs_review");
        if a.terminal {
            self.release(a.binding.run_id());
        }
        Ok(())
    }
    pub async fn refresh(&self, task: String) -> Result<TaskDetail, AppCommandError> {
        let mut s = self.state.lock().await;
        if let Some(a) = s.active.get_mut(&task) {
            self.pump(&task, a).await?;
        }
        drop(s);
        get_task(self.co.database(), task).await
    }
    pub async fn session(&self, task: String) -> Result<SessionView, AppCommandError> {
        let s = self.state.lock().await;
        let d = get_task(self.co.database(), task.clone()).await?;
        let id = d
            .attempts
            .last()
            .and_then(|a| a.conversation_id)
            .ok_or_else(denied)?;
        let mut connection = None;
        if let Some(a) = s.active.get(&task) {
            if !a.terminal {
                if let Some(live) = self.manager.get_state(a.binding.connection_id()).await {
                    if live
                        .read()
                        .await
                        .cli_run
                        .as_ref()
                        .is_some_and(|tag| tag.run_id == a.binding.run_id())
                    {
                        connection = Some(a.binding.connection_id().into());
                    }
                }
            }
        }
        let engine = self
            .co
            .database()
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT engine FROM hall_member WHERE member_id=?",
                vec![d.task.member_id.clone().into()],
            ))
            .await
            .map_err(crate::hall::tasks::store::storage_error)?
            .ok_or_else(denied)?
            .try_get::<String>("", "engine")
            .map_err(crate::hall::tasks::store::storage_error)?;
        Ok(SessionView {
            conversation_id: id,
            connection_id: connection,
            agent_type: engine,
        })
    }
    pub async fn recover(&self, task: String) -> Result<TaskDetail, AppCommandError> {
        let mut s = self.state.lock().await;
        if !s.active.contains_key(&task) {
            drop(s);
            // The API must work even if the boot background sweep has not run.
            self.co.host.reconcile_previous_boots().await?;
            return self.co.recover_persisted(&task).await;
        }
        let a = s
            .active
            .get_mut(&task)
            .filter(|a| a.paused && !a.terminal)
            .ok_or_else(denied)?;
        let before = get_task(self.co.database(), task.clone()).await?;
        if !before
            .runtimes
            .last()
            .is_some_and(|r| r.stage == "needs_review")
        {
            self.co.needs_review(&a.claim).await?;
        }
        if self.co.recover_exit(&a.claim, &a.binding).await? {
            a.terminal = true;
            self.release(a.binding.run_id());
        }
        drop(s);
        get_task(self.co.database(), task).await
    }
    /// Host-only call after validating the local desktop window or TCP peer.
    pub(crate) async fn resolve_orphan(
        &self,
        task: String,
        attempt: String,
        revision: i64,
        ack: bool,
        reason: String,
    ) -> Result<TaskDetail, AppCommandError> {
        let mut state = self.state.lock().await;
        let detail = self
            .co
            .resolve_orphan(&task, &attempt, revision, ack, &reason)
            .await?;
        if let Some(active) = state.active.get_mut(&task) {
            active.terminal = true;
            active.paused = true;
            self.release(active.binding.run_id());
        }
        Ok(detail)
    }
    /// Local authenticated UI action; never exposed by team/model tools.
    pub async fn resolve_best_effort(
        &self,
        task: String,
        expected_revision: i64,
        acknowledge_unverified_descendants: bool,
    ) -> Result<TaskDetail, AppCommandError> {
        if !acknowledge_unverified_descendants {
            return Err(denied());
        }
        let mut state = self.state.lock().await;
        let active = state.active.get_mut(&task).ok_or_else(denied)?;
        if active.resolution_revision == Some(expected_revision) {
            return get_task(self.co.database(), task).await;
        }
        if !active.paused || active.terminal {
            return Err(denied());
        }
        self.co
            .resolve_best_effort(&active.claim, &active.binding, expected_revision)
            .await?;
        active.terminal = true;
        active.resolution_revision = Some(expected_revision);
        self.release(active.binding.run_id());
        drop(state);
        get_task(self.co.database(), task).await
    }

    pub fn spawn_background(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let notify = self.notify.clone();
        tokio::spawn(async move {
            // No work or model wake-up while idle. Boot recovery only fences old
            // work; the no-model host probe runs once so readiness is known.
            if let Some(c) = weak.upgrade() {
                if c.co.host.reconcile_previous_boots().await.is_err() {
                    return;
                }
                if let Some(root) = &c.root {
                    root.sweep();
                    let mut s = c.state.lock().await;
                    if let Err(e) = c.certify(&mut s).await {
                        tracing::warn!("[hall] team host probe failed: {e}");
                    }
                }
            }
            loop {
                notify.notified().await;
                loop {
                    let Some(c) = weak.upgrade() else { return };
                    let mut s = c.state.lock().await;
                    let mut running = false;
                    for (id, a) in &mut s.active {
                        if !a.terminal && !a.paused {
                            if c.pump(id, a).await.is_err() {
                                let _ = c.co.needs_review(&a.claim).await;
                                a.paused = true;
                            }
                            running |= !a.terminal && !a.paused;
                        }
                    }
                    drop(s);
                    drop(c);
                    if !running {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
