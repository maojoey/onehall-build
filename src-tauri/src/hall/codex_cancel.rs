//! Keep cancelled Codex tools scoped to the prompt that created them.
//!
//! Codex's interrupt ends a turn, but a shell which has yielded can outlive it.
//! AIR announces that shell separately, sometimes AFTER the prompt response.
//! Remember the tool ownership across turn boundaries, so a late announcement
//! can use the existing single-task stop request without stopping older work.

use std::collections::{BTreeMap, BTreeSet};

use crate::acp::types::AsyncTaskDelta;

#[derive(Default)]
pub(crate) struct CodexCancelledTools {
    next_turn: u64,
    active_turn: Option<u64>,
    owners: BTreeMap<String, Option<u64>>,
    cancelled_turns: BTreeSet<u64>,
    tasks: BTreeMap<String, ShellTask>,
    requested: BTreeSet<String>,
}

struct ShellTask {
    tool_id: Option<String>,
    can_stop: bool,
    shell: bool,
    terminal: bool,
}

impl CodexCancelledTools {
    pub(crate) fn protect_existing_tool(&mut self, id: &str) {
        self.owners.entry(id.to_string()).or_insert(None);
    }

    pub(crate) fn begin_turn(&mut self) {
        self.next_turn += 1;
        self.active_turn = Some(self.next_turn);
    }

    pub(crate) fn finish_turn(&mut self) {
        self.active_turn = None;
    }

    /// Only a tool's first announcement assigns ownership. Status updates
    /// from an older background shell must never adopt the current prompt.
    pub(crate) fn observe_tool(&mut self, update: &serde_json::Value) {
        if update.get("sessionUpdate").and_then(|v| v.as_str()) != Some("tool_call") {
            return;
        }
        if let Some(id) = update.get("toolCallId").and_then(|v| v.as_str()) {
            self.owners
                .entry(id.to_string())
                .or_insert(self.active_turn);
        }
    }

    pub(crate) fn observe_task(&mut self, delta: &AsyncTaskDelta) {
        if delta.spawned {
            // A task without an observed launching call has no proven owner.
            // In particular, a replay during a new turn is not new work.
            if let Some(id) = &delta.tool_call_id {
                self.owners.entry(id.clone()).or_insert(None);
            }
            self.tasks
                .entry(delta.task_id.clone())
                .or_insert(ShellTask {
                    tool_id: None,
                    can_stop: false,
                    shell: false,
                    terminal: false,
                });
        }
        let Some(task) = self.tasks.get_mut(&delta.task_id) else {
            return;
        };
        if let Some(id) = &delta.tool_call_id {
            task.tool_id = Some(id.clone());
        }
        if let Some(can_stop) = delta.can_stop {
            task.can_stop = can_stop;
        }
        if let Some(kind) = &delta.task_type {
            task.shell = kind == "shell";
        }
        if let Some(state) = &delta.state {
            task.terminal = matches!(state.as_str(), "completed" | "failed" | "stopped");
        }
    }

    pub(crate) fn cancel_turn(&mut self) {
        if let Some(turn) = self.active_turn {
            self.cancelled_turns.insert(turn);
        }
    }

    /// Claims each proven, cancellable shell once. The caller sends the
    /// adapter's task id, never an OS pid, command text or guessed alias.
    pub(crate) fn take_stops(&mut self) -> Vec<String> {
        let mut result = Vec::new();
        for (id, task) in &self.tasks {
            let owner = task.tool_id.as_ref().and_then(|tool| self.owners.get(tool));
            if task.can_stop
                && task.shell
                && !task.terminal
                && owner
                    .and_then(|turn| *turn)
                    .is_some_and(|turn| self.cancelled_turns.contains(&turn))
                && self.requested.insert(id.clone())
            {
                result.push(id.clone());
            }
        }
        result
    }
}
