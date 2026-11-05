import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

/**
 * onehall: an opened workspace tab keeps its connection until the tab is
 * closed or the program exits. Switching tabs only re-routes `getTransport()`;
 * it must neither destroy nor re-create the transport behind it.
 */

import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  beginBackendSwitch,
  configureRemoteDesktopTransport,
  configureWebBranchTransport,
  getTransport,
  releaseRemoteDesktopTransport,
  releaseWebBranchTransport,
  releaseAllWorkspaceTransports,
  isRemoteDesktopMode,
} from "@/lib/transport"
import type { RemoteTransportConfig, Transport } from "./types"

const made: {
  id: string
  destroyed: boolean
  config?: RemoteTransportConfig
}[] = []

function fake(id: string, config?: RemoteTransportConfig): Transport {
  const rec = { id, destroyed: false, config }
  made.push(rec)
  return {
    call: async () => undefined as never,
    subscribe: async () => () => {},
    isDesktop: () => true,
    destroy: () => {
      rec.destroyed = true
    },
  } as unknown as Transport
}

const cfg = (
  over: Partial<RemoteTransportConfig> = {}
): RemoteTransportConfig => ({
  id: 7,
  name: "A",
  baseUrl: "https://a.example",
  token: "t1",
  windowInstanceId: "w7",
  ...over,
})

const BRANCH = "11111111-1111-4111-8111-111111111111"

beforeEach(() => {
  made.length = 0
  __setTransportFactoriesForTests({
    tauri: () => fake("shell"),
    remote: (c) => fake(`remote:${c.id}`, c),
    web: (url) => fake(`web:${url}`),
  })
  __resetTransportForTests()
  made.length = 0
})
afterEach(() => {
  __resetTransportForTests()
})

describe("persistent workspace transports", () => {
  it("keeps a remote transport alive across a backend switch and reuses it", () => {
    configureRemoteDesktopTransport(cfg())
    const first = made.find((m) => m.id === "remote:7")!
    beginBackendSwitch()
    expect(first.destroyed).toBe(false)
    expect(isRemoteDesktopMode()).toBe(false) // routed to the shell meanwhile
    configureRemoteDesktopTransport(cfg())
    expect(made.filter((m) => m.id === "remote:7")).toHaveLength(1)
    expect(first.destroyed).toBe(false)
    expect(isRemoteDesktopMode()).toBe(true)
  })

  it("keeps two remotes alive while a third backend is shown", () => {
    configureRemoteDesktopTransport(cfg({ id: 7 }))
    beginBackendSwitch()
    configureRemoteDesktopTransport(cfg({ id: 8, windowInstanceId: "w8" }))
    beginBackendSwitch()
    expect(made.filter((m) => m.destroyed)).toHaveLength(0)
  })

  it("rebuilds only when the credentials or address changed", () => {
    configureRemoteDesktopTransport(cfg())
    beginBackendSwitch()
    configureRemoteDesktopTransport(cfg({ token: "t2" }))
    const sevens = made.filter((m) => m.id === "remote:7")
    expect(sevens).toHaveLength(2)
    expect(sevens[0].destroyed).toBe(true)
    expect(sevens[1].destroyed).toBe(false)
  })

  it("releases exactly one remote on tab close", () => {
    configureRemoteDesktopTransport(cfg({ id: 7 }))
    beginBackendSwitch()
    configureRemoteDesktopTransport(cfg({ id: 8, windowInstanceId: "w8" }))
    releaseRemoteDesktopTransport(7)
    expect(made.find((m) => m.id === "remote:7")!.destroyed).toBe(true)
    expect(made.find((m) => m.id === "remote:8")!.destroyed).toBe(false)
    releaseRemoteDesktopTransport(7) // idempotent
  })

  it("delivers a 401 that arrived while the tab was in the background", () => {
    const first = vi.fn()
    configureRemoteDesktopTransport(cfg({ onUnauthorized: first }))
    const wired = made.find((m) => m.id === "remote:7")!.config!
    beginBackendSwitch()
    wired.onUnauthorized?.() // hidden tab's token is refused
    expect(first).not.toHaveBeenCalled() // the old gate is gone
    const second = vi.fn()
    configureRemoteDesktopTransport(cfg({ onUnauthorized: second }))
    expect(second).toHaveBeenCalledTimes(1)
  })

  it("routes a live 401 to the newest gate only", () => {
    const first = vi.fn()
    const second = vi.fn()
    configureRemoteDesktopTransport(cfg({ onUnauthorized: first }))
    const wired = made.find((m) => m.id === "remote:7")!.config!
    beginBackendSwitch()
    configureRemoteDesktopTransport(cfg({ onUnauthorized: second }))
    wired.onUnauthorized?.()
    expect(first).not.toHaveBeenCalled()
    expect(second).toHaveBeenCalledTimes(1)
  })

  it("keeps a browser branch transport across switches and releases it on close", () => {
    vi.stubGlobal("window", {
      location: { origin: "https://hq.example" },
    } as unknown as Window)
    // browser mode: not desktop
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
    configureWebBranchTransport(BRANCH)
    beginBackendSwitch()
    configureWebBranchTransport(BRANCH)
    const webs = made.filter((m) => m.id.startsWith("web:"))
    expect(webs).toHaveLength(1)
    expect(webs[0].destroyed).toBe(false)
    releaseWebBranchTransport(BRANCH)
    expect(webs[0].destroyed).toBe(true)
    vi.unstubAllGlobals()
  })

  it("destroys everything on full teardown", () => {
    configureRemoteDesktopTransport(cfg())
    getTransport()
    releaseAllWorkspaceTransports()
    expect(
      made.filter((m) => m.id.startsWith("remote:") && !m.destroyed)
    ).toHaveLength(0)
  })
})
