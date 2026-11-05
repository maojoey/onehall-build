import {
  act,
  cleanup,
  fireEvent,
  render,
  waitFor,
} from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { PetPanel } from "./PetPanel"
import { petChatStorageKey } from "@/lib/pet/chat"
import messages from "@/i18n/messages/en.json"

const mocks = vi.hoisted(() => ({
  remote: 17 as number | null,
  list: vi.fn(),
  folders: vi.fn(),
  detail: vi.fn(),
  find: vi.fn(),
  snapshot: vi.fn(),
  prompt: vi.fn(),
  connect: vi.fn(),
  cancel: vi.fn(),
  respond: vi.fn(),
  pin: vi.fn(),
  close: vi.fn(),
  sessions: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  listAllConversations: mocks.list,
  listAllFolderDetails: mocks.folders,
  getFolderConversation: mocks.detail,
  acpFindConnectionForConversation: mocks.find,
  acpGetSessionSnapshot: mocks.snapshot,
  acpPrompt: mocks.prompt,
  acpConnect: mocks.connect,
  acpCancel: mocks.cancel,
  acpRespondPermission: mocks.respond,
}))
vi.mock("@/lib/transport", () => ({
  isDesktop: () => true,
  getShellTransport: () => ({ subscribe: async () => () => {} }),
}))
vi.mock("@/lib/pet/remote", () => ({
  petRemoteConnectionId: () => mocks.remote,
  usePetLinkState: () => "connected",
  reconnectPetLink: vi.fn(),
}))
vi.mock("@/lib/pet/api", () => ({
  closePetPanel: mocks.close,
  getPetPanelPinned: async () => true,
  setPetPanelPinned: mocks.pin,
  resizePetPanel: async () => {},
  focusConversation: vi.fn(),
}))
vi.mock("../../pet/_hooks/usePetSessions", () => ({
  usePetSessions: mocks.sessions,
}))
vi.mock("motion/react", () => ({
  AnimatePresence: ({ children }: { children: React.ReactNode }) => children,
  motion: { li: "li" },
}))

beforeEach(() => {
  vi.resetAllMocks()
  localStorage.clear()
  mocks.remote = 17
  mocks.sessions.mockReturnValue({ sessions: [] })
  mocks.list.mockResolvedValue([
    {
      id: 7,
      folder_id: 2,
      external_id: "existing-remote-session",
      agent_type: "codex",
      kind: "regular",
      title: "Remote task",
      updated_at: "2026-10-19",
    },
  ])
  mocks.folders.mockResolvedValue([{ id: 2, path: "/remote/project" }])
  mocks.find.mockResolvedValue({ connection_id: "remote-connection" })
  mocks.detail.mockResolvedValue({ turns: [] })
  mocks.snapshot.mockResolvedValue({
    status: "awaiting_permission",
    pending_permission: {
      request_id: "original-request",
      tool_call: { title: "rm hello.txt" },
      options: [
        {
          option_id: "reject-original",
          name: "Keep file",
          kind: "reject_once",
        },
      ],
    },
  })
  mocks.pin.mockResolvedValue(undefined)
  mocks.close.mockResolvedValue(undefined)
  mocks.respond.mockResolvedValue(undefined)
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    }
  )
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

function panel(remote = 17) {
  return render(
    <NextIntlClientProvider locale="en" messages={messages}>
      <PetPanel chat remoteConnectionId={remote} />
    </NextIntlClientProvider>
  )
}

it("pin changes and blur keep the real remote draft and permission card without answering it", async () => {
  localStorage.setItem(petChatStorageKey(null, 7), "local draft")
  const view = panel()
  const input = await view.findByRole("textbox")
  fireEvent.change(input, { target: { value: "remote draft" } })
  await view.findByRole("button", { name: "Keep file" })
  fireEvent.click(view.getByRole("button", { name: "Pin panel" }))
  await waitFor(() => expect(mocks.pin).toHaveBeenLastCalledWith(false))
  await waitFor(() =>
    expect(view.getByRole("button", { name: "Pin panel" })).not.toBeDisabled()
  )
  fireEvent.click(view.getByRole("button", { name: "Pin panel" }))
  await waitFor(() => expect(mocks.pin).toHaveBeenLastCalledWith(true))
  act(() => window.dispatchEvent(new Event("blur")))
  expect(input).toHaveValue("remote draft")
  expect(view.getByRole("button", { name: "Keep file" })).toBeInTheDocument()
  expect(localStorage.getItem(petChatStorageKey(17, 7))).toBe("remote draft")
  expect(localStorage.getItem(petChatStorageKey(null, 7))).toBe("local draft")
  expect(mocks.respond).not.toHaveBeenCalled()
  expect(mocks.cancel).not.toHaveBeenCalled()
  expect(mocks.prompt).not.toHaveBeenCalled()
  expect(mocks.connect).not.toHaveBeenCalled()
  expect(mocks.close).not.toHaveBeenCalled()

  fireEvent.click(view.getByRole("button", { name: "Keep file" }))
  await waitFor(() =>
    expect(mocks.respond).toHaveBeenCalledWith(
      "remote-connection",
      "original-request",
      "reject-original"
    )
  )
})

it("explicit close and reopen restore the remote draft and still-unanswered permission", async () => {
  const first = panel()
  fireEvent.change(await first.findByRole("textbox"), {
    target: { value: "keep this draft" },
  })
  await first.findByRole("button", { name: "Keep file" })
  fireEvent.keyDown(window, { key: "Escape" })
  expect(mocks.close).toHaveBeenCalledTimes(1)
  first.unmount()
  const next = panel()
  expect(await next.findByRole("textbox")).toHaveValue("keep this draft")
  expect(
    await next.findByRole("button", { name: "Keep file" })
  ).toBeInTheDocument()
  expect(mocks.respond).not.toHaveBeenCalled()
  expect(mocks.cancel).not.toHaveBeenCalled()
  expect(mocks.prompt).not.toHaveBeenCalled()
})

it("pinning an unavailable saved backend never loads local conversations or answers permissions", async () => {
  mocks.remote = null
  const view = panel(17)
  expect(view.getByRole("alert")).toHaveTextContent("unavailable")
  fireEvent.click(view.getByRole("button", { name: "Pin panel" }))
  await waitFor(() => expect(mocks.pin).toHaveBeenCalledWith(false))
  expect(mocks.sessions).toHaveBeenCalledWith(false)
  expect(mocks.list).not.toHaveBeenCalled()
  expect(mocks.find).not.toHaveBeenCalled()
  expect(mocks.respond).not.toHaveBeenCalled()
  expect(mocks.prompt).not.toHaveBeenCalled()
})
