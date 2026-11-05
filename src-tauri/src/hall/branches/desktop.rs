//! A distinct saved connection gives every desktop branch its own existing
//! event/terminal/transfer proxy namespace. Only headquarters credentials are copied.
use super::{
    model::{BranchConnection, HallBranchSummary},
    service::{self, database_error},
};
use crate::{
    app_error::AppCommandError, db::service::remote_workspace_connection_service as remote,
    models::ToHeaderMap,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use std::time::Duration;
use tokio::sync::Mutex;
static DERIVED_WRITES: Mutex<()> = Mutex::const_new(());

pub async fn connect_branch(
    db: &DatabaseConnection,
    hq: i32,
    id: &str,
) -> Result<BranchConnection, AppCommandError> {
    service::validate_uuid(id)?;
    let headquarters = remote::get(db, hq)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppCommandError::not_found("Headquarters connection not found"))?;
    // This rejects branch prefixes even if a caller changed/deleted a mapping.
    let base = service::validate_endpoint(&headquarters.base_url)?;
    let derived_hq = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT connection_id FROM hall_branch_desktop WHERE connection_id=?",
            [hq.into()],
        ))
        .await
        .map_err(database_error)?;
    if derived_hq.is_some() {
        return Err(AppCommandError::invalid_input(
            "A branch cannot be used as headquarters",
        ));
    }
    let response = service::client()?
        .post(format!("{base}/api/hall_list_branches"))
        .bearer_auth(headquarters.token.trim())
        .headers(headquarters.headers.to_header_map())
        .json(&serde_json::json!({}))
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|_| AppCommandError::network("Headquarters branch list could not be reached"))?;
    if !response.status().is_success() {
        return Err(AppCommandError::network(
            "Headquarters branch list was rejected",
        ));
    }
    let value = super::relay_http::read_json(response, 1024 * 1024)
        .await
        .map_err(|_| AppCommandError::network("Invalid headquarters branch list"))?;
    let branches: Vec<HallBranchSummary> = serde_json::from_value(value)
        .map_err(|_| AppCommandError::network("Invalid headquarters branch list"))?;
    if branches.len() > 32 {
        return Err(AppCommandError::network("Invalid headquarters branch list"));
    }
    let branch = branches
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| AppCommandError::not_found("Branch is not registered at headquarters"))?;
    if !branch.enabled {
        return Err(AppCommandError::permission_denied("Branch is disabled"));
    }
    if branch.name.trim().is_empty()
        || branch.name.chars().count() > 128
        || branch.name.chars().any(char::is_control)
    {
        return Err(AppCommandError::network("Invalid branch name"));
    }
    let endpoint = format!("{base}/branches/{id}");
    let _guard = DERIVED_WRITES.lock().await;
    let txn = db.begin().await.map_err(database_error)?;
    let current = remote::get(&txn, hq)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppCommandError::not_found("Headquarters connection not found"))?;
    if current.base_url != headquarters.base_url
        || current.token != headquarters.token
        || current.headers != headquarters.headers
    {
        return Err(AppCommandError::configuration_invalid(
            "Headquarters connection changed; retry opening the branch",
        ));
    }

    let mapping = txn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT connection_id FROM hall_branch_desktop WHERE headquarters_id=? AND branch_id=?",
            [hq.into(), id.into()],
        ))
        .await
        .map_err(database_error)?;
    let connection = if let Some(mapping) = mapping {
        let connection_id: i32 = mapping
            .try_get("", "connection_id")
            .map_err(database_error)?;
        remote::update(
            &txn,
            connection_id,
            &branch.name,
            &endpoint,
            &headquarters.token,
            &headquarters.headers,
        )
        .await
        .map_err(|_| database_error("derived connection"))?
    } else {
        let created = remote::create(
            &txn,
            &branch.name,
            &endpoint,
            &headquarters.token,
            &headquarters.headers,
        )
        .await
        .map_err(|_| database_error("derived connection"))?;
        txn.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
            "INSERT INTO hall_branch_desktop(headquarters_id,branch_id,connection_id) VALUES (?,?,?)",[hq.into(),id.into(),created.id.into()])).await.map_err(database_error)?;
        created
    };
    txn.commit().await.map_err(database_error)?;
    Ok(BranchConnection {
        connection_id: connection.id,
        name: branch.name,
    })
}
