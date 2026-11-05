//! Read-only saved-data checks. No engine launch, login probe, credential read,
//! permission enforcement, configuration write or activation authorization.
use super::super::{HallMember, MemberError};
use super::{combine, load, Diagnostic};
use crate::app_error::AppCommandError;
use crate::db::entities::{assistant, folder};
use crate::models::assistant::{AssistantAgentConfig, MAX_LABEL_SNAPSHOT_BYTES};
use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, QuerySelect,
};
use serde::Serialize;

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Passed,
    Warning,
    Blocked,
    Pending,
}

#[derive(Serialize)]
struct Check {
    id: &'static str,
    status: Status,
    code: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberCheckReport {
    member_id: String,
    revision: i64,
    check_version: String,
    configuration_valid: bool,
    activation_available: bool,
    checks: Vec<Check>,
    diagnostics: Vec<Diagnostic>,
    template_recorded_revision: Option<i64>,
    template_current_revision: Option<i64>,
    execution_preview_supported: bool,
    snapshot_storage_supported: bool,
}

impl MemberCheckReport {
    pub(super) fn data_valid(&self) -> bool {
        self.configuration_valid
    }
}

// Deliberately exclude instructions, avatar, description and names. Parse
// options strictly, never through the UI reader's tolerant default fallback.
#[derive(FromQueryResult, PartialEq)]
struct Template {
    agent_type: String,
    enabled: bool,
    revision: i64,
    deleted_at: Option<DateTime<Utc>>,
    agent_options: String,
    label_snapshot: String,
}
#[derive(FromQueryResult, PartialEq)]
struct Project {
    parent_id: Option<i32>,
    kind: folder::FolderKind,
    deleted_at: Option<DateTime<Utc>>,
}
#[derive(PartialEq)]
struct References {
    template: Option<Template>,
    project: Option<Project>,
}

async fn references(
    db: &DatabaseConnection,
    member: &HallMember,
) -> Result<References, AppCommandError> {
    let template = if let Some(uid) = &member.assistant_uid {
        assistant::Entity::find()
            .select_only()
            .columns([
                assistant::Column::AgentType,
                assistant::Column::Enabled,
                assistant::Column::Revision,
                assistant::Column::DeletedAt,
                assistant::Column::AgentOptions,
                assistant::Column::LabelSnapshot,
            ])
            .filter(assistant::Column::Uid.eq(uid))
            .into_model::<Template>()
            .one(db)
            .await
            .map_err(MemberError::Database)?
    } else {
        None
    };
    let project = if let Some(id) = member.project_id {
        folder::Entity::find_by_id(id)
            .select_only()
            .columns([
                folder::Column::ParentId,
                folder::Column::Kind,
                folder::Column::DeletedAt,
            ])
            .into_model::<Project>()
            .one(db)
            .await
            .map_err(MemberError::Database)?
    } else {
        None
    };
    Ok(References { template, project })
}

fn template_valid(row: &Template) -> bool {
    if row.agent_options.len() > MAX_LABEL_SNAPSHOT_BYTES
        || row.label_snapshot.len() > MAX_LABEL_SNAPSHOT_BYTES
    {
        return false;
    }
    let (Ok(agent_options), Ok(label_snapshot)) = (
        serde_json::from_str(&row.agent_options),
        serde_json::from_str(&row.label_snapshot),
    ) else {
        return false;
    };
    let Some(agent_type) = crate::hall::engine::agent_type(&row.agent_type) else {
        return false;
    };
    AssistantAgentConfig {
        agent_type,
        agent_options,
        label_snapshot,
    }
    .validate()
    .is_ok()
}

fn template_check(member: &HallMember, row: Option<&Template>) -> Check {
    let (status, code) = if member.assistant_uid.is_none() {
        if member.assistant_revision.is_some() {
            (Status::Blocked, "template_invalid")
        } else {
            (Status::Warning, "template_not_bound")
        }
    } else {
        match row {
            None => (Status::Blocked, "template_missing"),
            Some(row) if row.deleted_at.is_some() => (Status::Blocked, "template_missing"),
            Some(row) if !row.enabled => (Status::Blocked, "template_disabled"),
            Some(row) if row.agent_type != member.engine => {
                (Status::Blocked, "template_engine_mismatch")
            }
            Some(row) if row.revision < 1 || !template_valid(row) => {
                (Status::Blocked, "template_invalid")
            }
            Some(row) if member.assistant_revision != Some(row.revision) => {
                (Status::Blocked, "template_changed")
            }
            Some(_) => (Status::Passed, "template_ok"),
        }
    };
    Check {
        id: "template",
        status,
        code,
    }
}

pub async fn hall_check_member_core(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
    expected_check_version: String,
) -> Result<MemberCheckReport, AppCommandError> {
    if expected_check_version.len() != 64
        || !expected_check_version
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppCommandError::invalid_input(
            "A member observation version is required",
        ));
    }
    let (member, inputs) = load(db, &member_id, expected_revision).await?;
    let directory_ok = inputs.is_some();
    let config = combine(&member, inputs);
    if config.check_version != expected_check_version {
        return Err(MemberError::RevisionConflict.into());
    }
    let refs = references(db, &member).await?;
    let project_ok = refs.project.as_ref().is_some_and(|row| {
        row.deleted_at.is_none()
            && row.parent_id.is_none()
            && row.kind == folder::FolderKind::Regular
    });
    let checks = vec![
        Check {
            id: "directory",
            status: if directory_ok {
                Status::Passed
            } else {
                Status::Blocked
            },
            code: if directory_ok {
                "directory_ok"
            } else {
                "directory_unavailable"
            },
        },
        Check {
            id: "declaration",
            status: if !directory_ok {
                Status::Pending
            } else if !config.valid {
                Status::Blocked
            } else if config.files.iter().all(|file| file.status == "missing") {
                Status::Warning
            } else {
                Status::Passed
            },
            code: if !directory_ok {
                "directory_unavailable"
            } else if !config.valid {
                "declaration_invalid"
            } else if config.files.iter().all(|file| file.status == "missing") {
                "defaults_only"
            } else {
                "declaration_valid"
            },
        },
        Check {
            id: "engine",
            status: if crate::hall::engine::supported(&member.engine) {
                Status::Passed
            } else {
                Status::Blocked
            },
            code: match member.engine.as_str() {
                "codex" => "engine_codex",
                "claude_code" => "engine_claude_code",
                _ => "engine_unsupported",
            },
        },
        template_check(&member, refs.template.as_ref()),
        Check {
            id: "project",
            status: if member.project_id.is_none() {
                Status::Warning
            } else if project_ok {
                Status::Passed
            } else {
                Status::Blocked
            },
            code: if member.project_id.is_none() {
                "project_not_bound"
            } else if project_ok {
                "project_ok"
            } else {
                "project_unavailable"
            },
        },
        Check {
            id: "runtime",
            status: Status::Pending,
            code: "runtime_unverified",
        },
        Check {
            id: "execution",
            status: Status::Pending,
            code: "execution_pending",
        },
    ];
    let (after_member, after_inputs) = load(db, &member_id, expected_revision).await?;
    let after = combine(&after_member, after_inputs);
    if after_member != member
        || after.check_version != expected_check_version
        || references(db, &member).await? != refs
    {
        return Err(MemberError::RevisionConflict.into());
    }
    Ok(MemberCheckReport {
        member_id,
        revision: expected_revision,
        check_version: expected_check_version,
        configuration_valid: !checks.iter().any(|row| row.status == Status::Blocked),
        activation_available: false,
        checks,
        diagnostics: config.diagnostics,
        template_recorded_revision: member.assistant_revision,
        template_current_revision: refs.template.map(|row| row.revision),
        execution_preview_supported: true,
        snapshot_storage_supported: true,
    })
}
