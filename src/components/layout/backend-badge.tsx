"use client"

// onehall: "where am I?" — every workspace window shows which backend its
// actions go to. Upstream keeps one window per backend (the local desktop is
// `main`, each saved remote connection its own `remote-workspace-{id}`), and
// the two look identical; the window title that tells them apart is hidden
// under macOS's overlay title bar. This badge sits in the chrome of BOTH kinds
// of window and names the backend in words, with the remote link's health.

import { useCallback, useSyncExternalStore } from "react"
import { useTranslations } from "next-intl"
import { Laptop, MonitorCloud } from "lucide-react"
import { useRemoteConnection } from "@/contexts/remote-connection-context"
import { getTransport, isRemoteDesktopMode } from "@/lib/transport"
import { cn } from "@/lib/utils"

type LinkState = "connected" | "reconnecting" | "disconnected"

type LinkTransport = {
  getConnectionSnapshot(): LinkState
  subscribeConnection(callback: () => void): () => void
}

function linkTransport(): LinkTransport | null {
  if (!isRemoteDesktopMode()) return null
  const candidate = getTransport() as unknown as Partial<LinkTransport>
  if (
    typeof candidate.getConnectionSnapshot !== "function" ||
    typeof candidate.subscribeConnection !== "function"
  ) {
    return null
  }
  return candidate as LinkTransport
}

function useRemoteLinkState(): LinkState | null {
  const subscribe = useCallback(
    (cb: () => void) => linkTransport()?.subscribeConnection(cb) ?? (() => {}),
    []
  )
  const read = useCallback(
    () => linkTransport()?.getConnectionSnapshot() ?? null,
    []
  )
  return useSyncExternalStore(subscribe, read, () => null)
}

/**
 * The backend this window talks to, as a short label for prose: the local
 * machine's label, or the remote connection's name. Used to prefix messages
 * that would otherwise read the same in every window ("… is not installed").
 */
export function useBackendLabel(): string {
  const t = useTranslations("RemoteWorkspace")
  const remote = useRemoteConnection()?.connection ?? null
  return remote ? remote.name : t("badgeLocal")
}

export function BackendBadge({ compact = false }: { compact?: boolean }) {
  const t = useTranslations("RemoteWorkspace")
  const remote = useRemoteConnection()?.connection ?? null
  const link = useRemoteLinkState()

  const isRemote = remote !== null
  const name = isRemote ? remote.name : t("badgeLocal")
  const kind = isRemote ? t("badgeRemote") : t("badgeLocalDevice")
  const stateKey =
    link === "disconnected"
      ? "badgeDisconnected"
      : link === "reconnecting"
        ? "badgeReconnecting"
        : "badgeConnected"
  const stateLabel = isRemote ? t(stateKey) : null
  const dot =
    link === "disconnected"
      ? "bg-red-500"
      : link === "reconnecting"
        ? "bg-amber-500"
        : "bg-emerald-500"

  return (
    <span
      role="status"
      aria-label={
        isRemote ? `${kind} ${name} · ${stateLabel}` : `${name} · ${kind}`
      }
      title={isRemote ? `${name}\n${remote.base_url}` : undefined}
      className={cn(
        "inline-flex max-w-56 select-none items-center gap-1.5 rounded-md border border-border/70 bg-background/70 px-2 text-xs text-foreground",
        compact ? "h-5" : "h-6 my-auto"
      )}
    >
      {isRemote ? (
        <MonitorCloud className="h-3.5 w-3.5 shrink-0" aria-hidden />
      ) : (
        <Laptop className="h-3.5 w-3.5 shrink-0" aria-hidden />
      )}
      <span className="truncate font-medium">{name}</span>
      <span className="truncate text-muted-foreground">· {kind}</span>
      {isRemote ? (
        <span className="flex shrink-0 items-center gap-1 text-muted-foreground">
          <span
            className={cn(
              "h-1.5 w-1.5 rounded-full",
              dot,
              link === "reconnecting" && "animate-pulse"
            )}
            aria-hidden
          />
          {!compact ? stateLabel : null}
        </span>
      ) : null}
    </span>
  )
}
