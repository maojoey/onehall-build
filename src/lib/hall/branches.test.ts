import { beforeEach, expect, it, vi } from "vitest"
const shell = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("@/lib/transport", () => ({ getShellTransport: () => shell }))
import { branchClient } from "./branches"
beforeEach(() => shell.call.mockReset())
it("always sends browser directory calls to the headquarters shell", async () => {
  shell.call.mockResolvedValue([])
  await branchClient(null).list()
  expect(shell.call).toHaveBeenCalledWith("hall_list_branches", {})
})
it("pins desktop directory calls to the headquarters rather than the selected derived branch", async () => {
  shell.call.mockResolvedValue([])
  await branchClient(7).list()
  expect(shell.call).toHaveBeenCalledWith("remote_http_call", {
    connectionId: 7,
    command: "hall_list_branches",
    args: {},
  })
})
