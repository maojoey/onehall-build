import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"

const calls: Array<{
  connectionId: number | null
  command: string
  args: Record<string, unknown>
}> = []
const versions: Record<number, Record<string, string>> = {
  1: { codex: "2.6.0", claude_code: "0.99.0" },
  2: { codex: "2.3.0", claude_code: "0.99.0" },
}
let failUpgrade = false

vi.mock("@/lib/transport", () => ({
  getShellTransport: () => ({
    call: async (command: string, args: Record<string, unknown>) => {
      if (command !== "remote_http_call")
        throw new Error("unexpected " + command)
      const connectionId = args.connectionId as number
      const inner = args.args as Record<string, unknown>
      calls.push({ connectionId, command: args.command as string, args: inner })
      if (args.command === "acp_adapter_status")
        return {
          adapter_version: versions[connectionId][inner.agentType as string],
        }
      if (failUpgrade) throw new Error("did not pass the startup check")
      versions[connectionId][inner.agentType as string] =
        inner.version as string
      return { version: inner.version }
    },
  }),
}))

import { BranchAdapterPanel } from "./adapter-panel"

function renderPanel() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <BranchAdapterPanel
        headquartersConnectionId={1}
        connectBranch={async () => 2}
      />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  calls.length = 0
  failUpgrade = false
  versions[2].codex = "2.3.0"
})

describe("BranchAdapterPanel", () => {
  it("lists headquarters and branch versions side by side", async () => {
    renderPanel()
    expect(await screen.findByText("2.3.0")).toBeTruthy()
    expect(screen.getByText("2.6.0")).toBeTruthy()
  })

  it("offers to match only where the versions differ", async () => {
    renderPanel()
    await screen.findByText("2.3.0")
    expect(
      screen.getAllByRole("button", { name: /headquarters version/i })
    ).toHaveLength(1)
  })

  it("installs headquarters' version on the branch, then refreshes", async () => {
    renderPanel()
    await userEvent.click(
      await screen.findByRole("button", { name: /headquarters version/i })
    )
    await waitFor(() =>
      expect(calls).toContainEqual({
        connectionId: 2,
        command: "acp_adapter_upgrade",
        args: { agentType: "codex", version: "2.6.0" },
      })
    )
    await screen.findByText(/Now on 2\.6\.0/)
    expect(
      screen.queryByRole("button", { name: /headquarters version/i })
    ).toBeNull()
  })

  it("reports a refused upgrade and keeps the old version shown", async () => {
    failUpgrade = true
    renderPanel()
    await userEvent.click(
      await screen.findByRole("button", { name: /headquarters version/i })
    )
    expect((await screen.findByRole("alert")).textContent).toMatch(
      /startup check/
    )
    expect(screen.getByText("2.3.0")).toBeTruthy()
  })
})
