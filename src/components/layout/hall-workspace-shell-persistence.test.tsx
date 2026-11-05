/**
 * onehall: switching bottom tabs must not tear down the event connection of an
 * opened remote; closing the tab (x) must. Real shell, gate and transports over
 * a fake Tauri IPC. See docs/onehall/plans/2026-11-01-multi-engine/10-*.md.
 */
import { act, render } from "@testing-library/react"
import { useEffect } from "react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

const log = vi.hoisted(() => ({
  calls: [] as string[],
  listens: 0,
  unlistens: 0,
}))

const fakeInternals = {
  transformCallback: () => 0,
  invoke: (command: string, args: Record<string, unknown> = {}) => {
    if (command.startsWith("remote_ws_")) {
      log.calls.push(`${command}:${args.connectionId}`)
      return Promise.resolve(undefined)
    }
    const cmd =
      command === "remote_http_call" ? (args.command as string) : command
    switch (cmd) {
      case "hall_get_settings":
        return Promise.resolve({ singleWindow: true })
      case "list_remote_workspace_connections":
        return Promise.resolve([
          { id: 7, name: "A" },
          { id: 8, name: "B" },
        ])
      case "get_remote_workspace_connection":
        return Promise.resolve({
          id: args.id ?? args.connectionId,
          name: "x",
          base_url: "https://h.example",
          token: "t",
        })
      case "pet_list_active_sessions":
        return Promise.resolve({
          runningCount: 0,
          waitingCount: 0,
          errorCount: 0,
        })
      default:
        return Promise.resolve(undefined)
    }
  },
}
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(),
}))
const translate = vi.hoisted(
  () => (k: string) => (k === "tabs.local" ? "local" : k)
)
vi.mock("next-intl", () => ({ useTranslations: () => translate }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: async () => {
    log.listens++
    return () => {
      log.unlistens++
    }
  },
}))
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setTitle: async () => {} }),
}))
vi.mock("@/components/connection/remote-connection-status", () => ({
  RemoteConnectionStatus: () => null,
}))
vi.mock("@/components/layout/remote-workspace-manage-dialog", () => ({
  RemoteWorkspaceManageDialog: () => null,
}))

import { RemoteDesktopTransport } from "@/lib/transport/remote-desktop-transport"
import { TauriTransport } from "@/lib/transport/tauri-transport"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  getTransport,
  isRemoteDesktopMode,
} from "@/lib/transport"
import {
  LOCAL_BACKEND_KEY,
  useHallWorkspaceStore,
} from "@/stores/hall-workspace-store"
import { HallWorkspaceShell } from "./hall-workspace-shell"
import {
  captureHallWorkspace,
  parkOwnedConnection,
  takeParkedOwnership,
} from "@/lib/hall/workspace-lifetime"

function Realm() {
  useEffect(() => {
    log.calls.push(`mount:${isRemoteDesktopMode() ? "remote" : "local"}`)
    // a realm that needs events subscribes through the active transport
    const t = getTransport()
    const p = t.subscribe("acp://event", () => {})
    // push __ready__ is not simulated; subscribe awaits waitForReady (5s fallback) – not awaited here
    void p.catch(() => {})
    return () => {
      log.calls.push("unmount")
    }
  }, [])
  return null
}
const settle = () =>
  act(async () => {
    for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0))
  })
const go = async (key: string) => {
  await act(async () => useHallWorkspaceStore.getState().activate(key))
  await settle()
}

beforeEach(() => {
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ =
    fakeInternals
  __setTransportFactoriesForTests({
    tauri: () => new TauriTransport(),
    remote: (c) => new RemoteDesktopTransport(c),
  })
  __resetTransportForTests()
  localStorage.clear()
  log.calls.length = 0
  log.listens = 0
  log.unlistens = 0
  useHallWorkspaceStore.setState({
    enabled: false,
    tabs: [],
    activeKey: LOCAL_BACKEND_KEY,
    pendingFocus: null,
  })
})
afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
})

it("keeps every opened tab connected across repeated switches and releases on close", async () => {
  render(
    <HallWorkspaceShell>
      <Realm />
    </HallWorkspaceShell>
  )
  await settle()
  await act(async () => useHallWorkspaceStore.getState().open(7, "A"))
  await settle()
  await act(async () => useHallWorkspaceStore.getState().open(8, "B"))
  await settle()
  log.calls.push("---- begin repeated switching ----")
  const listensBefore = log.listens
  for (let i = 0; i < 3; i++) {
    await go("local")
    await go("remote:7")
    await go("remote:8")
  }
  const c = log.calls
  const count = (p: string) => c.filter((x) => x.startsWith(p)).length
  // Each opened backend subscribed once at first show and was never torn down
  // by switching (baseline before the fix: 8 subscribes / 7 unsubscribes).
  const after = c.slice(c.indexOf("---- begin repeated switching ----"))
  expect(after.filter((x) => x.startsWith("remote_ws_unsubscribe"))).toEqual([])
  expect(after.filter((x) => x.startsWith("remote_ws_subscribe"))).toEqual([])
  expect(count("remote_ws_subscribe:7")).toBe(1)
  expect(count("remote_ws_subscribe:8")).toBe(1)
  // No listener is added or dropped by switching.
  expect(log.listens).toBe(listensBefore)
  expect(log.unlistens).toBe(0)

  // Closing a tab releases exactly that connection.
  await act(async () => useHallWorkspaceStore.getState().close("remote:7"))
  await settle()
  expect(c.filter((x) => x === "remote_ws_unsubscribe:7")).toHaveLength(1)
  expect(c.filter((x) => x === "remote_ws_unsubscribe:8")).toHaveLength(0)
  expect(log.unlistens).toBe(1)
}, 60000)

it("forgets ownership parked by a closing active workspace", async () => {
  function Owner() {
    useEffect(() => {
      const workspace = captureHallWorkspace()
      return () => {
        if (workspace?.isLeaving())
          parkOwnedConnection(workspace.key, "conversation", "engine")
      }
    }, [])
    return null
  }
  render(
    <HallWorkspaceShell>
      <Owner />
    </HallWorkspaceShell>
  )
  await settle()
  await act(async () => useHallWorkspaceStore.getState().open(7, "A"))
  await settle()
  await act(async () => useHallWorkspaceStore.getState().close("remote:7"))
  await settle()
  expect(takeParkedOwnership("remote:7", "conversation")).toBeNull()
})
