import {
  act,
  cleanup,
  fireEvent,
  render,
  waitFor,
} from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { DbConversationSummary, FolderDetail } from "@/lib/types"
import { PetChat } from "./PetChat"
import { petChatStorageKey } from "@/lib/pet/chat"

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  folders: vi.fn(),
  detail: vi.fn(),
  find: vi.fn(),
  snapshot: vi.fn(),
  connect: vi.fn(),
  prompt: vi.fn(),
  cancel: vi.fn(),
  subscribe: vi.fn(),
  remote: 17 as number | null,
}))
vi.mock("next-intl", () => ({ useTranslations: () => (key: string) => key }))
vi.mock("@/lib/api", () => ({
  listAllConversations: mocks.list,
  listAllFolderDetails: mocks.folders,
  getFolderConversation: mocks.detail,
  acpFindConnectionForConversation: mocks.find,
  acpGetSessionSnapshot: mocks.snapshot,
  acpConnect: mocks.connect,
  acpPrompt: mocks.prompt,
  acpCancel: mocks.cancel,
}))
vi.mock("@/lib/transport", () => ({
  getShellTransport: () => ({ subscribe: mocks.subscribe }),
}))
vi.mock("@/lib/pet/remote", () => ({
  petRemoteConnectionId: () => mocks.remote,
}))
vi.mock("./SessionDetail", () => ({
  summarizeProgress: () => ({ text: "", tools: [] }),
}))
vi.mock("./PanelPermissionCard", () => ({
  PanelPermissionCard: () => <div>Permission required</div>,
}))

const target = {
  id: 7,
  folder_id: 2,
  external_id: "session-existing",
  agent_type: "claude_code",
  kind: "regular",
  parent_id: null,
  title: "Remote conversation",
  updated_at: "2026-10-18",
} as DbConversationSummary

beforeEach(() => {
  vi.resetAllMocks()
  localStorage.clear()
  mocks.remote = 17
  mocks.list.mockResolvedValue([target])
  mocks.folders.mockResolvedValue([
    { id: 2, path: "/remote/project" } as FolderDetail,
  ])
  mocks.detail.mockResolvedValue({ turns: [] })
  mocks.find.mockResolvedValue(null)
  mocks.snapshot.mockResolvedValue(null)
  mocks.connect.mockResolvedValue("resumed")
  mocks.prompt.mockResolvedValue(undefined)
  mocks.subscribe.mockResolvedValue(() => {})
})
afterEach(cleanup)

it("does not fetch local data when a pet's saved remote connection cannot be loaded", () => {
  mocks.remote = null
  const view = render(<PetChat remoteConnectionId={17} />)
  expect(view.getByRole("alert")).toHaveTextContent("unavailable")
  expect(mocks.list).not.toHaveBeenCalled()
  expect(mocks.folders).not.toHaveBeenCalled()
  expect(mocks.connect).not.toHaveBeenCalled()
  expect(mocks.prompt).not.toHaveBeenCalled()
})

it("does not send locally if the bound transport disappears while resuming", async () => {
  let finish!: (connectionId: string) => void
  mocks.connect.mockImplementationOnce(
    () =>
      new Promise<string>((resolve) => {
        finish = resolve
      })
  )
  const view = render(<PetChat remoteConnectionId={17} />)
  fireEvent.change(await view.findByRole("textbox", { name: "input" }), {
    target: { value: "stay remote" },
  })
  fireEvent.click(view.getByRole("button", { name: "send" }))
  await waitFor(() => expect(mocks.connect).toHaveBeenCalledTimes(1))
  mocks.remote = null
  await act(async () => finish("remote-connection"))
  expect(mocks.prompt).not.toHaveBeenCalled()
  expect(localStorage.getItem(petChatStorageKey(17, 7))).toBe("stay remote")
})

it("focuses a restored remote draft without starting an engine or sending a prompt", async () => {
  localStorage.setItem(petChatStorageKey(17, 7), "remote draft")
  localStorage.setItem(petChatStorageKey(null, 7), "local draft")
  const view = render(<PetChat />)
  const input = await view.findByRole("textbox", { name: "input" })
  expect(input).toHaveValue("remote draft")
  await waitFor(() => expect(input).toHaveFocus())
  expect(mocks.connect).not.toHaveBeenCalled()
  expect(mocks.prompt).not.toHaveBeenCalled()
})

it("sends only on submit, resumes the selected conversation, and preserves the other realm draft", async () => {
  localStorage.setItem(petChatStorageKey(null, 7), "local draft")
  const view = render(<PetChat />)
  fireEvent.change(await view.findByRole("textbox", { name: "input" }), {
    target: { value: "hello" },
  })
  fireEvent.click(view.getByRole("button", { name: "send" }))
  await waitFor(() => expect(mocks.prompt).toHaveBeenCalledTimes(1))
  expect(mocks.connect).toHaveBeenCalledWith(
    "claude_code",
    "/remote/project",
    "session-existing"
  )
  expect(mocks.prompt).toHaveBeenCalledWith(
    "resumed",
    [{ type: "text", text: "hello" }],
    2,
    7,
    expect.stringMatching(/^optimistic-/)
  )
  expect(localStorage.getItem(petChatStorageKey(null, 7))).toBe("local draft")
})

it("keeps a failed prompt and reuses its message id after the panel closes and reopens", async () => {
  mocks.prompt.mockRejectedValueOnce(new Error("response lost"))
  const first = render(<PetChat />)
  fireEvent.change(await first.findByRole("textbox", { name: "input" }), {
    target: { value: "retry me" },
  })
  fireEvent.click(first.getByRole("button", { name: "send" }))
  await first.findByRole("alert")
  const id = mocks.prompt.mock.calls[0][4]
  first.unmount()
  const next = render(<PetChat />)
  expect(await next.findByRole("textbox", { name: "input" })).toHaveValue(
    "retry me"
  )
  fireEvent.click(next.getByRole("button", { name: "send" }))
  await waitFor(() => expect(mocks.prompt).toHaveBeenCalledTimes(2))
  expect(mocks.prompt.mock.calls[1][4]).toBe(id)
})

it("does not erase a newer draft when an old panel's send finishes late", async () => {
  let finish!: () => void
  mocks.prompt.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve
      })
  )
  const first = render(<PetChat />)
  fireEvent.change(await first.findByRole("textbox", { name: "input" }), {
    target: { value: "old message" },
  })
  fireEvent.click(first.getByRole("button", { name: "send" }))
  await waitFor(() => expect(mocks.prompt).toHaveBeenCalledTimes(1))
  first.unmount()
  const next = render(<PetChat />)
  fireEvent.change(await next.findByRole("textbox", { name: "input" }), {
    target: { value: "new draft" },
  })
  await act(async () => finish())
  expect(localStorage.getItem(petChatStorageKey(17, 7))).toBe("new draft")
  expect(next.getByRole("textbox", { name: "input" })).toHaveValue("new draft")
})
