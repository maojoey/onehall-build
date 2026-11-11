import { describe, expect, it, vi } from "vitest"
import { deliverQueuedSteer, deliverQueuedRowSteer } from "./queued-steer"

describe("queued native insert", () => {
  it("reports delivery without reordering the row when the insert lands", async () => {
    const steer = vi.fn(async () => {})
    const prioritize = vi.fn()
    expect(await deliverQueuedSteer(steer, prioritize)).toBe(true)
    expect(steer).toHaveBeenCalledOnce()
    expect(prioritize).not.toHaveBeenCalled()
  })

  it("prioritizes the queued row without sending when the turn ended before insertion", async () => {
    const steer = vi.fn(async () => {
      throw new Error("no active turn for feedback")
    })
    const prioritize = vi.fn()
    expect(await deliverQueuedSteer(steer, prioritize)).toBe(false)
    expect(steer).toHaveBeenCalledOnce()
    expect(prioritize).toHaveBeenCalledOnce()
  })

  it("rethrows any other failure and leaves the row where it was", async () => {
    const failure = new Error("connection lost")
    const prioritize = vi.fn()
    await expect(
      deliverQueuedSteer(async () => {
        throw failure
      }, prioritize)
    ).rejects.toBe(failure)
    expect(prioritize).not.toHaveBeenCalled()
  })

  it("does not prioritize or report delivery when the turn is busy", async () => {
    const failure = new Error("turn already in progress")
    const prioritize = vi.fn()
    await expect(
      deliverQueuedSteer(async () => {
        throw failure
      }, prioritize)
    ).rejects.toBe(failure)
    expect(prioritize).not.toHaveBeenCalled()
  })
})

describe("durable queued steering", () => {
  it.each(["accepted", "ended", "lost"] as const)(
    "preserves the full row for %s",
    async (outcome) => {
      const { createMessageQueueStore } = await import("./message-queue-store")
      const store = createMessageQueueStore()
      const draft = {
        displayText: "synthetic",
        blocks: [
          { type: "image" as const, data: "YQ==", mime_type: "image/png" },
        ],
      }
      store.commit([{ id: "one", draft, modeId: "plan" }])
      const queue = {
        beginSend: store.beginSend,
        settle: store.settle,
        moveToFront: vi.fn(),
      }
      const send = vi.fn(async (payload) => {
        expect(payload).toEqual(draft)
        if (outcome === "ended") throw new Error("no active turn for feedback")
        if (outcome === "lost") throw new Error("lost response")
      })
      if (outcome === "lost")
        await expect(
          deliverQueuedRowSteer(queue, "one", send, () => true)
        ).rejects.toThrow("lost response")
      else
        expect(
          await deliverQueuedRowSteer(queue, "one", send, () => true)
        ).toBe(outcome === "accepted")
      expect(store.read().length).toBe(outcome === "accepted" ? 0 : 1)
      if (outcome === "lost") {
        expect(store.read()[0]).toMatchObject({ draft, status: "unknown" })
        expect(await store.beginSend()).toBeUndefined()
      }
      if (outcome === "ended") {
        expect(store.read()[0]).toMatchObject({
          id: "one",
          draft,
          status: "queued",
        })
        expect(queue.moveToFront).toHaveBeenCalledWith("one")
      }
    }
  )
})
