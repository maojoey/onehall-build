//! Virtual overlay only. This module has no filesystem or database write path.
use super::{check_revision, combine, load, merge, read, ConfigField, Diagnostic};
use crate::app_error::AppCommandError;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberConfigDraftPreview {
    member_id: String,
    revision: i64,
    file_version: String,
    valid: bool,
    effective: Option<Value>,
    fields: Vec<ConfigField>,
    diagnostics: Vec<Diagnostic>,
    changes: Vec<Change>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Change {
    field: &'static str,
    before: Value,
    after: Value,
    before_source: &'static str,
    after_source: &'static str,
}

pub async fn hall_preview_member_config_draft_core(
    db: &DatabaseConnection,
    member_id: String,
    expected_revision: i64,
    expected_file_version: String,
    draft: Value,
) -> Result<MemberConfigDraftPreview, AppCommandError> {
    if expected_file_version.len() != 64
        || !expected_file_version.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppCommandError::invalid_input(
            "A configuration file version is required",
        ));
    }
    let (member, inputs) = load(db, &member_id, expected_revision).await?;
    let baseline = combine(&member, inputs.clone());
    if !baseline.valid || baseline.file_version.as_deref() != Some(&expected_file_version) {
        return Err(AppCommandError::already_exists(
            "Configuration changed; read it again before previewing",
        ));
    }
    let mut result = MemberConfigDraftPreview {
        member_id: member_id.clone(),
        revision: expected_revision,
        file_version: expected_file_version,
        valid: false,
        effective: None,
        fields: Vec::new(),
        diagnostics: Vec::new(),
        changes: Vec::new(),
    };
    // Bound both representations. Never include raw parser diagnostics or input
    // in a response, including unknown keys that may contain sensitive values.
    if let Some(yaml) = merge::encode(&draft) {
        let mut proposed =
            inputs.ok_or_else(|| AppCommandError::io_error("Configuration reader failed"))?;
        proposed[2] = read::ReadFile {
            path: read::PATHS[2],
            data: Some(yaml),
            error: None,
        };
        let merged = combine(&member, Some(proposed));
        result.valid = merged.valid;
        result.diagnostics = merged
            .diagnostics
            .into_iter()
            .map(|mut diagnostic| {
                if diagnostic.source == read::PATHS[2] {
                    diagnostic.source = "draft";
                }
                diagnostic
            })
            .collect();
        if let (Some(before), Some(after)) = (&baseline.effective, &merged.effective) {
            for field in merge::FIELDS {
                let pointer = format!("/{}", field.replace('.', "/"));
                let before_value = &before.pointer(&pointer).expect("complete valid baseline");
                let after_value = &after.pointer(&pointer).expect("complete valid draft");
                let before_source = baseline
                    .fields
                    .iter()
                    .find(|row| row.field == field)
                    .expect("known field")
                    .source;
                let after_source = merged
                    .fields
                    .iter()
                    .find(|row| row.field == field)
                    .expect("known field")
                    .source;
                if before_value != after_value || before_source != after_source {
                    result.changes.push(Change {
                        field,
                        before: (*before_value).clone(),
                        after: (*after_value).clone(),
                        before_source,
                        after_source,
                    });
                }
            }
        }
        result.effective = merged.effective;
        result.fields = merged.fields;
    } else {
        result.diagnostics.push(Diagnostic {
            code: "too_large",
            source: "draft",
            field: "document",
        });
    }
    check_revision(db, &member_id, expected_revision).await?;
    Ok(result)
}
