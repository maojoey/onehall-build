//! Owned execution candidates. Full template content stays internal; the API
//! exposes a digest and summary, never instructions or template option values.
use super::super::execution_policy::{
    capabilities, consistent, effective_mode, ExecutionCapabilities, ExecutionMode, Platform,
};
use super::super::scope::CompiledMemberScope;
use super::super::{HallMember, MemberError};
use super::{combine, hall_check_member_core, load, MemberCheckReport};
use crate::acp::delegation::types::AgentDelegationDefaults;
use crate::app_error::AppCommandError;
use crate::cli_run::PermissionPolicy;
use crate::db::entities::assistant;
use crate::db::service::assistant_service::MAX_INSTRUCTIONS_CHARS;
use crate::models::assistant::{AssistantAgentConfig, MAX_LABEL_SNAPSHOT_BYTES};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, QuerySelect,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize, FromQueryResult)]
struct Template {
    name: String,
    agent_type: String,
    enabled: bool,
    revision: i64,
    deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_options: String,
    label_snapshot: String,
    instructions: String,
}

#[derive(Serialize, Deserialize)]
struct Scope {
    read: Vec<String>,
    write: Vec<String>,
    shell: Vec<String>,
    network: String,
}
#[derive(Serialize, Deserialize)]
struct Skills {
    shared: Vec<String>,
    own: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Declaration {
    model: Option<String>,
    scope: Scope,
    skills: Skills,
    /// Skipped while `none` so snapshots saved before the field existed keep
    /// their exact bytes (and fingerprint).
    #[serde(default = "no_key", skip_serializing_if = "is_no_key")]
    onedesk_key: String,
    /// Skipped while unset so older snapshots keep their exact bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_mode: Option<ExecutionMode>,
}
fn no_key() -> String {
    "none".to_owned()
}
fn is_no_key(value: &String) -> bool {
    value == "none"
}

// No Debug or public serialization path: this data contains full source and
// desired configuration. Immutable storage preserves it; task attempts must
// later reference the record and still pass the trusted launch gates.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateData {
    schema_version: u8,
    member: HallMember,
    check_version: String,
    template: Option<Template>,
    declaration: Declaration,
    desired_options: AgentDelegationDefaults,
    permission_policy: PermissionPolicy,
    /// Host-generated at capture and only when a mode was chosen; part of the
    /// snapshot digest so evidence or version changes need a new snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_capabilities: Option<ExecutionCapabilities>,
}

pub(super) struct FrozenCandidate {
    data: CandidateData,
    compiled_scope: CompiledMemberScope,
}

#[derive(Serialize)]
pub struct ExecutionIssue {
    code: &'static str,
    field: &'static str,
    index: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionSummary {
    pub(super) snapshot_version: String,
    engine: String,
    requested_mode: String,
    mode_source: &'static str,
    model_source: &'static str,
    permission_policy: &'static str,
    option_count: usize,
    template_revision: Option<i64>,
    instructions_included: bool,
    read_rule_count: usize,
    write_rule_count: usize,
    shell_rule_count: usize,
    network: String,
    runtime_verified: bool,
    /// `None` for a member that never chose a mode and runs on Claude.
    execution_mode: Option<ExecutionMode>,
    capabilities: ExecutionCapabilities,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberExecutionPreview {
    member_id: String,
    revision: i64,
    check_version: String,
    valid: bool,
    launch_allowed: bool,
    summary: Option<ExecutionSummary>,
    issues: Vec<ExecutionIssue>,
    preflight: MemberCheckReport,
}

async fn template(
    db: &DatabaseConnection,
    member: &HallMember,
) -> Result<Option<Template>, AppCommandError> {
    let Some(uid) = &member.assistant_uid else {
        return Ok(None);
    };
    assistant::Entity::find()
        .select_only()
        .columns([
            assistant::Column::Name,
            assistant::Column::AgentType,
            assistant::Column::Enabled,
            assistant::Column::Revision,
            assistant::Column::DeletedAt,
            assistant::Column::AgentOptions,
            assistant::Column::LabelSnapshot,
            assistant::Column::Instructions,
        ])
        .filter(assistant::Column::Uid.eq(uid))
        .into_model::<Template>()
        .one(db)
        .await
        .map_err(|error| MemberError::Database(error).into())
}

fn options(
    member: &HallMember,
    row: Option<&Template>,
) -> Result<AgentDelegationDefaults, AppCommandError> {
    let Some(row) = row else {
        return if member.assistant_uid.is_none() {
            Ok(AgentDelegationDefaults::default())
        } else {
            Err(MemberError::RevisionConflict.into())
        };
    };
    if !row.enabled
        || row.deleted_at.is_some()
        || row.agent_type != member.engine
        || member.assistant_revision != Some(row.revision)
        || row.agent_options.len() > MAX_LABEL_SNAPSHOT_BYTES
        || row.label_snapshot.len() > MAX_LABEL_SNAPSHOT_BYTES
    {
        return Err(MemberError::RevisionConflict.into());
    }
    let config = AssistantAgentConfig {
        agent_type: crate::hall::engine::agent_type(&member.engine)
            .ok_or(MemberError::RevisionConflict)?,
        agent_options: serde_json::from_str(&row.agent_options)
            .map_err(|_| MemberError::RevisionConflict)?,
        label_snapshot: serde_json::from_str(&row.label_snapshot)
            .map_err(|_| MemberError::RevisionConflict)?,
    };
    config
        .validate()
        .map_err(|_| MemberError::RevisionConflict)?;
    Ok(config.agent_options)
}

fn build(
    member: HallMember,
    check_version: String,
    row: Option<Template>,
    declaration: Declaration,
    desired: AgentDelegationDefaults,
) -> (Option<FrozenCandidate>, Vec<ExecutionIssue>) {
    build_inner(member, check_version, row, declaration, desired, None)
}

fn build_inner(
    member: HallMember,
    check_version: String,
    row: Option<Template>,
    declaration: Declaration,
    mut desired: AgentDelegationDefaults,
    // `None`: a live capture, validated for this host. `Some`: a restoration,
    // which keeps the stored capabilities and only checks they are plausible.
    stored: Option<Option<ExecutionCapabilities>>,
) -> (Option<FrozenCandidate>, Vec<ExecutionIssue>) {
    let mut issues = Vec::new();
    let validate_native = stored.is_none();
    let exec_mode = declaration.execution_mode;
    let mut mode_issue = |code: &'static str, field: &'static str| {
        issues.push(ExecutionIssue {
            code,
            field,
            index: None,
        });
    };
    if desired.config_values.contains_key("execution_mode")
        || desired.config_values.contains_key("executionMode")
    {
        mode_issue("template_sets_execution_mode", "execution_mode");
    }
    if exec_mode == Some(ExecutionMode::Restricted) && member.engine != "codex" {
        mode_issue("unsupported_execution_mode", "execution_mode");
    }
    if validate_native
        && exec_mode == Some(ExecutionMode::Restricted)
        && Platform::current() == Platform::Windows
    {
        mode_issue("execution_mode_unavailable", "execution_mode");
    }
    if exec_mode == Some(ExecutionMode::LocalAssistant) && declaration.onedesk_key != "none" {
        mode_issue("key_not_allowed", "onedesk_key");
    }
    let compiled_scope =
        match CompiledMemberScope::compile(&declaration.scope.read, &declaration.scope.write) {
            Ok(scope) => Some(scope),
            Err(fault) => {
                issues.push(ExecutionIssue {
                    code: fault.code,
                    field: fault.field,
                    index: fault.index,
                });
                None
            }
        };
    if validate_native
        && member.engine == "codex"
        && effective_mode(&member.engine, exec_mode) == Some(ExecutionMode::Restricted)
    {
        for (field, rules) in [
            ("scope.read", &declaration.scope.read),
            ("scope.write", &declaration.scope.write),
        ] {
            for (index, rule) in rules.iter().enumerate() {
                if crate::hall::members::launch::scope_directory(rule).is_err() {
                    issues.push(ExecutionIssue {
                        code: "invalid_path_rule",
                        field,
                        index: Some(index),
                    });
                }
            }
        }
    }
    if row
        .as_ref()
        .is_some_and(|row| row.instructions.chars().count() > MAX_INSTRUCTIONS_CHARS)
    {
        issues.push(ExecutionIssue {
            code: "template_content_invalid",
            field: "template",
            index: None,
        });
    }
    let mode = desired.mode_id.as_deref();
    let config_mode = desired.config_values.get("mode").map(String::as_str);
    if mode.zip(config_mode).is_some_and(|(a, b)| a != b) {
        issues.push(ExecutionIssue {
            code: "conflicting_mode",
            field: "mode",
            index: None,
        });
    }
    let selected = mode.or(config_mode);
    let engine = member.engine.clone();
    if selected.is_some_and(|mode| !crate::hall::engine::safe_mode(&engine, mode)) {
        issues.push(ExecutionIssue {
            code: "unsafe_mode",
            field: "mode",
            index: None,
        });
    }
    if declaration.scope.write.is_empty()
        && selected.is_some_and(|mode| crate::hall::engine::mode_writes(&engine, mode))
    {
        issues.push(ExecutionIssue {
            code: "mode_exceeds_scope",
            field: "scope.write",
            index: None,
        });
    }
    let execution_capabilities = match (exec_mode, stored) {
        (None, None | Some(None)) => None,
        (Some(m), None) => Some(capabilities(&engine, Some(m), Platform::current())),
        (Some(m), Some(Some(caps))) if consistent(&engine, m, &caps) => Some(caps),
        // Stripped, forged, or attached to a member that never chose a mode.
        _ => {
            issues.push(ExecutionIssue {
                code: "invalid_capabilities",
                field: "capabilities",
                index: None,
            });
            None
        }
    };
    if !issues.is_empty() {
        return (None, issues);
    }
    let requested = selected
        .unwrap_or(crate::hall::engine::mode_for_scope(
            &engine,
            !declaration.scope.write.is_empty(),
        ))
        .to_owned();
    desired.mode_id = Some(requested);
    if let Some(model) = &declaration.model {
        desired
            .config_values
            .insert("model".to_owned(), model.clone());
    }
    (
        Some(FrozenCandidate {
            data: CandidateData {
                schema_version: 1,
                member,
                check_version,
                template: row,
                declaration,
                desired_options: desired,
                permission_policy: PermissionPolicy::Wait,
                execution_capabilities,
            },
            compiled_scope: compiled_scope.expect("validated scope"),
        }),
        issues,
    )
}

impl FrozenCandidate {
    /// Frozen when a mode was chosen; otherwise what this host would claim for
    /// the historical behavior (not hashed, so old digests stay valid).
    fn capabilities(&self) -> ExecutionCapabilities {
        self.data.execution_capabilities.clone().unwrap_or_else(|| {
            capabilities(
                &self.data.member.engine,
                effective_mode(&self.data.member.engine, None),
                Platform::current(),
            )
        })
    }
    pub(super) fn project_id(&self) -> Option<i32> {
        self.data.member.project_id
    }
    pub(super) fn summary(&self) -> Result<ExecutionSummary, AppCommandError> {
        use sha2::{Digest, Sha256};
        let raw = serde_json::to_vec(&self.data)
            .map_err(|_| AppCommandError::io_error("Execution candidate could not be encoded"))?;
        let mut hash = Sha256::new();
        hash.update(b"onehall-execution-candidate-v1");
        hash.update(raw);
        let (read_rule_count, write_rule_count) = self.compiled_scope.rule_counts();
        let source_has_mode = self.data.template.as_ref().is_some_and(|row| {
            serde_json::from_str::<AgentDelegationDefaults>(&row.agent_options).is_ok_and(|value| {
                value.mode_id.is_some() || value.config_values.contains_key("mode")
            })
        });
        Ok(ExecutionSummary {
            snapshot_version: format!("{:x}", hash.finalize()),
            engine: self.data.member.engine.clone(),
            requested_mode: self
                .data
                .desired_options
                .mode_id
                .clone()
                .unwrap_or_default(),
            mode_source: if source_has_mode { "template" } else { "scope" },
            model_source: if self.data.declaration.model.is_some() {
                "member"
            } else if self
                .data
                .desired_options
                .config_values
                .contains_key("model")
            {
                "template"
            } else {
                "engine_default"
            },
            permission_policy: self.data.permission_policy.as_str(),
            option_count: self.data.desired_options.config_values.len(),
            template_revision: self.data.template.as_ref().map(|row| row.revision),
            instructions_included: self
                .data
                .template
                .as_ref()
                .is_some_and(|row| !row.instructions.is_empty()),
            read_rule_count,
            write_rule_count,
            shell_rule_count: self.data.declaration.scope.shell.len(),
            network: self.data.declaration.scope.network.clone(),
            runtime_verified: false,
            execution_mode: effective_mode(
                &self.data.member.engine,
                self.data.declaration.execution_mode,
            ),
            capabilities: self.capabilities(),
        })
    }
}

pub(super) async fn capture(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
    expected_check_version: String,
) -> Result<(MemberExecutionPreview, Option<FrozenCandidate>), AppCommandError> {
    let preflight = hall_check_member_core(
        db,
        member_id.clone(),
        expected_revision,
        expected_check_version.clone(),
    )
    .await?;
    let mut result = MemberExecutionPreview {
        member_id: member_id.clone(),
        revision: expected_revision,
        check_version: expected_check_version.clone(),
        valid: false,
        launch_allowed: false,
        summary: None,
        issues: Vec::new(),
        preflight,
    };
    if !result.preflight.data_valid() {
        result.issues.push(ExecutionIssue {
            code: "member_data_invalid",
            field: "member",
            index: None,
        });
        return Ok((result, None));
    }
    let (member, inputs) = load(db, &member_id, expected_revision).await?;
    let config = combine(&member, inputs);
    if !config.valid || config.check_version != expected_check_version {
        return Err(MemberError::RevisionConflict.into());
    }
    let row = template(db, &member).await?;
    let desired = options(&member, row.as_ref())?;
    let declaration =
        serde_json::from_value(config.effective.ok_or(MemberError::RevisionConflict)?)
            .map_err(|_| AppCommandError::invalid_input("Member declaration is invalid"))?;
    let (frozen, issues) = build(
        member.clone(),
        expected_check_version.clone(),
        row.clone(),
        declaration,
        desired,
    );
    let after = hall_check_member_core(
        db,
        member_id.clone(),
        expected_revision,
        expected_check_version.clone(),
    )
    .await?;
    let (after_member, after_inputs) = load(db, &member_id, expected_revision).await?;
    if !after.data_valid()
        || after_member != member
        || combine(&after_member, after_inputs).check_version != expected_check_version
        || template(db, &member).await? != row
    {
        return Err(MemberError::RevisionConflict.into());
    }
    result.preflight = after;
    result.issues = issues;
    if let Some(frozen) = &frozen {
        result.summary = Some(frozen.summary()?);
        result.valid = true;
    }
    Ok((result, frozen))
}

pub async fn hall_preview_member_execution_core(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
    expected_check_version: String,
) -> Result<MemberExecutionPreview, AppCommandError> {
    capture(db, member_id, expected_revision, expected_check_version)
        .await
        .map(|(preview, _)| preview)
}

impl FrozenCandidate {
    pub(super) fn encoded(&self) -> Result<String, AppCommandError> {
        serde_json::to_string(&self.data).map_err(|_| invalid_archive())
    }
    pub(super) fn identity(&self) -> (&str, i64, &str) {
        (
            &self.data.member.member_id,
            self.data.member.revision,
            &self.data.check_version,
        )
    }
    pub(super) fn restore(raw: &str) -> Result<Self, AppCommandError> {
        if raw.len() > 1_048_576 {
            return Err(invalid_archive());
        }
        let data: CandidateData = serde_json::from_str(raw).map_err(|_| invalid_archive())?;
        if data.schema_version != 1
            || data.permission_policy != PermissionPolicy::Wait
            || !crate::hall::engine::supported(&data.member.engine)
            || data.member.revision < 1
            || uuid::Uuid::parse_str(&data.member.member_id).is_err()
            || !digest_valid(&data.check_version)
        {
            return Err(invalid_archive());
        }
        // Validate the stored declaration through the same input grammar. No
        // live files, templates or accounts are consulted during restoration.
        let mut declaration =
            serde_json::to_value(&data.declaration).map_err(|_| invalid_archive())?;
        declaration["schema_version"] = serde_json::json!(1);
        if data.declaration.model.is_none() {
            declaration
                .as_object_mut()
                .expect("declaration object")
                .remove("model");
        }
        super::merge::parse(declaration.to_string().as_bytes(), &data.member, true)
            .map_err(|_| invalid_archive())?;
        let desired =
            options(&data.member, data.template.as_ref()).map_err(|_| invalid_archive())?;
        let original = serde_json::to_vec(&data).map_err(|_| invalid_archive())?;
        // Reject unsupported nested fields too. Deserializers borrowed from
        // upstream can otherwise discard them before the fingerprint check.
        if serde_json::from_slice::<serde_json::Value>(&original).map_err(|_| invalid_archive())?
            != serde_json::from_str::<serde_json::Value>(raw).map_err(|_| invalid_archive())?
        {
            return Err(invalid_archive());
        }
        // Archive verification preserves all historically valid declarations.
        // Current capture and launch apply the new native representability gate.
        let (frozen, _) = build_inner(
            data.member,
            data.check_version,
            data.template,
            data.declaration,
            desired,
            Some(data.execution_capabilities),
        );
        let frozen = frozen.ok_or_else(invalid_archive)?;
        if serde_json::to_vec(&frozen.data).map_err(|_| invalid_archive())? != original {
            return Err(invalid_archive());
        }
        Ok(frozen)
    }
}

pub(super) fn digest_valid(raw: &str) -> bool {
    raw.len() == 64 && raw.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub(super) fn invalid_archive() -> AppCommandError {
    AppCommandError::io_error("Stored execution snapshot could not be verified")
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;

#[derive(Clone)]
pub struct LaunchProfile {
    pub member_id: String,
    /// `hall_member.engine` frozen in the snapshot.
    pub engine: String,
    pub member_revision: i64,
    pub cwd: String,
    pub assistant_uid: Option<String>,
    pub mode: String,
    pub config: std::collections::BTreeMap<String, String>,
    pub brief: String,
    /// The member's declared reach, frozen in the snapshot; the launch turns
    /// it into engine-native limits (see `hall::members::launch`).
    pub access: MemberAccess,
    /// As chosen in the snapshot; `None` keeps the historical engine behavior.
    pub execution_mode: Option<ExecutionMode>,
    /// Frozen host claims, present only with `execution_mode`.
    pub capabilities: Option<ExecutionCapabilities>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberAccess {
    pub read: Vec<String>,
    pub write: Vec<String>,
    pub network: String,
    pub onedesk_key: String,
}
impl FrozenCandidate {
    pub(super) fn launch_profile(&self, snapshot_id: uuid::Uuid) -> LaunchProfile {
        LaunchProfile {
            member_id: self.data.member.member_id.clone(),
            engine: self.data.member.engine.clone(),
            member_revision: self.data.member.revision,
            cwd: self.data.member.cwd.clone(),
            assistant_uid: self.data.member.assistant_uid.clone(),
            mode: self
                .data
                .desired_options
                .mode_id
                .clone()
                .unwrap_or_default(),
            config: self.data.desired_options.config_values.clone(),
            brief: self
                .data
                .template
                .as_ref()
                .and_then(|t| {
                    crate::acp::agent_mentions::render_snapshot_assistant_brief(
                        &t.name,
                        &t.instructions,
                        snapshot_id,
                    )
                })
                .unwrap_or_default(),
            access: MemberAccess {
                read: self.data.declaration.scope.read.clone(),
                write: self.data.declaration.scope.write.clone(),
                network: self.data.declaration.scope.network.clone(),
                onedesk_key: self.data.declaration.onedesk_key.clone(),
            },
            execution_mode: self.data.declaration.execution_mode,
            capabilities: self.data.execution_capabilities.clone(),
        }
    }
}
