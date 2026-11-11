import { IDBFactory } from "fake-indexeddb"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  createMessageQueueStore,
  resolveMessageQueueScope,
} from "./message-queue-store"
import type { QueuedMessage } from "@/hooks/use-message-queue"

function row(id: string): QueuedMessage {
  return {
    id,
    modeId: "plan",
    adoptSendTimeMode: true,
    draft: {
      displayText: id,
      blocks: [
        { type: "text", text: id },
        {
          type: "image",
          data: "YQ==",
          mime_type: "image/png",
          uri: "upload://synthetic",
        },
      ],
    },
  }
}
let serial: Promise<unknown>
beforeEach(() => {
  localStorage.clear()
  serial = Promise.resolve()
  vi.stubGlobal("navigator", {
    locks: {
      request: (_key: string, claim: () => unknown) => {
        const result = serial.then(claim)
        serial = result.catch(() => {})
        return result
      },
    },
  })
})

afterEach(() => vi.unstubAllGlobals())

describe("recoverable ordinary conversation queue", () => {
  it("restores full ordered drafts, attachments and explicit/adopted modes after a new store", () => {
    const a = createMessageQueueStore("backend-a/conversation-7")
    a.commit([
      row("one"),
      { ...row("two"), modeId: null, adoptSendTimeMode: false },
    ])
    expect(createMessageQueueStore("backend-a/conversation-7").read()).toEqual(
      a.read()
    )
    expect(createMessageQueueStore("backend-b/conversation-7").read()).toEqual(
      []
    )
    expect(createMessageQueueStore("backend-a/conversation-8").read()).toEqual(
      []
    )
  })
  it("two concurrent windows claim only once; restart makes the in-flight write unknown", async () => {
    const a = createMessageQueueStore("shared"),
      b = createMessageQueueStore("shared")
    a.commit([row("one"), row("two")])
    const claims = await Promise.all([a.beginSend(), b.beginSend()])
    expect(claims.filter(Boolean)).toHaveLength(1)
    expect(createMessageQueueStore("shared").read()[0].status).toBe("unknown")
    expect(await b.beginSend()).toBeUndefined()
    expect(a.read()[0].status).toBe("sending")
    a.settle("one", "accepted")
    expect(b.read()).toEqual([row("two")])
  })
  it("an explicit busy rejection preserves identity and FIFO, an unknown response blocks retry", async () => {
    const a = createMessageQueueStore("busy")
    a.commit([row("one"), row("two")])
    await a.beginSend()
    a.settle("one", "busy")
    expect((await a.beginSend())?.id).toBe("one")
    a.settle("one", "unknown")
    expect(await a.beginSend()).toBeUndefined()
    expect(a.read().map((item) => item.id)).toEqual(["one", "two"])
    a.settle("one", "accepted") // a stale/duplicate callback cannot change it
    expect(a.read()[0].status).toBe("unknown")
  })
  it("a late acceptance after unmount removes only its own row, preserving concurrent enqueue", async () => {
    const a = createMessageQueueStore("late"),
      b = createMessageQueueStore("late")
    a.commit([row("one")])
    await a.beginSend()
    b.commit([...b.read(), row("two")])
    a.settle("one", "accepted")
    expect(createMessageQueueStore("late").read()).toEqual([row("two")])
  })
  it("a stale window reorder cannot reset a claimed or accepted delivery", async () => {
    const a = createMessageQueueStore("stale"),
      b = createMessageQueueStore("stale")
    a.commit([row("one"), row("two")])
    const stale = b.read()
    const claim = await a.beginSend()
    b.commit([...stale].reverse())
    expect(b.read().find((item) => item.id === "one")?.status).toBe("unknown")
    a.settle("one", "accepted", claim?.deliveryToken)
    b.commit(stale)
    expect(b.read().map((item) => item.id)).toEqual(["two"])
  })
  it("pins send-time mode on the first claim and rejects a previous attempt's late callback", async () => {
    const a = createMessageQueueStore("tokens")
    a.commit([row("one")])
    const first = (await a.beginSend(undefined, "selected-mode"))!
    expect(first).toMatchObject({
      modeId: "selected-mode",
      adoptSendTimeMode: false,
    })
    a.settle("one", "busy", first.deliveryToken)
    const next = (await a.beginSend(undefined, "another-mode"))!
    expect(next.modeId).toBe("selected-mode")
    a.settle("one", "accepted", first.deliveryToken)
    expect(a.read()[0].status).toBe("sending")
    a.settle("one", "unknown", next.deliveryToken)
    a.retryUnknown("one")
    await serial
    expect((await a.beginSend())?.id).toBe("one")
  })
  it("storage failure retains the memory copy, exposes failure and never starts IO", async () => {
    const a = createMessageQueueStore("quota")
    const spy = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {
        throw new Error("quota")
      })
    try {
      a.commit([row("one")])
      expect(a.getError()).toBe(true)
      expect(a.read()).toEqual([row("one")])
      expect(await a.beginSend()).toBeUndefined()
    } finally {
      spy.mockRestore()
    }
  })
  it("does not erase corrupt saved data or silently claim it", async () => {
    const key = "onehall:message-queue:v1:corrupt:draft:one"
    localStorage.setItem(key, "{bad")
    const a = createMessageQueueStore("corrupt")
    expect(a.getError()).toBe(true)
    expect(await a.beginSend()).toBeUndefined()
    expect(localStorage.getItem(key)).toBe("{bad")
  })
  it("serializes cross-window claims with IndexedDB when Web Locks is unavailable", async () => {
    vi.stubGlobal("navigator", {})
    vi.stubGlobal("indexedDB", new IDBFactory())
    try {
      const a = createMessageQueueStore("idb-fallback"),
        b = createMessageQueueStore("idb-fallback")
      a.commit([row("one")])
      const claims = await Promise.all([a.beginSend(), b.beginSend()])
      expect(claims.filter(Boolean)).toHaveLength(1)
      expect(a.getError()).toBe(false)
      expect(b.getError()).toBe(false)
    } finally {
      vi.stubGlobal("indexedDB", undefined)
    }
  })
  it("fails closed when cross-window atomic claims are unavailable", async () => {
    const a = createMessageQueueStore("no-lock")
    a.commit([row("one")])
    vi.stubGlobal("navigator", {})
    expect(await a.beginSend()).toBeUndefined()
    expect(a.read()).toEqual([row("one")])
    expect(a.getError()).toBe(true)
  })
  it("the bound draft and canonical conversation restore the same queue scope", () => {
    const draftScope = resolveMessageQueueScope("remote:7", "new-a", null)
    expect(resolveMessageQueueScope("remote:7", "new-a", 42, draftScope)).toBe(
      draftScope
    )
    expect(resolveMessageQueueScope("remote:7", "conv-1-codex-42", 42)).toBe(
      draftScope
    )
    expect(resolveMessageQueueScope("local", "conv-1-codex-42", 42)).not.toBe(
      draftScope
    )
  })
})
