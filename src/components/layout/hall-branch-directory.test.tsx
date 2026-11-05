import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { localTab, useHallWorkspaceStore } from "@/stores/hall-workspace-store"
const mock = vi.hoisted(() => ({
  list: vi.fn(),
  probe: vi.fn(),
  connect: vi.fn(),
  register: vi.fn(),
  setEnabled: vi.fn(),
}))
vi.mock("@/lib/hall/branches", () => ({ branchClient: () => mock }))
vi.mock("next-intl", () => ({ useTranslations: () => (key: string) => key }))
import { HallBranchDirectory } from "./hall-branch-directory"
const branch = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "ARM分堂",
  enabled: true,
  revision: 1,
  status: "online",
  checkedAt: null,
}
beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
  useHallWorkspaceStore.setState({
    tabs: [localTab("总堂")],
    activeKey: "local",
    enabled: true,
    webMode: true,
  })
  mock.list.mockResolvedValue([branch])
  mock.probe.mockResolvedValue(branch)
})
it("shows headquarters branches and opens a browser branch without credentials", async () => {
  render(<HallBranchDirectory headquartersConnectionId={null} web />)
  fireEvent.click(screen.getByRole("button", { name: "directory" }))
  fireEvent.click(await screen.findByRole("button", { name: /ARM分堂/ }))
  expect(useHallWorkspaceStore.getState().activeKey).toBe(`branch:${branch.id}`)
  expect(mock.connect).not.toHaveBeenCalled()
})
it("uses a dedicated derived connection on desktop", async () => {
  mock.connect.mockResolvedValue({ connectionId: 42, name: branch.name })
  render(<HallBranchDirectory headquartersConnectionId={7} web={false} />)
  fireEvent.click(screen.getByRole("button", { name: "directory" }))
  fireEvent.click(await screen.findByRole("button", { name: /ARM分堂/ }))
  await waitFor(() =>
    expect(useHallWorkspaceStore.getState().activeKey).toBe("remote:42")
  )
  expect(
    useHallWorkspaceStore.getState().tabs.slice(-1)[0]?.headquartersConnectionId
  ).toBe(7)
})
it("does not navigate when a previous headquarters connection finishes late", async () => {
  let finish!: (v: { connectionId: number; name: string }) => void
  mock.connect.mockReturnValue(
    new Promise((r) => {
      finish = r
    })
  )
  const view = render(
    <HallBranchDirectory headquartersConnectionId={7} web={false} />
  )
  fireEvent.click(screen.getByRole("button", { name: "directory" }))
  fireEvent.click(await screen.findByRole("button", { name: /ARM分堂/ }))
  view.rerender(
    <HallBranchDirectory headquartersConnectionId={8} web={false} />
  )
  await act(async () => {
    finish({ connectionId: 42, name: branch.name })
  })
  expect(useHallWorkspaceStore.getState().activeKey).toBe("local")
})
