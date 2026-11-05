//! Tool presentation is not authorization. Every request is checked again by
//! the broker against its token-bound connection and persistent role binding.
use serde_json::{json, Value};
pub fn member_tool(name: &str) -> bool {
    matches!(
        name,
        "hall_own_task"
            | "hall_read_mailbox"
            | "hall_ack_message"
            | "hall_report"
            | "hall_list_inputs"
            | "hall_read_input"
            | "hall_submit_output"
    )
}
pub fn pm_tool(name: &str) -> bool {
    matches!(
        name,
        "hall_list_members"
            | "hall_assign"
            | "hall_task_status"
            | "hall_send_message"
            | "hall_list_branches"
            | "hall_dispatch_branch"
            | "hall_branch_dispatch_status"
    )
}
/// A team run's permission card for one of these tools on the injected
/// codeg-mcp needs no person: the broker re-checks every call against the
/// caller's bound role, and the tools do not execute files or commands. Branch dispatch sends only
/// a pending request; acceptance and execution remain host-operator actions. `input` is
/// the session's tool-call input (`{server, tool, arguments}`).
pub fn auto_allowed(input: &Value) -> bool {
    input.get("server").and_then(Value::as_str) == Some("codeg-mcp")
        && input
            .get("tool")
            .and_then(Value::as_str)
            .is_some_and(|t| member_tool(t) || pm_tool(t))
}
/// The same check for a card that names the tool itself, as Claude Code does
/// (`_meta.claudeCode.toolName`, e.g. `mcp__codeg-mcp__hall_report`).
pub fn auto_allowed_tool_name(name: &str) -> bool {
    name.strip_prefix("mcp__codeg-mcp__")
        .is_some_and(|t| member_tool(t) || pm_tool(t))
}
pub fn schemas() -> Vec<Value> {
    serde_json::from_str(include_str!("tool_schema.json")).expect("embedded hall schemas")
}
pub fn input(name: &str, args: Value) -> Result<Value, &'static str> {
    if !member_tool(name) && !pm_tool(name) {
        return Err("Unknown hall tool");
    }
    let Value::Object(mut fields) = args else {
        return Err("Hall tool arguments must be an object");
    };
    if fields.contains_key("action") {
        return Err("Hall tool action is host selected");
    }
    fields.insert("action".into(), Value::String(name.into()));
    let input = Value::Object(fields);
    serde_json::from_value::<super::ToolRequest>(input.clone())
        .map_err(|_| "Invalid hall tool arguments")?;
    Ok(input)
}
pub fn render(outcome: &Value) -> Value {
    json!({"content":[{"type":"text","text":outcome.to_string()}],"isError":outcome.get("ok")==Some(&Value::Bool(false))})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schemas_match_group_names_and_reject_identity_injection() {
        let rows = schemas();
        assert_eq!(rows.len(), 14);
        for row in rows {
            let name = row["name"].as_str().unwrap();
            assert!(member_tool(name) ^ pm_tool(name));
            assert_eq!(row["inputSchema"]["additionalProperties"], false);
        }
        assert!(input("hall_own_task", json!({})).is_ok());
        assert!(input(
            "hall_dispatch_branch",
            json!({"draft": {
                "submissionId":"same-key", "branchId":"branch", "memberRef":"worker",
                "title":"Work", "description":"body", "command":"forbidden"
            }})
        )
        .is_err());
        assert!(input(
            "hall_branch_dispatch_status",
            json!({"dispatch_id":"id", "caller":"pm"})
        )
        .is_err());
        for args in [
            json!({"action":"hall_list_members"}),
            json!({"member_id":"pm"}),
            json!({"depth":0}),
            json!({"caller":"pm"}),
        ] {
            assert!(input("hall_own_task", args).is_err());
        }
    }
    #[test]
    fn model_errors_are_explicit_and_do_not_pretend_execution() {
        assert_eq!(
            render(&json!({"ok":false,"message":"denied"}))["isError"],
            true
        );
        assert_eq!(
            render(&json!({"ok":true,"value":{"launchAllowed":false}}))["isError"],
            false
        );
    }
}
