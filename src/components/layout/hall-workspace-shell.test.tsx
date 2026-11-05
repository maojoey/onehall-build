import { act, render, screen, waitFor } from "@testing-library/react"
import { useEffect, useState } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

/**
 * onehall, Mac acceptance G3: after switching from the remote tab back to the
 * local one, the local workspace showed the remote's folder, conversation
 * and recent list. Cause: the reset ran while the remote realm was still
 * mounted, and the local realm's first fetches left through the remote
 * transport (still configured until the gate's effect, which runs after its
 * children's). These tests drive the real shell, gate and transports over a
 * fake Tauri IPC: local has no folders, the remote has one.
 */

type Pending = { resolve: (v: unknown) => void; reject: (e: unknown) => void }

const ipc = vi.hoisted(() => ({
  /** Pending `list_folders` per backend, oldest first. */
  folders: { local: [] as Pending[], remote: [] as Pending[] },
  assistantReplies: [] as Pending[],
  holdRemoteAssistants: false,
  log: [] as string[],
}))

const REMOTE_ROW = { id: 1, path: "/home/ubuntu/onehall/workspaces/demo" }

const fakeInternals = {
  transformCallback: () => 0,
  invoke: (command: string, args: Record<string, unknown> = {}) => {
    const remote = command === "remote_http_call"
    const cmd = remote ? (args.command as string) : command
    if (cmd === "list_folders") {
      return new Promise((resolve, reject) =>
        ipc.folders[remote ? "remote" : "local"].push({ resolve, reject })
      )
    }
    switch (cmd) {
      case "assistant_list":
        if (remote && ipc.holdRemoteAssistants) {
          return new Promise((resolve, reject) =>
            ipc.assistantReplies.push({ resolve, reject })
          )
        }
        return Promise.resolve([assistantRow(remote)])
      case "list_open_folder_details":
      case "list_all_folder_details":
        return Promise.resolve(remote ? [REMOTE_ROW] : [])
      case "list_folder_groups":
        return Promise.resolve([])
      case "list_all_conversations":
        return Promise.resolve(
          remote
            ? [
                {
                  id: 42,
                  folder_id: 1,
                  title: "回复确认",
                  agent_type: "claude_code",
                  message_count: 2,
                },
              ]
            : []
        )
      case "list_opened_tabs":
        return Promise.resolve({
          version: 0,
          items: remote
            ? [
                {
                  folder_id: 1,
                  conversation_id: 42,
                  agent_type: "claude_code",
                  is_pinned: true,
                  is_active: true,
                },
              ]
            : [],
        })
      case "hall_connect_branch":
        ipc.log.push("refresh-branch-credentials")
        return Promise.resolve({ connectionId: 7, name: "ARM" })
      case "hall_get_settings":
        return Promise.resolve({ singleWindow: true })
      case "list_remote_workspace_connections":
        return Promise.resolve([{ id: 7, name: "堂务" }])
      case "get_remote_workspace_connection":
        return Promise.resolve({
          id: 7,
          name: "堂务",
          base_url: "https://hall.example",
          token: "t",
        })
      case "pet_list_active_sessions":
        return Promise.resolve({
          runningCount: 0,
          waitingCount: 0,
          errorCount: 0,
        })
      default:
        return Promise.resolve(undefined)
    }
  },
}

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(),
}))
// Stable like next-intl's own `t`: the shell's init effect depends on it.
const translate = vi.hoisted(
  () => (key: string) => (key === "tabs.local" ? "本机" : key)
)
vi.mock("next-intl", () => ({ useTranslations: () => translate }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: async () => () => {},
}))
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setTitle: async () => {} }),
}))
vi.mock("@/components/connection/remote-connection-status", () => ({
  RemoteConnectionStatus: () => null,
}))
vi.mock("@/components/layout/remote-workspace-manage-dialog", () => ({
  RemoteWorkspaceManageDialog: () => null,
}))

import { RemoteDesktopTransport } from "@/lib/transport/remote-desktop-transport"
import { TauriTransport } from "@/lib/transport/tauri-transport"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  getTransport,
  isRemoteDesktopMode,
} from "@/lib/transport"
import {
  __clearRegisteredBackendScopedStoreResets,
  registerBackendScopedStoreReset,
} from "@/stores/backend-scoped-store-reset"
import {
  loadMessageInputDraft,
  saveMessageInputDraft,
} from "@/lib/message-input-draft"
import {
  LOCAL_BACKEND_KEY,
  useHallWorkspaceStore,
} from "@/stores/hall-workspace-store"
import { HallWorkspaceShell } from "./hall-workspace-shell"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "@/stores/app-workspace-store"
import { resetTabStore, useTabStore } from "@/stores/tab-store"
import {
  resetAssistantsStore,
  useAssistantsStore,
} from "@/stores/assistants-store"
import {
  loadLastActiveContext,
  saveLastActiveContext,
} from "@/lib/last-active-context-storage"
import {
  peekClosedTab,
  pushClosedTab,
  resetClosedTabStackForTests,
} from "@/lib/closed-tab-stack"

function assistantRow(remote: boolean) {
  return {
    id: 5,
    uid: remote ? "remote-assistant" : "local-assistant",
    name: remote ? "堂务助手" : "本机助手",
    revision: remote ? 99 : 1,
    enabled: true,
    deleted_at: null,
    sort_order: 0,
  }
}

// A stand-in for the backend-scoped stores (folders / conversations /
// recents): a module singleton filled by the realm and reset on a switch.
let folderRows: { id: number; path: string }[] | null = null
const rowListeners = new Set<() => void>()
function setRows(rows: typeof folderRows) {
  folderRows = rows
  for (const l of rowListeners) l()
}

function Realm() {
  const [, force] = useState(0)
  useEffect(() => {
    const l = () => force((n) => n + 1)
    rowListeners.add(l)
    const backend = isRemoteDesktopMode() ? "remote" : "local"
    ipc.log.push(`mount:${backend}`)
    void getTransport()
      .call<{ id: number; path: string }[]>("list_folders")
      .then(setRows, () => {})
    return () => {
      rowListeners.delete(l)
      ipc.log.push(`unmount:${backend}`)
    }
  }, [])
  return (
    <ul data-testid="folders">
      {(folderRows ?? []).map((r) => (
        <li key={r.id}>{r.path}</li>
      ))}
    </ul>
  )
}

// Exercise the actual stores as well as the small realm above: conversation
// tabs are fetched from the backend, while draft tabs live in localStorage.
function StoredRealm() {
  const folders = useAppWorkspaceStore((s) => s.folders)
  const conversations = useAppWorkspaceStore((s) => s.conversations)
  const tabs = useTabStore((s) => s.rawTabs)
  const assistants = useAssistantsStore((s) => s.byId)
  useEffect(() => {
    void useAppWorkspaceStore.getState().fetchFolders()
    void useAppWorkspaceStore.getState().refreshConversations()
    void useAssistantsStore.getState().fetchAll()
    return useTabStore.getState().hydrate()
  }, [])
  return (
    <div>
      <div data-testid="stored-folders">
        {folders.map((f) => f.path).join(",")}
      </div>
      <div data-testid="stored-recents">
        {conversations.map((c) => c.title).join(",")}
      </div>
      <div data-testid="stored-tabs">{tabs.map((t) => t.id).join(",")}</div>
      <div data-testid="stored-assistants">
        {Array.from(assistants.values(), (a) => a.name).join(",")}
      </div>
    </div>
  )
}

const DRAFT_KEY = "draft:tab-1"

async function settle() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0))
  })
}

async function answer(backend: "local" | "remote", rows: unknown) {
  await act(async () => {
    ipc.folders[backend].shift()?.resolve(rows)
  })
  await settle()
}

function shownPaths() {
  return Array.from(
    screen.getByTestId("folders").querySelectorAll("li"),
    (li) => li.textContent
  )
}

async function activate(key: string) {
  await act(async () => {
    useHallWorkspaceStore.getState().activate(key)
  })
  await settle()
}

async function mountOnLocal() {
  render(
    <HallWorkspaceShell>
      <Realm />
    </HallWorkspaceShell>
  )
  await settle()
  expect(ipc.folders.local).toHaveLength(1)
  await answer("local", [])
  expect(shownPaths()).toEqual([])
}

async function openRemote() {
  await act(async () => {
    useHallWorkspaceStore.getState().open(7, "堂务")
  })
  await settle()
  expect(ipc.folders.remote).toHaveLength(1)
  await answer("remote", [REMOTE_ROW])
  expect(shownPaths()).toEqual([REMOTE_ROW.path])
}

beforeEach(() => {
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ =
    fakeInternals
  __setTransportFactoriesForTests({
    tauri: () => new TauriTransport(),
    remote: (config) => new RemoteDesktopTransport(config),
  })
  __resetTransportForTests()
  __clearRegisteredBackendScopedStoreResets()
  registerBackendScopedStoreReset(() => {
    ipc.log.push("reset")
    setRows(null)
  })
  ipc.folders.local.length = 0
  ipc.folders.remote.length = 0
  ipc.log.length = 0
  ipc.assistantReplies.length = 0
  ipc.holdRemoteAssistants = false
  folderRows = null
  localStorage.clear()
  resetAppWorkspaceStore()
  resetTabStore()
  resetClosedTabStackForTests()
  resetAssistantsStore()
  registerBackendScopedStoreReset(resetAppWorkspaceStore)
  registerBackendScopedStoreReset(resetTabStore)
  registerBackendScopedStoreReset(resetAssistantsStore)
  useHallWorkspaceStore.setState({
    enabled: false,
    tabs: [],
    activeKey: LOCAL_BACKEND_KEY,
    pendingFocus: null,
  })
})

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
})

describe("HallWorkspaceShell backend switch", () => {
  it("shows only the returning backend's parked cache while it refreshes", async () => {
    registerBackendScopedStoreReset(() => {}, {
      capture: () => folderRows,
      restore: (rows) => setRows(rows as typeof folderRows),
    })
    await mountOnLocal()
    await openRemote()
    await activate(LOCAL_BACKEND_KEY)
    expect(shownPaths()).toEqual([])
    await activate("remote:7")
    expect(shownPaths()).toEqual([REMOTE_ROW.path])
    expect(ipc.folders.remote).toHaveLength(1)
    await answer("remote", [REMOTE_ROW])
  })
  it("refreshes derived branch credentials on direct tab activation", async () => {
    render(
      <HallWorkspaceShell>
        <StoredRealm />
      </HallWorkspaceShell>
    )
    await settle()
    await act(async () =>
      useHallWorkspaceStore
        .getState()
        .openDerivedBranch(9, 7, "11111111-1111-4111-8111-111111111111", "ARM")
    )
    await settle()
    expect(
      ipc.log.filter((v) => v === "refresh-branch-credentials")
    ).toHaveLength(1)
    await act(async () => useHallWorkspaceStore.getState().activate("local"))
    await settle()
    await act(async () => useHallWorkspaceStore.getState().activate("remote:7"))
    await settle()
    expect(
      ipc.log.filter((v) => v === "refresh-branch-credentials")
    ).toHaveLength(2)
  })

  it("keeps assistants with colliding ids and different revisions on their own backend", async () => {
    render(
      <HallWorkspaceShell>
        <StoredRealm />
      </HallWorkspaceShell>
    )
    await settle()
    expect(screen.getByTestId("stored-assistants")).toHaveTextContent(
      "本机助手"
    )
    for (let i = 0; i < 2; i++) {
      await act(async () => useHallWorkspaceStore.getState().open(7, "堂务"))
      await settle()
      expect(screen.getByTestId("stored-assistants")).toHaveTextContent(
        "堂务助手"
      )
      expect(screen.getByTestId("stored-assistants")).not.toHaveTextContent(
        "本机助手"
      )
      await activate(LOCAL_BACKEND_KEY)
      expect(screen.getByTestId("stored-assistants")).toHaveTextContent(
        "本机助手"
      )
      expect(screen.getByTestId("stored-assistants")).not.toHaveTextContent(
        "堂务助手"
      )
    }
  })

  it("drops a late remote assistant snapshot even when its revision is higher", async () => {
    render(
      <HallWorkspaceShell>
        <StoredRealm />
      </HallWorkspaceShell>
    )
    await settle()
    ipc.holdRemoteAssistants = true
    await act(async () => useHallWorkspaceStore.getState().open(7, "堂务"))
    await settle()
    expect(ipc.assistantReplies).toHaveLength(1)
    await activate(LOCAL_BACKEND_KEY)
    await act(async () =>
      ipc.assistantReplies.shift()!.resolve([assistantRow(true)])
    )
    await settle()
    expect(screen.getByTestId("stored-assistants")).toHaveTextContent(
      "本机助手"
    )
    expect(useAssistantsStore.getState().byId.get(5)?.revision).toBe(1)
  })

  it("remote (non-empty) → local (empty): local fetches from the shell and shows nothing of the remote", async () => {
    await mountOnLocal()
    await openRemote()

    await activate(LOCAL_BACKEND_KEY)
    // The new realm's first fetch went to the local shell, not the remote.
    expect(ipc.folders.remote).toHaveLength(0)
    expect(ipc.folders.local).toHaveLength(1)
    // Nothing of the remote is on screen while local is loading…
    expect(shownPaths()).toEqual([])
    await answer("local", [])
    // …nor after it answered.
    expect(shownPaths()).toEqual([])
  })

  it("local (empty) → remote (non-empty) → local: each side shows only its own rows", async () => {
    await mountOnLocal()
    await openRemote()
    await activate(LOCAL_BACKEND_KEY)
    await answer("local", [])
    await activate("remote:7")
    expect(ipc.folders.local).toHaveLength(0)
    await answer("remote", [REMOTE_ROW])
    expect(shownPaths()).toEqual([REMOTE_ROW.path])
    await activate(LOCAL_BACKEND_KEY)
    await answer("local", [])
    expect(shownPaths()).toEqual([])
  })

  it("unmounts the old realm before resetting, and resets before the new realm mounts", async () => {
    await mountOnLocal()
    await openRemote()
    ipc.log.length = 0
    await activate(LOCAL_BACKEND_KEY)
    expect(ipc.log).toEqual(["unmount:remote", "reset", "mount:local"])
  })

  it("drops a slow remote answer that arrives after switching to local", async () => {
    await mountOnLocal()
    // Open the remote but leave its folder list unanswered.
    await act(async () => {
      useHallWorkspaceStore.getState().open(7, "堂务")
    })
    await settle()
    expect(ipc.folders.remote).toHaveLength(1)
    await activate(LOCAL_BACKEND_KEY)
    await answer("local", [])
    // The remote finally answers — into a workspace that is gone.
    await answer("remote", [REMOTE_ROW])
    expect(shownPaths()).toEqual([])
  })

  it("drops a slow local answer that arrives after switching to the remote", async () => {
    render(
      <HallWorkspaceShell>
        <Realm />
      </HallWorkspaceShell>
    )
    await settle()
    expect(ipc.folders.local).toHaveLength(1)
    await act(async () => {
      useHallWorkspaceStore.getState().open(7, "堂务")
    })
    await settle()
    await answer("remote", [REMOTE_ROW])
    await answer("local", [{ id: 9, path: "/Users/me/local" }])
    expect(shownPaths()).toEqual([REMOTE_ROW.path])
  })

  it("keeps an unsent draft with the backend it was typed in", async () => {
    await mountOnLocal()
    await openRemote()
    saveMessageInputDraft(DRAFT_KEY, "堂务草稿-G3")
    await activate(LOCAL_BACKEND_KEY)
    await answer("local", [])
    expect(loadMessageInputDraft(DRAFT_KEY)).toBeNull()
    await activate("remote:7")
    await answer("remote", [REMOTE_ROW])
    await waitFor(() =>
      expect(loadMessageInputDraft(DRAFT_KEY)).toBe("堂务草稿-G3")
    )
  })

  it("isolates real folder, recent, conversation-tab, draft-tab and recovery state", async () => {
    render(
      <HallWorkspaceShell>
        <StoredRealm />
      </HallWorkspaceShell>
    )
    await settle()
    expect(screen.getByTestId("stored-folders")).toBeEmptyDOMElement()
    await act(async () => useHallWorkspaceStore.getState().open(7, "堂务"))
    await settle()
    expect(screen.getByTestId("stored-folders")).toHaveTextContent(
      REMOTE_ROW.path
    )
    expect(screen.getByTestId("stored-recents")).toHaveTextContent("回复确认")
    expect(screen.getByTestId("stored-tabs")).toHaveTextContent(
      "conv-1-claude_code-42"
    )
    await act(async () => {
      useTabStore.getState().openNewConversationTab(1, REMOTE_ROW.path)
    })
    const draftId = useTabStore.getState().activeTabId!
    saveMessageInputDraft(`draft:${draftId}`, "堂务真实草稿")
    saveLastActiveContext({ folderId: 1, isChat: false })
    pushClosedTab({
      kind: "file",
      key: "remote-file",
      index: 0,
      path: `${REMOTE_ROW.path}/a.txt`,
      folderId: 1,
    })

    await activate(LOCAL_BACKEND_KEY)
    expect(screen.getByTestId("stored-folders")).toBeEmptyDOMElement()
    expect(screen.getByTestId("stored-recents")).toBeEmptyDOMElement()
    expect(screen.getByTestId("stored-tabs")).toBeEmptyDOMElement()
    expect(loadLastActiveContext()).toBeNull()
    expect(peekClosedTab()).toBeNull()
    expect(loadMessageInputDraft(`draft:${draftId}`)).toBeNull()

    await activate("remote:7")
    expect(screen.getByTestId("stored-recents")).toHaveTextContent("回复确认")
    expect(screen.getByTestId("stored-tabs")).toHaveTextContent(draftId)
    expect(loadMessageInputDraft(`draft:${draftId}`)).toBe("堂务真实草稿")
    expect(loadLastActiveContext()).toEqual({ folderId: 1, isChat: false })
    expect(peekClosedTab()?.key).toBe("remote-file")
  })

  it("restores each backend's own draft tabs across repeated switches", async () => {
    render(
      <HallWorkspaceShell>
        <StoredRealm />
      </HallWorkspaceShell>
    )
    await settle()
    await act(async () => {
      useTabStore.getState().openNewConversationTab(0, "")
    })
    const localDraftId = useTabStore.getState().activeTabId!
    saveMessageInputDraft(`draft:${localDraftId}`, "本机草稿")
    await act(async () => useHallWorkspaceStore.getState().open(7, "堂务"))
    await settle()
    expect(screen.getByTestId("stored-tabs")).not.toHaveTextContent(
      localDraftId
    )
    await act(async () => {
      useTabStore.getState().openNewConversationTab(1, REMOTE_ROW.path)
    })
    const remoteDraftId = useTabStore.getState().activeTabId!
    saveMessageInputDraft(`draft:${remoteDraftId}`, "堂务草稿")
    for (let i = 0; i < 2; i++) {
      await activate(LOCAL_BACKEND_KEY)
      expect(screen.getByTestId("stored-tabs")).toHaveTextContent(localDraftId)
      expect(screen.getByTestId("stored-tabs")).not.toHaveTextContent(
        remoteDraftId
      )
      expect(loadMessageInputDraft(`draft:${localDraftId}`)).toBe("本机草稿")
      await activate("remote:7")
      expect(screen.getByTestId("stored-tabs")).toHaveTextContent(remoteDraftId)
      expect(screen.getByTestId("stored-tabs")).not.toHaveTextContent(
        localDraftId
      )
      expect(loadMessageInputDraft(`draft:${remoteDraftId}`)).toBe("堂务草稿")
    }
  })
})
