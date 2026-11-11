"use client"

// onehall: single-window workspace shell. Hosts ONE workspace realm at a time
// (keyed by backend, so switching remounts it) above a bottom tab bar that
// lists the local machine and every open remote connection. See
// `src/stores/hall-workspace-store.ts` for the model and the invariants.
//
// Falls back to the plain `RemoteConnectionGate` (upstream's one-window-per-
// backend behaviour) when: not the desktop runtime, the hall setting is off,
// or this window was opened with `?remoteConnectionId=` (a legacy remote
// workspace window).

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { useSearchParams } from "next/navigation"
import { useTranslations } from "next-intl"
import { Laptop, MonitorCloud, Plus, Settings, X } from "lucide-react"
import { toast } from "sonner"
import { toErrorMessage } from "@/lib/app-error"
import {
  beginBackendSwitch,
  isDesktop,
  releaseRemoteDesktopTransport,
} from "@/lib/transport"
import {
  resetBackendScopedStores,
  parkBackendScopedStores,
  restoreBackendScopedStores,
  forgetBackendScopedStores,
} from "@/stores/backend-scoped-store-reset"
import { hallGetSettings } from "@/lib/hall/api"
import {
  forgetParkedOwnerships,
  forgetPendingBackendTabs,
} from "@/lib/hall/workspace-lifetime"
import {
  getLocalMachineName,
  listRemoteWorkspaceConnections,
} from "@/lib/remote-workspace"
import type { RemoteWorkspaceConnection } from "@/lib/types"
import { RemoteConnectionGate } from "@/contexts/remote-connection-context"
import {
  LOCAL_BACKEND_KEY,
  localTab,
  readPersistedTabs,
  useHallWorkspaceStore,
  type HallBackendTab,
} from "@/stores/hall-workspace-store"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { RemoteWorkspaceManageDialog } from "@/components/layout/remote-workspace-manage-dialog"
import { cn } from "@/lib/utils"
import { HallBranchDirectory } from "./hall-branch-directory"
import { HallWebWorkspaceShell } from "./hall-web-workspace-shell"
import { branchClient } from "@/lib/hall/branches"

const HEALTH_POLL_MS = 8_000
export const HALL_BOTTOM_BAR_PX = 32

export function HallWorkspaceShell({ children }: { children: ReactNode }) {
  return isDesktop() ? (
    <DesktopHallWorkspaceShell>{children}</DesktopHallWorkspaceShell>
  ) : (
    <HallWebWorkspaceShell>{children}</HallWebWorkspaceShell>
  )
}

function DesktopHallWorkspaceShell({ children }: { children: ReactNode }) {
  const t = useTranslations("Hall")
  const searchParams = useSearchParams()
  const legacyRemoteWindow = searchParams.get("remoteConnectionId") !== null
  const [decided, setDecided] = useState<"pending" | "hall" | "upstream">(
    "pending"
  )
  const enabled = useHallWorkspaceStore((s) => s.enabled)
  const tabs = useHallWorkspaceStore((s) => s.tabs)
  const activeKey = useHallWorkspaceStore((s) => s.activeKey)

  // Decide the mode once per window: desktop + setting on + not a legacy
  // remote window. Restore the tab list from this machine's storage, dropping
  // remotes whose connection record is gone.
  useEffect(() => {
    let cancelled = false
    if (!isDesktop() || legacyRemoteWindow) {
      setDecided("upstream")
      return
    }
    void (async () => {
      try {
        const settings = await hallGetSettings()
        if (cancelled) return
        if (!settings.singleWindow) {
          setDecided("upstream")
          return
        }
        const connections = await listRemoteWorkspaceConnections().catch(
          () => [] as RemoteWorkspaceConnection[]
        )
        if (cancelled) return
        const persisted = readPersistedTabs()
        const byId = new Map(connections.map((c) => [c.id, c]))
        const remotes: HallBackendTab[] = (persisted?.remotes ?? [])
          .filter(
            (r) =>
              byId.has(r.id) &&
              (!r.branchId || byId.has(r.headquartersConnectionId ?? -1))
          )
          .map((r) => ({
            key: `remote:${r.id}`,
            remoteConnectionId: r.id,
            name: byId.get(r.id)?.name ?? r.name,
            windowId: crypto.randomUUID(),
            ...(r.branchId && byId.has(r.headquartersConnectionId ?? -1)
              ? {
                  branchId: r.branchId,
                  headquartersConnectionId: r.headquartersConnectionId,
                }
              : {}),
          }))
        const machine = await getLocalMachineName().catch(() => "")
        if (cancelled) return
        const store = useHallWorkspaceStore.getState()
        useHallWorkspaceStore.setState({ webMode: false })
        store.hydrate(
          [
            localTab(
              machine ? `${t("tabs.local")} · ${machine}` : t("tabs.local")
            ),
            ...remotes,
          ],
          persisted?.activeKey ?? LOCAL_BACKEND_KEY
        )
        store.setEnabled(true)
        setDecided("hall")
      } catch (err) {
        console.warn("[hall] shell init failed, using upstream windows:", err)
        if (!cancelled) setDecided("upstream")
      }
    })()
    return () => {
      cancelled = true
    }
  }, [legacyRemoteWindow, t])

  // `open_remote_workspace` / `focus_conversation` in single-window mode tell
  // the main window which backend to show.
  useEffect(() => {
    if (decided !== "hall") return
    let off: (() => void) | null = null
    let cancelled = false
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event")
      const unlisten = await listen<{
        remoteConnectionId: number | null
        name?: string
      }>("hall://open-backend", (event) => {
        const { remoteConnectionId, name } = event.payload ?? {
          remoteConnectionId: null,
        }
        const store = useHallWorkspaceStore.getState()
        if (remoteConnectionId === null) store.activate(LOCAL_BACKEND_KEY)
        else store.open(remoteConnectionId, name ?? `#${remoteConnectionId}`)
      })
      if (cancelled) unlisten()
      else off = unlisten
    })()
    return () => {
      cancelled = true
      off?.()
    }
  }, [decided])

  // An opened tab keeps its connection while it is in the background; only
  // closing the tab (x) releases it. Program exit is drained in the backend.
  const heldRemoteIds = useRef<Set<number>>(new Set())
  useEffect(() => {
    if (decided !== "hall") return
    const now = new Set<number>()
    for (const tab of tabs)
      if (tab.remoteConnectionId !== null) now.add(tab.remoteConnectionId)
    for (const id of heldRemoteIds.current) {
      if (now.has(id)) continue
      releaseRemoteDesktopTransport(id)
      forgetParkedOwnerships(`remote:${id}`)
      forgetBackendScopedStores(`remote:${id}`)
    }
    heldRemoteIds.current = now
  }, [decided, tabs])

  // Window title follows the active tab.
  const active = useMemo(
    () => tabs.find((tab) => tab.key === activeKey) ?? null,
    [tabs, activeKey]
  )
  useEffect(() => {
    if (decided !== "hall" || !active) return
    void import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) =>
        getCurrentWindow().setTitle(`一堂 · ${active.name}`)
      )
      .catch(() => {})
  }, [decided, active])

  // The realm currently mounted. A switch goes through one commit with NO
  // realm mounted (`mountedKey !== activeKey`): the old workspace unmounts
  // completely (cleanups, detaches), then — with nothing of it left to
  // re-render or refetch — the effect below invalidates its in-flight calls,
  // drops the remote transport, clears the backend-scoped stores, and only
  // then mounts the new realm. Doing the reset while the old realm was still
  // mounted let the incoming local realm fetch through the still-configured
  // remote transport and show the remote's folders (Mac acceptance G3).
  const [mountedKey, setMountedKey] = useState<string | null>(null)
  const stateKey = useRef<string | null>(null)
  const [activationError, setActivationError] = useState<{
    key: string
    message: string
  } | null>(null)
  const [activationAttempt, setActivationAttempt] = useState(0)
  useEffect(() => {
    if (decided !== "hall" || !enabled || !active) return
    if (mountedKey === active.key) return
    if (stateKey.current) {
      if (tabs.some((tab) => tab.key === stateKey.current))
        parkBackendScopedStores(stateKey.current)
      else {
        forgetBackendScopedStores(stateKey.current)
        forgetPendingBackendTabs(stateKey.current)
      }
    }
    beginBackendSwitch()
    resetBackendScopedStores()
    restoreBackendScopedStores(active.key)
    stateKey.current = active.key
    let cancelled = false
    void (async () => {
      try {
        if (active.branchId && active.headquartersConnectionId != null) {
          const connection = await branchClient(
            active.headquartersConnectionId
          ).connect(active.branchId)
          if (cancelled) return
          if (connection.connectionId !== active.remoteConnectionId) {
            const store = useHallWorkspaceStore.getState()
            store.openDerivedBranch(
              active.headquartersConnectionId,
              connection.connectionId,
              active.branchId,
              connection.name
            )
            store.close(active.key)
            return
          }
        }
        if (!cancelled) {
          setActivationError(null)
          setMountedKey(active.key)
        }
      } catch (error) {
        if (!cancelled)
          setActivationError({
            key: active.key,
            message: toErrorMessage(error),
          })
      }
    })()
    return () => {
      cancelled = true
    }
  }, [decided, enabled, active, mountedKey, activationAttempt, tabs])

  if (decided === "upstream") {
    return <RemoteConnectionGate>{children}</RemoteConnectionGate>
  }
  if (decided === "pending" || !enabled || !active) return null

  return (
    <div
      data-hall-shell
      style={{ ["--hall-bottom-bar" as string]: `${HALL_BOTTOM_BAR_PX}px` }}
    >
      {/* Keyed by backend: a switch unmounts the old realm (its providers,
          subscriptions and attach streams) and mounts a fresh one against
          the new transport. Agents keep running on their side. */}
      {mountedKey === active.key ? (
        <RemoteConnectionGate
          key={active.key}
          backend={{
            remoteConnectionId: active.remoteConnectionId,
            windowId: active.windowId,
          }}
        >
          {children}
        </RemoteConnectionGate>
      ) : null}
      {activationError?.key === active.key && mountedKey !== active.key ? (
        <div role="alert" className="p-6">
          <p>{activationError.message}</p>
          <button
            type="button"
            onClick={() => setActivationAttempt((n) => n + 1)}
          >
            {t("branches.refresh")}
          </button>
        </div>
      ) : null}
      <HallWorkspaceTabBar />
    </div>
  )
}

// ─── bottom tab bar ─────────────────────────────────────────────────────

type Health = {
  running: number
  waiting: number
  error: number
  link: "ok" | "down" | "unknown"
}

const EMPTY_HEALTH: Health = {
  running: 0,
  waiting: 0,
  error: 0,
  link: "unknown",
}

async function pollHealth(tab: HallBackendTab): Promise<Health> {
  type Payload = {
    runningCount: number
    waitingCount: number
    errorCount: number
  }
  try {
    if (!isDesktop() && tab.branchId) {
      const status = await branchClient(null).probe(tab.branchId)
      return {
        ...EMPTY_HEALTH,
        link: status.enabled && status.status === "online" ? "ok" : "down",
      }
    }
    let payload: Payload
    if (tab.remoteConnectionId === null) {
      const { getShellTransport } = await import("@/lib/transport")
      payload = await getShellTransport().call<Payload>(
        "pet_list_active_sessions"
      )
    } else {
      const { invoke } = await import("@tauri-apps/api/core")
      payload = await invoke<Payload>("remote_http_call", {
        connectionId: tab.remoteConnectionId,
        command: "pet_list_active_sessions",
        args: {},
        timeoutMs: 5_000,
      })
    }
    return {
      running: payload.runningCount ?? 0,
      waiting: payload.waitingCount ?? 0,
      error: payload.errorCount ?? 0,
      link: "ok",
    }
  } catch {
    return {
      ...EMPTY_HEALTH,
      link: isDesktop() && tab.remoteConnectionId === null ? "ok" : "down",
    }
  }
}

function useBackendHealth(tabs: HallBackendTab[]): Record<string, Health> {
  const [health, setHealth] = useState<Record<string, Health>>({})
  useEffect(() => {
    let cancelled = false
    const tick = async () => {
      // Read the live tab list from the store: the effect is keyed on the set
      // of tab keys below, so the closure never goes stale on a name change.
      const current = useHallWorkspaceStore.getState().tabs
      const results = await Promise.all(
        current.map(async (tab) => [tab.key, await pollHealth(tab)] as const)
      )
      if (cancelled) return
      setHealth(Object.fromEntries(results))
    }
    void tick()
    const id = setInterval(() => void tick(), HEALTH_POLL_MS)
    return () => {
      cancelled = true
      clearInterval(id)
    }
    // Re-run when the set of tabs changes (keys), not on every render.
  }, [tabs.map((t) => t.key).join("|")]) // eslint-disable-line react-hooks/exhaustive-deps
  return health
}

export function HallWorkspaceTabBar() {
  const t = useTranslations("Hall")
  const tabs = useHallWorkspaceStore((s) => s.tabs)
  const activeKey = useHallWorkspaceStore((s) => s.activeKey)
  const health = useBackendHealth(tabs)
  const [connections, setConnections] = useState<RemoteWorkspaceConnection[]>(
    []
  )
  const [manageOpen, setManageOpen] = useState(false)
  const web = !isDesktop()
  const active = tabs.find((tab) => tab.key === activeKey)
  const headquartersConnectionId =
    active?.headquartersConnectionId ?? active?.remoteConnectionId ?? null

  const refreshConnections = useCallback(async () => {
    try {
      setConnections(
        (await listRemoteWorkspaceConnections()).filter(
          (c) => !/\/branches\/[0-9a-f-]+\/?$/.test(c.base_url ?? "")
        )
      )
    } catch (err) {
      toast.error(toErrorMessage(err))
    }
  }, [])

  const store = useHallWorkspaceStore

  return (
    <>
      <nav
        aria-label={t("tabs.ariaLabel")}
        className="fixed inset-x-0 bottom-0 z-40 flex h-8 items-stretch gap-1 border-t border-border bg-muted px-1.5 text-xs"
      >
        {tabs.map((tab) => {
          const isActive = tab.key === activeKey
          const h = health[tab.key] ?? EMPTY_HEALTH
          const isRemote = tab.remoteConnectionId !== null || !!tab.branchId
          const isLocal = !isRemote && !web
          return (
            <div
              key={tab.key}
              className={cn(
                "my-1 flex max-w-60 items-center gap-1.5 rounded-md border px-2",
                isLocal && "border-primary/30 bg-primary/10 text-primary",
                isActive
                  ? isLocal
                    ? "border-primary/60 bg-primary/15 font-semibold"
                    : "border-border bg-background font-medium text-foreground"
                  : isLocal
                    ? "hover:bg-primary/15"
                    : "border-transparent text-muted-foreground hover:bg-background/60 hover:text-foreground"
              )}
            >
              <button
                type="button"
                aria-current={isActive ? "page" : undefined}
                onClick={() => store.getState().activate(tab.key)}
                className="flex min-w-0 items-center gap-1.5"
                title={
                  isRemote
                    ? `${t("tabs.remoteHint", { name: tab.name })}`
                    : web
                      ? t("branches.headquarters")
                      : t("tabs.localHint")
                }
              >
                {isRemote || web ? (
                  <MonitorCloud className="h-3.5 w-3.5 shrink-0" aria-hidden />
                ) : (
                  <Laptop className="h-3.5 w-3.5 shrink-0" aria-hidden />
                )}
                <span className="truncate">{tab.name}</span>
                {isRemote ? (
                  <span
                    className={cn(
                      "h-1.5 w-1.5 shrink-0 rounded-full",
                      h.link === "ok"
                        ? "bg-emerald-500"
                        : h.link === "down"
                          ? "bg-red-500"
                          : "bg-zinc-400"
                    )}
                    aria-label={
                      h.link === "down" ? t("tabs.linkDown") : t("tabs.linkOk")
                    }
                  />
                ) : null}
                {h.error > 0 ? (
                  <Badge
                    tone="error"
                    label={t("tabs.error", { count: h.error })}
                  />
                ) : null}
                {h.waiting > 0 ? (
                  <Badge
                    tone="waiting"
                    label={t("tabs.waiting", { count: h.waiting })}
                  />
                ) : null}
                {h.running > 0 ? (
                  <Badge
                    tone="running"
                    label={t("tabs.running", { count: h.running })}
                  />
                ) : null}
              </button>
              {isRemote ? (
                <button
                  type="button"
                  aria-label={t("tabs.close", { name: tab.name })}
                  title={t("tabs.closeHint")}
                  onClick={() => store.getState().close(tab.key)}
                  className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
                >
                  <X className="h-3 w-3" aria-hidden />
                </button>
              ) : null}
            </div>
          )
        })}
        {!web && (
          <DropdownMenu
            onOpenChange={(open) => open && void refreshConnections()}
          >
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                aria-label={t("tabs.add")}
                title={t("tabs.add")}
                className="my-1 flex items-center gap-1 rounded-md px-2 text-muted-foreground hover:bg-background/60 hover:text-foreground"
              >
                <Plus className="h-3.5 w-3.5" aria-hidden />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" side="top" className="w-72">
              {connections.length === 0 ? (
                <div className="px-3 py-2 text-sm text-muted-foreground">
                  {t("tabs.noConnections")}
                </div>
              ) : (
                connections.map((c) => (
                  <DropdownMenuItem
                    key={c.id}
                    onSelect={() => store.getState().open(c.id, c.name)}
                  >
                    <MonitorCloud />
                    <span className="min-w-0">
                      <span className="block truncate">{c.name}</span>
                      <span className="block truncate text-xs text-muted-foreground">
                        {c.base_url}
                      </span>
                    </span>
                  </DropdownMenuItem>
                ))
              )}
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => setManageOpen(true)}>
                <Settings />
                {t("tabs.manage")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        )}
        <HallBranchDirectory
          headquartersConnectionId={headquartersConnectionId}
          web={web}
        />
      </nav>
      {!web && (
        <RemoteWorkspaceManageDialog
          open={manageOpen}
          onOpenChange={setManageOpen}
          onChanged={() => void refreshConnections()}
        />
      )}
    </>
  )
}

function Badge({
  tone,
  label,
}: {
  tone: "error" | "waiting" | "running"
  label: string
}) {
  return (
    <span
      className={cn(
        "shrink-0 rounded-full px-1.5 text-3xs font-semibold leading-4 text-white",
        tone === "error"
          ? "bg-red-500"
          : tone === "waiting"
            ? "bg-amber-500"
            : "bg-blue-500"
      )}
    >
      {label}
    </span>
  )
}
