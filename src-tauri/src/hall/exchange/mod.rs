//! Host-only file exchange core. HTTP and model tools belong to later packages.
mod files;
pub mod manifest;
pub(crate) mod migration;
pub(crate) mod protocol_migration;
pub mod api;
pub mod desktop_transfer;
pub mod review;
pub(crate) mod runtime;
pub mod service;
mod store;
mod types;
pub use store::{
    DispatchBinding, DispatchInput, Exchange, MaterializedAttempt, OutputTarget, PreparedArtifact, ReceivedArtifact,
    ReceivedInput, SealPreparation,
};
pub use types::*;
#[cfg(test)]
mod tests;
