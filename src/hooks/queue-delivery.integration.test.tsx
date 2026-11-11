/** Production auto-flush + recoverable queue + real lifecycle sender.
 * The engine/connection IO boundary is controlled; no live model is invoked. */

import { act, cleanup, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { TurnBusyError } from "@/lib/turn-busy"
import { useHallWorkspaceStore } from "@/stores/hall-workspace-store"
import { useRef } from "react"
import { useQueueAutoFlush } from "./use-queue-auto-flush"
import { useMessageQueue } from "./use-message-queue"

const toastError = vi.fn()

vi.mock("sonner", () => ({
  toast: {
    error: (...args: unknown[]) => toastError(...args),
    success: vi.fn(),
    warning: vi.fn(),
    message: vi.fn(),
  },
}))

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string, params?: Record<string, unknown>) =>
    params ? `${key}:${JSON.stringify(params)}` : key,
}))

const sendPrompt = vi.fn()
const disconnect = vi.fn().mockResolvedValue(undefined)

vi.mock("@/contexts/acp-connections-context", () => ({
  useAcpActions: () => ({ setActiveKey: vi.fn(), touchActivity: vi.fn() }),
}))

vi.mock("@/contexts/task-context", () => ({
  useTaskContext: () => ({
    addTask: vi.fn(),
    updateTask: vi.fn(),
    removeTask: vi.fn(),
  }),
}))

vi.mock("@/hooks/use-connection", () => ({
  useConnection: () => ({
    connectionId: "owned-connection",
    status: "connected",
    selectorsReady: true,
    connect: vi.fn().mockResolvedValue(undefined),
    disconnect,
    sendPrompt,
    setMode: vi.fn().mockResolvedValue(undefined),
    setConfigOption: vi.fn().mockResolvedValue(undefined),
    cancel: vi.fn().mockResolvedValue(undefined),
    respondPermission: vi.fn().mockResolvedValue(undefined),
    modes: null,
    configOptions: null,
    hasCachedSelectors: true,
    isViewer: false,
    backgroundOutstanding: 0,
  }),
}))

import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"

function draft() {
  return {
    blocks: [{ type: "text" as const, text: "hello" }],
    displayText: "hello",
  }
}

// Integration reproduces the production idle auto-flush sequence against the
// real lifecycle sender. Only the connection/engine IO boundary is controlled.
function renderQueue(
  engine: "codex" | "claude_code",
  ready = false,
  scope?: string
) {
  return renderHook(
    ({
      idle,
      blocked,
      queueScope,
    }: {
      idle: boolean
      blocked: boolean
      queueScope?: string
    }) => {
      const queue = useMessageQueue(queueScope ?? scope)
      const sender = useConnectionLifecycle({
        contextKey: "queue-flow",
        agentType: engine,
        isActive: false,
      })
      const bounceAt = useRef(0)
      useQueueAutoFlush({
        queue,
        ready: idle,
        blocked,
        bounceAt,
        send: (item, callbacks) =>
          sender.handleSend(item.draft, item.modeId, {
            clientMessageId: item.id,
            onSendAccepted: callbacks.onAccepted,
            onTurnInProgress: callbacks.onNotSent,
            onSendFailed: callbacks.onUnknown,
          }),
      })
      return queue
    },
    {
      initialProps: { idle: ready, blocked: false, queueScope: scope } as {
        idle: boolean
        blocked: boolean
        queueScope?: string
      },
    }
  )
}

describe("ordinary queue delivery through the lifecycle", () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.clearAllMocks()
    localStorage.clear()
    useHallWorkspaceStore.setState({ enabled: false })
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })
  it.each(["codex", "claude_code"] as const)(
    "%s retains the claimed head until backend acceptance",
    async (engine) => {
      let accept!: () => void
      sendPrompt.mockReturnValue(
        new Promise<void>((resolve) => {
          accept = resolve
        })
      )
      const view = renderQueue(engine)
      const prompt = {
        blocks: [
          { type: "text" as const, text: "synthetic queue" },
          { type: "image" as const, data: "YQ==", mime_type: "image/png" },
        ],
        displayText: "synthetic queue",
      }
      act(() => {
        view.result.current.enqueue(prompt, "plan")
        view.result.current.enqueue(draft(), null)
      })
      view.rerender({ idle: true, blocked: false })
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0)
      })
      expect(sendPrompt).toHaveBeenCalledTimes(1)
      expect(sendPrompt.mock.calls[0][0]).toEqual(prompt.blocks)
      expect(view.result.current.queue.map((row) => row.draft)).toEqual([
        prompt,
        draft(),
      ])
      await act(async () => {
        accept()
        await Promise.resolve()
      })
      expect(view.result.current.queue.map((row) => row.draft)).toEqual([
        draft(),
      ])
      view.rerender({ idle: false, blocked: false })
      view.rerender({ idle: true, blocked: false })
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0)
      })
      expect(sendPrompt).toHaveBeenCalledTimes(2)
      expect(sendPrompt.mock.calls[1][0]).toEqual(draft().blocks)
      expect(view.result.current.queue).toEqual([])
    }
  )
  it("preserves an uncertain write without resending it or the next row", async () => {
    sendPrompt.mockRejectedValue(
      new Error("synthetic lost acceptance response")
    )
    const view = renderQueue("codex")
    act(() => {
      view.result.current.enqueue(draft(), "plan")
      view.result.current.enqueue(draft(), null)
    })
    view.rerender({ idle: true, blocked: false })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50)
    })
    expect(view.result.current.queue).toHaveLength(2)
    expect(view.result.current.queue[0]).toMatchObject({
      status: "unknown",
      modeId: "plan",
      draft: draft(),
    })
    expect(sendPrompt).toHaveBeenCalledTimes(1)
  })
  it("waits through approval/disconnection, then sends from a kept-alive background tab", async () => {
    sendPrompt.mockResolvedValue(undefined)
    const view = renderQueue("claude_code")
    act(() => view.result.current.enqueue(draft(), null))
    view.rerender({ idle: true, blocked: true })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    expect(sendPrompt).not.toHaveBeenCalled()
    view.rerender({ idle: false, blocked: false })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    expect(sendPrompt).not.toHaveBeenCalled()
    view.rerender({ idle: true, blocked: false })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(sendPrompt).toHaveBeenCalledTimes(1)
    expect(view.result.current.queue).toEqual([])
  })
  it("busy/cancel-to-idle keeps FIFO and the same id without a retry storm", async () => {
    sendPrompt
      .mockRejectedValueOnce(new TurnBusyError())
      .mockResolvedValue(undefined)
    const view = renderQueue("codex")
    act(() => {
      view.result.current.enqueue(draft(), "plan")
      view.result.current.enqueue(draft(), null)
    })
    const id = view.result.current.queue[0].id
    view.rerender({ idle: true, blocked: false })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(view.result.current.queue[0].id).toBe(id)
    expect(sendPrompt).toHaveBeenCalledTimes(1)
    view.rerender({ idle: false, blocked: false })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000)
    })
    view.rerender({ idle: true, blocked: false })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(sendPrompt.mock.calls[1][1].clientMessageId).toBe(id)
    expect(view.result.current.queue).toHaveLength(1)
  })
  it("acceptance after unmount settles the original backend queue; a remount does not duplicate", async () => {
    Object.defineProperty(navigator, "locks", {
      configurable: true,
      value: {
        request: (_key: string, fn: () => unknown) => Promise.resolve(fn()),
      },
    })
    let accept!: () => void
    sendPrompt.mockReturnValue(
      new Promise<void>((resolve) => {
        accept = resolve
      })
    )
    const view = renderQueue("codex", false, "synthetic-remount")
    act(() => view.result.current.enqueue(draft(), "plan"))
    view.rerender({ idle: true, blocked: false })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    view.unmount()
    const restored = renderQueue("codex", true, "synthetic-remount")
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000)
    })
    expect(sendPrompt).toHaveBeenCalledTimes(1)
    expect(restored.result.current.queue).toHaveLength(1)
    await act(async () => {
      accept()
      await Promise.resolve()
    })
    expect(restored.result.current.queue).toEqual([])
  })
  it("does not redirect a claim waiting for a lock into a newly selected backend", async () => {
    let release!: () => void
    let hold = true
    Object.defineProperty(navigator, "locks", {
      configurable: true,
      value: {
        request: (_key: string, fn: () => unknown) =>
          hold
            ? new Promise((resolve) => {
                release = () => resolve(fn())
              })
            : Promise.resolve(fn()),
      },
    })
    const view = renderQueue("codex", true, "backend-before-lock")
    act(() => view.result.current.enqueue(draft(), null))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    view.rerender({
      idle: true,
      blocked: false,
      queueScope: "backend-after-lock",
    })
    hold = false
    await act(async () => {
      release()
      await Promise.resolve()
    })
    expect(sendPrompt).not.toHaveBeenCalled()
    expect(view.result.current.queue).toEqual([])
    const original = renderQueue("codex", false, "backend-before-lock")
    expect(original.result.current.queue[0]).toMatchObject({
      draft: draft(),
      status: "queued",
    })
  })
})
