use super::{
    AbortStatus, Binding, ExitEvidence, ExitObservation, Outcome, RunPlan, RuntimeEvent,
    StartIdentity,
};
use crate::{
    acp::manager::ConnectionManager,
    app_error::AppCommandError,
    cli_run::{
        wire::{self, EventBody, PromptBlock, StartRequest},
        CliRunService, PermissionPolicy,
    },
};
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;

#[async_trait]
pub trait RuntimePort: Send + Sync {
    /// Host-only preflight; no subprocess or model side effects.
    async fn validate_start(&self, _: &RunPlan) -> Result<(), AppCommandError> {
        Ok(())
    }
    /// Cleanup by host-issued run even if no Binding was returned.
    async fn abort_start(&self, _: &StartIdentity) -> Result<AbortStatus, AppCommandError> {
        Ok(AbortStatus::ExitUnverified)
    }
    /// A missing in-memory domain after reboot is never proof of no process.
    async fn recover_start(&self, _: &StartIdentity) -> Result<AbortStatus, AppCommandError> {
        Ok(AbortStatus::ExitUnverified)
    }
    async fn start_observation(&self, _: &StartIdentity) -> Option<ExitObservation> {
        None
    }
    fn revoke_run(&self, _: &str) {}
    /// Reservation must have no agent/process side effects.
    async fn reserve(&self) -> Result<String, AppCommandError>;
    async fn reserve_for(&self, _: &RunPlan) -> Result<String, AppCommandError> {
        self.reserve().await
    }
    async fn start(&self, run: &str, plan: &RunPlan) -> Result<Binding, AppCommandError>;
    async fn activate(&self, _: &Binding) -> Result<(), AppCommandError> {
        Ok(())
    }
    async fn cancel(&self, b: &Binding) -> Result<(), AppCommandError>;
    async fn events(
        &self,
        b: &Binding,
        after: u64,
    ) -> Result<Vec<(u64, RuntimeEvent)>, AppCommandError>;
    async fn confirm_exit(&self, b: &Binding) -> Result<Option<ExitEvidence>, AppCommandError>;
    async fn exit_observation(&self, _: &Binding) -> Option<ExitObservation> {
        None
    }
    fn revoke(&self, _: &Binding) {}
}
/// Reuses the existing runtime; no shell, new ACP implementation or credential
/// handling. Admission stays false until separate capability/B10 validation.
pub struct CliPort {
    db: Option<sea_orm::DatabaseConnection>,
    service: Arc<CliRunService>,
    manager: ConnectionManager,
    gate: Arc<super::admission::Gate>,
    starting: std::sync::Mutex<std::collections::HashSet<String>>,
}
fn closed() -> AppCommandError {
    AppCommandError::permission_denied(
        "Real team execution requires verified runtime and cancellation \
    admission",
    )
}
fn failed() -> AppCommandError {
    AppCommandError::io_error("Team runtime response is unavailable or unverified")
}
impl CliPort {
    /// Host diagnostic registration only; it never grants native descendant
    /// coverage or opens dispatch. Use a distinct diagnostic run identity.
    pub fn prepare_native_observation(
        &self,
        run: &str,
        p: &RunPlan,
    ) -> Result<ExitObservation, AppCommandError> {
        self.manager
            .hall_exits
            .prepare(run, p.owner.clone())
            .map(|d| d.observation())
            .map_err(|_| failed())
    }
    /// Reviewed host-only setup; opens an existing kernel domain without
    /// starting a process or changing the real-execution gate.
    pub fn prepare_linux_domain(
        &self,
        run: &str,
        p: &RunPlan,
        path: &std::path::Path,
    ) -> Result<(), AppCommandError> {
        self.manager
            .hall_exits
            .prepare_linux(run, p.owner.clone(), path)
            .map_err(|_| failed())
    }
    pub fn closed(service: Arc<CliRunService>, manager: ConnectionManager) -> Self {
        Self {
            db: None,
            service,
            manager,
            gate: Arc::default(),
            starting: Default::default(),
        }
    }
    pub(crate) fn controlled(
        service: Arc<CliRunService>,
        manager: ConnectionManager,
        gate: Arc<super::admission::Gate>,
        db: sea_orm::DatabaseConnection,
    ) -> Self {
        Self {
            db: Some(db),
            service,
            manager,
            gate,
            starting: Default::default(),
        }
    }
}
impl CliPort {
    async fn cleanup_start(
        &self,
        id: &StartIdentity,
        live_failure: bool,
    ) -> Result<AbortStatus, AppCommandError> {
        let tracked = self.starting.lock().unwrap().contains(&id.run);
        let owned = self
            .manager
            .hall_exits
            .for_recovery(&id.run, &id.owner)
            .map_err(|_| failed())?;
        // Fence model tools before cancellation, without constructing a fake Binding.
        self.manager.hall_identities.revoke(&id.run);
        let Some((domain, connection)) = owned else {
            return Ok(if live_failure && tracked {
                AbortStatus::NotStarted
            } else {
                AbortStatus::ExitUnverified
            });
        };
        domain.cancel().map_err(|_| failed())?;
        if let Some(connection) = connection {
            if let Some(state) = self.manager.get_state(&connection).await {
                if state
                    .read()
                    .await
                    .cli_run
                    .as_ref()
                    .is_none_or(|tag| tag.run_id != id.run)
                {
                    return Err(failed());
                }
                self.manager
                    .disconnect(&connection)
                    .await
                    .map_err(|_| failed())?;
            }
        }
        let cancelled = self
            .service
            .cancel(&id.run, wire::CancelReason::User)
            .await
            .is_ok();
        if !cancelled {
            return Ok(AbortStatus::ExitUnverified);
        }
        if domain.never_started() {
            return Ok(AbortStatus::NotStarted);
        }
        Ok(if domain.observation().complete {
            AbortStatus::ExitVerified
        } else {
            AbortStatus::ExitUnverified
        })
    }
}

#[async_trait]
impl RuntimePort for CliPort {
    async fn validate_start(&self, p: &RunPlan) -> Result<(), AppCommandError> {
        crate::hall::members::execution_policy::check_start(p.launch_profile())
    }
    async fn reserve(&self) -> Result<String, AppCommandError> {
        Err(closed())
    }
    async fn reserve_for(&self, p: &RunPlan) -> Result<String, AppCommandError> {
        self.gate.run(p.task_id())
    }
    async fn start(&self, run: &str, p: &RunPlan) -> Result<Binding, AppCommandError> {
        self.starting.lock().unwrap().insert(run.into());
        let plan = crate::hall::tasks::review::approved_plan(
            self.db.as_ref().ok_or_else(closed)?,
            p.task_id(),
        )
        .await?;
        // The role manual, native limits and key, before anything starts.
        let env = crate::hall::members::launch::launch_env(&p.profile).await?;
        let group = self.gate.consume(p.task_id(), &plan.plan_hash, run)?;
        // Root tag carries this host-reserved run from connection creation,
        // before an eager MCP call can escape into an unscoped delegate.
        self.manager.hall_identities.prepare(run, p);
        self.manager.hall_identities.set_launch_env(run, env);
        self.manager
            .hall_exits
            .prepare_group(run, p.owner.clone(), group)
            .map_err(|_| failed())?;
        let request = StartRequest {
            run_id: run.into(),
            workspace: p.cwd().into(),
            agent: Some(p.engine().into()),
            assistant: None,
            prompt: vec![PromptBlock::Text { text: p.prompt() }],
            model: None,
            effort: None,
            mode: Some(p.mode().into()),
            config: p.options().clone(),
            permissions: PermissionPolicy::Wait,
            title: None,
            timeout_ms: None,
            caller: None,
            open: false,
            worktree: None,
            output_schema: None,
        };
        let started = self
            .service
            .prepare_team_start(request)
            .await
            .map_err(|_| failed())?;
        let state = self
            .manager
            .get_state(&started.connection_id)
            .await
            .ok_or_else(failed)?;
        let state = state.read().await;
        if started.run_id != run
            || state.cli_run.as_ref().is_none_or(|tag| tag.run_id != run)
            || Some(state.agent_type) != crate::hall::engine::agent_type(p.engine())
            || state.conversation_id != Some(started.conversation_id)
            || state.working_dir.as_ref().and_then(|v| v.to_str()) != Some(p.cwd())
        {
            return Err(failed());
        }
        let domain = self
            .manager
            .hall_exits
            .for_spawn(run, &started.connection_id)
            .map_err(|_| failed())?
            .ok_or_else(failed)?;
        let b = Binding {
            owner: p.owner.clone(),
            lease_id: domain.lease_id().into(),
            run_id: run.into(),
            connection_id: started.connection_id,
            conversation_id: started.conversation_id,
            external_session_id: state.external_id.clone().ok_or_else(failed)?,
            started_at: Utc::now(),
        };
        self.manager.hall_identities.bind(&b)?;
        Ok(b)
    }
    async fn abort_start(&self, id: &StartIdentity) -> Result<AbortStatus, AppCommandError> {
        self.cleanup_start(id, true).await
    }
    async fn recover_start(&self, id: &StartIdentity) -> Result<AbortStatus, AppCommandError> {
        self.cleanup_start(id, false).await
    }
    async fn start_observation(&self, id: &StartIdentity) -> Option<ExitObservation> {
        self.manager
            .hall_exits
            .for_recovery(&id.run, &id.owner)
            .ok()
            .flatten()
            .map(|(domain, _)| domain.observation())
    }
    fn revoke_run(&self, run: &str) {
        self.manager.hall_identities.revoke(run);
        self.starting.lock().unwrap().remove(run);
    }
    async fn activate(&self, b: &Binding) -> Result<(), AppCommandError> {
        self.manager.hall_exits.bound(b).ok_or_else(failed)?;
        self.service
            .activate_team_run(&b.run_id)
            .map_err(|_| failed())
    }
    async fn cancel(&self, b: &Binding) -> Result<(), AppCommandError> {
        self.manager.hall_exits.bound(b).ok_or_else(failed)?;
        self.service
            .cancel(&b.run_id, wire::CancelReason::User)
            .await
            .map_err(|_| failed())
    }
    async fn events(
        &self,
        b: &Binding,
        after: u64,
    ) -> Result<Vec<(u64, RuntimeEvent)>, AppCommandError> {
        self.manager.hall_exits.bound(b).ok_or_else(failed)?;
        let response = self
            .service
            .events(&wire::EventsRequest {
                run_id: b.run_id.clone(),
                after,
                wait_ms: 0,
            })
            .await
            .map_err(|_| failed())?;
        if response.gap
            || response
                .events
                .iter()
                .any(|e| matches!(e.body, EventBody::Gap {}))
        {
            return Err(failed());
        }
        Ok(response
            .events
            .into_iter()
            .map(|e| {
                let body = match e.body {
                    EventBody::Permission {
                        request_id,
                        decision,
                        ..
                    } => RuntimeEvent::WaitingInput {
                        key: format!("permission:{request_id}"),
                        waiting: decision == wire::PermissionVerdict::Waiting,
                    },
                    EventBody::Question {
                        question_id,
                        decision,
                        ..
                    } => RuntimeEvent::WaitingInput {
                        key: format!("question:{question_id}"),
                        waiting: decision == wire::QuestionVerdict::Waiting,
                    },
                    EventBody::Result(r) => RuntimeEvent::Result {
                        outcome: if r.subtype == wire::Subtype::ErrorInterrupted {
                            Outcome::Cancelled
                        } else if r.is_error {
                            Outcome::Failed
                        } else {
                            Outcome::Succeeded
                        },
                        body: r.result.unwrap_or_default(),
                    },
                    _ => RuntimeEvent::Observed,
                };
                (e.seq, body)
            })
            .collect())
    }
    async fn confirm_exit(&self, b: &Binding) -> Result<Option<ExitEvidence>, AppCommandError> {
        let domain = self.manager.hall_exits.bound(b).ok_or_else(failed)?;
        // Called only after the Coordinator verifies finishing/cancelling.
        // Safe ownership-specific closure; no name-based/pid-tree sweep.
        domain.cancel().map_err(|_| failed())?;
        if let Some(state) = self.manager.get_state(&b.connection_id).await {
            if state
                .read()
                .await
                .cli_run
                .as_ref()
                .is_none_or(|tag| tag.run_id != b.run_id)
            {
                return Err(failed());
            }
            self.manager
                .disconnect(&b.connection_id)
                .await
                .map_err(|_| failed())?;
        }
        let report = domain.observation();
        Ok(report.complete.then(|| ExitEvidence {
            owner: b.owner.clone(),
            lease_id: b.lease_id.clone(),
            run_id: b.run_id.clone(),
            connection_id: b.connection_id.clone(),
            confirmed_at: report.observed_at,
        }))
    }
    async fn exit_observation(&self, b: &Binding) -> Option<ExitObservation> {
        self.manager.hall_exits.bound(b).map(|d| d.observation())
    }
    fn revoke(&self, b: &Binding) {
        self.revoke_run(&b.run_id);
        self.manager.hall_exits.remove(b);
    }
}
