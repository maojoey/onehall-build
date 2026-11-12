import { getShellTransport } from "@/lib/transport"
import type { AgentType } from "@/lib/types"

import { adapterSyncClient, SYNCED_AGENTS } from "./adapter-sync"

export interface TargetMember {
  name: string
  role: string
  engine: string
  enabled: boolean
}

export interface TargetAgentVersion {
  agent: AgentType
  adapter: string | null
  cli: string | null
}

export interface TargetInfo {
  members: TargetMember[]
  agents: TargetAgentVersion[]
}

/**
 * What a task sent to another machine can land on: its members and the agent
 * versions behind them. Read-only. The machine's own user still chooses who
 * runs an accepted task.
 */
export async function readTargetInfo(
  connectionId: number
): Promise<TargetInfo> {
  const shell = getShellTransport()
  const versions = adapterSyncClient(connectionId)
  const [members, agents] = await Promise.all([
    shell
      .call<TargetMember[]>("remote_http_call", {
        connectionId,
        command: "hall_list_members",
        args: {},
      })
      // An older machine without members is still a valid target.
      .catch(() => [] as TargetMember[]),
    Promise.all(
      SYNCED_AGENTS.map(async (agent): Promise<TargetAgentVersion> => {
        try {
          const status = await versions.status(agent)
          return {
            agent,
            adapter: status.adapter_version,
            cli: status.cli_version,
          }
        } catch {
          return { agent, adapter: null, cli: null }
        }
      })
    ),
  ])
  return {
    members: members.map(({ name, role, engine, enabled }) => ({
      name,
      role,
      engine,
      enabled,
    })),
    agents,
  }
}
