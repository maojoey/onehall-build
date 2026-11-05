"use client"

import { Fragment, useEffect, useRef, useState, type ReactNode } from "react"
import { useTranslations } from "next-intl"
import {
  beginBackendSwitch,
  captureSubmissionBackend,
  configureWebBranchTransport,
  getTransport,
  releaseAllWorkspaceTransports,
  releaseWebBranchTransport,
} from "@/lib/transport"
import type { WebTransport } from "@/lib/transport/web-transport"
import { WebBranchStatus } from "@/components/connection/web-branch-status"
import {
  forgetParkedOwnerships,
  forgetPendingBackendTabs,
} from "@/lib/hall/workspace-lifetime"
import {
  resetBackendScopedStores,
  parkBackendScopedStores,
  restoreBackendScopedStores,
  forgetBackendScopedStores,
} from "@/stores/backend-scoped-store-reset"
import {
  localTab,
  readPersistedTabs,
  useHallWorkspaceStore,
} from "@/stores/hall-workspace-store"
import { HallWorkspaceTabBar, HALL_BOTTOM_BAR_PX } from "./hall-workspace-shell"

/** Browser local means the serving headquarters, never the browser machine. */
export function HallWebWorkspaceShell({ children }: { children: ReactNode }) {
  const t = useTranslations("Hall.branches")
  const [headquartersName] = useState(() => t("headquarters"))
  const [ready, setReady] = useState(false)
  const [mountedKey, setMountedKey] = useState<string | null>(null)
  const stateKey = useRef<string | null>(null)
  const tabs = useHallWorkspaceStore((s) => s.tabs)
  const activeKey = useHallWorkspaceStore((s) => s.activeKey)
  const active = tabs.find((tab) => tab.key === activeKey)

  useEffect(() => {
    let cancelled = false
    queueMicrotask(() => {
      if (cancelled) return
      const saved = readPersistedTabs(true)
      const store = useHallWorkspaceStore.getState()
      useHallWorkspaceStore.setState({ webMode: true })
      store.hydrate(
        [
          localTab(headquartersName),
          ...(saved?.webBranches ?? []).map((branch) => ({
            key: `branch:${branch.id}`,
            remoteConnectionId: null,
            name: branch.name,
            branchId: branch.id,
            windowId: crypto.randomUUID(),
          })),
        ],
        saved?.activeKey ?? "local"
      )
      store.setEnabled(true)
      setReady(true)
    })
    return () => {
      cancelled = true
    }
  }, [headquartersName])

  useEffect(
    () => () => {
      // Parent cleanup precedes descendant cleanup. Keep their original target
      // until all child releases have run; a replacement shell may already have
      // switched by the time this microtask executes.
      const departing = captureSubmissionBackend()
      queueMicrotask(() => {
        if (departing.isCurrent()) {
          beginBackendSwitch()
          releaseAllWorkspaceTransports()
          forgetParkedOwnerships()
          forgetBackendScopedStores()
        }
      })
    },
    []
  )

  // A background branch tab keeps its connection; closing it releases it.
  const heldBranchIds = useRef<Set<string>>(new Set())
  useEffect(() => {
    if (!ready) return
    const now = new Set<string>()
    for (const tab of tabs) if (tab.branchId) now.add(tab.branchId)
    for (const id of heldBranchIds.current) {
      if (now.has(id)) continue
      releaseWebBranchTransport(id)
      forgetParkedOwnerships(`branch:${id}`)
      forgetBackendScopedStores(`branch:${id}`)
    }
    heldBranchIds.current = now
  }, [ready, tabs])

  useEffect(() => {
    if (!ready || !active || mountedKey === active.key) return
    // The outgoing children were removed in the preceding render. Their
    // cleanups still saw their original transport.
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
    if (active.branchId) configureWebBranchTransport(active.branchId)
    let cancelled = false
    queueMicrotask(() => {
      if (!cancelled) setMountedKey(active.key)
    })
    return () => {
      cancelled = true
    }
  }, [ready, active, mountedKey, tabs])

  if (!ready || !active) return null
  return (
    <div
      data-hall-shell
      style={{ ["--hall-bottom-bar" as string]: `${HALL_BOTTOM_BAR_PX}px` }}
    >
      {mountedKey === active.key ? (
        <Fragment key={active.key}>
          {children}
          {active.branchId && (
            <WebBranchStatus
              name={active.name}
              transport={getTransport() as WebTransport}
              onHeadquarters={() =>
                useHallWorkspaceStore.getState().activate("local")
              }
            />
          )}
        </Fragment>
      ) : null}
      <HallWorkspaceTabBar />
    </div>
  )
}
