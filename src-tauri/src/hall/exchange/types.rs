use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_MANIFEST_ITEMS: usize = 200;
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub type Result<T> = std::result::Result<T, ExchangeError>;
#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("exchange_invalid_input")]
    Invalid,
    #[error("exchange_forbidden")]
    Forbidden,
    #[error("exchange_conflict")]
    Conflict,
    #[error("exchange_quota_exceeded")]
    Quota,
    #[error("exchange_unstable_file")]
    Unstable,
    #[error("exchange_storage_unavailable")]
    Storage,
}
impl From<std::io::Error> for ExchangeError {
    fn from(_: std::io::Error) -> Self {
        Self::Storage
    }
}
impl From<sea_orm::DbErr> for ExchangeError {
    fn from(_: sea_orm::DbErr) -> Self {
        Self::Storage
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectRef {
    pub object_id: String,
    pub display_name: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputSlot {
    pub slot_id: String,
    pub display_name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExchangeManifest {
    pub schema_version: u32,
    pub manifest_id: String,
    pub inputs: Vec<ObjectRef>,
    pub outputs: Vec<OutputSlot>,
    pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    #[serde(flatten)]
    pub object: ObjectRef,
    pub slot_id: String,
    pub dispatch_id: String,
    pub branch_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub run_id: String,
    pub sealed_at: DateTime<Utc>,
    pub exit_verified: bool,
}
#[derive(Clone)]
pub struct Administrator {
    pub(super) owner: String,
}
impl Administrator {
    /// Trusted host authentication only; never deserialize a request as authority.
    pub(crate) fn authenticated(owner: &str) -> Result<Self> {
        identity(owner)?;
        Ok(Self {
            owner: owner.into(),
        })
    }
}
#[derive(Clone)]
pub struct DispatchOwner {
    pub(super) issuer: String,
    pub(super) dispatch: String,
    pub(super) branch: String,
}
impl DispatchOwner {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    pub fn dispatch(&self) -> &str {
        &self.dispatch
    }
    // Host protocol wiring lands in 05; authority stays crate-private.
    #[allow(dead_code)]
    pub(crate) fn authenticated(issuer: &str, dispatch: &str, branch: &str) -> Result<Self> {
        identity(issuer)?;
        uuid(dispatch)?;
        uuid(branch)?;
        Ok(Self {
            issuer: issuer.into(),
            dispatch: dispatch.into(),
            branch: branch.into(),
        })
    }
}
#[derive(Clone)]
pub struct AttemptOwner {
    pub(super) dispatch: DispatchOwner,
    pub(super) task: String,
    pub(super) attempt: String,
    pub(super) run: String,
    pub(super) generation: i64,
}
impl AttemptOwner {
    /// Whether this owner is for the given host attempt id.
    pub(crate) fn is_attempt(&self, attempt: &str) -> bool {
        self.attempt == attempt
    }
    /// Use the host's real attempt_number, not a model-supplied generation.
    #[allow(dead_code)] // Minted by the host runtime integration in 05.
    pub(crate) fn authenticated(
        dispatch: &DispatchOwner,
        task: &str,
        attempt: &str,
        run: &str,
        generation: i64,
    ) -> Result<Self> {
        uuid(task)?;
        uuid(attempt)?;
        uuid(run)?;
        if generation < 1 {
            return Err(ExchangeError::Invalid);
        }
        Ok(Self {
            dispatch: dispatch.clone(),
            task: task.into(),
            attempt: attempt.into(),
            run: run.into(),
            generation,
        })
    }
}
#[derive(Clone, Copy)]
pub enum ExitVerification {
    Verified,
    Unverified,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadTicket {
    pub object_id: String,
    pub transfer_id: String,
}
pub(super) fn identity(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 200 || s.chars().any(char::is_control) {
        return Err(ExchangeError::Invalid);
    }
    Ok(())
}
pub(super) fn uuid(s: &str) -> Result<()> {
    if uuid::Uuid::parse_str(s)
        .map_err(|_| ExchangeError::Invalid)?
        .to_string()
        != s
    {
        return Err(ExchangeError::Invalid);
    }
    Ok(())
}
pub(super) fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
