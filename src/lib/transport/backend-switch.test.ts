import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

/**
 * onehall: single-window mode swaps the backend behind `getTransport()` in
 * place. `beginBackendSwitch()` must (1) put the shell back in front so the
 * next realm's first fetches cannot go to the backend just left, and (2)
 * drop answers and events that belong to the previous backend, however late
 * they arrive (Mac acceptance G3: the local tab showed the remote's folders).
 */

type Pending = {
  command: string
  resolve: (v: unknown) => void
  reject: (e: unknown) => void
}

// The real Tauri / remote-desktop transports, over a fake Tauri IPC
// (`window.__TAURI_INTERNALS__`): plain commands
// are the local shell, `remote_http_call` is the remote server.
const fake = vi.hoisted(() => ({
  shell: [] as Pending[],
  remote: [] as Pending[],
  callbacks: new Map<number, (e: { payload: unknown }) => void>(),
  listeners: new Map<string, number>(),
  nextId: 1,
}))

const fakeInternals = {
  transformCallback: (cb: (e: { payload: unknown }) => void) => {
    const id = fake.nextId++
    fake.callbacks.set(id, cb)
    return id
  },
  invoke: (command: string, args: Record<string, unknown> = {}) => {
    if (command === "plugin:event|listen") {
      fake.listeners.set(args.event as string, args.handler as number)
      return Promise.resolve(fake.nextId++)
    }
    if (command.startsWith("plugin:") || command.startsWith("remote_ws_")) {
      return Promise.resolve(undefined)
    }
    return new Promise((resolve, reject) => {
      if (command === "remote_http_call") {
        fake.remote.push({ command: args.command as string, resolve, reject })
      } else {
        fake.shell.push({ command, resolve, reject })
      }
    })
  },
}

/** Deliver a Tauri event to whoever listens on the shell. */
function emit(event: string, payload: unknown) {
  const id = fake.listeners.get(event)
  if (id !== undefined) fake.callbacks.get(id)?.({ payload })
}

/** Let the transports' dynamic imports and IPC hops run. */
async function flush() {
  for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0))
}

import { RemoteDesktopTransport } from "./remote-desktop-transport"
import { TauriTransport } from "./tauri-transport"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  beginBackendSwitch,
  configureRemoteDesktopTransport,
  getTransport,
  isRemoteDesktopMode,
} from "./index"

const REMOTE = {
  id: 7,
  name: "堂务",
  baseUrl: "https://hall.example",
  token: "t",
  windowInstanceId: "w",
}

/** Settled within a few macrotasks, or still pending. */
async function state(p: Promise<unknown>) {
  let out: string = "pending"
  p.then(
    () => (out = "resolved"),
    () => (out = "rejected")
  )
  await flush()
  return out
}

beforeEach(() => {
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ =
    fakeInternals
  __setTransportFactoriesForTests({
    tauri: () => new TauriTransport(),
    remote: (config) => new RemoteDesktopTransport(config),
  })
  __resetTransportForTests()
  fake.shell.length = 0
  fake.remote.length = 0
  fake.callbacks.clear()
  fake.listeners.clear()
})

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
})

describe("beginBackendSwitch", () => {
  it("routes the next realm's first call to the shell, not the remote just left", async () => {
    configureRemoteDesktopTransport(REMOTE)
    expect(isRemoteDesktopMode()).toBe(true)
    beginBackendSwitch()
    expect(isRemoteDesktopMode()).toBe(false)
    void getTransport().call("list_folders")
    await flush()
    expect(fake.shell.map((c) => c.command)).toEqual(["list_folders"])
    expect(fake.remote).toHaveLength(0)
  })

  it("drops a slow remote answer that arrives after switching to local", async () => {
    configureRemoteDesktopTransport(REMOTE)
    const slow = getTransport().call("list_folders")
    beginBackendSwitch()
    const local = getTransport().call("list_folders")
    await flush()
    fake.remote[0].resolve([{ id: 1, path: "/home/ubuntu/demo" }])
    fake.shell[0].resolve([])
    expect(await state(slow)).toBe("pending")
    await expect(local).resolves.toEqual([])
  })

  it("drops a slow local answer that arrives after switching to a remote", async () => {
    const slow = getTransport().call("list_folders")
    beginBackendSwitch()
    configureRemoteDesktopTransport(REMOTE)
    const remote = getTransport().call("list_folders")
    await flush()
    fake.shell[0].resolve([{ id: 2, path: "/Users/me/local" }])
    fake.remote[0].resolve([{ id: 1, path: "/home/ubuntu/demo" }])
    expect(await state(slow)).toBe("pending")
    await expect(remote).resolves.toEqual([
      { id: 1, path: "/home/ubuntu/demo" },
    ])
  })

  it("swallows the old backend's errors too, so no stale toast shows up", async () => {
    configureRemoteDesktopTransport(REMOTE)
    const slow = getTransport().call("list_folders")
    beginBackendSwitch()
    await flush()
    fake.remote[0].reject(new Error("socket closed"))
    expect(await state(slow)).toBe("pending")
  })

  it("keeps calls and errors of the current backend untouched", async () => {
    const ok = getTransport().call("a")
    const bad = getTransport().call("b")
    await flush()
    fake.shell[0].resolve(1)
    fake.shell[1].reject(new Error("boom"))
    await expect(ok).resolves.toBe(1)
    await expect(bad).rejects.toThrow("boom")
  })

  it("mutes event subscriptions made before the switch", async () => {
    const before = vi.fn()
    await getTransport().subscribe("folder://changed", before)
    beginBackendSwitch()
    const after = vi.fn()
    await getTransport().subscribe("folder://other", after)
    emit("folder://changed", { id: 1 })
    emit("folder://other", { id: 2 })
    expect(before).not.toHaveBeenCalled()
    expect(after).toHaveBeenCalledWith({ id: 2 })
  })

  it("hands back the same wrapper for the same backend", () => {
    expect(getTransport()).toBe(getTransport())
  })
})
