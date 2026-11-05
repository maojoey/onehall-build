import { beforeEach, expect, it, vi } from "vitest"
import type {
  DbConversationSummary,
  FolderDetail,
  MessageTurn,
} from "@/lib/types"
import {
  connectPetChat,
  getPetChatSettings,
  lastAssistantText,
  petChatStorageKey,
  setPetChatShortcut,
} from "./chat"
import { getPetPanelPinned, setPetPanelPinned } from "./api"

const mocks = vi.hoisted(() => ({
  find: vi.fn(),
  connect: vi.fn(),
  shell: vi.fn(),
}))
vi.mock("@/lib/api", () => ({
  acpConnect: mocks.connect,
  acpFindConnectionForConversation: mocks.find,
}))
vi.mock("@/lib/transport", () => ({
  getShellTransport: () => ({ call: mocks.shell }),
}))
beforeEach(() => vi.resetAllMocks())

it("keeps local and remote targets and drafts separate even with identical ids", () => {
  expect(
    new Set([
      petChatStorageKey(null),
      petChatStorageKey(1),
      petChatStorageKey(2),
      petChatStorageKey(null, 7),
      petChatStorageKey(1, 7),
      petChatStorageKey(2, 7),
    ]).size
  ).toBe(6)
})

it("shortcut settings always use the local shell", async () => {
  await getPetChatSettings()
  await setPetChatShortcut("")
  expect(mocks.shell.mock.calls).toEqual([
    ["hall_get_pet_chat_settings"],
    ["hall_set_pet_chat_shortcut", { shortcut: "" }],
  ])
})

it("panel pinning uses the local shell without touching any agent connection", async () => {
  await getPetPanelPinned()
  await setPetPanelPinned(false)
  expect(mocks.shell.mock.calls).toEqual([
    ["get_pet_panel_pinned"],
    ["set_pet_panel_pinned", { pinned: false }],
  ])
  expect(mocks.find).not.toHaveBeenCalled()
  expect(mocks.connect).not.toHaveBeenCalled()
})

const target = {
  id: 7,
  external_id: "session-existing",
  agent_type: "claude_code",
} as DbConversationSummary
const folder = { path: "/remote/project" } as FolderDetail

it("reuses an existing conversation connection without launching another engine", async () => {
  mocks.find.mockResolvedValue({ connection_id: "live" })
  await expect(connectPetChat(target, folder)).resolves.toBe("live")
  expect(mocks.connect).not.toHaveBeenCalled()
})

it("resumes the selected conversation using its own engine, directory and session", async () => {
  mocks.find.mockResolvedValue(null)
  mocks.connect.mockResolvedValue("resumed")
  await expect(connectPetChat(target, folder)).resolves.toBe("resumed")
  expect(mocks.connect).toHaveBeenCalledWith(
    "claude_code",
    "/remote/project",
    "session-existing"
  )
})

it("shows the last assistant reply rather than the later user prompt", () => {
  const turns = [
    { role: "assistant", blocks: [{ type: "text", text: "older" }] },
    { role: "assistant", blocks: [{ type: "text", text: "latest" }] },
    { role: "user", blocks: [{ type: "text", text: "not a reply" }] },
  ] as MessageTurn[]
  expect(lastAssistantText(turns)).toBe("latest")
})
