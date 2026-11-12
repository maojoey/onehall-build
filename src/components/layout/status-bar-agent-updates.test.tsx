import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { toast } from "sonner"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import {
  acpAdapterStatus,
  acpAdapterUpgrade,
  acpFetchAgentLatestRelease,
} from "@/lib/api"
import { markChecked, setAutoApply } from "@/lib/agent-update-check"

import { StatusBarAgentUpdates } from "./status-bar-agent-updates"

vi.mock("@/lib/api", () => ({
  acpAdapterStatus: vi.fn(),
  acpAdapterUpgrade: vi.fn(),
  acpFetchAgentLatestRelease: vi.fn(),
}))
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }))

function offerCodexUpdate() {
  vi.mocked(acpAdapterStatus).mockImplementation(
    async (agent) =>
      ({
        adapter_version: agent === "codex" ? "2.3.0" : "0.99.0",
      }) as Awaited<ReturnType<typeof acpAdapterStatus>>
  )
  vi.mocked(acpFetchAgentLatestRelease).mockImplementation(async (agent) => ({
    version: agent === "codex" ? "2.6.0" : "0.99.0",
  }))
}

function renderChip() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <StatusBarAgentUpdates />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
})

describe("StatusBarAgentUpdates", () => {
  it("shows nothing when every adapter is current", async () => {
    vi.mocked(acpAdapterStatus).mockResolvedValue({
      adapter_version: "1.0.0",
    } as never)
    vi.mocked(acpFetchAgentLatestRelease).mockResolvedValue(null)
    const { container } = renderChip()
    await waitFor(() => expect(acpAdapterStatus).toHaveBeenCalled())
    expect(container.textContent).toBe("")
  })

  it("does not check again within a day", async () => {
    markChecked()
    renderChip()
    await new Promise((r) => setTimeout(r, 20))
    expect(acpAdapterStatus).not.toHaveBeenCalled()
  })

  it("offers an update and applies it on request", async () => {
    offerCodexUpdate()
    vi.mocked(acpAdapterUpgrade).mockResolvedValue({} as never)
    renderChip()
    await userEvent.click(
      await screen.findByRole("button", { name: /1 agent update/i })
    )
    expect(screen.getByText(/2\.3\.0 → 2\.6\.0/)).toBeTruthy()
    await userEvent.click(screen.getByRole("button", { name: "Update" }))
    await waitFor(() =>
      expect(acpAdapterUpgrade).toHaveBeenCalledWith("codex", "2.6.0")
    )
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        expect.stringContaining("2.6.0")
      )
    )
  })

  it("applies by itself only when switched on", async () => {
    setAutoApply(true)
    offerCodexUpdate()
    vi.mocked(acpAdapterUpgrade).mockResolvedValue({} as never)
    renderChip()
    await waitFor(() =>
      expect(acpAdapterUpgrade).toHaveBeenCalledWith("codex", "2.6.0")
    )
  })

  it("stays quiet when an automatic update is refused", async () => {
    setAutoApply(true)
    offerCodexUpdate()
    vi.mocked(acpAdapterUpgrade).mockRejectedValue(new Error("stays active"))
    renderChip()
    await waitFor(() => expect(acpAdapterUpgrade).toHaveBeenCalled())
    await new Promise((r) => setTimeout(r, 20))
    expect(toast.error).not.toHaveBeenCalled()
  })
})
