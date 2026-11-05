import { getShellTransport } from "@/lib/transport"

export interface HallBranchSummary {
  id: string
  name: string
  enabled: boolean
  revision: number
  status: "unknown" | "online" | "unreachable" | "auth_failed"
  checkedAt: string | null
}

/** This client stays attached to headquarters even while viewing a branch. */
export function branchClient(headquartersConnectionId: number | null) {
  const shell = getShellTransport()
  const call = <T>(command: string, args: Record<string, unknown> = {}) =>
    headquartersConnectionId === null
      ? shell.call<T>(command, args)
      : shell.call<T>("remote_http_call", {
          connectionId: headquartersConnectionId,
          command,
          args,
        })
  return {
    list: () => call<HallBranchSummary[]>("hall_list_branches"),
    probe: (branchId: string) =>
      call<HallBranchSummary>("hall_probe_branch", { branchId }),
    register: (input: { name: string; baseUrl: string; token: string }) =>
      call<HallBranchSummary>("hall_register_branch", { input }),
    setEnabled: (branch: HallBranchSummary, enabled: boolean) =>
      call<HallBranchSummary>("hall_set_branch_enabled", {
        branchId: branch.id,
        enabled,
        expectedRevision: branch.revision,
      }),
    connect: (branchId: string) => {
      if (headquartersConnectionId === null)
        throw new Error("Desktop headquarters connection required")
      return shell.call<{ connectionId: number; name: string }>(
        "hall_connect_branch",
        { headquartersConnectionId, branchId }
      )
    },
  }
}
