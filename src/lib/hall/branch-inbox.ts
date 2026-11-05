import { detectEnvironment } from "@/lib/transport/detect"
import { getActiveBackendKey, getShellTransport } from "@/lib/transport"
import { createHallMembersClient } from "./members"
import type { ExchangeManifest } from "./exchange"
export interface BranchInboxItem {
  issuerId: string
  dispatchId: string
  memberRef: string
  title: string
  description: string
  status: string
  revision: number
  launchAllowed: false
  taskId?: string | null
  /** v2 items: what accepting brings in. Older hosts omit it. */
  exchangeManifest?: ExchangeManifest
}
export interface LocalTaskSelection {
  issuerId: string
  dispatchId: string
  expectedRevision: number
  memberId: string
  snapshotId: string
  projectId: number | null
}
/** Remote workspaces cannot silently fall back to this computer's inbox. */
export function localInboxClient() {
  if (detectEnvironment() !== "tauri" || getActiveBackendKey() !== "local")
    return null
  const shell = getShellTransport()
  const members = createHallMembersClient(shell)
  const call = <T>(request: Record<string, unknown>) =>
    shell.call<T>("hall_dispatch_local", { request })
  return {
    list: () => call<BranchInboxItem[]>({ action: "list_inbox" }),
    members: members.list,
    snapshots: members.listSnapshots,
    accept: (selection: LocalTaskSelection) =>
      call<{ taskId: string }>({ action: "accept", selection }),
    reject: (item: BranchInboxItem) =>
      call<unknown>({
        action: "reject",
        issuer_id: item.issuerId,
        dispatch_id: item.dispatchId,
        expected_revision: item.revision,
      }),
  }
}
export type LocalInboxClient = NonNullable<ReturnType<typeof localInboxClient>>
