import { render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"

const settings = vi.hoisted(() => ({ singleWindow: false }))
const remote = vi.hoisted(() => ({
  value: { connection: { id: 7 } } as { connection: { id: number } } | null,
}))

vi.mock("@/lib/platform", () => ({ isDesktop: () => true }))
vi.mock("@/lib/hall/api", () => ({
  hallGetSettings: vi.fn(async () => settings),
}))
vi.mock("@/contexts/remote-connection-context", () => ({
  useRemoteConnection: () => remote.value,
}))
vi.mock("@/components/layout/hall-branch-directory", () => ({
  HallBranchDirectory: (props: {
    headquartersConnectionId: number | null
    multiWindow?: boolean
  }) => (
    <button>
      directory {props.headquartersConnectionId} {String(props.multiWindow)}
    </button>
  ),
}))

import { StatusBarBranches } from "./status-bar-branches"

function renderIt() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <StatusBarBranches />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  settings.singleWindow = false
  remote.value = { connection: { id: 7 } }
})

describe("StatusBarBranches", () => {
  it("shows the directory in a remote window when single window is off", async () => {
    renderIt()
    expect(await screen.findByText(/directory 7 true/)).toBeTruthy()
  })

  it("stays out of the way in single-window mode", async () => {
    settings.singleWindow = true
    const { container } = renderIt()
    await waitFor(() => expect(container.textContent).toBe(""))
  })

  it("renders nothing in the local window", async () => {
    remote.value = null
    const { container } = renderIt()
    await waitFor(() => expect(container.textContent).toBe(""))
  })
})
