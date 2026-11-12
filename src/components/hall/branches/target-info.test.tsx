import { render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"

const members = [
  { name: "worker", role: "builder", engine: "codex", enabled: true },
  { name: "reviewer", role: "", engine: "claude_code", enabled: true },
  { name: "retired", role: "", engine: "codex", enabled: false },
]
let membersFail = false

vi.mock("@/lib/transport", () => ({
  getShellTransport: () => ({
    call: async (command: string, args: Record<string, unknown>) => {
      if (command !== "remote_http_call") throw new Error("unexpected")
      if (args.command === "hall_list_members") {
        if (membersFail) throw new Error("not found")
        return members
      }
      return {
        adapter_version:
          args.args &&
          (args.args as { agentType: string }).agentType === "codex"
            ? "2.6.0"
            : "0.106.0",
        cli_version: "1.0.0",
      }
    },
  }),
}))

import { BranchTargetInfo } from "./target-info"

const renderIt = (connect: () => Promise<number>) =>
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <BranchTargetInfo connectBranch={connect} />
    </NextIntlClientProvider>
  )

describe("BranchTargetInfo", () => {
  it("lists the active members and agent versions of the target", async () => {
    membersFail = false
    renderIt(async () => 7)
    expect(await screen.findByText("worker")).toBeTruthy()
    expect(screen.getByText("reviewer")).toBeTruthy()
    expect(screen.queryByText("retired")).toBeNull()
    expect(screen.getByText(/codex 2\.6\.0/)).toBeTruthy()
    expect(screen.getByText(/accepts the task/i)).toBeTruthy()
  })

  it("still shows agent versions when the machine has no member list", async () => {
    membersFail = true
    renderIt(async () => 7)
    expect(await screen.findByText(/No active members/)).toBeTruthy()
    expect(screen.getByText(/claude_code 0\.106\.0/)).toBeTruthy()
  })

  it("says so when the machine cannot be reached", async () => {
    renderIt(async () => {
      throw new Error("offline")
    })
    expect((await screen.findByRole("alert")).textContent).toMatch(
      /Could not read/
    )
  })
})
