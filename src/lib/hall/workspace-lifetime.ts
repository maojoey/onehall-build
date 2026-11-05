import { getActiveBackendKey } from "@/lib/transport"
import type { Transport } from "@/lib/transport/types"
import { CONNECTION_KEEPALIVE_INTERVAL_MS } from "@/lib/constants"
import { useHallWorkspaceStore } from "@/stores/hall-workspace-store"

/** Capture before a switch. During React's cleanup the transport still belongs
 * to the outgoing workspace, while the shell already selected the next tab. */
export function captureHallWorkspace() {
  if (!useHallWorkspaceStore.getState().enabled) return null
  const key = getActiveBackendKey()
  return {
    key,
    isLeaving: () => useHallWorkspaceStore.getState().activeKey !== key,
  }
}

const submissions = new Map<string, number>()

export function retainHallSubmission(
  connectionId: string,
  backendKey = getActiveBackendKey()
): () => void {
  const key = JSON.stringify([backendKey, connectionId])
  submissions.set(key, (submissions.get(key) ?? 0) + 1)
  return () => {
    const remaining = (submissions.get(key) ?? 1) - 1
    if (remaining > 0) submissions.set(key, remaining)
    else submissions.delete(key)
  }
}

export function hasHallSubmission(
  connectionId: string | null,
  backendKey = getActiveBackendKey()
): boolean {
  return (
    connectionId !== null &&
    submissions.has(JSON.stringify([backendKey, connectionId]))
  )
}

// Leaving a tab keeps the engines this client started running, but the realm's
// connection store unmounts with it. Remember, per backend and conversation
// surface, which connection was ours, so the return re-attaches as its owner
// instead of as a viewer that may never release it.
type ParkedConnection = {
  connectionId: string
  agentType?: string
  workingDir?: string | null
}
const parkedOwners = new Map<string, Map<string, ParkedConnection>>()
type ParkedKeepalive = {
  connectionId: string
  call: Transport["call"]
  inFlight: boolean
}
const parkedKeepalives = new Map<string, Map<string, ParkedKeepalive>>()
let keepaliveTimer: ReturnType<typeof setInterval> | null = null

function retireEmptyKeepalives(): void {
  for (const [key, entries] of parkedKeepalives) {
    if (entries.size === 0) parkedKeepalives.delete(key)
  }
  if (parkedKeepalives.size === 0 && keepaliveTimer !== null) {
    clearInterval(keepaliveTimer)
    keepaliveTimer = null
  }
}

/** Only open surfaces survive their realm unmount. The pinned call belongs to
 * their original transport; its replies never enter another workspace's store.
 * One timer covers all parked engines, with one in-flight request per surface. */
export function retainParkedWorkspaceConnection(
  backendKey: string,
  contextKey: string,
  connectionId: string,
  call: Transport["call"]
): void {
  const workspace = useHallWorkspaceStore.getState()
  if (!workspace.enabled || !workspace.tabs.some((t) => t.key === backendKey))
    return
  let entries = parkedKeepalives.get(backendKey)
  if (!entries) {
    entries = new Map()
    parkedKeepalives.set(backendKey, entries)
  }
  entries.set(contextKey, { connectionId, call, inFlight: false })
  if (keepaliveTimer !== null) return
  keepaliveTimer = setInterval(() => {
    const workspace = useHallWorkspaceStore.getState()
    if (!workspace.enabled) {
      releaseParkedWorkspaceKeepalives()
      return
    }
    for (const [backend, held] of parkedKeepalives) {
      if (!workspace.tabs.some((tab) => tab.key === backend)) {
        releaseParkedWorkspaceKeepalives(backend)
        continue
      }
      if (workspace.activeKey === backend) continue
      for (const [context, entry] of held) {
        if (entry.inFlight) continue
        entry.inFlight = true
        void Promise.resolve()
          .then(() => {
            if (parkedKeepalives.get(backend)?.get(context) !== entry) return
            const current = useHallWorkspaceStore.getState()
            if (
              !current.enabled ||
              current.activeKey === backend ||
              !current.tabs.some((tab) => tab.key === backend)
            )
              return
            return entry.call<boolean>("acp_touch_connection", {
              connectionId: entry.connectionId,
            })
          })
          .then((live) => {
            if (
              live !== false ||
              parkedKeepalives.get(backend)?.get(context) !== entry
            )
              return
            held.delete(context)
            const owned = parkedOwners.get(backend)
            if (owned?.get(context)?.connectionId === entry.connectionId)
              owned.delete(context)
          })
          .catch(() => {
            // Offline is inconclusive. Never spawn/replay work on a timer.
          })
          .finally(() => {
            entry.inFlight = false
            retireEmptyKeepalives()
          })
      }
    }
  }, CONNECTION_KEEPALIVE_INTERVAL_MS)
}

/** A mounted provider resumes the usual open-surface heartbeat. */
export function releaseParkedWorkspaceKeepalives(backendKey?: string): void {
  if (backendKey === undefined) parkedKeepalives.clear()
  else parkedKeepalives.delete(backendKey)
  retireEmptyKeepalives()
}
const workbenchRoutes = new Map<string, string>()
type TranscriptPosition = { itemKey: string | null; atEnd: boolean }
const transcriptPositions = new Map<string, Map<string, TranscriptPosition>>()
const pendingBackendTabs = new Map<string, unknown>()
const workspaceTargets = new Map<string, string>()

/** Returns true when an open workspace now points at a different server. */
export function rememberWorkspaceTarget(key: string, baseUrl: string): boolean {
  const target = baseUrl.replace(/\/+$/, "")
  const previous = workspaceTargets.get(key)
  workspaceTargets.set(key, target)
  return previous !== undefined && previous !== target
}

export function savePendingBackendTabs(key: string, value: unknown): void {
  pendingBackendTabs.set(key, value)
}

export function readPendingBackendTabs(key: string): unknown {
  return pendingBackendTabs.get(key)
}

export function forgetPendingBackendTabs(key?: string): void {
  if (key === undefined) pendingBackendTabs.clear()
  else pendingBackendTabs.delete(key)
}

export function readTranscriptPosition(
  backend: string,
  scope: string
): TranscriptPosition | undefined {
  return transcriptPositions.get(backend)?.get(scope)
}

export function saveTranscriptPosition(
  backend: string,
  scope: string,
  position: TranscriptPosition
): void {
  let positions = transcriptPositions.get(backend)
  if (!positions) {
    positions = new Map()
    transcriptPositions.set(backend, positions)
  }
  positions.set(scope, position)
}

export function savedWorkbenchRoute(backendKey: string): string | undefined {
  return workbenchRoutes.get(backendKey)
}

export function saveWorkbenchRoute(backendKey: string, route: string): void {
  workbenchRoutes.set(backendKey, route)
}

export function parkOwnedConnection(
  backendKey: string,
  contextKey: string,
  connectionId: string,
  identity?: { agentType: string; workingDir: string | null }
): void {
  let owned = parkedOwners.get(backendKey)
  if (!owned) {
    owned = new Map()
    parkedOwners.set(backendKey, owned)
  }
  owned.set(contextKey, { connectionId, ...identity })
}

/** Drafts have no persisted conversation id to discover. Match the original
 * request before reusing their parked engine; leave it parked while probing. */
export function peekParkedDraftOwnership(
  backendKey: string,
  contextKey: string,
  agentType: string,
  workingDir: string | null
): string | null {
  const parked = parkedOwners.get(backendKey)?.get(contextKey)
  return parked?.agentType === agentType && parked.workingDir === workingDir
    ? parked.connectionId
    : null
}

/** The connection `contextKey` owned when its realm was last left; consumed. */
export function takeParkedOwnership(
  backendKey: string,
  contextKey: string
): string | null {
  const owned = parkedOwners.get(backendKey)
  const connectionId = owned?.get(contextKey)?.connectionId ?? null
  owned?.delete(contextKey)
  if (owned?.size === 0) parkedOwners.delete(backendKey)
  return connectionId
}

/** Tab closed: this client no longer claims that backend's engines. Omit the
 * key to forget every backend. */
export function forgetParkedOwnerships(backendKey?: string): void {
  releaseParkedWorkspaceKeepalives(backendKey)
  if (backendKey === undefined) {
    parkedOwners.clear()
    workbenchRoutes.clear()
    transcriptPositions.clear()
    forgetPendingBackendTabs()
    workspaceTargets.clear()
  } else {
    parkedOwners.delete(backendKey)
    workbenchRoutes.delete(backendKey)
    transcriptPositions.delete(backendKey)
    forgetPendingBackendTabs(backendKey)
    workspaceTargets.delete(backendKey)
  }
}
