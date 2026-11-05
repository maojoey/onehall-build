import { describe, expect, it, vi } from "vitest"
import type { Transport } from "@/lib/transport"
import {
  createHallTasksClient,
  isTasksUnavailable,
  type HallTask,
} from "./tasks"

const mocks = vi.hoisted(() => ({ getTransport: vi.fn(), shell: vi.fn() }))
vi.mock("@/lib/transport", () => ({
  getTransport: mocks.getTransport,
  getShellTransport: mocks.shell,
}))

describe("team client backend ownership", () => {
  it("keeps requests on the captured backend including writes and conflicts", async () => {
    const call = vi.fn().mockResolvedValue([])
    const other = vi.fn()
    const client = createHallTasksClient({ call } as unknown as Transport)
    mocks.getTransport.mockReturnValue({ call: other })
    await client.list({ projectId: 7 })
    await client.members()
    await client.snapshots("member")
    await client.get("task")
    await client.cancel({ taskId: "task", revision: 3 } as HallTask)
    expect(call.mock.calls.map((c) => c[0])).toEqual([
      "hall_list_tasks",
      "hall_list_members",
      "hall_list_member_snapshots",
      "hall_get_task",
      "hall_cancel_task",
    ])
    expect(call).toHaveBeenLastCalledWith("hall_cancel_task", {
      taskId: "task",
      expectedRevision: 3,
    })
    expect(other).not.toHaveBeenCalled()
    expect(mocks.shell).not.toHaveBeenCalled()
  })
  it("reports an unsupported backend without retrying on the shell", async () => {
    const failure = new Error("command hall_list_tasks not found")
    const call = vi.fn().mockRejectedValue(failure)
    const client = createHallTasksClient({ call } as unknown as Transport)
    await expect(client.list()).rejects.toBe(failure)
    expect(isTasksUnavailable(failure)).toBe(true)
    expect(
      isTasksUnavailable(new Error("command hall_list_members not found"))
    ).toBe(true)
    expect(call).toHaveBeenCalledOnce()
    expect(mocks.shell).not.toHaveBeenCalled()
  })
  it("recognizes the old desktop proxy's wrapped 501 without masking network failures", () => {
    expect(
      isTasksUnavailable({
        code: "network_error",
        message: "Remote returned HTTP 501 Not Implemented",
        detail: JSON.stringify({
          code: "not_implemented",
          message:
            "API endpoint 'hall_list_tasks' is not available in web mode",
        }),
      })
    ).toBe(true)
    expect(
      isTasksUnavailable({
        code: "network_error",
        message: "Remote returned HTTP 404 Not Found",
        detail: "not found",
      })
    ).toBe(false)
    expect(isTasksUnavailable(new Error("offline"))).toBe(false)
  })
})
