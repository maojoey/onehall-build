import { afterEach, describe, expect, it, vi } from "vitest"

const platform = vi.hoisted(() => ({
  desktop: false,
  win: null as null | { label: string; close: () => Promise<void> },
}))
vi.mock("@/lib/platform", async (original) => ({
  ...(await original<typeof import("@/lib/platform")>()),
  isDesktop: () => platform.desktop,
  getCurrentWindow: async () => platform.win,
}))

import { backToChat, settingsNavItemsFor } from "./settings-shell"

describe("settingsNavItemsFor", () => {
  it("drops the Web service page in a browser session and nothing else", () => {
    const labels = settingsNavItemsFor("web").map((item) => item.labelKey)
    // There is no second server to configure from inside the one you are
    // already talking to.
    expect(labels).not.toContain("web_service")
    // The Browser page is mostly a desktop browser engine, but site rules,
    // what a server starting in a terminal does and the terminal's link menu
    // all still decide something here — and this page is the only place they
    // can be set, so the entry has to lead there.
    expect(labels).toContain("browser")
    expect(labels).toEqual(
      settingsNavItemsFor("tauri")
        .map((item) => item.labelKey)
        .filter((label) => label !== "web_service")
    )
  })

  it("keeps collaboration under General and the two Use pages together", () => {
    const labels = settingsNavItemsFor("tauri").map((item) => item.labelKey)
    const after = (label: (typeof labels)[number]) =>
      labels[labels.indexOf(label) + 1]
    expect(after("general")).toBe("collaboration")
    expect(after("quick_messages")).toBe("browser")
    expect(after("browser")).toBe("computer_use")
  })

  it("keeps every page on the desktop", () => {
    const labels = settingsNavItemsFor("tauri").map((item) => item.labelKey)
    expect(labels).toContain("web_service")
    expect(labels).toContain("browser")
  })
})

describe("backToChat", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    platform.desktop = false
    platform.win = null
  })

  it("closes the desktop settings window", async () => {
    const close = vi.fn().mockResolvedValue(undefined)
    platform.desktop = true
    platform.win = { label: "remote-settings-1", close }
    await backToChat()
    expect(close).toHaveBeenCalled()
  })

  it("never closes the main window; it goes back to the workspace in place", async () => {
    const close = vi.fn().mockResolvedValue(undefined)
    const assign = vi.fn()
    platform.desktop = true
    platform.win = { label: "main", close }
    vi.stubGlobal("window", {
      ...window,
      location: { ...window.location, search: "?remoteConnectionId=1", assign },
    })
    await backToChat()
    expect(close).not.toHaveBeenCalled()
    expect(assign).toHaveBeenCalledWith("/workspace")
  })

  it("closes a settings window the app opened, revealing the chat", async () => {
    const close = vi.fn()
    const assign = vi.fn()
    vi.stubGlobal("window", {
      ...window,
      opener: {},
      close,
      location: { ...window.location, assign },
    })
    await backToChat()
    expect(close).toHaveBeenCalled()
    expect(assign).not.toHaveBeenCalled()
  })

  it("goes to the workspace when settings was opened directly in a tab", async () => {
    const close = vi.fn()
    const assign = vi.fn()
    vi.stubGlobal("window", {
      ...window,
      opener: null,
      close,
      location: { ...window.location, assign },
    })
    await backToChat()
    expect(close).not.toHaveBeenCalled()
    expect(assign).toHaveBeenCalledWith("/")
  })
})
