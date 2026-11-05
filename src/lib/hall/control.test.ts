import { describe, it, expect, vi } from "vitest"
import { createTeamControlClient } from "./control"
import type { Transport } from "@/lib/transport"
import type { HallTaskDetail } from "./tasks"
describe("team control transport", () => {
  it("closes a reviewed best-effort run with an explicit acknowledgement", async () => {
    const call = vi.fn().mockResolvedValue({})
    const client = createTeamControlClient({ call } as unknown as Transport)
    await client.resolveBestEffort({
      taskId: "t",
      revision: 5,
    } as HallTaskDetail)
    expect(call).toHaveBeenCalledWith("hall_team_control", {
      request: {
        action: "resolve_best_effort",
        task_id: "t",
        expected_revision: 5,
        acknowledge_unverified_descendants: true,
      },
    })
  })
  it("sends task/revision intent without accepting a PID or connection identity", async () => {
    const call = vi.fn().mockResolvedValue({})
    const client = createTeamControlClient({ call } as unknown as Transport)
    await client.dispatch({ taskId: "t", revision: 2 } as HallTaskDetail)
    expect(call).toHaveBeenCalledWith("hall_team_control", {
      request: { action: "dispatch", task_id: "t", expected_revision: 2 },
    })
  })
})
