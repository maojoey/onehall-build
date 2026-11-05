import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { WebTransport } from "./web-transport"

class Socket {
  static OPEN = 1
  static instances: Socket[] = []
  static failConstruction = false
  readyState = 0
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  sent: unknown[] = []

  constructor() {
    if (Socket.failConstruction) throw new Error("WebSocket unavailable")
    Socket.instances.push(this)
  }
  send(data: string) {
    this.sent.push(JSON.parse(data))
  }
  close() {
    this.readyState = 3
  }
  open() {
    this.readyState = 1
    this.onopen?.()
  }
  message(frame: object) {
    this.onmessage?.({ data: JSON.stringify(frame) })
  }
  ready() {
    this.message({ channel: "__ready__", payload: null })
  }
  drop() {
    this.readyState = 3
    this.onclose?.()
  }
}

function latestSocket(): Socket {
  return Socket.instances[Socket.instances.length - 1]
}

const transports: WebTransport[] = []
const fetcher = vi.fn<typeof fetch>()
let visibility: ReturnType<typeof vi.spyOn>
function create(ready = false) {
  const transport = new WebTransport("http://localhost/branches/example")
  transports.push(transport)
  transport.eventStream()
  const socket = latestSocket()
  if (ready) {
    socket.open()
    socket.ready()
  }
  return { transport, socket }
}
function branchError(code: string) {
  return new Response(JSON.stringify({ code, message: "private upstream" }), {
    status: 503,
  })
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(0)
  visibility = vi.spyOn(document, "visibilityState", "get")
  visibility.mockReturnValue("visible")
  Socket.instances = []
  Socket.failConstruction = false
  vi.stubGlobal("WebSocket", Socket)
  localStorage.setItem("codeg_token", "tok")
  fetcher.mockReset().mockImplementation(async () => new Response("{}"))
  vi.stubGlobal("fetch", fetcher)
})
afterEach(() => {
  transports.splice(0).forEach((transport) => transport.destroy())
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  vi.useRealTimers()
  localStorage.clear()
})

describe("WebTransport readiness recovery", () => {
  it("bounds a socket that never opens and recovers without an onclose event", async () => {
    const { transport, socket } = create()
    await vi.advanceTimersByTimeAsync(15_000)
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    expect(socket.readyState).toBe(3)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(Socket.instances).toHaveLength(2)
  })

  it("bounds open-to-ready by five seconds and the total create deadline", async () => {
    const { transport, socket } = create()
    await vi.advanceTimersByTimeAsync(12_000)
    socket.open()
    await vi.advanceTimersByTimeAsync(3_000)
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    expect(socket.readyState).toBe(3)
  })

  it("recovers five seconds after open when the ready frame never arrives", async () => {
    const { transport, socket } = create()
    socket.open()
    await vi.advanceTimersByTimeAsync(5_000)
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    await vi.advanceTimersByTimeAsync(1_000)
    expect(Socket.instances).toHaveLength(2)
  })

  it("preserves pending readiness across multiple failures before the grace period", async () => {
    const { transport, socket } = create()
    let ready = false
    const waiting = transport.waitForReady().then(() => {
      ready = true
    })
    socket.drop()
    await vi.advanceTimersByTimeAsync(1_000)
    latestSocket().drop()
    await vi.advanceTimersByTimeAsync(2_000)
    latestSocket().open()
    latestSocket().ready()
    await vi.advanceTimersByTimeAsync(0)
    expect(ready).toBe(true)
    await waiting
  })

  it("starts a fresh readiness wait after an already-ready socket drops", async () => {
    const { transport, socket } = create(true)
    socket.drop()
    let ready = false
    const waiting = transport.waitForReady().then(() => {
      ready = true
    })
    await vi.advanceTimersByTimeAsync(1_000)
    expect(ready).toBe(false)
    latestSocket().open()
    latestSocket().ready()
    await waiting
    expect(ready).toBe(true)
  })

  it("keeps newly subscribing consumers behind an in-flight health probe", async () => {
    const { transport, socket } = create(true)
    let finish!: (value: Response) => void
    fetcher.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        })
    )
    socket.drop()
    await vi.advanceTimersByTimeAsync(1_000)
    const waiting = transport.subscribe("later", () => {})
    expect(Socket.instances).toHaveLength(1)
    finish(new Response("{}"))
    await vi.advanceTimersByTimeAsync(0)
    latestSocket().open()
    latestSocket().ready()
    await waiting
    expect(Socket.instances).toHaveLength(2)
  })

  it("handles onerror without needing the browser to deliver onclose", async () => {
    const { transport, socket } = create(true)
    socket.onerror?.()
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    expect(socket.readyState).toBe(3)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(Socket.instances).toHaveLength(2)
  })

  it("settles pending readiness and cancels creation deadlines on destroy", async () => {
    const { transport, socket } = create()
    const waiting = transport.waitForReady()
    const lateOpen = socket.onopen!
    const lateMessage = socket.onmessage!
    transport.destroy()
    lateOpen()
    lateMessage({ data: JSON.stringify({ channel: "__ready__" }) })
    await waiting
    await vi.advanceTimersByTimeAsync(120_000)
    expect(Socket.instances).toHaveLength(1)
    expect(fetcher).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
  })

  it("keeps the legacy grace resolution but starts controlled recovery", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {})
    const { transport, socket } = create()
    const waiting = transport.waitForReady()
    await vi.advanceTimersByTimeAsync(5_000)
    await waiting
    expect(socket.readyState).toBe(3)
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
  })

  it("grows backoff after repeated open-without-ready failures", async () => {
    const { socket } = create()
    socket.open()
    await vi.advanceTimersByTimeAsync(6_000)
    const second = latestSocket()
    second.open()
    await vi.advanceTimersByTimeAsync(5_000)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(Socket.instances).toHaveLength(2)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(Socket.instances).toHaveLength(3)
  })

  it("handles a throwing WebSocket constructor as a visible recoverable failure", async () => {
    Socket.failConstruction = true
    const transport = new WebTransport("http://localhost")
    transports.push(transport)
    expect(() => transport.eventStream()).not.toThrow()
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    Socket.failConstruction = false
    await vi.advanceTimersByTimeAsync(1_000)
    expect(Socket.instances).toHaveLength(1)
  })

  it("ignores queued callbacks from a replaced socket and duplicate ready frames", async () => {
    const { transport, socket } = create(true)
    const ready = vi.fn()
    transport.onReady(ready)
    socket.ready()
    expect(ready).not.toHaveBeenCalled()
    const staleReady = socket.onmessage!
    const staleClose = socket.onclose!
    socket.drop()
    await vi.advanceTimersByTimeAsync(1_000)
    const replacement = latestSocket()
    staleReady({ data: JSON.stringify({ channel: "__ready__" }) })
    staleClose()
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    replacement.open()
    replacement.ready()
    expect(ready).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(Socket.instances).toHaveLength(2)
  })

  it("does not replace a connecting socket on repeated reconnect or wake nudges", () => {
    const { transport, socket } = create()
    transport.reconnectNow()
    transport.reconnectNow()
    window.dispatchEvent(new Event("online"))
    document.dispatchEvent(new Event("visibilitychange"))
    expect(socket.readyState).toBe(0)
    expect(fetcher).not.toHaveBeenCalled()
    expect(Socket.instances).toHaveLength(1)
  })
})

describe("WebTransport application heartbeat", () => {
  it("detects a foreground half-open socket after three unanswered opportunities", async () => {
    const { transport, socket } = create(true)
    await vi.advanceTimersByTimeAsync(15_000)
    expect(socket.sent).toEqual([{ action: "ping" }])
    await vi.advanceTimersByTimeAsync(30_000)
    expect(transport.getConnectionSnapshot()).toBe("connected")
    await vi.advanceTimersByTimeAsync(15_000)
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    expect(socket.readyState).toBe(3)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(Socket.instances).toHaveLength(2)
  })

  it("accepts application pong frames and does not repeatedly reattach subscriptions", async () => {
    const { transport, socket } = create(true)
    transport.eventStream().attach(
      "conn",
      {},
      {
        onSnapshot: vi.fn(),
        onReplay: vi.fn(),
        onEvent: vi.fn(),
        onDetached: vi.fn(),
      }
    )
    for (let i = 0; i < 8; i++) {
      await vi.advanceTimersByTimeAsync(15_000)
      socket.message({ type: "pong" })
    }
    expect(transport.getConnectionSnapshot()).toBe("connected")
    expect(
      socket.sent.filter(
        (frame) => (frame as { action: string }).action === "ping"
      )
    ).toHaveLength(8)
    expect(
      socket.sent.filter(
        (frame) => (frame as { action: string }).action === "attach"
      )
    ).toHaveLength(1)
  })

  it("ignores background elapsed time and first probes afresh when visible", async () => {
    const { transport, socket } = create(true)
    await vi.advanceTimersByTimeAsync(45_000)
    visibility.mockReturnValue("hidden")
    document.dispatchEvent(new Event("visibilitychange"))
    await vi.advanceTimersByTimeAsync(300_000)
    expect(transport.getConnectionSnapshot()).toBe("connected")
    visibility.mockReturnValue("visible")
    document.dispatchEvent(new Event("visibilitychange"))
    expect(socket.sent[socket.sent.length - 1]).toEqual({ action: "ping" })
    expect(Socket.instances).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(30_000)
    expect(transport.getConnectionSnapshot()).toBe("connected")
    socket.message({ type: "pong" })
    await vi.advanceTimersByTimeAsync(15_000)
    expect(transport.getConnectionSnapshot()).toBe("connected")
  })

  it("resets unanswered opportunities after a long scheduler suspension", async () => {
    const { transport, socket } = create(true)
    await vi.advanceTimersByTimeAsync(45_000)
    vi.setSystemTime(600_000)
    await vi.advanceTimersByTimeAsync(15_000)
    expect(transport.getConnectionSnapshot()).toBe("connected")
    expect(socket.sent[socket.sent.length - 1]).toEqual({ action: "ping" })
    socket.message({ type: "pong" })
    await vi.advanceTimersByTimeAsync(30_000)
    expect(transport.getConnectionSnapshot()).toBe("connected")
  })

  it("online probes healthy sockets without tearing them down", async () => {
    const { transport, socket } = create(true)
    window.dispatchEvent(new Event("online"))
    expect(socket.sent[socket.sent.length - 1]).toEqual({ action: "ping" })
    expect(transport.getConnectionSnapshot()).toBe("connected")
    expect(Socket.instances).toHaveLength(1)
    expect(fetcher).not.toHaveBeenCalled()
  })

  it("cancels readiness and heartbeat work on destroy or terminal authentication", async () => {
    const { transport } = create(true)
    transport.markUnauthorized()
    const count = Socket.instances.length
    window.dispatchEvent(new Event("online"))
    document.dispatchEvent(new Event("visibilitychange"))
    transport.eventStream()
    await vi.advanceTimersByTimeAsync(120_000)
    expect(Socket.instances).toHaveLength(count)
    expect(fetcher).not.toHaveBeenCalled()
    transport.destroy()
    expect(vi.getTimerCount()).toBe(0)
  })
})

describe("WebTransport total HTTP deadline", () => {
  it.each([200, 503])(
    "times out a hung HTTP %s body without retrying the command",
    async (status) => {
      const transport = new WebTransport("http://localhost")
      transports.push(transport)
      fetcher.mockResolvedValue({
        status,
        ok: status === 200,
        text: () => new Promise(() => {}),
        json: () => new Promise(() => {}),
      } as unknown as Response)
      const result = transport
        .call("write_once", {}, { timeoutMs: 100 })
        .catch((error) => error)
      await vi.advanceTimersByTimeAsync(100)
      const error = await Promise.race([
        result,
        Promise.resolve("still pending"),
      ])
      expect(error).toBeInstanceOf(Error)
      expect((error as Error).message).toBe("Request timed out")
      expect(fetcher).toHaveBeenCalledTimes(1)
    }
  )

  it("uses one deadline for fetch and body instead of restarting after headers", async () => {
    const transport = new WebTransport("http://localhost")
    transports.push(transport)
    fetcher.mockImplementation(
      () =>
        new Promise((resolve) =>
          setTimeout(
            () =>
              resolve({
                status: 200,
                ok: true,
                text: () => new Promise(() => {}),
              } as unknown as Response),
            80
          )
        )
    )
    const result = transport
      .call("write_once", {}, { timeoutMs: 100 })
      .catch((error) => error)
    await vi.advanceTimersByTimeAsync(100)
    expect(
      await Promise.race([result, Promise.resolve("still pending")])
    ).toMatchObject({ message: "Request timed out" })
  })

  it.each([200, 503])(
    "normalizes AbortError while consuming HTTP %s body",
    async (status) => {
      const transport = new WebTransport("http://localhost")
      transports.push(transport)
      const abort = () =>
        Promise.reject(new DOMException("Aborted", "AbortError"))
      fetcher.mockResolvedValue({
        status,
        ok: status === 200,
        text: abort,
        json: abort,
      } as unknown as Response)
      await expect(transport.call("write_once")).rejects.toThrow(
        "Request timed out"
      )
      expect(fetcher).toHaveBeenCalledTimes(1)
    }
  )

  it("surfaces 401 immediately without reading the body", async () => {
    const { transport } = create(true)
    fetcher.mockResolvedValue(
      new Response(new ReadableStream(), {
        status: 401,
        headers: { "www-authenticate": 'Bearer realm="codeg"' },
      })
    )
    await expect(transport.call("write_once")).rejects.toThrow("Unauthorized")
    expect(transport.getConnectionSnapshot()).toBe("unauthorized")
    await vi.advanceTimersByTimeAsync(0)
    expect(vi.getTimerCount()).toBe(0)
  })
})

describe("WebTransport branch health classification", () => {
  it.each(["branch_auth_failed", "branch_disabled", "branch_not_found"])(
    "stops retrying definitive %s and preserves headquarters credentials",
    async (code) => {
      const { transport, socket } = create(true)
      fetcher.mockImplementation(async () => branchError(code))
      socket.drop()
      await vi.advanceTimersByTimeAsync(1_000)
      expect(transport.getConnectionSnapshot()).toBe("branchUnavailable")
      expect(transport.getBranchFailure()).toBe(code)
      expect(localStorage.getItem("codeg_token")).toBe("tok")
      await vi.advanceTimersByTimeAsync(120_000)
      expect(fetcher).toHaveBeenCalledTimes(1)
      window.dispatchEvent(new Event("online"))
      expect(fetcher).toHaveBeenCalledTimes(1)
      fetcher.mockImplementation(async () => new Response("{}"))
      transport.reconnectNow()
      expect(transport.getBranchFailure()).toBeNull()
      await vi.advanceTimersByTimeAsync(0)
      latestSocket().open()
      latestSocket().ready()
      expect(transport.getConnectionSnapshot()).toBe("connected")
    }
  )

  it.each([
    "branch_busy",
    "branch_unreachable",
    "branch_timeout",
    "unknown_code",
  ])("keeps retrying temporary %s", async (code) => {
    const { transport, socket } = create(true)
    fetcher.mockImplementation(async () => branchError(code))
    socket.drop()
    await vi.advanceTimersByTimeAsync(3_000)
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    expect(transport.getBranchFailure()).toBeNull()
    expect(fetcher).toHaveBeenCalledTimes(2)
  })

  it("includes a hung branch error body in the eight second health deadline", async () => {
    const { transport, socket } = create(true)
    fetcher.mockResolvedValue({
      status: 503,
      ok: false,
      json: () => new Promise(() => {}),
    } as unknown as Response)
    socket.drop()
    await vi.advanceTimersByTimeAsync(1_000)
    transport.reconnectNow()
    expect(fetcher).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(7_999)
    expect(fetcher).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(transport.getConnectionSnapshot()).toBe("reconnecting")
    await vi.advanceTimersByTimeAsync(2_000)
    expect(fetcher).toHaveBeenCalledTimes(2)
  })

  it("ignores branch error parsing that completes after unauthorized or destroy", async () => {
    const { transport, socket } = create(true)
    let complete!: (value: object) => void
    fetcher.mockResolvedValue({
      status: 503,
      ok: false,
      json: () =>
        new Promise((resolve) => {
          complete = resolve
        }),
    } as unknown as Response)
    socket.drop()
    await vi.advanceTimersByTimeAsync(1_000)
    transport.markUnauthorized()
    complete({ code: "branch_disabled" })
    await vi.advanceTimersByTimeAsync(120_000)
    expect(transport.getConnectionSnapshot()).toBe("unauthorized")
    expect(fetcher).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(0)
  })
})
