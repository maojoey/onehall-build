import { afterEach, beforeEach, expect, it } from "vitest"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  beginBackendSwitch,
  configureWebBranchTransport,
} from "@/lib/transport"
import { WebTransport } from "@/lib/transport/web-transport"
import {
  saveMessageInputDraft,
  loadMessageInputDraft,
} from "@/lib/message-input-draft"
import {
  saveLastActiveContext,
  loadLastActiveContext,
} from "@/lib/last-active-context-storage"
import {
  pushClosedTab,
  popClosedTab,
  resetClosedTabStackForTests,
} from "@/lib/closed-tab-stack"
import { saveCommandSelection, loadCommandSelection } from "./command-selection"
import { captureHallWorkspace } from "./workspace-lifetime"
import { useHallWorkspaceStore } from "@/stores/hall-workspace-store"
const a = "11111111-1111-4111-8111-111111111111",
  b = "22222222-2222-4222-8222-222222222222"
beforeEach(() => {
  localStorage.clear()
  resetClosedTabStackForTests()
  __resetTransportForTests()
  __setTransportFactoriesForTests({ web: (url) => new WebTransport(url) })
})
afterEach(() => __resetTransportForTests())
it("keeps same-ID drafts, folders, command selection and closed tabs separate across branches", () => {
  configureWebBranchTransport(a)
  saveMessageInputDraft("same-conversation", "draft A")
  saveLastActiveContext({ folderId: 1, isChat: false })
  saveCommandSelection(1, 7)
  pushClosedTab({
    kind: "file",
    key: "file1",
    index: 0,
    folderId: 1,
    path: "/branch-a/file",
  })
  beginBackendSwitch()
  configureWebBranchTransport(b)
  expect(loadMessageInputDraft("same-conversation")).toBeNull()
  expect(loadLastActiveContext()).toBeNull()
  expect(loadCommandSelection(1)).toBeNull()
  expect(popClosedTab()).toBeNull()
  saveMessageInputDraft("same-conversation", "draft B")
  beginBackendSwitch()
  configureWebBranchTransport(a)
  expect(loadMessageInputDraft("same-conversation")).toBe("draft A")
  expect(loadLastActiveContext()?.folderId).toBe(1)
  expect(loadCommandSelection(1)).toBe(7)
  expect(popClosedTab()).toMatchObject({ path: "/branch-a/file" })
})
it("recognizes leaving a branch for headquarters during outgoing agent cleanup", () => {
  configureWebBranchTransport(a)
  useHallWorkspaceStore.setState({ enabled: true, activeKey: `branch:${a}` })
  const lifetime = captureHallWorkspace()
  useHallWorkspaceStore.setState({ activeKey: "local" })
  expect(lifetime?.key).toBe(`branch:${a}`)
  expect(lifetime?.isLeaving()).toBe(true)
})
