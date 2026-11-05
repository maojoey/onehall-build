import { detectEnvironment } from "@/lib/transport/detect"
import { getShellTransport } from "@/lib/transport"
import { createExchangeClient, type ExchangeClient } from "./exchange"
import {
  browserTransfer,
  desktopTransfer,
  tauriHost,
  type ByteTransfer,
} from "./exchange-transfer"

export interface BranchDispatch {
  dispatchId: string
  branchId: string
  memberRef: string
  title: string
  status:
    | "pending_delivery"
    | "awaiting_acceptance"
    | "rejected"
    | "accepted"
    | "running"
    | "awaiting_permission"
    | "succeeded"
    | "failed"
    | "interrupted"
    | "outcome_unknown"
  revision: number
  createdAt: string
  updatedAt: string
  resultText?: string | null
  taskStatus?: string | null
  syncError?: string | null
  /** Optional: older servers omit them, and the UI then shows "unknown". */
  exitConfirmedAt?: string | null
  reported?: boolean
}
export interface DispatchDraft {
  submissionId: string
  branchId: string
  memberRef: string
  title: string
  description: string
  /** Frozen by the host when the dispatch is created. */
  exchangeManifestId?: string
}
export interface DispatchClient {
  /** Stable across panel lifetimes. Optional for existing injected clients. */
  submissionContext?: string
  list: () => Promise<BranchDispatch[]>
  create: (draft: DispatchDraft) => Promise<BranchDispatch>
  sync: (dispatchId: string) => Promise<BranchDispatch>
  /** Absent when this backend or shell cannot run the file exchange. */
  exchange?: ExchangeClient
  /** Byte channel for this HQ; absent where none can carry the bytes. */
  bytes?: ByteTransfer
}

export function dispatchClient(
  headquartersConnectionId: number | null
): DispatchClient {
  const call = <T>(request: Record<string, unknown>) => {
    const shell = getShellTransport()
    return headquartersConnectionId === null
      ? shell.call<T>("hall_dispatch_control", { request })
      : shell.call<T>("remote_http_call", {
          connectionId: headquartersConnectionId,
          command: "hall_dispatch_control",
          args: { request },
        })
  }
  const exchangeCall = <T>(request: Record<string, unknown>) => {
    const shell = getShellTransport()
    return headquartersConnectionId === null
      ? shell.call<T>("hall_exchange", { request })
      : shell.call<T>("remote_http_call", {
          connectionId: headquartersConnectionId,
          command: "hall_exchange",
          args: { request },
        })
  }
  const local = headquartersConnectionId === null
  const desktop = detectEnvironment() === "tauri"
  return {
    exchange: createExchangeClient(exchangeCall),
    // Desktop: the host inspects and streams (a remote HQ through its saved
    // connection). Browser: only the same-origin HQ that served the page.
    bytes: desktop
      ? desktopTransfer(
          headquartersConnectionId,
          tauriHost.invoke,
          tauriHost.dialogs
        )
      : local
        ? browserTransfer()
        : undefined,
    submissionContext:
      headquartersConnectionId === null
        ? "shell"
        : `hq:${headquartersConnectionId}`,
    list: () => call<BranchDispatch[]>({ action: "list" }),
    create: (draft: DispatchDraft) =>
      call<BranchDispatch>({ action: "create", draft }),
    sync: (dispatchId: string) =>
      call<BranchDispatch>({ action: "sync", dispatch_id: dispatchId }),
  }
}
