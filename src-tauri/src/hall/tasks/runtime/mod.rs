//! Closed production admission; real execution core reused behind an injectable
//! port. Test fixtures alone exercise the lifecycle until B10/capability gates.
pub mod authority;
mod lifecycle;
pub(crate) mod migration;
pub mod port;
mod types;
pub use lifecycle::Coordinator;
pub use types::{
    Binding, ExitEvidence, Outcome, RunPlan, RuntimeEvent, RuntimeObservation, StoredResult,
};
#[cfg(test)]
pub(crate) mod tests;

pub(crate) mod exit;
pub use exit::ExitObservation;
pub(crate) use types::ExecutionOwner;

pub(crate) mod admission;
pub mod control;
pub mod control_api;

pub(crate) mod recovery_migration;
pub(crate) mod start_recovery;
pub use start_recovery::{AbortStatus, Resolution, StartIdentity, StartObservation};
