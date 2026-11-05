import { beforeEach, describe, expect, it, vi } from "vitest"

const resetMock = vi.hoisted(() => vi.fn())
vi.mock("@/stores/backend-scoped-store-reset", () => ({
  resetBackendScopedStores: resetMock,
}))

import {
  LOCAL_BACKEND_KEY,
  localTab,
  readPersistedTabs,
  useHallWorkspaceStore,
} from "./hall-workspace-store"

describe("hall workspace store", () => {
  beforeEach(() => {
    localStorage.clear()
    resetMock.mockClear()
    useHallWorkspaceStore.setState({
      enabled: true,
      tabs: [localTab("本机")],
      activeKey: LOCAL_BACKEND_KEY,
      pendingFocus: null,
      webMode: false,
    })
  })

  it("opens a remote as a tab and activates it without touching the scoped stores", () => {
    const s = useHallWorkspaceStore.getState()
    s.open(7, "堂务")
    expect(useHallWorkspaceStore.getState().activeKey).toBe("remote:7")
    expect(useHallWorkspaceStore.getState().tabs.map((t) => t.key)).toEqual([
      "local",
      "remote:7",
    ])
    // The shell resets the stores once the old realm is unmounted (G3); a
    // reset here would run while the old realm can still refetch.
    expect(resetMock).not.toHaveBeenCalled()
    // Opening the same connection again only selects it: no duplicate tab.
    useHallWorkspaceStore.getState().open(7, "堂务")
    expect(useHallWorkspaceStore.getState().tabs).toHaveLength(2)
  })

  it("persists the remote tabs and the active key", () => {
    useHallWorkspaceStore.getState().open(7, "堂务")
    expect(readPersistedTabs()).toEqual({
      remotes: [{ id: 7, name: "堂务" }],
      activeKey: "remote:7",
    })
    useHallWorkspaceStore.getState().activate(LOCAL_BACKEND_KEY)
    expect(readPersistedTabs()?.activeKey).toBe("local")
  })

  it("never closes the local tab and falls back to it when the active remote closes", () => {
    const s = useHallWorkspaceStore.getState()
    s.open(7, "堂务")
    s.close(LOCAL_BACKEND_KEY)
    expect(useHallWorkspaceStore.getState().tabs).toHaveLength(2)
    useHallWorkspaceStore.getState().close("remote:7")
    expect(useHallWorkspaceStore.getState().activeKey).toBe("local")
    expect(useHallWorkspaceStore.getState().tabs).toHaveLength(1)
  })

  it("hands a pending focus only to the backend it was meant for", () => {
    const s = useHallWorkspaceStore.getState()
    s.setPendingFocus({
      backendKey: "remote:7",
      folderId: 1,
      conversationId: 2,
      agent: "claude_code",
    })
    expect(
      useHallWorkspaceStore.getState().takePendingFocus("local")
    ).toBeNull()
    expect(
      useHallWorkspaceStore.getState().takePendingFocus("remote:7")
        ?.conversationId
    ).toBe(2)
    expect(useHallWorkspaceStore.getState().pendingFocus).toBeNull()
  })

  it("keeps a derived desktop branch attached to its headquarters after persistence", () => {
    useHallWorkspaceStore
      .getState()
      .openDerivedBranch(7, 12, "11111111-1111-4111-8111-111111111111", "ARM")
    const tab = useHallWorkspaceStore.getState().tabs.slice(-1)[0]
    expect(tab?.key).toBe("remote:12")
    expect(tab?.headquartersConnectionId).toBe(7)
    expect(readPersistedTabs()?.remotes[0]).toMatchObject({
      id: 12,
      headquartersConnectionId: 7,
      branchId: tab?.branchId,
    })
  })

  it("persists browser branches separately and returns to headquarters when closed", () => {
    const id = "11111111-1111-4111-8111-111111111111"
    useHallWorkspaceStore.setState({ webMode: true })
    useHallWorkspaceStore.getState().openWebBranch(id, "Windows")
    expect(useHallWorkspaceStore.getState().activeKey).toBe(`branch:${id}`)
    expect(readPersistedTabs()).toBeNull()
    expect(readPersistedTabs(true)?.webBranches).toEqual([
      { id, name: "Windows" },
    ])
    useHallWorkspaceStore.getState().close(`branch:${id}`)
    expect(useHallWorkspaceStore.getState().activeKey).toBe("local")
  })
})
