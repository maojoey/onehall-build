use crate::app_error::AppCommandError;
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) fn storage(_: sea_orm::DbErr) -> AppCommandError {
    AppCommandError::io_error("Team mailbox storage is unavailable")
}
pub(super) fn denied() -> AppCommandError {
    AppCommandError::permission_denied(
        "A current trusted team identity and role binding are required",
    )
}
pub(super) fn conflict() -> AppCommandError {
    AppCommandError::already_exists("Team mailbox request or role binding changed")
}
pub(super) fn uuid(v: &str) -> Result<String, AppCommandError> {
    uuid::Uuid::parse_str(v)
        .map(|v| v.to_string())
        .map_err(|_| AppCommandError::invalid_input("Invalid team mailbox identity"))
}
#[derive(Serialize, FromQueryResult)]
#[serde(rename_all = "camelCase")]
pub struct RoleBinding {
    pub member_id: Option<String>,
    pub revision: i64,
}
#[derive(Serialize, FromQueryResult)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub message_id: String,
    pub sender_member_id: Option<String>,
    pub recipient_member_id: String,
    pub task_id: Option<String>,
    pub attempt_id: Option<String>,
    pub kind: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    /// Current display names, so a reader (often the PM model) sees who wrote
    /// instead of bare IDs. Display only; identity stays the member ID.
    pub sender_name: Option<String>,
    pub recipient_name: Option<String>,
}
/// `hall_message` rows with the current sender / recipient names.
const MESSAGES: &str = "SELECT m.*,CASE WHEN m.kind='report' AND m.sender_member_id IS NULL THEN 'Branch report' ELSE s.name END AS sender_name,r.name AS recipient_name \
    FROM hall_message m LEFT JOIN hall_member s ON \
    s.member_id=m.sender_member_id LEFT JOIN hall_member r ON \
    r.member_id=m.recipient_member_id";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDraft {
    pub submission_id: String,
    pub recipient_member_id: String,
    pub body: String,
}
#[derive(Serialize)]
pub(super) struct Delivery<'a> {
    pub submission: &'a str,
    pub sender: Option<&'a str>,
    pub recipient: &'a str,
    pub task: Option<&'a str>,
    pub attempt: Option<&'a str>,
    pub kind: &'a str,
    pub body: &'a str,
}
pub async fn role(db: &DatabaseConnection) -> Result<RoleBinding, AppCommandError> {
    RoleBinding::find_by_statement(Statement::from_string(
        DbBackend::Sqlite,
        "SELECT member_id,revision FROM hall_role_binding WHERE role='pm'",
    ))
    .one(db)
    .await
    .map_err(storage)?
    .ok_or_else(denied)
}
/// Trusted application-user administration only; never exposed as a model tool.
pub async fn bind_pm(
    db: &DatabaseConnection,
    member: Option<String>,
    revision: i64,
) -> Result<RoleBinding, AppCommandError> {
    let member = member.map(|v| uuid(&v)).transpose()?;
    if !(1..i64::MAX).contains(&revision) {
        return Err(conflict());
    }
    if db
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_role_binding SET member_id=?,revision=revision+1 \
        WHERE role='pm' AND revision=? AND (? IS NULL OR EXISTS(SELECT \
        1 FROM hall_member WHERE member_id=? AND engine IN ('codex',\
        'claude_code')))",
            vec![
                member.clone().into(),
                revision.into(),
                member.clone().into(),
                member.into(),
            ],
        ))
        .await
        .map_err(storage)?
        .rows_affected()
        != 1
    {
        return Err(conflict());
    }
    role(db).await
}
pub(super) async fn send(
    db: &DatabaseConnection,
    d: Delivery<'_>,
) -> Result<Message, AppCommandError> {
    let submission = uuid(d.submission)?;
    let recipient = uuid(d.recipient)?;
    if d.body.trim().is_empty()
        || d.body.chars().count() > 8000
        || d.body
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(AppCommandError::invalid_input(
            "Team message must contain at most 8000 text characters",
        ));
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                "onehall-message-v1",
                &submission,
                d.sender,
                &recipient,
                d.task,
                d.attempt,
                d.kind,
                d.body
            ))
            .map_err(|_| denied())?
        )
    );
    let tx = db.begin().await.map_err(storage)?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_message(message_id,submission_id,request_hash,\
        sender_member_id,recipient_member_id,task_id,attempt_id,kind,\
        body,created_at) SELECT ?,?,?,?,?,?,?,?,?,? WHERE EXISTS(SELECT \
        1 FROM hall_member WHERE member_id=?) ON \
        CONFLICT(submission_id) DO NOTHING",
        vec![
            uuid::Uuid::new_v4().to_string().into(),
            submission.clone().into(),
            hash.clone().into(),
            d.sender.map(str::to_owned).into(),
            recipient.clone().into(),
            d.task.map(str::to_owned).into(),
            d.attempt.map(str::to_owned).into(),
            d.kind.to_owned().into(),
            d.body.to_owned().into(),
            Utc::now().into(),
            recipient.into(),
        ],
    ))
    .await
    .map_err(storage)?;
    let row = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT request_hash FROM hall_message WHERE submission_id=?",
            vec![submission.clone().into()],
        ))
        .await
        .map_err(storage)?
        .ok_or_else(denied)?;
    if row.try_get::<String>("", "request_hash").map_err(storage)? != hash {
        return Err(conflict());
    }
    let result = Message::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        format!("{MESSAGES} WHERE m.submission_id=?"),
        vec![submission.into()],
    ))
    .one(&tx)
    .await
    .map_err(storage)?
    .ok_or_else(denied)?;
    tx.commit().await.map_err(storage)?;
    Ok(result)
}
pub async fn inbox(db: &DatabaseConnection, member: &str) -> Result<Vec<Message>, AppCommandError> {
    Message::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        format!(
            "{MESSAGES} WHERE m.recipient_member_id=? ORDER BY m.created_at \
            DESC,m.message_id DESC LIMIT 100"
        ),
        vec![uuid(member)?.into()],
    ))
    .all(db)
    .await
    .map_err(storage)
}
pub async fn acknowledge(
    db: &DatabaseConnection,
    member: &str,
    message: &str,
) -> Result<(), AppCommandError> {
    if db
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_message SET \
        acknowledged_at=COALESCE(acknowledged_at,?) WHERE message_id=? \
        AND recipient_member_id=?",
            vec![
                Utc::now().into(),
                uuid(message)?.into(),
                uuid(member)?.into(),
            ],
        ))
        .await
        .map_err(storage)?
        .rows_affected()
        != 1
    {
        return Err(denied());
    }
    Ok(())
}
pub async fn send_as_user(
    db: &DatabaseConnection,
    d: MessageDraft,
) -> Result<Message, AppCommandError> {
    send(
        db,
        Delivery {
            submission: &d.submission_id,
            sender: None,
            recipient: &d.recipient_member_id,
            task: None,
            attempt: None,
            kind: "message",
            body: &d.body,
        },
    )
    .await
}

/// Called by the host after synchronizing a branch. This only appends a report;
/// it never starts or wakes a coordinator, task, or remote process.
pub(crate) async fn notify_branch_result(
    db: &DatabaseConnection,
    dispatch_id: &str,
) -> Result<(), AppCommandError> {
    #[derive(FromQueryResult)]
    struct Receipt {
        dispatch_id: String,
        branch_id: String,
        title: String,
        status: String,
        remote_revision: i64,
        remote_task_revision: i64,
        result_text: Option<String>,
    }
    let Some(row) = Receipt::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT dispatch_id,branch_id,title,status,remote_revision,remote_task_revision,result_text \
         FROM hall_branch_dispatch WHERE dispatch_id=? AND remote_revision>0 \
         AND status IN ('rejected','succeeded','failed','interrupted','outcome_unknown')",
        vec![uuid(dispatch_id)?.into()],
    )).one(db).await.map_err(storage)? else { return Ok(()); };
    let Some(recipient) = role(db).await?.member_id else {
        return Ok(());
    };
    let digest = Sha256::digest(
        serde_json::to_vec(&(
            "onehall-branch-notice-v1",
            &row.dispatch_id,
            row.remote_revision,
            row.remote_task_revision,
            &row.status,
            &recipient,
        ))
        .map_err(|_| denied())?,
    );
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let submission = uuid::Uuid::from_bytes(bytes).to_string();
    // JSON keeps model-produced result text explicitly data and escapes control
    // characters. Limit even worst-case JSON escaping to the mailbox's bound.
    let result = row
        .result_text
        .as_ref()
        .map(|text| text.chars().take(1000).collect::<String>());
    let body = serde_json::to_string(&serde_json::json!({
        "source":"registered_branch", "dispatchId":row.dispatch_id,
        "branchId":row.branch_id,"title":row.title,"status":row.status,
        "taskRevision":row.remote_task_revision,"result":result,
        "resultTruncated":row.result_text.as_ref().is_some_and(|text| text.chars().count()>1000),
        "executionRequested":false
    }))
    .map_err(|_| denied())?;
    send(
        db,
        Delivery {
            submission: &submission,
            sender: None,
            recipient: &recipient,
            task: None,
            attempt: None,
            kind: "report",
            body: &body,
        },
    )
    .await?;
    Ok(())
}
