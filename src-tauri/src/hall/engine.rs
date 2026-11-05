//! Engines a team member can run on (user decision D2, 10-26): Codex stays the
//! default, Claude Code is the second. Every engine-specific choice the hall
//! makes lives here, so the rest of the team code never names an engine.
use crate::models::agent::AgentType;

pub(crate) const DEFAULT: &str = "codex";

/// The stored `hall_member.engine` values a team run accepts.
pub(crate) fn supported(engine: &str) -> bool {
    agent_type(engine).is_some()
}

pub(crate) fn agent_type(engine: &str) -> Option<AgentType> {
    match engine {
        "codex" => Some(AgentType::Codex),
        "claude_code" => Some(AgentType::ClaudeCode),
        _ => None,
    }
}

/// The session mode matching a member's scope. Without write rules: Codex's
/// read-only sandbox; Claude's default mode, where every edit and command
/// waits for a person. With write rules: Codex may write its workspace,
/// Claude accepts file edits (commands still ask).
pub(crate) fn mode_for_scope(engine: &str, writable: bool) -> &'static str {
    match (engine, writable) {
        ("claude_code", false) => "default",
        ("claude_code", true) => "acceptEdits",
        (_, false) => "read-only",
        (_, true) => "workspace-write",
    }
}

/// Modes a member may request for this engine (never a bypass / auto mode).
pub(crate) fn safe_mode(engine: &str, mode: &str) -> bool {
    mode == mode_for_scope(engine, false) || mode == mode_for_scope(engine, true)
}

/// Whether a safe mode lets the agent write without asking.
pub(crate) fn mode_writes(engine: &str, mode: &str) -> bool {
    mode == mode_for_scope(engine, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_engine_maps_scope_to_its_own_safe_modes() {
        assert_eq!(mode_for_scope("codex", false), "read-only");
        assert_eq!(mode_for_scope("codex", true), "workspace-write");
        assert_eq!(mode_for_scope("claude_code", false), "default");
        assert_eq!(mode_for_scope("claude_code", true), "acceptEdits");
        for (engine, mode) in [
            ("codex", "full-access"),
            ("claude_code", "bypassPermissions"),
            ("claude_code", "auto"),
            ("claude_code", "read-only"),
            ("codex", "acceptEdits"),
        ] {
            assert!(!safe_mode(engine, mode), "{engine} {mode}");
        }
        assert!(mode_writes("claude_code", "acceptEdits"));
        assert!(!mode_writes("claude_code", "default"));
        assert!(supported("codex") && supported("claude_code"));
        assert!(!supported("gemini") && !supported(""));
    }
}
