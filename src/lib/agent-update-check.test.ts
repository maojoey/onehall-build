import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  acpAdapterStatus,
  acpAdapterUpgrade,
  acpFetchAgentLatestRelease,
} from "@/lib/api"

import {
  applyAgentUpdate,
  CHECK_INTERVAL_MS,
  compareVersions,
  findAgentUpdates,
  getAutoApply,
  isCheckDue,
  markChecked,
  setAutoApply,
} from "./agent-update-check"

vi.mock("@/lib/api", () => ({
  acpAdapterStatus: vi.fn(),
  acpAdapterUpgrade: vi.fn(),
  acpFetchAgentLatestRelease: vi.fn(),
}))

const status = (adapter_version: string | null) =>
  ({ adapter_version }) as Awaited<ReturnType<typeof acpAdapterStatus>>

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
})

describe("compareVersions", () => {
  it("orders numerically, not as text", () => {
    expect(compareVersions("2.9.0", "2.10.0")).toBeLessThan(0)
    expect(compareVersions("2.3.0", "v2.3.0")).toBe(0)
    expect(compareVersions("3.0", "2.9.9")).toBeGreaterThan(0)
  })
  it("gives up on versions it cannot read", () => {
    expect(compareVersions("abc", "1.0.0")).toBeNull()
  })
})

describe("daily schedule", () => {
  it("is due until a check is recorded, then for a day", () => {
    expect(isCheckDue(1_000)).toBe(true)
    markChecked(1_000)
    expect(isCheckDue(1_000 + CHECK_INTERVAL_MS - 1)).toBe(false)
    expect(isCheckDue(1_000 + CHECK_INTERVAL_MS)).toBe(true)
  })
  it("keeps automatic apply off until turned on", () => {
    expect(getAutoApply()).toBe(false)
    setAutoApply(true)
    expect(getAutoApply()).toBe(true)
  })
})

describe("findAgentUpdates", () => {
  it("offers only agents whose installed adapter is older", async () => {
    vi.mocked(acpAdapterStatus).mockImplementation(async (agent) =>
      status(agent === "codex" ? "2.3.0" : "0.99.0")
    )
    vi.mocked(acpFetchAgentLatestRelease).mockImplementation(async (agent) =>
      agent === "codex" ? { version: "2.6.0" } : { version: "0.99.0" }
    )
    expect(await findAgentUpdates()).toEqual([
      { agentType: "codex", installed: "2.3.0", latest: "2.6.0" },
    ])
  })
  it("skips an agent that cannot be checked and keeps the others", async () => {
    vi.mocked(acpAdapterStatus).mockImplementation(async (agent) => {
      if (agent === "claude_code") throw new Error("offline")
      return status("2.3.0")
    })
    vi.mocked(acpFetchAgentLatestRelease).mockResolvedValue({
      version: "2.4.0",
    })
    const found = await findAgentUpdates()
    expect(found.map((o) => o.agentType)).toEqual(["codex"])
  })
  it("offers nothing when the installed version is unknown", async () => {
    vi.mocked(acpAdapterStatus).mockResolvedValue(status(null))
    vi.mocked(acpFetchAgentLatestRelease).mockResolvedValue({
      version: "9.0.0",
    })
    expect(await findAgentUpdates()).toEqual([])
  })
})

describe("applyAgentUpdate", () => {
  const offer = {
    agentType: "codex" as const,
    installed: "2.3.0",
    latest: "2.6.0",
  }
  it("installs the offered version", async () => {
    vi.mocked(acpAdapterUpgrade).mockResolvedValue({} as never)
    expect(await applyAgentUpdate(offer)).toEqual({ ok: true })
    expect(acpAdapterUpgrade).toHaveBeenCalledWith("codex", "2.6.0")
  })
  it("reports a refused upgrade instead of throwing", async () => {
    vi.mocked(acpAdapterUpgrade).mockRejectedValue(new Error("stays active"))
    expect(await applyAgentUpdate(offer)).toEqual({
      ok: false,
      message: "stays active",
    })
  })
})
