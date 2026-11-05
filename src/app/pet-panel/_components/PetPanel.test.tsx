import { render, screen, act, fireEvent, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { ReactElement } from "react"

// Mocks MUST be declared before importing the component under test.
const shortcut = vi.hoisted(() => ({
  focus: undefined as (() => void) | undefined,
  off: vi.fn(),
}))
vi.mock("@/lib/transport", () => ({
  isDesktop: vi.fn(() => true),
  getShellTransport: () => ({
    subscribe: async (_event: string, focus: () => void) => {
      shortcut.focus = focus
      return shortcut.off
    },
  }),
}))
vi.mock("./PetChat", () => ({
  PetChat: () => <div data-testid="quick-chat">Quick chat</div>,
}))
// onehall: the panel's remote-link banner reads the pet link state.
vi.mock("@/lib/pet/remote", () => ({
  usePetLinkState: () => "local",
  reconnectPetLink: vi.fn(),
  petRemoteConnectionId: () => null,
}))
vi.mock("@/lib/pet/api", () => ({
  closePetPanel: vi.fn(() => Promise.resolve()),
  getPetPanelPinned: vi.fn(() => Promise.resolve(true)),
  setPetPanelPinned: vi.fn(() => Promise.resolve()),
  resizePetPanel: vi.fn(() => Promise.resolve()),
}))
vi.mock("../../pet/_hooks/usePetSessions", () => ({
  usePetSessions: vi.fn(),
}))
vi.mock("motion/react", () => ({
  AnimatePresence: ({ children }: { children: React.ReactNode }) => children,
  motion: { li: "li" },
}))
vi.mock("./SessionRow", () => ({
  SessionRow: ({ session }: { session: { connectionId: string } }) => (
    <div data-testid="session-row">{session.connectionId}</div>
  ),
}))

import { PetPanel } from "./PetPanel"
import { isDesktop } from "@/lib/transport"
import {
  closePetPanel,
  getPetPanelPinned,
  resizePetPanel,
  setPetPanelPinned,
} from "@/lib/pet/api"
import { usePetSessions } from "../../pet/_hooks/usePetSessions"
import enMessages from "@/i18n/messages/en.json"
import type { PetSessionEntry, PetSessionsPayload } from "@/lib/pet/types"

const mockIsDesktop = vi.mocked(isDesktop)
const mockResize = vi.mocked(resizePetPanel)
const mockClose = vi.mocked(closePetPanel)
const mockGetPinned = vi.mocked(getPetPanelPinned)
const mockSetPinned = vi.mocked(setPetPanelPinned)
const mockUsePetSessions = vi.mocked(usePetSessions)

// The wrapper's measured box height; drives what resize_pet_panel receives.
let measuredHeight = 150
// Captured ResizeObserver callback so a test can simulate a content resize.
let roCallback: ResizeObserverCallback | null = null

function payload(sessions: PetSessionEntry[] = []): PetSessionsPayload {
  return {
    runningCount: sessions.length,
    waitingCount: 0,
    errorCount: 0,
    sessions,
  }
}

function session(id: string): PetSessionEntry {
  return {
    connectionId: id,
    conversationId: 1,
    folderId: 1,
    agentType: "claude_code",
    title: id,
    status: "prompting",
  }
}

function renderPanel(): ReactElement {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <PetPanel />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  shortcut.focus = undefined
  shortcut.off.mockClear()
  mockIsDesktop.mockReturnValue(true)
  mockResize.mockClear()
  mockClose.mockClear()
  mockGetPinned.mockReset().mockResolvedValue(true)
  mockSetPinned.mockReset().mockResolvedValue(undefined)
  mockUsePetSessions.mockReturnValue(payload([]))
  measuredHeight = 150
  roCallback = null

  // rAF runs synchronously so the measure→resize path completes in-test.
  // Return 0 (not a real id): the production code assigns the id *after* the
  // async callback runs, so with a synchronous stub a non-zero id would stick
  // in `raf` and the `if (raf) return` guard would block the next schedule.
  // The `lastSent` integer dedup (what we're actually testing) is unaffected.
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    cb(0)
    return 0
  })
  vi.stubGlobal("cancelAnimationFrame", () => {})
  // Capture the RO callback; jsdom has no ResizeObserver.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(cb: ResizeObserverCallback) {
        roCallback = cb
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    }
  )
  // jsdom does no layout; feed a controllable box height.
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(
    () => ({ height: measuredHeight, width: 300 }) as DOMRect
  )
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe("PetPanel", () => {
  it("starts pinned and lets the user disable and re-enable native blur protection", async () => {
    render(renderPanel())
    const button = screen.getByRole("button", { name: "Pin panel" })
    expect(button).toHaveAttribute("aria-pressed", "true")
    fireEvent.click(button)
    await waitFor(() => expect(button).toHaveAttribute("aria-pressed", "false"))
    expect(mockSetPinned).toHaveBeenLastCalledWith(false)
    fireEvent.click(button)
    await waitFor(() => expect(button).toHaveAttribute("aria-pressed", "true"))
    expect(mockSetPinned).toHaveBeenLastCalledWith(true)
    expect(mockClose).not.toHaveBeenCalled()
  })

  it("does not overwrite a user change with a late initial pin read", async () => {
    let finish!: (value: boolean) => void
    mockGetPinned.mockImplementationOnce(
      () => new Promise<boolean>((resolve) => (finish = resolve))
    )
    render(renderPanel())
    const button = screen.getByRole("button", { name: "Pin panel" })
    fireEvent.click(button)
    await waitFor(() => expect(button).toHaveAttribute("aria-pressed", "false"))
    await act(async () => finish(true))
    expect(button).toHaveAttribute("aria-pressed", "false")
  })

  it("keeps the previous pin state and shows an error if native pinning fails", async () => {
    mockSetPinned.mockRejectedValueOnce(new Error("window unavailable"))
    render(renderPanel())
    const button = screen.getByRole("button", { name: "Pin panel" })
    fireEvent.click(button)
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not change panel pinning"
    )
    expect(button).toHaveAttribute("aria-pressed", "true")
    expect(mockClose).not.toHaveBeenCalled()
  })

  it("explicitly closes a pinned panel with its close button", () => {
    render(renderPanel())
    fireEvent.click(screen.getByRole("button", { name: "Close" }))
    expect(mockClose).toHaveBeenCalledTimes(1)
  })

  it("blocks the session feed and rows when a saved remote connection is missing", () => {
    mockUsePetSessions.mockReturnValue(
      payload([session("must-not-show-local")])
    )
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <PetPanel remoteConnectionId={17} />
      </NextIntlClientProvider>
    )
    expect(mockUsePetSessions).toHaveBeenCalledWith(false)
    expect(screen.getByRole("alert")).toHaveTextContent("unavailable")
    expect(screen.queryByTestId("session-row")).not.toBeInTheDocument()
  })
  it("switches an existing panel to chat and repeated shortcut presses keep it open", async () => {
    render(renderPanel())
    await act(async () => {
      shortcut.focus?.()
    })
    expect(screen.getAllByTestId("quick-chat")).toHaveLength(1)
    await act(async () => {
      shortcut.focus?.()
    })
    expect(screen.getAllByTestId("quick-chat")).toHaveLength(1)
    expect(mockClose).not.toHaveBeenCalled()
  })
  it("renders a compact empty state when there are no sessions", () => {
    render(renderPanel())
    expect(screen.getByText("No active sessions")).toBeInTheDocument()
    expect(
      screen.getByText("Running agents and ones that need you appear here.")
    ).toBeInTheDocument()
    // No header count and no rows in the empty state.
    expect(screen.queryByTestId("session-row")).not.toBeInTheDocument()
  })

  it("renders the session list with a header count when sessions exist", () => {
    mockUsePetSessions.mockReturnValue(payload([session("c1"), session("c2")]))
    render(renderPanel())
    expect(screen.getAllByTestId("session-row")).toHaveLength(2)
    expect(screen.getByText("(2)")).toBeInTheDocument()
    expect(screen.queryByText("No active sessions")).not.toBeInTheDocument()
  })

  it("fits the window to measured content height on mount", () => {
    render(renderPanel())
    expect(mockResize).toHaveBeenCalledTimes(1)
    expect(mockResize).toHaveBeenCalledWith(150)
  })

  it("does not re-resize when the measured height is unchanged", () => {
    render(renderPanel())
    expect(mockResize).toHaveBeenCalledTimes(1)
    // Same height → deduped.
    act(() => roCallback?.([], {} as ResizeObserver))
    expect(mockResize).toHaveBeenCalledTimes(1)
  })

  it("re-resizes when the content height changes", () => {
    render(renderPanel())
    expect(mockResize).toHaveBeenLastCalledWith(150)
    measuredHeight = 248
    act(() => roCallback?.([], {} as ResizeObserver))
    expect(mockResize).toHaveBeenCalledTimes(2)
    expect(mockResize).toHaveBeenLastCalledWith(248)
  })

  it("never resizes when not running on the desktop", () => {
    mockIsDesktop.mockReturnValue(false)
    render(renderPanel())
    expect(mockResize).not.toHaveBeenCalled()
    // A later content change is also ignored (no observer was attached).
    expect(roCallback).toBeNull()
  })

  it("closes the panel on Escape", () => {
    render(renderPanel())
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
    })
    expect(mockClose).toHaveBeenCalledTimes(1)
  })
})
