import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { WebTransport } from "./web-transport"
import type { AttachHandlers, Transport } from "./types"
import {
  bridgeStatus,
  bridgeStatusSnapshot,
  bridgeOpen,
  bridgeClose,
  resetBridgeStatusForTests,
} from "@/lib/browser/browser-bridge"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  beginBackendSwitch,
  configureWebBranchTransport,
  getActiveBackendKey,
  getServerBaseUrl,
  getShellTransport,
  getTransport,
  captureSubmissionBackend,
  isRemoteDesktopMode,
} from "./index"

const branch = "11111111-1111-4111-8111-111111111111"
const other = "22222222-2222-4222-8222-222222222222"

beforeEach(() => {
  __resetTransportForTests()
  __setTransportFactoriesForTests({ web: (url) => new WebTransport(url) })
  localStorage.setItem("codeg_token", "hq-test-credential")
  resetBridgeStatusForTests()
})
afterEach(() => {
  __resetTransportForTests()
  vi.unstubAllGlobals()
  localStorage.clear()
})

it("routes branch calls to headquarters prefix while discovery stays at headquarters", async () => {
  const fetcher = vi
    .fn()
    .mockImplementation(() => Promise.resolve(new Response("[]")))
  vi.stubGlobal("fetch", fetcher)
  configureWebBranchTransport(branch)
  expect(isRemoteDesktopMode()).toBe(false)
  expect(getActiveBackendKey()).toBe(`branch:${branch}`)
  expect(getServerBaseUrl()).toBe(`${location.origin}/branches/${branch}`)
  await getTransport().call("list_all_conversations")
  await getShellTransport().call("hall_list_branches")
  expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
    `${location.origin}/branches/${branch}/api/list_all_conversations`,
    `${location.origin}/api/hall_list_branches`,
  ])
  expect(fetcher.mock.calls[0][1].headers.Authorization).toBe(
    "Bearer hq-test-credential"
  )
})

it("preserves a captured submission target while invalidating old realm results", async () => {
  let finish!: (v: Response) => void
  const fetcher = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r
        })
    )
    .mockImplementation(() => Promise.resolve(new Response("[]")))
  vi.stubGlobal("fetch", fetcher)
  configureWebBranchTransport(branch)
  const received = vi.fn()
  void getTransport().call("list_all_conversations").then(received)
  const submitted = captureSubmissionBackend()
  beginBackendSwitch()
  configureWebBranchTransport(other)
  finish(new Response('[{"id":1,"title":"old branch"}]'))
  await submitted.call("create_conversation")
  await getTransport().call("list_all_conversations")
  await new Promise((r) => setTimeout(r, 0))
  expect(received).not.toHaveBeenCalled()
  expect(submitted.key).toBe(`branch:${branch}`)
  expect(submitted.isCurrent()).toBe(false)
  expect(fetcher.mock.calls[1][0]).toContain(`/branches/${branch}/`)
  expect(fetcher.mock.calls[2][0]).toContain(`/branches/${other}/`)
  beginBackendSwitch()
  expect(getActiveBackendKey()).toBe("local")
  expect(getServerBaseUrl()).toBe(location.origin)
})

it("rejects selectors that could escape the branch path", () => {
  for (const id of [
    "../api",
    "a/b",
    "https://example.com",
    "",
    branch + "?x=1",
  ]) {
    expect(() => configureWebBranchTransport(id)).toThrow()
  }
})

it("mutes late attach snapshots and replay after a branch switch", () => {
  let handlers!: AttachHandlers
  const fake: Transport = {
    call: async <T>() => [] as T,
    subscribe: async () => () => {},
    isDesktop: () => false,
    eventStream: () => ({
      attach: (_id, _options, next) => {
        handlers = next
        return { subscriptionId: "same", detach: () => {} }
      },
    }),
  }
  __setTransportFactoriesForTests({ web: () => fake })
  configureWebBranchTransport(branch)
  const received = vi.fn()
  getTransport().eventStream?.().attach(
    "same",
    {},
    {
      onSnapshot: received,
      onReplay: received,
      onEvent: received,
      onDetached: received,
    }
  )
  beginBackendSwitch()
  configureWebBranchTransport(other)
  handlers.onReplay([], 1)
  handlers.onDetached("connection_gone")
  expect(received).not.toHaveBeenCalled()
})

it("does not reuse a headquarters preview origin inside a branch", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        new Response(
          '{"enabled":true,"ports":[],"publicHost":null,"hostPattern":"auto"}'
        )
      )
    )
  )
  expect((await bridgeStatus()).enabled).toBe(true)
  configureWebBranchTransport(branch)
  expect((await bridgeStatus()).enabled).toBe(false)
  expect(bridgeStatusSnapshot()?.enabled).toBe(false)
})

it("releases a pending headquarters preview on its original backend after switching", async () => {
  let finish!: (r: Response) => void
  const fetcher = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r
        })
    )
    .mockImplementation(() => Promise.resolve(new Response("null")))
  vi.stubGlobal("fetch", fetcher)
  const opening = bridgeOpen("http://localhost:3001", "hold-test")
  beginBackendSwitch()
  configureWebBranchTransport(branch)
  finish(new Response('{"targetPort":3001}'))
  await opening
  await bridgeClose("hold-test")
  expect(fetcher.mock.calls[1][0]).toBe(
    `${location.origin}/api/browser_bridge_close`
  )
})

it("starts a fresh HQ status request after returning while the old request is unresolved", async () => {
  const responses: Array<(response: Response) => void> = []
  const fetcher = vi.fn(
    () => new Promise<Response>((resolve) => responses.push(resolve))
  )
  vi.stubGlobal("fetch", fetcher)
  const oldStatus = bridgeStatus()
  beginBackendSwitch()
  configureWebBranchTransport(branch)
  expect((await bridgeStatus()).enabled).toBe(false)
  beginBackendSwitch()
  expect(bridgeStatusSnapshot()).toBeNull()
  const freshStatus = bridgeStatus()
  expect(fetcher).toHaveBeenCalledTimes(2)
  responses[1](
    new Response(
      '{"enabled":true,"ports":[3081],"publicHost":null,"hostPattern":null}'
    )
  )
  expect((await freshStatus).ports).toEqual([3081])
  responses[0](
    new Response(
      '{"enabled":true,"ports":[9999],"publicHost":null,"hostPattern":null}'
    )
  )
  expect((await oldStatus).enabled).toBe(false)
  expect(bridgeStatusSnapshot()?.ports).toEqual([3081])
})

it("keeps status retries on their original HQ without replacing the new HQ cache", async () => {
  vi.useFakeTimers()
  try {
    const urls: string[] = []
    const fetcher = vi.fn(async (url: string) => {
      urls.push(url)
      if (urls.length === 1) throw new Error("HQ starting")
      return new Response(
        '{"enabled":true,"ports":[3081],"publicHost":null,"hostPattern":null}'
      )
    })
    vi.stubGlobal("fetch", fetcher)
    const oldStatus = bridgeStatus()
    await Promise.resolve()
    beginBackendSwitch()
    configureWebBranchTransport(branch)
    await vi.advanceTimersByTimeAsync(1000)
    expect((await oldStatus).enabled).toBe(false)
    expect(urls).toEqual([
      `${location.origin}/api/browser_bridge_status`,
      `${location.origin}/api/browser_bridge_status`,
    ])
    expect((await bridgeStatus()).enabled).toBe(false)
    beginBackendSwitch()
    expect((await bridgeStatus()).ports).toEqual([3081])
    expect(fetcher).toHaveBeenCalledTimes(3)
  } finally {
    vi.useRealTimers()
  }
})
