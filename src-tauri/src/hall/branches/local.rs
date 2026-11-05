//! Host-local setup and confirmation. Never mounted in the HTTP router.
use super::{binding, inbox, trust};
use crate::app_error::AppCommandError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    CreateTrust {
        name: String,
        #[serde(rename = "exportPath")]
        export_path: PathBuf,
    },
    ImportCredential {
        #[serde(rename = "branchId")]
        branch_id: String,
        path: PathBuf,
        #[serde(default, rename = "acknowledgeTrustedTransport")]
        acknowledge_trusted_transport: bool,
    },
    ListTrust {},
    RevokeTrust {
        issuer_id: String,
        expected_revision: i64,
    },
    ListInbox {},
    Accept {
        selection: binding::Request,
    },
    Reject {
        issuer_id: String,
        dispatch_id: String,
        expected_revision: i64,
    },
}
// No Serialize/Debug: only the protected file and private DB see this value.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialFile {
    version: u32,
    issuer_id: String,
    secret: String,
}
fn invalid() -> AppCommandError {
    AppCommandError::invalid_input("Invalid local dispatch request")
}
fn file_error() -> AppCommandError {
    AppCommandError::permission_denied(
        "Credential file must be a new or existing protected regular file owned by this user",
    )
}
fn storage(_: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::io_error("Local dispatch storage is unavailable")
}
fn sql(q: &str, v: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, q, v)
}
pub fn parse_input(bytes: &[u8]) -> Result<Request, AppCommandError> {
    if bytes.len() > 16 * 1024 {
        return Err(invalid());
    }
    serde_json::from_slice(bytes).map_err(|_| invalid())
}
pub async fn execute_json(db: &DatabaseConnection, bytes: &[u8]) -> Result<Value, AppCommandError> {
    execute(db, parse_input(bytes)?).await
}

#[cfg(unix)]
fn protected_open(path: &Path, create: bool) -> Result<File, AppCommandError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let mut options = OpenOptions::new();
    options
        .read(!create)
        .write(create)
        .create_new(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path).map_err(|_| file_error())?;
    let metadata = file.metadata().map_err(|_| file_error())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(file_error());
    }
    Ok(file)
}
#[cfg(windows)]
fn protected_open(path: &Path, create: bool) -> Result<File, AppCommandError> {
    // Credential files must remain exclusive for the entire read/write.
    windows_protected_open(path, create, 0)
}
#[cfg(windows)]
fn windows_protected_open(
    path: &Path,
    create: bool,
    share_mode: u32,
) -> Result<File, AppCommandError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    // The parent must already be private BEFORE create_new inherits its ACL.
    // No secret is written to a file created under an unverified directory.
    windows_acl(path.parent().ok_or_else(file_error)?)?;
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & 0x400 != 0) {
        return Err(file_error());
    }
    let mut options = OpenOptions::new();
    options
        .read(!create)
        .write(create)
        .create_new(create)
        .share_mode(share_mode)
        .custom_flags(0x00200000); // OPEN_REPARSE_POINT
    let file = options.open(path).map_err(|_| file_error())?;
    let metadata = file.metadata().map_err(|_| file_error())?;
    if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
        return Err(file_error());
    }
    windows_acl(path)?;
    Ok(file)
}
#[cfg(windows)]
fn windows_acl(path: &Path) -> Result<(), AppCommandError> {
    use std::os::windows::process::CommandExt;
    // Fixed script, path supplied through environment, never interpreted as code.
    // Allow only current user and SYSTEM; reject broad, inherited or custom ACLs.
    let script = r#"$ErrorActionPreference='Stop'; $u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $a=Get-Acl -LiteralPath $env:ONEHALL_CREDENTIAL_PATH; if ($a.Owner -ne [Security.Principal.WindowsIdentity]::GetCurrent().Name -and $a.Owner -ne $u) { exit 2 }; $r=$a.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]); if ($r.Count -eq 0) { exit 2 }; foreach($x in $r) { if ($x.AccessControlType -eq 'Allow' -and $x.IdentityReference.Value -ne $u -and $x.IdentityReference.Value -ne 'S-1-5-18' -and $x.IdentityReference.Value -ne 'S-1-5-32-544') { exit 2 } }; exit 0"#;
    let status = std::process::Command::new("powershell.exe")
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW; match process::configure_std_command.
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("ONEHALL_CREDENTIAL_PATH", path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| file_error())?;
    if !status.success() {
        return Err(file_error());
    }
    Ok(())
}
#[cfg(not(any(unix, windows)))]
fn protected_open(_: &Path, _: bool) -> Result<File, AppCommandError> {
    Err(file_error())
}

async fn export(
    db: &DatabaseConnection,
    name: &str,
    path: &Path,
) -> Result<Value, AppCommandError> {
    if !path.is_absolute() {
        return Err(file_error());
    }
    let mut file = protected_open(path, true)?;
    let registration = match trust::create_local(db, name).await {
        Ok(v) => v,
        Err(e) => {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(e);
        }
    };
    let packet =
        json!({"version":1,"issuerId":registration.issuer_id,"secret":registration.secret});
    let written = serde_json::to_vec(&packet)
        .map_err(storage)
        .and_then(|bytes| {
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| file_error())
        });
    if let Err(e) = written {
        // Revocation must succeed before reporting the failed export.
        trust::revoke_local(db, &registration.issuer_id, 1).await?;
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(json!({"issuerId":registration.issuer_id,"enabled":true,"revision":1}))
}
async fn import(
    db: &DatabaseConnection,
    branch: &str,
    path: &Path,
    acknowledge: bool,
) -> Result<Value, AppCommandError> {
    super::service::validate_uuid(branch)?;
    let mut bytes = Vec::new();
    protected_open(path, false)?
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| file_error())?;
    if bytes.len() > 4096 {
        return Err(file_error());
    }
    let packet: CredentialFile = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    super::service::validate_uuid(&packet.issuer_id)?;
    if packet.version != 1
        || packet.secret.len() != 43
        || URL_SAFE_NO_PAD
            .decode(&packet.secret)
            .map_or(true, |v| v.len() != 32)
    {
        return Err(invalid());
    }
    let target=db.query_one(sql("SELECT r.base_url FROM hall_branch b JOIN remote_workspace_connection r ON r.id=b.connection_id WHERE b.id=? AND b.enabled=1",vec![branch.into()])).await.map_err(storage)?.ok_or_else(||AppCommandError::permission_denied("Branch is unavailable or disabled"))?;
    let endpoint: String = target.try_get("", "base_url").map_err(storage)?;
    let endpoint = super::service::validate_endpoint(&endpoint)?;
    let url = reqwest::Url::parse(&endpoint).map_err(|_| invalid())?;
    if url.scheme() != "https" && !acknowledge {
        return Err(AppCommandError::permission_denied(
            "Confirm trusted encrypted transport for this registered endpoint",
        ));
    }
    let changed=db.execute(sql("INSERT INTO hall_branch_dispatch_credential(branch_id,issuer_id,secret,endpoint) SELECT ?,?,?,? WHERE EXISTS (SELECT 1 FROM hall_branch b JOIN remote_workspace_connection r ON r.id=b.connection_id WHERE b.id=? AND b.enabled=1 AND r.base_url=?) ON CONFLICT(branch_id) DO UPDATE SET issuer_id=excluded.issuer_id,secret=excluded.secret,endpoint=excluded.endpoint",vec![branch.into(),packet.issuer_id.clone().into(),packet.secret.into(),endpoint.clone().into(),branch.into(),endpoint.into()])).await.map_err(storage)?;
    if changed.rows_affected() != 1 {
        return Err(AppCommandError::permission_denied(
            "Branch changed; retry credential import",
        ));
    }
    Ok(json!({"branchId":branch,"issuerId":packet.issuer_id,"imported":true}))
}
pub async fn execute(db: &DatabaseConnection, request: Request) -> Result<Value, AppCommandError> {
    match request {
        Request::CreateTrust { name, export_path } => export(db, &name, &export_path).await,
        Request::ImportCredential {
            branch_id,
            path,
            acknowledge_trusted_transport,
        } => import(db, &branch_id, &path, acknowledge_trusted_transport).await,
        Request::ListTrust {} => {
            serde_json::to_value(trust::list_local(db).await?).map_err(storage)
        }
        Request::RevokeTrust {
            issuer_id,
            expected_revision,
        } => {
            trust::revoke_local(db, &issuer_id, expected_revision).await?;
            Ok(json!({"revoked":true}))
        }
        Request::Accept { selection } => {
            serde_json::to_value(binding::accept_local(db, selection).await?).map_err(storage)
        }
        Request::Reject {
            issuer_id,
            dispatch_id,
            expected_revision,
        } => serde_json::to_value(
            inbox::decide(
                db,
                &issuer_id,
                &dispatch_id,
                expected_revision,
                inbox::Decision::Reject,
            )
            .await?,
        )
        .map_err(storage),
        Request::ListInbox {} => {
            let rows=db.query_all(sql("SELECT issuer_id,dispatch_id FROM hall_branch_inbox ORDER BY created_at,dispatch_id LIMIT 1000",vec![])).await.map_err(storage)?;
            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let issuer: String = row.try_get("", "issuer_id").map_err(storage)?;
                let dispatch: String = row.try_get("", "dispatch_id").map_err(storage)?;
                let mut detail = serde_json::to_value(inbox::get(db, &issuer, &dispatch).await?)
                    .map_err(storage)?;
                detail["taskId"] = json!(binding::task_id(db, &issuer, &dispatch).await?);
                // v2 items show what the operator is accepting: input names,
                // sizes and output slots. A manifest failing its digest is an error.
                if let Some(manifest) =
                    crate::hall::exchange::review::for_dispatch(db, &issuer, &dispatch).await?
                {
                    detail["exchangeManifest"] = serde_json::to_value(manifest).map_err(storage)?;
                }
                result.push(detail);
            }
            Ok(json!(result))
        }
    }
}
#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn hall_dispatch_local(
    window: tauri::WebviewWindow,
    db: tauri::State<'_, crate::db::AppDatabase>,
    request: Request,
) -> Result<Value, AppCommandError> {
    let url = window
        .url()
        .map_err(|_| AppCommandError::permission_denied("Unable to verify local desktop"))?;
    crate::update::desktop_auth::validate_window(window.label(), &url)
        .map_err(AppCommandError::permission_denied)?;
    // Desktop exposes local decisions only; credentials remain stdin/file setup.
    if !matches!(
        request,
        Request::ListInbox {} | Request::Accept { .. } | Request::Reject { .. }
    ) {
        return Err(AppCommandError::permission_denied(
            "Credential setup requires the local CLI",
        ));
    }
    execute(&db.conn, request).await
}

/// CLI-only preflight before initializing the DB. Existing storage is never
/// chmodded implicitly. Newly created storage is private before opening SQLite.
pub fn prepare_database(data_dir: &Path, database_name: &str) -> Result<(), AppCommandError> {
    if !data_dir.is_absolute()
        || data_dir
            .ancestors()
            .any(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
    {
        return Err(file_error());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        if !data_dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(data_dir)
                .map_err(|_| file_error())?;
        }
        let metadata = std::fs::symlink_metadata(data_dir).map_err(|_| file_error())?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(file_error());
        }
    }
    #[cfg(windows)]
    {
        if !data_dir.is_dir() {
            return Err(file_error());
        }
        windows_acl(data_dir)?;
    }
    let path = data_dir.join(database_name);
    let create = !path.try_exists().map_err(|_| file_error())?;
    #[cfg(windows)]
    // SQLite keeps read/write handles open while the server is running. Share
    // those accesses only for DB preflight; retain ACL/reparse/owner checks and
    // exclude FILE_SHARE_DELETE so the file cannot be replaced during validation.
    let file = windows_protected_open(&path, create, 0x1 | 0x2)?;
    #[cfg(not(windows))]
    let file = protected_open(&path, create)?;
    drop(file);
    Ok(())
}
