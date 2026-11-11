import { act, render, screen } from "@testing-library/react"
import { useEffect } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

/**
 * onehall T10a: activating a branch tab used to await headquarters'
 * `hall_connect_branch` (an HTTP round trip over the slow link) before the
 * workspace mounted, on every switch. A branch whose derived connection is
 * already known now mounts at once and refreshes credentials in the
 * background; only a first open has to wait.
 */

type Pending = { resolve: (v: unknown) => void; reject: (e: unknown) => void }

const BRANCH = "11111111-1111-4111-8111-111111111111"

const ipc = vi.hoisted(() => ({
  connects: [] as Pending[],
  log: [] as string[],
}))

const fakeInternals = {
  transformCallback: () => 0,
  invoke: (command: string, args: Record<string, unknown> = {}) => {
    const cmd =
      command === "remote_http_call" ? (args.command as string) : command
    switch (cmd) {
      case "hall_connect_branch":
        return new Promise((resolve, reject) =>
          ipc.connects.push({ resolve, reject })
        )
      case "hall_get_settings":
        return Promise.resolve({ singleWindow: true })
      case "list_remote_workspace_connections":
        return Promise.resolve([{ id: 7, name: "堂务" }])
      case "get_remote_workspace_connection":
        return Promise.resolve({
          id: 7,
          name: "堂务",
          base_url: "https://hall.example",
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
  () => (key: string) => (key === "tabs.local" ? "本机" : key)
)
vi.mock("next-intl", () => ({ useTranslations: () => translate }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: async () => () => {},
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
} from "@/lib/transport"
import { __clearRegisteredBackendScopedStoreResets } from "@/stores/backend-scoped-store-reset"
import {
  LOCAL_BACKEND_KEY,
  useHallWorkspaceStore,
} from "@/stores/hall-workspace-store"
import { HallWorkspaceShell } from "./hall-workspace-shell"

function Realm() {
  useEffect(() => {
    ipc.log.push("mount")
    return () => {
      ipc.log.push("unmount")
    }
  }, [])
  return <div data-testid="realm" />
}

async function settle() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0))
  })
}

async function store<T>(
  fn: (s: ReturnType<typeof useHallWorkspaceStore.getState>) => T
) {
  let out!: T
  await act(async () => {
    out = fn(useHallWorkspaceStore.getState())
  })
  await settle()
  return out
}

async function answerConnect(connectionId: number) {
  await act(async () => {
    ipc.connects.shift()?.resolve({ connectionId, name: "ARM" })
  })
  await settle()
}

/** Mount on local, open the branch once (first open waits for connect). */
async function openBranchOnce() {
  render(
    <HallWorkspaceShell>
      <Realm />
    </HallWorkspaceShell>
  )
  await settle()
  await store((s) => s.openDerivedBranch(9, 7, BRANCH, "ARM"))
  await answerConnect(7)
  await store((s) => s.activate(LOCAL_BACKEND_KEY))
  ipc.connects.length = 0
  ipc.log.length = 0
}

beforeEach(() => {
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ =
    fakeInternals
  __setTransportFactoriesForTests({
    tauri: () => new TauriTransport(),
    remote: (config) => new RemoteDesktopTransport(config),
  })
  __resetTransportForTests()
  __clearRegisteredBackendScopedStoreResets()
  ipc.connects.length = 0
  ipc.log.length = 0
  localStorage.clear()
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

describe("branch tab activation", () => {
  it("mounts a known branch without waiting for headquarters", async () => {
    await openBranchOnce()
    await store((s) => s.activate("remote:7"))
    expect(ipc.connects).toHaveLength(1)
    expect(screen.queryByTestId("realm")).not.toBeNull()
    expect(ipc.log).toContain("mount")
  })

  it("keeps the mounted workspace when the refresh returns the same connection", async () => {
    await openBranchOnce()
    await store((s) => s.activate("remote:7"))
    ipc.log.length = 0
    await answerConnect(7)
    expect(ipc.log).toEqual([])
    expect(useHallWorkspaceStore.getState().activeKey).toBe("remote:7")
  })

  it("re-derives the tab when the refresh returns a different connection", async () => {
    await openBranchOnce()
    await store((s) => s.activate("remote:7"))
    await answerConnect(8)
    const { tabs, activeKey } = useHallWorkspaceStore.getState()
    expect(activeKey).toBe("remote:8")
    expect(tabs.some((t) => t.key === "remote:7")).toBe(false)
  })

  it("keeps the workspace up when the background refresh fails", async () => {
    await openBranchOnce()
    await store((s) => s.activate("remote:7"))
    ipc.log.length = 0
    await act(async () => {
      ipc.connects.shift()?.reject(new Error("headquarters unreachable"))
    })
    await settle()
    expect(screen.queryByTestId("realm")).not.toBeNull()
    expect(ipc.log).toEqual([])
  })
})
