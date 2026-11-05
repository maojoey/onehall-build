import { describe, expect, it, vi } from "vitest"
import type { Transport } from "@/lib/transport"
import { createHallMailboxClient, isMailboxUnavailable } from "./mailbox"
describe("mailbox transport", () => {
  it("keeps role and mailbox operations on the bound transport with explicit identities", async () => {
    const call = vi.fn().mockResolvedValue([])
    const client = createHallMailboxClient({ call } as unknown as Transport)
    await client.role()
    await client.bind(null, 7)
    await client.inbox("member")
    await client.acknowledge("member", "message")
    expect(call.mock.calls).toEqual([
      ["hall_get_pm_binding", {}],
      ["hall_bind_pm", { memberId: null, expectedRevision: 7 }],
      ["hall_read_mailbox", { memberId: "member" }],
      ["hall_ack_message", { memberId: "member", messageId: "message" }],
    ])
  })
  it("recognizes missing old commands and propagates network failures without a fallback", async () => {
    expect(
      isMailboxUnavailable(new Error("command hall_read_mailbox not found"))
    ).toBe(true)
    expect(isMailboxUnavailable(new Error("connection closed"))).toBe(false)
    const failure = new Error("connection closed")
    const call = vi.fn().mockRejectedValue(failure)
    await expect(
      createHallMailboxClient({ call } as unknown as Transport).inbox("member")
    ).rejects.toBe(failure)
    expect(call).toHaveBeenCalledOnce()
  })
})
