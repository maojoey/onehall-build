import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { acpConnect } from "./api"
import { WebTransport } from "./transport/web-transport"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
} from "./transport"

beforeEach(() => {
  vi.useFakeTimers()
  __resetTransportForTests()
  __setTransportFactoriesForTests({ web: (url) => new WebTransport(url) })
  localStorage.setItem("codeg_token", "test-token")
})
afterEach(() => {
  __resetTransportForTests()
  vi.unstubAllGlobals()
  vi.useRealTimers()
  localStorage.clear()
})

it("keeps an ACP restore response after the backend's 60 second handshake wait", async () => {
  let finish!: (response: Response) => void
  vi.stubGlobal(
    "fetch",
    (_url: string, init: RequestInit) =>
      new Promise<Response>((resolve, reject) => {
        finish = resolve
        init.signal!.addEventListener("abort", () =>
          reject(new DOMException("Aborted", "AbortError"))
        )
      })
  )
  const result = acpConnect("claude_code", "/tmp/project", "session-1").then(
    (value) => ({ value }),
    (error) => ({ error: error.message })
  )
  await vi.advanceTimersByTimeAsync(61_000)
  finish(new Response('"restored-connection"'))
  expect(await result).toEqual({ value: "restored-connection" })
})

it("ends an unanswered connect at its own deadline without replaying it", async () => {
  let requests = 0
  vi.stubGlobal("fetch", (_url: string, init: RequestInit) => {
    requests++
    return new Promise((_resolve, reject) => {
      init.signal!.addEventListener("abort", () =>
        reject(new DOMException("Aborted", "AbortError"))
      )
    })
  })
  const result = acpConnect("claude_code").catch((error) => error.message)
  await vi.advanceTimersByTimeAsync(75_000)
  expect(await result).toBe("Request timed out")
  expect(requests).toBe(1)
})

it("rejects a malformed successful connect response instead of accepting a non-ID", async () => {
  vi.stubGlobal("fetch", async () => new Response('{"unexpected":"value"}'))
  await expect(acpConnect("claude_code")).rejects.toBeInstanceOf(SyntaxError)
})
