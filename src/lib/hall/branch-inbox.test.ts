import { beforeEach, expect, it, vi } from "vitest"
const state = vi.hoisted(() => ({
  env: "tauri",
  backend: "local",
  call: vi.fn(),
}))
vi.mock("@/lib/transport/detect", () => ({
  detectEnvironment: () => state.env,
}))
vi.mock("@/lib/transport", () => ({
  getActiveBackendKey: () => state.backend,
  getShellTransport: () => ({ call: state.call }),
}))
import { localInboxClient } from "./branch-inbox"
beforeEach(() => {
  state.env = "tauri"
  state.backend = "local"
  state.call.mockReset()
})
it("refuses a web or remote context instead of operating on the local computer", () => {
  state.env = "web"
  expect(localInboxClient()).toBeNull()
  state.env = "tauri"
  state.backend = "remote:7"
  expect(localInboxClient()).toBeNull()
  expect(state.call).not.toHaveBeenCalled()
})
it("uses the guarded local command for a local acceptance", async () => {
  state.call.mockResolvedValue({ taskId: "local-task" })
  const selection = {
    issuerId: "hq",
    dispatchId: "id",
    expectedRevision: 1,
    memberId: "member",
    snapshotId: "snapshot",
    projectId: null,
  }
  await localInboxClient()!.accept(selection)
  expect(state.call).toHaveBeenCalledWith("hall_dispatch_local", {
    request: { action: "accept", selection },
  })
})
