import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { toast } from "sonner"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import {
  acpAdapterRollback,
  acpAdapterStatus,
  acpAdapterUpgrade,
} from "@/lib/api"
import type { AdapterStatus } from "@/lib/types"

import {
  AcpAdapterRuntimeCard,
  hasManagedAdapter,
} from "./acp-adapter-runtime-card"

vi.mock("@/lib/api", () => ({
  acpAdapterStatus: vi.fn(),
  acpAdapterUpgrade: vi.fn(),
  acpAdapterRollback: vi.fn(),
}))
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}))

const status = (over: Partial<AdapterStatus> = {}): AdapterStatus => ({
  managed: false,
  adapter_version: "2.3.0",
  cli_version: "0.164.0",
  current: null,
  previous: null,
  installed_versions: [],
  ...over,
})

function renderCard() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <AcpAdapterRuntimeCard agentType="codex" />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(acpAdapterStatus).mockResolvedValue(status())
})

describe("AcpAdapterRuntimeCard", () => {
  it("is offered for Claude Code and Codex only", () => {
    expect(hasManagedAdapter("codex")).toBe(true)
    expect(hasManagedAdapter("claude_code")).toBe(true)
    expect(hasManagedAdapter("gemini")).toBe(false)
  })

  it("shows the adapter and bundled CLI versions", async () => {
    renderCard()
    expect(await screen.findByText(/2\.3\.0/)).toBeTruthy()
    expect(screen.getByText(/0\.164\.0/)).toBeTruthy()
  })

  it("renders nothing while the status is unavailable", async () => {
    vi.mocked(acpAdapterStatus).mockRejectedValue(new Error("offline"))
    const { container } = renderCard()
    await waitFor(() => expect(acpAdapterStatus).toHaveBeenCalled())
    expect(container.textContent).toBe("")
  })

  it("upgrades to the latest when no version is typed", async () => {
    vi.mocked(acpAdapterUpgrade).mockResolvedValue({
      version: "2.4.0",
      previous: "2.3.0",
      probe: { ok: true, needs_login: false, detail: "" },
    })
    renderCard()
    await userEvent.click(
      await screen.findByRole("button", { name: /latest/i })
    )
    await waitFor(() =>
      expect(acpAdapterUpgrade).toHaveBeenCalledWith("codex", null)
    )
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        expect.stringContaining("2.4.0")
      )
    )
    expect(acpAdapterStatus).toHaveBeenCalledTimes(2)
  })

  it("installs the version that was typed", async () => {
    vi.mocked(acpAdapterUpgrade).mockResolvedValue({
      version: "2.2.2",
      previous: null,
      probe: { ok: true, needs_login: false, detail: "" },
    })
    renderCard()
    await userEvent.type(await screen.findByRole("textbox"), " 2.2.2 ")
    await userEvent.click(screen.getByRole("button", { name: /install/i }))
    await waitFor(() =>
      expect(acpAdapterUpgrade).toHaveBeenCalledWith("codex", "2.2.2")
    )
  })

  it("says so when the new adapter works but still needs a login", async () => {
    vi.mocked(acpAdapterUpgrade).mockResolvedValue({
      version: "2.4.0",
      previous: null,
      probe: { ok: true, needs_login: true, detail: "" },
    })
    renderCard()
    await userEvent.click(
      await screen.findByRole("button", { name: /latest/i })
    )
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        expect.stringMatching(/login/i)
      )
    )
  })

  it("reports a failed startup check and keeps the card usable", async () => {
    vi.mocked(acpAdapterUpgrade).mockRejectedValue(
      new Error("2.4.0 did not pass the startup check, so 2.3.0 stays active")
    )
    renderCard()
    await userEvent.click(
      await screen.findByRole("button", { name: /latest/i })
    )
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        expect.stringContaining("stays active")
      )
    )
    expect(
      (screen.getByRole("button", { name: /latest/i }) as HTMLButtonElement)
        .disabled
    ).toBe(false)
  })

  it("offers a rollback only when there is a previous version", async () => {
    renderCard()
    await screen.findByText(/2\.3\.0/)
    expect(screen.queryByRole("button", { name: /roll back/i })).toBeNull()
  })

  it("rolls back to the previous version", async () => {
    vi.mocked(acpAdapterStatus).mockResolvedValue(
      status({ managed: true, current: "2.3.0", previous: "2.2.2" })
    )
    vi.mocked(acpAdapterRollback).mockResolvedValue(
      status({ managed: true, current: "2.2.2", previous: "2.3.0" })
    )
    renderCard()
    await userEvent.click(
      await screen.findByRole("button", { name: /roll back.*2\.2\.2/i })
    )
    await waitFor(() =>
      expect(acpAdapterRollback).toHaveBeenCalledWith("codex")
    )
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        expect.stringContaining("2.2.2")
      )
    )
  })
})
