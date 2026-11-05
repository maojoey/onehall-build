import { beforeEach, expect, it, vi } from "vitest"
const shell = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("@/lib/transport", () => ({ getShellTransport: () => shell }))
import { dispatchClient } from "./dispatch"
beforeEach(() => shell.call.mockReset())
it("pins dispatch requests to HQ even when a desktop branch is selected", async () => {
  shell.call.mockResolvedValue([])
  await dispatchClient(7).list()
  expect(shell.call).toHaveBeenCalledWith("remote_http_call", {
    connectionId: 7,
    command: "hall_dispatch_control",
    args: { request: { action: "list" } },
  })
})
it("keeps the submission identity and task body unchanged on a retry", async () => {
  shell.call.mockResolvedValue({ dispatchId: "saved" })
  const draft = {
    submissionId: "same",
    branchId: "branch",
    memberRef: "worker",
    title: "Work",
    description: "Keep the exact body",
  }
  const client = dispatchClient(null)
  await client.create(draft)
  await client.create(draft)
  expect(shell.call.mock.calls[0]).toEqual(shell.call.mock.calls[1])
  expect(shell.call).toHaveBeenCalledWith("hall_dispatch_control", {
    request: { action: "create", draft },
  })
  await client.sync("saved")
  expect(shell.call).toHaveBeenLastCalledWith("hall_dispatch_control", {
    request: { action: "sync", dispatch_id: "saved" },
  })
})
