import { useEffect } from "react"
import { act, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useHallWorkspaceStore } from "@/stores/hall-workspace-store"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  getActiveBackendKey,
} from "@/lib/transport"
import { WebTransport } from "@/lib/transport/web-transport"
vi.mock("next-intl", () => {
  const t = (key: string) => key
  return { useTranslations: () => t }
})
vi.mock("@/components/layout/hall-workspace-shell", () => ({
  HALL_BOTTOM_BAR_PX: 32,
  HallWorkspaceTabBar: () => <div>tabs</div>,
}))
vi.mock("@/stores/backend-scoped-store-reset", () => ({
  resetBackendScopedStores: vi.fn(),
  parkBackendScopedStores: vi.fn(),
  restoreBackendScopedStores: vi.fn(),
  forgetBackendScopedStores: vi.fn(),
}))
import { HallWebWorkspaceShell } from "./hall-web-workspace-shell"
const branch = "11111111-1111-4111-8111-111111111111"
beforeEach(() => {
  localStorage.clear()
  __resetTransportForTests()
  __setTransportFactoriesForTests({ web: (url) => new WebTransport(url) })
  useHallWorkspaceStore.setState({
    enabled: false,
    tabs: [],
    activeKey: "local",
    webMode: false,
  })
})
afterEach(() => __resetTransportForTests())
it("unmounts the old realm before retargeting, and returns to headquarters", async () => {
  const cleanups: string[] = []
  function Workspace() {
    const key = getActiveBackendKey()
    useEffect(
      () => () => {
        cleanups.push(getActiveBackendKey())
      },
      []
    )
    return <p>{key}</p>
  }
  render(
    <HallWebWorkspaceShell>
      <Workspace />
    </HallWebWorkspaceShell>
  )
  await screen.findByText("local")
  act(() => useHallWorkspaceStore.getState().openWebBranch(branch, "ARM"))
  await screen.findByText(`branch:${branch}`)
  expect(cleanups).toEqual(["local"])
  act(() => useHallWorkspaceStore.getState().activate("local"))
  await screen.findByText("local")
  expect(cleanups).toEqual(["local", `branch:${branch}`])
})
it("restores browser branch tabs without treating them as local desktop connections", async () => {
  localStorage.setItem(
    `hall.workspaces:web:${location.origin}`,
    JSON.stringify({
      remotes: [],
      webBranches: [{ id: branch, name: "ARM" }],
      activeKey: `branch:${branch}`,
    })
  )
  render(
    <HallWebWorkspaceShell>
      <div>workspace</div>
    </HallWebWorkspaceShell>
  )
  await waitFor(() => expect(getActiveBackendKey()).toBe(`branch:${branch}`))
  expect(useHallWorkspaceStore.getState().tabs[1].remoteConnectionId).toBeNull()
})

it("keeps the branch transport during descendant cleanup on full shell teardown", async () => {
  const cleanups: string[] = []
  function Workspace() {
    useEffect(
      () => () => {
        cleanups.push(getActiveBackendKey())
      },
      []
    )
    return <p>ready-workspace</p>
  }
  const view = render(
    <HallWebWorkspaceShell>
      <Workspace />
    </HallWebWorkspaceShell>
  )
  await screen.findByText("ready-workspace")
  act(() => useHallWorkspaceStore.getState().openWebBranch(branch, "ARM"))
  await waitFor(() => expect(getActiveBackendKey()).toBe(`branch:${branch}`))
  await screen.findByText("ready-workspace")
  view.unmount()
  expect(cleanups.slice(-1)[0]).toBe(`branch:${branch}`)
  await waitFor(() => expect(getActiveBackendKey()).toBe("local"))
})
