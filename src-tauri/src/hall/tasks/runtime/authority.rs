//! Host-issued identity only. No Deserialize/Debug, caller-name/depth authority.
use super::{Binding, RunPlan};
use crate::app_error::AppCommandError;
use std::{
    collections::{BTreeMap, HashMap},
    sync::RwLock,
};

#[derive(Clone, Copy, PartialEq)]
pub enum Role {
    Member,
    Pm,
}
#[derive(Clone)]
struct Scope {
    member: String,
    task: String,
    attempt: String,
    snapshot: String,
    cwd: String,
    engine: String,
    assistant: Option<String>,
    mode: String,
    config: BTreeMap<String, String>,
    connection: Option<String>,
    role: Role,
    /// What the launch hands this run's agent process (may hold secrets).
    launch_env: BTreeMap<String, String>,
    revoked: bool,
}
pub struct Actor {
    run: String,
    connection: String,
}
impl Actor {
    /// The host-reserved run this actor was bound to; never model supplied.
    pub(crate) fn run_id(&self) -> &str {
        &self.run
    }
}
#[derive(Default)]
pub struct Registry {
    rows: RwLock<HashMap<String, Scope>>,
    pm: RwLock<Option<String>>,
}
fn denied() -> AppCommandError {
    AppCommandError::permission_denied(
        "A trusted host identity and non-expanding member scope are \
    required",
    )
}
pub struct DerivedRequest<'a> {
    pub member: &'a str,
    pub snapshot: &'a str,
    pub cwd: &'a str,
    pub engine: &'a str,
    pub assistant: Option<&'a str>,
    pub mode: &'a str,
    pub config: &'a BTreeMap<String, String>,
}
impl Registry {
    pub(super) fn prepare(&self, run: &str, p: &RunPlan) {
        let role = if self.pm.read().unwrap().as_deref() == Some(p.member_id()) {
            Role::Pm
        } else {
            Role::Member
        };
        let mut rows = self.rows.write().unwrap();
        if rows.get(run).is_some_and(|scope| scope.revoked) {
            return;
        }
        rows.insert(
            run.into(),
            Scope {
                member: p.member_id().into(),
                task: p.task_id.clone(),
                attempt: p.attempt_id.clone(),
                snapshot: p.snapshot_id.clone(),
                cwd: p.cwd().into(),
                engine: p.engine().into(),
                assistant: p.profile.assistant_uid.clone(),
                mode: p.mode().into(),
                config: p.options().clone(),
                connection: None,
                role,
                launch_env: BTreeMap::new(),
                revoked: false,
            },
        );
    }
    pub(super) fn set_launch_env(&self, run: &str, env: BTreeMap<String, String>) {
        if let Some(s) = self
            .rows
            .write()
            .unwrap()
            .get_mut(run)
            .filter(|s| !s.revoked)
        {
            s.launch_env = env;
        }
    }
    /// The member environment for a host-prepared team run, if any.
    pub fn launch_env(&self, run: &str) -> Option<BTreeMap<String, String>> {
        self.rows
            .read()
            .unwrap()
            .get(run)
            .filter(|s| !s.revoked)
            .map(|s| s.launch_env.clone())
            .filter(|env| !env.is_empty())
    }
    pub(super) fn bind(&self, b: &Binding) -> Result<Actor, AppCommandError> {
        let mut rows = self.rows.write().unwrap();
        let s = rows.get_mut(&b.run_id).ok_or_else(denied)?;
        if s.revoked || s.connection.as_ref().is_some_and(|c| c != &b.connection_id) {
            return Err(denied());
        }
        s.connection = Some(b.connection_id.clone());
        Ok(Actor {
            run: b.run_id.clone(),
            connection: b.connection_id.clone(),
        })
    }
    pub(crate) fn configure_pm(&self, member: Option<String>) {
        *self.pm.write().unwrap() = member;
    }
    pub fn protects_connection(&self, connection: &str) -> bool {
        self.rows
            .read()
            .unwrap()
            .values()
            .any(|s| s.connection.as_deref() == Some(connection))
    }
    pub(crate) fn pm_run(&self, run: &str) -> bool {
        self.rows
            .read()
            .unwrap()
            .get(run)
            .is_some_and(|s| !s.revoked && s.role == Role::Pm)
    }
    pub fn protects_run(&self, run: &str) -> bool {
        self.rows.read().unwrap().contains_key(run)
    }
    pub(super) fn revoke(&self, run: &str) {
        if let Some(scope) = self.rows.write().unwrap().get_mut(run) {
            // Keep the host restriction tombstone until the manager is gone.
            // Removing it could turn a surviving old process into an ordinary
            // unrestricted caller of delegate_to_agent.
            scope.revoked = true;
            scope.launch_env.clear();
            scope.config.clear();
        }
    }
    /// Called only with connection/run established and verified by the host.
    pub(crate) fn actor_for_bound_connection(
        &self,
        run: &str,
        connection: &str,
    ) -> Result<Actor, AppCommandError> {
        let rows = self.rows.read().unwrap();
        let s = rows.get(run).ok_or_else(denied)?;
        if s.revoked || s.connection.as_deref() != Some(connection) {
            return Err(denied());
        }
        Ok(Actor {
            run: run.into(),
            connection: connection.into(),
        })
    }
    pub fn authorize_member(&self, a: &Actor, member: &str) -> Result<(), AppCommandError> {
        let rows = self.rows.read().unwrap();
        let s = rows.get(&a.run).ok_or_else(denied)?;
        if s.revoked || s.connection.as_deref() != Some(&a.connection) || s.member != member {
            return Err(denied());
        }
        Ok(())
    }
    pub fn authorize_pm(&self, a: &Actor) -> Result<(), AppCommandError> {
        let rows = self.rows.read().unwrap();
        let s = rows.get(&a.run).ok_or_else(denied)?;
        if s.revoked || s.connection.as_deref() != Some(&a.connection) || s.role != Role::Pm {
            return Err(denied());
        }
        Ok(())
    }
    pub fn authorize_derived(
        &self,
        a: &Actor,
        request: DerivedRequest<'_>,
    ) -> Result<(), AppCommandError> {
        let DerivedRequest {
            member,
            snapshot,
            cwd,
            engine,
            assistant,
            mode,
            config,
        } = request;
        let rows = self.rows.read().unwrap();
        let s = rows.get(&a.run).ok_or_else(denied)?;
        // Exact saved scope/option equality is conservative; glob-string subset
        // guessing and a model-supplied depth never grant broader authority.
        if s.revoked
            || s.connection.as_deref() != Some(&a.connection)
            || s.member != member
            || s.snapshot != snapshot
            || s.cwd != cwd
            || engine != s.engine
            || s.assistant.as_deref() != assistant
            || !(mode == s.mode || mode == crate::hall::engine::mode_for_scope(&s.engine, false))
            || &s.config != config
        {
            return Err(denied());
        }
        Ok(())
    }
    pub fn own_task(&self, a: &Actor) -> Result<(String, String, String), AppCommandError> {
        let rows = self.rows.read().unwrap();
        let s = rows.get(&a.run).ok_or_else(denied)?;
        if s.revoked || s.connection.as_deref() != Some(&a.connection) {
            return Err(denied());
        }
        Ok((s.member.clone(), s.task.clone(), s.attempt.clone()))
    }
    // No public/model role setter. PM registration is a future trusted-host
    // administrative binding, never the member's descriptive role/name.
    #[cfg(test)]
    pub(super) fn configure_fixture_pm(&self, member: &str) {
        *self.pm.write().unwrap() = Some(member.into());
    }
}

#[cfg(test)]
impl Registry {
    pub(crate) async fn fixture_actor(
        &self,
        db: &sea_orm::DatabaseConnection,
        c: &crate::hall::tasks::queue::Claim,
        b: &super::Binding,
    ) -> Actor {
        let task = crate::hall::tasks::store::by_id(db, c.task_id())
            .await
            .unwrap();
        let profile =
            crate::hall::members::snapshot_launch_profile(db, &task.member_id, &task.snapshot_id)
                .await
                .unwrap();
        let p = super::RunPlan {
            owner: b.owner.clone(),
            profile,
            task_id: task.task_id,
            attempt_id: c.attempt_id().into(),
            snapshot_id: task.snapshot_id,
            description: task.description,
        };
        self.prepare(b.run_id(), &p);
        self.bind(b).unwrap()
    }
}

#[cfg(test)]
impl Registry {
    pub(crate) fn fixture_run(&self, run: &str) {
        self.rows.write().unwrap().insert(
            run.into(),
            Scope {
                member: "fixture".into(),
                task: "fixture".into(),
                attempt: "fixture".into(),
                snapshot: "fixture".into(),
                cwd: String::new(),
                engine: String::new(),
                assistant: None,
                mode: String::new(),
                config: BTreeMap::new(),
                connection: None,
                role: Role::Member,
                launch_env: BTreeMap::new(),
                revoked: false,
            },
        );
    }
}
