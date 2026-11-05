//! Headquarters owns endpoint selection and credentials. Public values are always summaries.
use super::model::{BranchStatus, HallBranchSummary, RegisterBranchInput};
use crate::{
    app_error::AppCommandError,
    db::service::remote_workspace_connection_service as remote,
    models::{RemoteWorkspaceConnectionInfo, ToHeaderMap},
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use std::{sync::OnceLock, time::Duration};
use tokio::sync::{Mutex, Semaphore};

// Serializes compound registry/derived-connection writes in this process. SQLite
// transactions additionally protect the bound count/mapping across connections.
static WRITES: Mutex<()> = Mutex::const_new(());
static PROBES: Semaphore = Semaphore::const_new(8);
const REGISTRY_LIMIT: usize = 32;

pub(super) fn database_error(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Branch registry database operation failed")
}
pub use super::validation::{validate_endpoint, validate_uuid};
pub(super) fn client() -> Result<&'static reqwest::Client, AppCommandError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(90))
                .gzip(false)
                .brotli(false)
                .build()
                .map_err(|_| "Branch HTTP client unavailable".to_owned())
        })
        .as_ref()
        .map_err(|e| AppCommandError::network(e.clone()))
}
#[derive(FromQueryResult)]
struct Row {
    id: String,
    name: String,
    connection_id: i32,
    enabled: bool,
    revision: i64,
    status: String,
    checked_at: Option<String>,
}
impl Row {
    fn summary(&self) -> HallBranchSummary {
        HallBranchSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            enabled: self.enabled,
            revision: self.revision,
            status: match self.status.as_str() {
                "online" => BranchStatus::Online,
                "unreachable" => BranchStatus::Unreachable,
                "auth_failed" => BranchStatus::AuthFailed,
                _ => BranchStatus::Unknown,
            },
            checked_at: self.checked_at.clone(),
        }
    }
}
fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, query, values)
}
async fn row(db: &DatabaseConnection, id: &str) -> Result<Row, AppCommandError> {
    validate_uuid(id)?;
    let result = db
        .query_one(sql(
            "SELECT * FROM hall_branch WHERE id = ?",
            vec![id.into()],
        ))
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppCommandError::not_found("Branch is not registered"))?;
    Row::from_query_result(&result, "").map_err(database_error)
}
pub async fn list(db: &DatabaseConnection) -> Result<Vec<HallBranchSummary>, AppCommandError> {
    db.query_all(sql("SELECT * FROM hall_branch ORDER BY name,id", vec![]))
        .await
        .map_err(database_error)?
        .iter()
        .map(|r| {
            Row::from_query_result(r, "")
                .map(|r| r.summary())
                .map_err(database_error)
        })
        .collect()
}
pub async fn register(
    db: &DatabaseConnection,
    input: RegisterBranchInput,
) -> Result<HallBranchSummary, AppCommandError> {
    let endpoint = validate_endpoint(&input.base_url)?;
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Err(AppCommandError::invalid_input(
            "Branch name must have 1 to 128 characters",
        ));
    }
    let headers = super::validation::credentials(&input.token, &input.headers)?;
    let _guard = WRITES.lock().await;
    let txn = db.begin().await.map_err(database_error)?;
    let count = txn
        .query_one(sql("SELECT COUNT(*) AS n FROM hall_branch", vec![]))
        .await
        .map_err(database_error)?
        .ok_or_else(|| database_error("count"))?
        .try_get::<i64>("", "n")
        .map_err(database_error)?;
    if count >= REGISTRY_LIMIT as i64 {
        return Err(AppCommandError::invalid_input(
            "Branch registry limit reached (32)",
        ));
    }
    let connection = remote::create(&txn, name, &endpoint, &input.token, &headers)
        .await
        .map_err(|_| database_error("connection"))?;
    let id = uuid::Uuid::new_v4().to_string();
    txn.execute(sql(
        "INSERT INTO hall_branch (id,name,connection_id) VALUES (?,?,?)",
        vec![id.clone().into(), name.into(), connection.id.into()],
    ))
    .await
    .map_err(database_error)?;
    txn.commit().await.map_err(database_error)?;
    Ok(HallBranchSummary {
        id,
        name: name.into(),
        enabled: true,
        revision: 1,
        status: BranchStatus::Unknown,
        checked_at: None,
    })
}
pub async fn set_enabled(
    db: &DatabaseConnection,
    id: &str,
    enabled: bool,
    expected_revision: i64,
) -> Result<HallBranchSummary, AppCommandError> {
    validate_uuid(id)?;
    let changed=db.execute(sql("UPDATE hall_branch SET enabled=?, revision=revision+1, status='unknown', checked_at=NULL WHERE id=? AND revision=?",
        vec![enabled.into(),id.into(),expected_revision.into()])).await.map_err(database_error)?;
    if changed.rows_affected() != 1 {
        row(db, id).await?;
        return Err(AppCommandError::already_exists(
            "Branch changed; refresh before trying again",
        ));
    }
    Ok(row(db, id).await?.summary())
}
// Never Serialize or Debug: only the resolved immutable request target sees secrets.
pub(super) struct Target {
    pub summary: HallBranchSummary,
    pub connection: RemoteWorkspaceConnectionInfo,
}
pub(super) async fn target(db: &DatabaseConnection, id: &str) -> Result<Target, AppCommandError> {
    let row = row(db, id).await?;
    if !row.enabled {
        return Err(AppCommandError::permission_denied("Branch is disabled"));
    }
    let mut connection = remote::get_registered(db, row.connection_id)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppCommandError::not_found("Branch connection unavailable"))?;
    // Revalidate stored rows rather than trusting a legacy/edit path.
    validate_endpoint(&connection.base_url)?;
    connection.headers = super::validation::credentials(&connection.token, &connection.headers)?;
    Ok(Target {
        summary: row.summary(),
        connection,
    })
}
pub async fn probe(
    db: &DatabaseConnection,
    id: &str,
) -> Result<HallBranchSummary, AppCommandError> {
    let _permit = PROBES
        .try_acquire()
        .map_err(|_| AppCommandError::network("Branch probes are busy; retry shortly"))?;
    let target = target(db, id).await?;
    let response = client()?
        .post(format!("{}/api/health", target.connection.base_url))
        .bearer_auth(target.connection.token.trim())
        .headers(target.connection.headers.to_header_map())
        .timeout(Duration::from_secs(5))
        .json(&serde_json::json!({}))
        .send()
        .await;
    let status = match response {
        Ok(r) if r.status() == 401 || r.status() == 403 => "auth_failed",
        Ok(r) if r.status().is_success() => "online",
        _ => "unreachable",
    };
    db.execute(sql(
        "UPDATE hall_branch SET status=?, checked_at=? WHERE id=? AND revision=?",
        vec![
            status.into(),
            chrono::Utc::now().to_rfc3339().into(),
            id.into(),
            target.summary.revision.into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    Ok(row(db, id).await?.summary())
}

// Kept in its own small module; used by the native-only IPC wrapper and tests.
pub use super::desktop::connect_branch;
