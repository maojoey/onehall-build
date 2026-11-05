use super::super::HallMember;
use serde_json::{json, Value as Json};
use serde_yaml::{Mapping, Value as Yaml};
use std::collections::{BTreeMap, HashSet};

pub(super) const FIELDS: [&str; 9] = [
    "model",
    "scope.read",
    "scope.write",
    "scope.shell",
    "scope.network",
    "skills.shared",
    "skills.own",
    "onedesk_key",
    "execution_mode",
];
pub(super) type Patch = BTreeMap<&'static str, Json>;
pub(super) type Fault = (&'static str, &'static str);
const IDENTITY: [&str; 7] = [
    "member_id",
    "name",
    "role",
    "cwd",
    "engine",
    "assistant_uid",
    "project_id",
];

pub(super) fn defaults() -> Patch {
    BTreeMap::from([
        ("model", Json::Null),
        ("scope.read", json!(["**"])),
        ("scope.write", json!([])),
        ("scope.shell", json!([])),
        ("scope.network", json!("none")),
        ("skills.shared", json!([])),
        ("skills.own", json!([])),
        ("onedesk_key", json!("none")),
        ("execution_mode", Json::Null),
    ])
}

fn map<'a>(node: &'a Yaml, field: &'static str) -> Result<&'a Mapping, Fault> {
    node.as_mapping().ok_or(("invalid_value", field))
}

fn no_tags(node: &Yaml) -> bool {
    match node {
        Yaml::Tagged(_) => false,
        Yaml::Mapping(m) => m.iter().all(|(k, v)| no_tags(k) && no_tags(v)),
        Yaml::Sequence(items) => items.iter().all(no_tags),
        _ => true,
    }
}

fn text(node: &Yaml, max: usize, empty: bool, field: &'static str) -> Result<String, Fault> {
    let raw = node.as_str().ok_or(("invalid_value", field))?;
    let text = raw.trim();
    if (!empty && text.is_empty()) || raw.chars().any(char::is_control) || raw.chars().count() > max
    {
        return Err(("invalid_value", field));
    }
    Ok(text.to_owned())
}

fn array(node: &Yaml, field: &'static str, paths: bool) -> Result<Json, Fault> {
    let items = node.as_sequence().ok_or(("invalid_value", field))?;
    if items.len() > 64 {
        return Err(("invalid_value", field));
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for item in items {
        let value = text(item, 1024, false, field)?;
        if !seen.insert(value.clone()) {
            return Err(("invalid_value", field));
        }
        if paths
            && (value.starts_with(['/', '~'])
                || value.contains('\\')
                || value.as_bytes().get(1) == Some(&b':')
                || value.split('/').any(|p| p == ".."))
        {
            return Err(("invalid_value", field));
        }
        result.push(value);
    }
    Ok(json!(result))
}

pub(super) fn parse(raw: &[u8], member: &HallMember, overlay: bool) -> Result<Patch, Fault> {
    // Never expose serde_yaml::Error: its display includes snippets/values.
    let raw = std::str::from_utf8(raw).map_err(|_| ("invalid_yaml", "document"))?;
    let root: Yaml = serde_yaml::from_str(raw).map_err(|_| ("invalid_yaml", "document"))?;
    if !no_tags(&root) {
        return Err(("invalid_value", "document"));
    }
    let root = map(&root, "document")?;
    if root
        .get(Yaml::String("schema_version".to_owned()))
        .and_then(Yaml::as_i64)
        != Some(1)
    {
        return Err(("unsupported_version", "schema_version"));
    }
    let mut patch = Patch::new();
    for (key, node) in root {
        let key = key.as_str().ok_or(("unknown_field", "document"))?;
        match key {
            "schema_version" => {}
            "model" => {
                let value = text(node, 160, true, "model")?;
                patch.insert(
                    "model",
                    if value.is_empty() {
                        Json::Null
                    } else {
                        json!(value)
                    },
                );
            }
            // `read`: the launch hands the member its own read-only OneDesk key
            // (`.onedesk/cli-key` in its directory) through the environment.
            "onedesk_key" => {
                let value = text(node, 16, false, "onedesk_key")?;
                if value != "none" && value != "read" {
                    return Err(("invalid_value", "onedesk_key"));
                }
                patch.insert("onedesk_key", json!(value));
            }
            // Chosen by the local operator and frozen in the snapshot; the
            // host derives capabilities from it, never from this text.
            "execution_mode" => {
                let value = text(node, 16, false, "execution_mode")?;
                if value != "restricted" && value != "local_assistant" {
                    return Err(("invalid_value", "execution_mode"));
                }
                patch.insert("execution_mode", json!(value));
            }
            "scope" | "skills" => {
                let parent = if key == "scope" { "scope" } else { "skills" };
                for (child, value) in map(node, parent)? {
                    let field = match (key, child.as_str()) {
                        ("scope", Some("read")) => "scope.read",
                        ("scope", Some("write")) => "scope.write",
                        ("scope", Some("shell")) => "scope.shell",
                        ("scope", Some("network")) => "scope.network",
                        ("skills", Some("shared")) => "skills.shared",
                        ("skills", Some("own")) => "skills.own",
                        _ => return Err(("unknown_field", parent)),
                    };
                    let parsed = if field == "scope.network" {
                        let network = text(value, 80, false, field)?;
                        // `local`: loopback only (e.g. OneDesk on this host).
                        if !matches!(network.as_str(), "none" | "local" | "default") {
                            return Err(("invalid_value", field));
                        }
                        json!(network)
                    } else {
                        array(
                            value,
                            field,
                            field == "scope.read" || field == "scope.write",
                        )?
                    };
                    patch.insert(field, parsed);
                }
            }
            _ => {
                let field = IDENTITY
                    .iter()
                    .copied()
                    .find(|field| *field == key)
                    .ok_or(("unknown_field", "document"))?;
                if overlay {
                    return Err(("unknown_field", field));
                }
                let expected = match field {
                    "member_id" => json!(member.member_id),
                    "name" => json!(member.name),
                    "role" => json!(member.role),
                    "cwd" => json!(member.cwd),
                    "engine" => json!(member.engine),
                    "assistant_uid" => json!(member.assistant_uid),
                    "project_id" => json!(member.project_id),
                    _ => unreachable!(),
                };
                let actual = if field == "name" || field == "role" {
                    json!(text(
                        node,
                        if field == "name" { 80 } else { 200 },
                        false,
                        field
                    )?)
                } else {
                    serde_json::to_value(node).map_err(|_| ("invalid_value", field))?
                };
                if actual != expected {
                    return Err(("identity_conflict", field));
                }
            }
        }
    }
    Ok(patch)
}

pub(super) fn effective(flat: &Patch) -> Json {
    json!({"model":flat["model"],
        "scope":{"read":flat["scope.read"],"write":flat["scope.write"],"shell":flat["scope.shell"],"network":flat["scope.network"]},
        "skills":{"shared":flat["skills.shared"],"own":flat["skills.own"]},
        "onedesk_key":flat["onedesk_key"],
        "execution_mode":flat["execution_mode"]})
}

/// Preserve omissions: inheriting a field must not freeze its current default.
pub(super) fn document(patch: &Patch) -> Json {
    let mut result = json!({"schema_version": 1});
    for (field, value) in patch {
        if let Some((group, leaf)) = field.split_once('.') {
            if result.get(group).is_none() {
                result[group] = json!({});
            }
            result[group][leaf] = value.clone();
        } else {
            result[*field] = if value.is_null() {
                json!("")
            } else {
                value.clone()
            };
        }
    }
    result
}

pub(super) fn encode(draft: &Json) -> Option<Vec<u8>> {
    if serde_json::to_vec(draft).ok()?.len() as u64 > super::read::LIMIT {
        return None;
    }
    let yaml = serde_yaml::to_string(draft).ok()?;
    (yaml.len() as u64 <= super::read::LIMIT).then(|| yaml.into_bytes())
}
