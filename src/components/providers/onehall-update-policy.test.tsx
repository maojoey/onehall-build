import { act, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { useEffect } from "react"
import { hydrateRoot } from "react-dom/client"
import { renderToString } from "react-dom/server"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => {
  const key = (byte: string, comment = "test") =>
    btoa(`untrusted comment: ${comment}\n${btoa(`Ed${byte.repeat(40)}`)}\n`)
  const ownKey = key("x")
  const upstreamKey = key("y")
  return {
    desktop: false,
    remote: false,
    call: vi.fn(),
    subscribe: vi.fn(async () => () => {}),
    check: vi.fn(),
    version: vi.fn(async () => "0.39.0"),
    mode: "manual",
    ownKey,
    upstreamKey,
    sameUpstreamKey: key("y", "different comment"),
    relabeledUpstreamKey: btoa(
      `untrusted comment: relabeled\n${btoa(`Ed${"z".repeat(8)}${"y".repeat(32)}`)}\n`
    ),
    pubkey: ownKey,
    endpoints: ["https://updates.example.com/latest.json"],
  }
})
vi.mock("../../../onehall.release.json", () => ({
  default: {
    product: "onehall",
    version: "0.39.1",
    get updateMode() {
      return mocks.mode
    },
    releasesUrl: "https://example.com/releases",
    upstream: {},
  },
}))
vi.mock("../../../src-tauri/tauri.conf.json", () => ({
  default: { plugins: { updater: { pubkey: mocks.upstreamKey } } },
}))
vi.mock("../../../src-tauri/tauri.hall.conf.json", () => ({
  default: {
    plugins: {
      updater: {
        get pubkey() {
          return mocks.pubkey
        },
        get endpoints() {
          return mocks.endpoints
        },
      },
    },
  },
}))
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: mocks.call, subscribe: mocks.subscribe }),
  isDesktop: () => mocks.desktop,
  isRemoteDesktopMode: () => mocks.remote,
  getActiveRemoteConnectionId: () => (mocks.remote ? 7 : null),
}))
vi.mock("@tauri-apps/api/app", () => ({ getVersion: mocks.version }))
vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }))
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }))

import {
  UpdateProvider,
  useAppUpdate,
  type UpdateContextValue,
} from "./update-provider"
import en from "@/i18n/messages/en.json"

let current: UpdateContextValue | null = null
function Probe() {
  const value = useAppUpdate()
  useEffect(() => {
    current = value
  }, [value])
  return <span>{value?.currentVersion}</span>
}
function mount() {
  return render(
    <NextIntlClientProvider locale="en" messages={en}>
      <UpdateProvider>
        <Probe />
      </UpdateProvider>
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  current = null
  localStorage.clear()
  vi.clearAllMocks()
  mocks.mode = "manual"
  mocks.pubkey = mocks.ownKey
  mocks.endpoints = ["https://updates.example.com/latest.json"]
  mocks.call.mockImplementation(async (command) => {
    if (command === "app_update_status") {
      return { currentVersion: "0.39.0", selfUpdateSupported: true }
    }
    if (command === "app_update_state") return { seq: 0, status: "idle" }
    if (command === "check_app_update") {
      return { currentVersion: "0.39.0", update: { version: "9.0.0" } }
    }
    throw new Error(`Unexpected update command: ${command}`)
  })
})
afterEach(() => vi.useRealTimers())

describe("OneHall production update policy", () => {
  it.each([
    ["missing key", "", ["https://updates.example.com/latest.json"]],
    ["malformed key", "malformed", ["https://updates.example.com/latest.json"]],
    [
      "upstream key",
      mocks.upstreamKey,
      ["https://updates.example.com/latest.json"],
    ],
    [
      "re-encoded upstream key",
      mocks.sameUpstreamKey,
      ["https://updates.example.com/latest.json"],
    ],
    [
      "insecure endpoint",
      mocks.ownKey,
      ["http://updates.example.com/latest.json"],
    ],
    ["missing endpoint", mocks.ownKey, []],
    [
      "relabeled upstream key id",
      mocks.relabeledUpstreamKey,
      ["https://updates.example.com/latest.json"],
    ],
    [
      "cross-origin fallback",
      mocks.ownKey,
      [
        "https://updates.example.com/latest.json",
        "https://other.example.com/latest.json",
      ],
    ],
  ])("stays manual for %s", async (_name, key, endpoints) => {
    mocks.mode = "desktop-signed"
    mocks.desktop = true
    mocks.remote = false
    mocks.pubkey = key
    mocks.endpoints = endpoints
    mount()
    await screen.findByText("0.39.0")
    expect(current?.manualOnly).toBe(true)
    expect(mocks.check).not.toHaveBeenCalled()
  })

  it("renders the manual policy during static export before detecting a desktop", () => {
    mocks.mode = "desktop-signed"
    mocks.desktop = true
    mocks.remote = false
    function PolicyProbe() {
      return <span>{useAppUpdate()?.manualOnly ? "manual" : "managed"}</span>
    }
    const html = renderToString(
      <NextIntlClientProvider locale="en" messages={en}>
        <UpdateProvider>
          <PolicyProbe />
        </UpdateProvider>
      </NextIntlClientProvider>
    )
    expect(html).toContain("manual")
  })

  it("does not remount the workspace while enabling the desktop updater after hydration", async () => {
    mocks.mode = "desktop-signed"
    mocks.desktop = true
    mocks.remote = false
    const onMount = vi.fn()
    function Workspace() {
      useEffect(() => {
        onMount()
      }, [])
      return <Probe />
    }
    const tree = (
      <NextIntlClientProvider locale="en" messages={en}>
        <UpdateProvider>
          <Workspace />
        </UpdateProvider>
      </NextIntlClientProvider>
    )
    const container = document.createElement("div")
    container.innerHTML = renderToString(tree)
    document.body.append(container)
    let root!: ReturnType<typeof hydrateRoot>
    await act(async () => {
      root = hydrateRoot(container, tree)
    })
    await waitFor(() => expect(current?.canInstallInPlace).toBe(true))
    expect(onMount).toHaveBeenCalledTimes(1)
    await act(async () => root.unmount())
    container.remove()
  })

  it("checks the OneHall updater on the local desktop when signed updates are enabled", async () => {
    mocks.mode = "desktop-signed"
    mocks.desktop = true
    mocks.remote = false
    const original = mocks.call.getMockImplementation()!
    mocks.call.mockImplementation(async (command) =>
      command === "check_app_update"
        ? {
            currentVersion: "0.39.0",
            update: { version: "0.39.2", body: "OneHall update" },
          }
        : original(command)
    )
    localStorage.setItem(
      "codeg.updateCheck.last",
      JSON.stringify({
        at: Date.now(),
        currentVersion: "0.39.0",
        info: { version: "9.0.0", body: "Codeg release" },
      })
    )
    mount()
    await screen.findByText("0.39.0")
    await waitFor(() => expect(current?.canInstallInPlace).toBe(true))
    expect(current?.available).toBeNull()
    await act(async () => current!.checkNow())
    await waitFor(() => expect(current?.available?.version).toBe("0.39.2"))
    expect(mocks.check).not.toHaveBeenCalled()
    expect(mocks.call.mock.calls.map(([name]) => name)).toContain(
      "check_app_update"
    )
  })

  it.each([
    ["web", false, false],
    ["remote desktop", true, true],
  ])(
    "keeps %s manual even when signed desktop updates are enabled",
    async (_name, desktop, remote) => {
      mocks.mode = "desktop-signed"
      mocks.desktop = desktop
      mocks.remote = remote
      mount()
      await screen.findByText("0.39.0")
      expect(current?.manualOnly).toBe(true)
      await act(async () => current!.checkNow())
      expect(mocks.check).not.toHaveBeenCalled()
      expect(mocks.call.mock.calls.map(([name]) => name)).not.toContain(
        "check_app_update"
      )
    }
  )

  it("never sends a stale desktop update action to a switched remote backend", async () => {
    mocks.mode = "desktop-signed"
    mocks.desktop = true
    mocks.remote = false
    mount()
    await screen.findByText("0.39.0")
    await waitFor(() => expect(current?.canInstallInPlace).toBe(true))
    const desktopActions = current!
    mocks.remote = true
    await act(async () => {
      await desktopActions.startUpdate()
      await desktopActions.restart()
      await desktopActions.checkNow()
      await desktopActions.rollback()
    })
    const calls = mocks.call.mock.calls.map(([name]) => name)
    for (const action of [
      "perform_app_update",
      "restart_app",
      "check_app_update",
      "rollback_app",
    ]) {
      expect(calls).not.toContain(action)
    }
  })

  it("does not fetch a remote snapshot when the desktop subscription finishes after a switch", async () => {
    mocks.mode = "desktop-signed"
    mocks.desktop = true
    mocks.remote = false
    let resolveSubscribe!: (value: () => void) => void
    mocks.subscribe.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveSubscribe = resolve
        })
    )
    mount()
    await waitFor(() => expect(mocks.subscribe).toHaveBeenCalled())
    mocks.remote = true
    await act(async () => resolveSubscribe(() => {}))
    expect(mocks.call.mock.calls.map(([name]) => name)).not.toContain(
      "app_update_state"
    )
  })

  it("does not check a remote backend if the switch happens during a desktop refresh", async () => {
    mocks.mode = "desktop-signed"
    mocks.desktop = true
    mocks.remote = false
    let resolveVersion!: (value: string) => void
    mocks.version.mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          resolveVersion = resolve
        })
    )
    mount()
    await waitFor(() => expect(mocks.version).toHaveBeenCalled())
    let check!: Promise<void>
    await act(async () => {
      check = current!.checkNow()
      mocks.remote = true
      resolveVersion("0.39.0")
      await check
    })
    expect(mocks.call.mock.calls.map(([name]) => name)).not.toContain(
      "check_app_update"
    )
  })

  it.each([
    ["web", false, false],
    ["local desktop", true, false],
    ["desktop connected to an older server", true, true],
  ])("ignores old Codeg offers in %s", async (_name, desktop, remote) => {
    mocks.desktop = desktop
    mocks.remote = remote
    localStorage.setItem(
      `codeg.updateCheck.last${remote ? ":remote-7" : ""}`,
      JSON.stringify({
        at: Date.now(),
        currentVersion: "0.39.0",
        info: { version: "9.0.0", body: "Codeg release" },
      })
    )
    mount()
    await screen.findByText("0.39.0")
    expect(current?.available).toBeNull()
    expect(current?.manualOnly).toBe(true)
    expect(current?.canInstallInPlace).toBe(false)
    expect(mocks.subscribe).not.toHaveBeenCalled()
    expect(mocks.check).not.toHaveBeenCalled()
    await expect(current!.startUpdate()).rejects.toThrow(/manual/i)
    await expect(current!.restart()).rejects.toThrow(/manual/i)
    await expect(current!.rollback()).rejects.toThrow(/manual/i)
    expect(mocks.call.mock.calls.map(([name]) => name)).toEqual(
      desktop && !remote ? [] : ["app_update_status"]
    )
  })

  it("does not poll the upstream after time passes or the window refocuses", async () => {
    mocks.desktop = false
    mocks.remote = false
    vi.useFakeTimers()
    mount()
    await act(async () => {})
    await act(async () => {
      await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000)
      window.dispatchEvent(new Event("focus"))
    })
    expect(mocks.call.mock.calls.map(([name]) => name)).toEqual([
      "app_update_status",
    ])
    expect(mocks.check).not.toHaveBeenCalled()
  })
})
