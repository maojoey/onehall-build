"use client"

import { useCallback, useEffect, useState, useSyncExternalStore } from "react"
import { useTranslations } from "next-intl"
import { Loader2, WifiOff } from "lucide-react"
import { Button } from "@/components/ui/button"
import type { WebTransport } from "@/lib/transport/web-transport"
import {
  subscribeWebConnection,
  getWebConnectionSnapshot,
  getWebConnectionServerSnapshot,
  notifyWebUnauthorized,
} from "@/lib/transport/web-connection-store"

type BranchHealth = Pick<
  WebTransport,
  | "getConnectionSnapshot"
  | "subscribeConnection"
  | "reconnectNow"
  | "eventStream"
> & { getBranchFailure?(): string | null }

/** Bound to this realm's branch transport, while the root guard owns HQ auth. */
export function WebBranchStatus({
  name,
  transport,
  onHeadquarters,
}: {
  name: string
  transport: BranchHealth
  onHeadquarters: () => void
}) {
  const t = useTranslations("WebConnection")
  const branch = useTranslations("Hall.branches")
  const subscribe = useCallback(
    (fn: () => void) => transport.subscribeConnection(fn),
    [transport]
  )
  const snapshot = useCallback(
    () => transport.getConnectionSnapshot(),
    [transport]
  )
  const failureSnapshot = useCallback(
    () => transport.getBranchFailure?.() ?? null,
    [transport]
  )
  const state = useSyncExternalStore(subscribe, snapshot, () => "connected")
  const failure = useSyncExternalStore(subscribe, failureSnapshot, () => null)
  const shell = useSyncExternalStore(
    subscribeWebConnection,
    getWebConnectionSnapshot,
    getWebConnectionServerSnapshot
  )
  const [grace, setGrace] = useState(false)

  useEffect(() => {
    transport.eventStream()
  }, [transport])
  useEffect(() => {
    if (state !== "reconnecting") return
    const timer = setTimeout(() => setGrace(true), 4_000)
    return () => {
      clearTimeout(timer)
      setGrace(false)
    }
  }, [state])
  useEffect(() => {
    if (state === "unauthorized" || state === "proxyUnauthorized") {
      notifyWebUnauthorized(state === "unauthorized" ? "codeg" : "proxy")
    }
  }, [state])
  useEffect(() => {
    const nudge = () => {
      if (transport.getConnectionSnapshot() === "reconnecting")
        transport.reconnectNow()
    }
    const visible = () => {
      if (document.visibilityState === "visible") nudge()
    }
    window.addEventListener("online", nudge)
    document.addEventListener("visibilitychange", visible)
    return () => {
      window.removeEventListener("online", nudge)
      document.removeEventListener("visibilitychange", visible)
    }
  }, [transport])

  if (
    shell !== "connected" ||
    state === "connected" ||
    state === "unauthorized" ||
    state === "proxyUnauthorized" ||
    (state === "reconnecting" && !grace)
  )
    return null
  const reason =
    failure === "branch_auth_failed"
      ? branch("status.auth_failed")
      : failure === "branch_disabled"
        ? branch("disabled")
        : failure
          ? branch("unavailable")
          : t("reconnectingDescription")
  return (
    <div
      role="status"
      className="fixed inset-x-4 bottom-[calc(var(--hall-bottom-bar,0px)+1rem)] z-40 mx-auto flex w-fit max-w-[calc(100%_-_2rem)] flex-wrap items-center gap-3 rounded-xl border bg-background px-4 py-3 text-sm shadow-lg"
    >
      {state === "reconnecting" ? (
        <Loader2 className="size-4 shrink-0 animate-spin" />
      ) : (
        <WifiOff className="size-4 shrink-0 text-destructive" />
      )}
      <div>
        <p>
          {name} · {t("disconnectedTitle")}
        </p>
        <p className="text-muted-foreground">{reason}</p>
      </div>
      <Button size="sm" variant="outline" onClick={onHeadquarters}>
        {branch("headquarters")}
      </Button>
      <Button size="sm" onClick={() => transport.reconnectNow()}>
        {t("reconnectNow")}
      </Button>
    </div>
  )
}
