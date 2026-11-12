import { getShellTransport } from "@/lib/transport"
import type {
  AdapterStatus,
  AdapterUpgradeOutcome,
  AgentType,
} from "@/lib/types"

/** Claude Code and Codex are the agents with a managed adapter. */
export const SYNCED_AGENTS: readonly AgentType[] = ["claude_code", "codex"]

/** An install downloads a CLI and can take several minutes. */
const UPGRADE_TIMEOUT_MS = 600_000

/**
 * Adapter versions on another machine. A connection id reaches a machine
 * through its web service (the same route the branch workspace uses); `null`
 * means the machine this page is served from.
 */
export function adapterSyncClient(connectionId: number | null) {
  const shell = getShellTransport()
  const call = <T>(
    command: string,
    args: Record<string, unknown>,
    timeoutMs?: number
  ) =>
    connectionId === null
      ? shell.call<T>(command, args)
      : shell.call<T>("remote_http_call", {
          connectionId,
          command,
          args,
          ...(timeoutMs ? { timeoutMs } : {}),
        })
  return {
    status: (agentType: AgentType) =>
      call<AdapterStatus>("acp_adapter_status", { agentType }),
    upgrade: (agentType: AgentType, version: string) =>
      call<AdapterUpgradeOutcome>(
        "acp_adapter_upgrade",
        { agentType, version },
        UPGRADE_TIMEOUT_MS
      ),
  }
}

/** Whether a machine runs a different adapter than the reference one. */
export function differsFrom(
  reference: string | null,
  actual: string | null
): boolean {
  return !!reference && !!actual && reference !== actual
}
