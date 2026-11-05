//! Member declarations, explicit UI-overlay saves and immutable snapshots.
//! No activation, engine probes, credential access or permission enforcement.
mod draft;
mod execution;
mod merge;
mod preflight;
pub(super) mod read;
mod save;
mod snapshots;
mod write;
pub use draft::{hall_preview_member_config_draft_core, MemberConfigDraftPreview};
pub use execution::{hall_preview_member_execution_core, MemberExecutionPreview};
pub use preflight::{hall_check_member_core, MemberCheckReport};
pub use save::hall_save_member_config_core;
pub use snapshots::{
    hall_list_member_snapshots_core, hall_save_member_snapshot_core, SnapshotRecord,
};

use super::{entity, HallMember, MemberError};
use crate::app_error::AppCommandError;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberConfigPreview {
    member_id: String,
    revision: i64,
    valid: bool,
    files: Vec<ConfigFile>,
    effective: Option<Value>,
    fields: Vec<ConfigField>,
    diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    overlay: Option<Value>,
    save_supported: bool,
    check_supported: bool,
    check_version: String,
}

#[derive(Serialize)]
struct ConfigFile {
    path: &'static str,
    status: &'static str,
}
#[derive(Serialize)]
struct ConfigField {
    field: &'static str,
    source: &'static str,
    overridden: bool,
}
#[derive(Serialize)]
struct Diagnostic {
    code: &'static str,
    source: &'static str,
    field: &'static str,
}

fn combine(member: &HallMember, inputs: Option<Vec<read::ReadFile>>) -> MemberConfigPreview {
    let mut result = MemberConfigPreview {
        member_id: member.member_id.clone(),
        revision: member.revision,
        valid: false,
        files: Vec::new(),
        effective: None,
        fields: Vec::new(),
        diagnostics: Vec::new(),
        file_version: None,
        overlay: None,
        save_supported: true,
        check_supported: true,
        check_version: observation_version(member, inputs.as_deref()),
    };
    let Some(inputs) = inputs else {
        result.diagnostics.push(Diagnostic {
            code: "directory_unavailable",
            source: "directory",
            field: "document",
        });
        return result;
    };
    if inputs[0].data.is_some() && inputs[1].data.is_some() {
        result.diagnostics.push(Diagnostic {
            code: "multiple_manuals",
            source: "agent.yaml",
            field: "document",
        });
    }
    let mut values = merge::defaults();
    let mut sources = std::collections::BTreeMap::new();
    let mut manual_values = merge::Patch::new();
    let version = fingerprint(member, &inputs);
    let mut overlay_patch = merge::Patch::new();
    for input in inputs {
        let mut status = if input.data.is_some() {
            "loaded"
        } else {
            "missing"
        };
        if let Some(code) = input.error {
            status = "error";
            result.diagnostics.push(Diagnostic {
                code,
                source: input.path,
                field: "document",
            });
        }
        if let Some(raw) = input.data {
            let overlay = input.path == ".onehall/agent.ui.yaml";
            match merge::parse(&raw, member, overlay) {
                Err((code, field)) => {
                    status = "error";
                    result.diagnostics.push(Diagnostic {
                        code,
                        source: input.path,
                        field,
                    });
                }
                Ok(patch) => {
                    if overlay {
                        overlay_patch = patch.clone();
                    }
                    for (field, value) in patch {
                        let overridden =
                            overlay && manual_values.get(field).is_some_and(|old| *old != value);
                        if !overlay {
                            manual_values.insert(field, value.clone());
                        }
                        values.insert(field, value);
                        sources.insert(field, (input.path, overridden));
                    }
                }
            }
        }
        result.files.push(ConfigFile {
            path: input.path,
            status,
        });
    }
    result.valid = result.diagnostics.is_empty();
    if result.valid {
        result.file_version = Some(version);
        result.overlay = Some(merge::document(&overlay_patch));
        result.effective = Some(merge::effective(&values));
        result.fields = merge::FIELDS
            .into_iter()
            .map(|field| {
                let (source, overridden) =
                    sources.get(field).copied().unwrap_or(("defaults", false));
                ConfigField {
                    field,
                    source,
                    overridden,
                }
            })
            .collect();
    }
    result
}

fn fingerprint(member: &HallMember, files: &[read::ReadFile]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    // Length prefixes distinguish missing, empty and differently split inputs.
    for part in [
        b"onehall-config-v1".as_slice(),
        member.member_id.as_bytes(),
        &member.revision.to_be_bytes(),
        member.cwd.as_bytes(),
    ] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    for file in files {
        hash.update(file.path.as_bytes());
        hash.update([u8::from(file.data.is_some())]);
        let bytes = file.data.as_deref().unwrap_or_default();
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

/// Separate from the save token: invalid/missing/unreadable observations must
/// also bind a preflight report without changing existing save semantics.
fn observation_version(member: &HallMember, files: Option<&[read::ReadFile]>) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"onehall-member-check-v1");
    hash.update([u8::from(files.is_some())]);
    hash.update(fingerprint(member, files.unwrap_or_default()).as_bytes());
    for file in files.unwrap_or_default() {
        let error = file.error.unwrap_or_default();
        hash.update((error.len() as u64).to_be_bytes());
        hash.update(error.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

async fn load<C: ConnectionTrait>(
    db: &C,
    member_id: &str,
    expected_revision: i64,
) -> Result<(HallMember, Option<Vec<read::ReadFile>>), AppCommandError> {
    if uuid::Uuid::parse_str(member_id).is_err() || expected_revision < 1 {
        return Err(AppCommandError::invalid_input(
            "A valid member ID and revision are required",
        ));
    }
    let member = entity::Entity::find_by_id(member_id)
        .one(db)
        .await
        .map_err(MemberError::Database)?
        .ok_or(MemberError::NotFound)?;
    if member.revision != expected_revision {
        return Err(MemberError::RevisionConflict.into());
    }
    let inputs = match super::member_directory(&member.cwd).await {
        Ok(path) if path == member.cwd => {
            tokio::task::spawn_blocking(move || read::read_all(std::path::Path::new(&path)).ok())
                .await
                .map_err(|_| AppCommandError::io_error("Configuration reader failed"))?
        }
        _ => None,
    };
    Ok((member, inputs))
}

async fn check_revision<C: ConnectionTrait>(
    db: &C,
    member_id: &str,
    revision: i64,
) -> Result<(), AppCommandError> {
    let current = entity::Entity::find_by_id(member_id)
        .one(db)
        .await
        .map_err(MemberError::Database)?
        .ok_or(MemberError::NotFound)?;
    if current.revision != revision {
        return Err(MemberError::RevisionConflict.into());
    }
    Ok(())
}

pub async fn hall_preview_member_config_core(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
) -> Result<MemberConfigPreview, AppCommandError> {
    let (member, inputs) = load(db, &member_id, expected_revision).await?;
    let result = combine(&member, inputs);
    check_revision(db, &member_id, expected_revision).await?;
    Ok(result)
}

pub(crate) use snapshots::snapshot_reference;
pub use snapshots::SnapshotReference;

pub use execution::{LaunchProfile, MemberAccess};
pub(crate) use snapshots::snapshot_launch_profile;
