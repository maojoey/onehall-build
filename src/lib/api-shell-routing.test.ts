import { beforeEach, describe, expect, it, vi } from "vitest"

/**
 * onehall: in single-window mode the main window's `getTransport()` points at
 * whichever backend the active workspace tab is bound to — a remote hall
 * server while the remote tab is active. Closing the window, its close
 * behavior, autostart and rendering are about the desktop app itself, so they
 * must always reach the local shell; sent to the remote server they fail
 * with `not_implemented` and the close button stops working (G1).
 */

const mocks = vi.hoisted(() => ({
  remoteCall: vi.fn(),
  remoteSubscribe: vi.fn(),
  shellCall: vi.fn(),
  shellSubscribe: vi.fn(),
  remoteActive: true,
}))

vi.mock("@/lib/transport", () => ({
  // Mirrors `getTransport()`: the remote transport when a remote tab is
  // active, otherwise the shell.
  getTransport: () =>
    mocks.remoteActive
      ? { call: mocks.remoteCall, subscribe: mocks.remoteSubscribe }
      : { call: mocks.shellCall, subscribe: mocks.shellSubscribe },
  getShellTransport: () => ({
    call: mocks.shellCall,
    subscribe: mocks.shellSubscribe,
  }),
  isDesktop: () => true,
  isRemoteDesktopMode: () => mocks.remoteActive,
  getActiveRemoteConnectionId: () => (mocks.remoteActive ? 1 : null),
  notifyRemoteDesktopUnauthorized: vi.fn(),
}))

import {
  getCliAccessStatus,
  getCliToolStatus,
  installCliTool,
  setCliAccessEnabled,
  getSystemAutostartSettings,
  getSystemCloseBehaviorSettings,
  getSystemRenderingSettings,
  listenCloseRequest,
  resolveCloseRequest,
  runLocalCommand,
  updateSystemAutostartSettings,
  updateSystemCloseBehaviorSettings,
  updateSystemRenderingSettings,
} from "@/lib/api"

beforeEach(() => {
  for (const fn of [
    mocks.remoteCall,
    mocks.remoteSubscribe,
    mocks.shellCall,
    mocks.shellSubscribe,
  ]) {
    fn.mockReset()
  }
  mocks.shellCall.mockResolvedValue(undefined)
  mocks.shellSubscribe.mockResolvedValue(() => {})
  mocks.remoteCall.mockRejectedValue({
    code: "not_implemented",
    message: "API endpoint is not available in web mode",
  })
})

describe.each([
  ["remote tab active", true],
  ["local tab active", false],
])("close prompt with the %s", (_label, remoteActive) => {
  beforeEach(() => {
    mocks.remoteActive = remoteActive
  })

  it("keeps CLI access and command installation on this computer", async () => {
    await getCliAccessStatus()
    await setCliAccessEnabled(false)
    await getCliToolStatus()
    await installCliTool()
    expect(mocks.shellCall.mock.calls).toEqual([
      ["cli_access_status"],
      ["cli_access_set_enabled", { enabled: false }],
      ["cli_tool_status"],
      ["cli_tool_install"],
    ])
    expect(mocks.remoteCall).not.toHaveBeenCalled()
  })

  it("runs an explicitly local command on this computer from either tab", async () => {
    await runLocalCommand("printf test", "/remote/repo", "hall.example", null)
    expect(mocks.shellCall).toHaveBeenCalledWith("run_local_command", {
      command: "printf test",
      folderPath: "/remote/repo",
      remoteHost: "hall.example",
      workingDir: null,
    })
    expect(mocks.remoteCall).not.toHaveBeenCalled()
  })

  it.each(["exit", "minimize", "cancel"] as const)(
    "answers %s on the local shell",
    async (action) => {
      await expect(resolveCloseRequest(action, false)).resolves.toBeUndefined()
      expect(mocks.shellCall).toHaveBeenCalledWith("resolve_close_request", {
        action,
        remember: false,
      })
      expect(mocks.remoteCall).not.toHaveBeenCalled()
    }
  )

  it("listens for close presses on the local shell", async () => {
    const handler = vi.fn()
    await listenCloseRequest(handler)
    expect(mocks.shellSubscribe).toHaveBeenCalledWith(
      "app://close-request",
      handler
    )
    expect(mocks.remoteSubscribe).not.toHaveBeenCalled()
  })
})

describe("desktop-only settings with a remote tab active", () => {
  beforeEach(() => {
    mocks.remoteActive = true
  })

  it("reads and writes them on the local shell", async () => {
    await getSystemCloseBehaviorSettings()
    await updateSystemCloseBehaviorSettings("ask")
    await getSystemAutostartSettings()
    await updateSystemAutostartSettings({ enabled: false } as never)
    await getSystemRenderingSettings()
    await updateSystemRenderingSettings({} as never)
    expect(mocks.shellCall.mock.calls.map(([command]) => command)).toEqual([
      "get_system_close_behavior_settings",
      "update_system_close_behavior_settings",
      "get_system_autostart_settings",
      "update_system_autostart_settings",
      "get_system_rendering_settings",
      "update_system_rendering_settings",
    ])
    expect(mocks.remoteCall).not.toHaveBeenCalled()
  })
})
