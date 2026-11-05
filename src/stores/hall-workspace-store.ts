// onehall: the single-window workspace model — which backends are open as
// bottom tabs in the main window and which one the UI is showing right now.
//
// Upstream runs one window per backend; each window is its own JS realm and
// the backend-scoped stores are module singletons inside it. OneHall keeps
// that invariant but inside ONE window: switching a tab remounts the whole
// workspace subtree (keyed by backend); the shell resets the backend-scoped
// stores and re-points the transport in between. Nothing here talks to a
// backend — sessions, tabs and folders live in each backend's own database
// and come back when that backend is shown again. Tasks keep running across switches:
// the only thing that disconnects an agent is the idle sweep, and it never
// touches a prompting or background-busy connection.

import { create } from "zustand"

export const LOCAL_BACKEND_KEY = "local"

export interface HallBackendTab {
  /** `local` or `remote:<connection id>`. */
  key: string
  /** Remote workspace connection id; null for the local backend. */
  remoteConnectionId: number | null
  name: string
  /** Stable per-session window id handed to the remote proxy as
   *  `remoteWindowId`; fresh each app start, like a remote window's. */
  windowId: string
  branchId?: string
  headquartersConnectionId?: number
}

export interface HallPendingFocus {
  backendKey: string
  folderId: number
  conversationId: number
  agent: string
}

interface HallWorkspaceState {
  /** True once the shell decided this window runs in single-window mode. */
  enabled: boolean
  webMode: boolean
  tabs: HallBackendTab[]
  activeKey: string
  /** A focus request for a backend that was not active when it arrived; the
   *  realm for that backend takes it on mount. */
  pendingFocus: HallPendingFocus | null
  setEnabled(enabled: boolean): void
  /** Replace the persisted tab list on boot (local is always first). */
  hydrate(tabs: HallBackendTab[], activeKey: string): void
  open(remoteConnectionId: number, name: string): void
  openDerivedBranch(
    headquartersConnectionId: number,
    remoteConnectionId: number,
    branchId: string,
    name: string
  ): void
  openWebBranch(branchId: string, name: string): void
  activate(key: string): void
  close(key: string): void
  setPendingFocus(focus: HallPendingFocus | null): void
  takePendingFocus(backendKey: string): HallPendingFocus | null
}

export function backendKeyFor(remoteConnectionId: number | null): string {
  return remoteConnectionId === null
    ? LOCAL_BACKEND_KEY
    : `remote:${remoteConnectionId}`
}

function newWindowId(): string {
  if (typeof globalThis.crypto?.randomUUID === "function") {
    return globalThis.crypto.randomUUID()
  }
  return `hall-${Date.now()}-${Math.random().toString(16).slice(2)}`
}

export function localTab(localName: string): HallBackendTab {
  return {
    key: LOCAL_BACKEND_KEY,
    remoteConnectionId: null,
    name: localName,
    windowId: newWindowId(),
  }
}

const STORAGE_KEY = "hall.workspaces"
const BRANCH_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
function storageKey(webMode: boolean) {
  return webMode ? `${STORAGE_KEY}:web:${window.location.origin}` : STORAGE_KEY
}

interface PersistedTabs {
  remotes: {
    id: number
    name: string
    branchId?: string
    headquartersConnectionId?: number
  }[]
  webBranches?: { id: string; name: string }[]
  activeKey: string
}

export function readPersistedTabs(webMode = false): PersistedTabs | null {
  try {
    const raw = localStorage.getItem(storageKey(webMode))
    if (!raw) return null
    const parsed = JSON.parse(raw) as Partial<PersistedTabs>
    const remotes = Array.isArray(parsed.remotes)
      ? parsed.remotes
          .filter(
            (r): r is PersistedTabs["remotes"][number] =>
              typeof r?.id === "number" && typeof r?.name === "string"
          )
          .map((r) => ({
            id: r.id,
            name: r.name,
            ...(typeof r.branchId === "string" &&
            BRANCH_ID.test(r.branchId) &&
            Number.isSafeInteger(r.headquartersConnectionId) &&
            (r.headquartersConnectionId ?? 0) > 0
              ? {
                  branchId: r.branchId,
                  headquartersConnectionId: r.headquartersConnectionId,
                }
              : {}),
          }))
      : []
    return {
      remotes,
      ...(Array.isArray(parsed.webBranches)
        ? {
            webBranches: parsed.webBranches.filter(
              (r) =>
                typeof r?.id === "string" &&
                BRANCH_ID.test(r.id) &&
                typeof r.name === "string"
            ),
          }
        : {}),
      activeKey:
        typeof parsed.activeKey === "string"
          ? parsed.activeKey
          : LOCAL_BACKEND_KEY,
    }
  } catch {
    return null
  }
}

function persist(
  tabs: HallBackendTab[],
  activeKey: string,
  webMode = false
): void {
  try {
    const payload: PersistedTabs = {
      remotes: tabs
        .filter((t) => t.remoteConnectionId !== null)
        .map((t) => ({
          id: t.remoteConnectionId as number,
          name: t.name,
          ...(t.branchId && t.headquartersConnectionId !== undefined
            ? {
                branchId: t.branchId,
                headquartersConnectionId: t.headquartersConnectionId,
              }
            : {}),
        })),
      ...(webMode
        ? {
            webBranches: tabs
              .filter((t) => t.branchId)
              .map((t) => ({ id: t.branchId!, name: t.name })),
          }
        : {}),
      activeKey,
    }
    localStorage.setItem(storageKey(webMode), JSON.stringify(payload))
  } catch {
    // Storage may be unavailable; the tab list is a convenience, not state
    // of record (each backend keeps its own).
  }
}

export const useHallWorkspaceStore = create<HallWorkspaceState>()(
  (set, get) => ({
    enabled: false,
    webMode: false,
    tabs: [],
    activeKey: LOCAL_BACKEND_KEY,
    pendingFocus: null,
    setEnabled: (enabled) => set({ enabled }),
    hydrate: (tabs, activeKey) => {
      const active = tabs.some((t) => t.key === activeKey)
        ? activeKey
        : LOCAL_BACKEND_KEY
      set({ tabs, activeKey: active })
    },
    open: (remoteConnectionId, name) => {
      const key = backendKeyFor(remoteConnectionId)
      const { tabs } = get()
      let next = tabs
      if (!tabs.some((t) => t.key === key)) {
        next = [
          ...tabs,
          { key, remoteConnectionId, name, windowId: newWindowId() },
        ]
      } else {
        next = tabs.map((t) => (t.key === key ? { ...t, name } : t))
      }
      set({ tabs: next, activeKey: key })
      persist(next, key, get().webMode)
    },
    openDerivedBranch: (
      headquartersConnectionId,
      remoteConnectionId,
      branchId,
      name
    ) => {
      if (!BRANCH_ID.test(branchId)) return
      const key = backendKeyFor(remoteConnectionId)
      const old = get().tabs.find((t) => t.key === key)
      const tab: HallBackendTab = {
        key,
        remoteConnectionId,
        name,
        branchId,
        headquartersConnectionId,
        windowId: old?.windowId ?? newWindowId(),
      }
      const tabs = old
        ? get().tabs.map((t) => (t.key === key ? tab : t))
        : [...get().tabs, tab]
      set({ tabs, activeKey: key })
      persist(tabs, key, false)
    },
    openWebBranch: (branchId, name) => {
      if (!BRANCH_ID.test(branchId)) return
      const key = `branch:${branchId}`
      const old = get().tabs.find((t) => t.key === key)
      const tab: HallBackendTab = {
        key,
        remoteConnectionId: null,
        name,
        branchId,
        windowId: old?.windowId ?? newWindowId(),
      }
      const tabs = old
        ? get().tabs.map((t) => (t.key === key ? tab : t))
        : [...get().tabs, tab]
      set({ tabs, activeKey: key })
      persist(tabs, key, true)
    },
    activate: (key) => {
      const { tabs, activeKey } = get()
      if (!tabs.some((t) => t.key === key) || key === activeKey) return
      // The shell does the switch (unmount, invalidate, reset, remount); see
      // `HallWorkspaceShell`. Resetting here, with the old realm still
      // mounted, let it refetch into the cleared stores.
      set({ activeKey: key })
      persist(tabs, key, get().webMode)
    },
    close: (key) => {
      if (key === LOCAL_BACKEND_KEY) return
      const { tabs, activeKey } = get()
      const next = tabs.filter((t) => t.key !== key)
      const nextActive = activeKey === key ? LOCAL_BACKEND_KEY : activeKey
      set({ tabs: next, activeKey: nextActive })
      persist(next, nextActive, get().webMode)
    },
    setPendingFocus: (focus) => set({ pendingFocus: focus }),
    takePendingFocus: (backendKey) => {
      const { pendingFocus } = get()
      if (!pendingFocus || pendingFocus.backendKey !== backendKey) return null
      set({ pendingFocus: null })
      return pendingFocus
    },
  })
)

/** The active tab, or null when single-window mode is off. */
export function activeHallTab(): HallBackendTab | null {
  const { enabled, tabs, activeKey } = useHallWorkspaceStore.getState()
  if (!enabled) return null
  return tabs.find((t) => t.key === activeKey) ?? null
}
