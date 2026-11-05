import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { useEffect } from "react"
import { useHallSubmission } from "@/hooks/use-hall-submission"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import {
  getTimelineTurns,
  resetConversationRuntimeStore,
  useConversationRuntimeStore,
} from "@/stores/conversation-runtime-store"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  beginBackendSwitch,
  configureRemoteDesktopTransport,
  getTransport,
} from "@/lib/transport"
import { TauriTransport } from "@/lib/transport/tauri-transport"
import { RemoteDesktopTransport } from "@/lib/transport/remote-desktop-transport"
import { localTab, useHallWorkspaceStore } from "@/stores/hall-workspace-store"
import {
  consumeHallSubmission,
  getHallSubmission,
  submitHallConversation,
  type HallSubmissionInput,
} from "./submissions"
import { captureHallWorkspace, hasHallSubmission } from "./workspace-lifetime"

const remote = {
  id: 7,
  name: "堂务",
  baseUrl: "https://hall.test",
  token: "fixture",
  windowInstanceId: "w",
}
type Call = { backend: string; command: string; args: Record<string, unknown> }
let calls: Call[]
let completeCreate: (id: number) => void
let completeMode: () => void
let rejectPrompt: ((error: unknown) => void) | null
let holdMode: boolean
let holdPrompt: boolean
let messages: Map<string, unknown[]>
let serial = 0
let surface: string
let parseHistory: boolean
const numbers = Array.from({ length: 300 }, (_, i) => String(i + 1)).join("\n")

const ipc = {
  transformCallback: () => 0,
  invoke: (command: string, args: Record<string, unknown> = {}) => {
    const backend = command === "remote_http_call" ? "remote:7" : "local"
    const cmd =
      command === "remote_http_call" ? (args.command as string) : command
    const payload =
      command === "remote_http_call"
        ? (args.args as Record<string, unknown>)
        : args
    calls.push({ backend, command: cmd, args: payload })
    if (cmd === "create_conversation" || cmd === "create_chat_conversation") {
      return new Promise((resolve) => {
        completeCreate = (id) =>
          resolve(
            cmd === "create_conversation"
              ? id
              : {
                  conversationId: id,
                  folderId: 2,
                  folder: { id: 2, path: "/remote/chat" },
                }
          )
      })
    }
    if (cmd === "acp_set_mode" && holdMode) {
      return new Promise<void>((resolve) => {
        completeMode = resolve
      })
    }
    if (cmd === "acp_prompt") {
      if (holdPrompt)
        return new Promise((_resolve, reject) => {
          rejectPrompt = reject
        })
      messages.set(backend, [...(messages.get(backend) ?? []), payload.blocks])
    }
    if (cmd === "get_folder_conversation" && parseHistory) {
      return Promise.resolve({
        summary: {
          id: payload.conversationId,
          external_id: "original-execution",
          kind: "regular",
        },
        turns: messages.get(backend)?.length
          ? [
              {
                id: "user-count",
                role: "user",
                blocks: [{ type: "text", text: "count" }],
                timestamp: "2026-10-18T00:00:00Z",
              },
              {
                id: "assistant-count",
                role: "assistant",
                blocks: [{ type: "text", text: numbers }],
                timestamp: "2026-10-18T00:00:01Z",
              },
            ]
          : [],
      })
    }
    if (cmd === "get_folder_conversation")
      return Promise.resolve(messages.get(backend) ?? [])
    return Promise.resolve(undefined)
  },
}

function switchTo(backend: "local" | "remote:7") {
  useHallWorkspaceStore.getState().activate(backend)
  beginBackendSwitch()
  if (backend === "remote:7") configureRemoteDesktopTransport(remote)
}

function input(extra: Partial<HallSubmissionInput> = {}): HallSubmissionInput {
  return {
    surfaceKey: surface,
    connectionId: "original-connection",
    folderId: 2,
    conversationId: null,
    agentType: "claude_code",
    title: "count",
    draft: { displayText: "count", blocks: [{ type: "text", text: "count" }] },
    clientMessageId: "optimistic-g4",
    ...extra,
  }
}

function Probe() {
  const job = useHallSubmission(surface)
  return (
    <output data-testid="job">
      {job ? `${job.status}:${job.result?.conversationId ?? ""}` : "none"}
    </output>
  )
}

function HistoryProbe() {
  const job = useHallSubmission(surface)
  const runtimeId = -800
  const boundId = useConversationRuntimeStore(
    (state) => state.byConversationId.get(runtimeId)?.dbConversationId ?? null
  )
  const dbConversationId = boundId ?? job?.result?.conversationId ?? null
  const view = useConversationDetail(runtimeId, { dbConversationId })
  useEffect(() => {
    if (job?.status !== "accepted") return
    consumeHallSubmission(job)
  }, [job])
  const text = getTimelineTurns(runtimeId)
    .flatMap(({ turn }) =>
      turn.blocks.flatMap((block) =>
        block.type === "text" ? [block.text] : []
      )
    )
    .join("\n")
  return (
    <output data-testid="history">
      {view.loading ? "loading" : text || "empty"}
    </output>
  )
}

async function flush() {
  for (let i = 0; i < 8; i++)
    await new Promise((resolve) => setTimeout(resolve, 0))
}

beforeEach(() => {
  calls = []
  holdMode = false
  holdPrompt = false
  rejectPrompt = null
  parseHistory = false
  messages = new Map()
  surface = `draft-${++serial}`
  Object.assign(window, { __TAURI_INTERNALS__: ipc })
  __setTransportFactoriesForTests({
    tauri: () => new TauriTransport(),
    remote: (config) => new RemoteDesktopTransport(config),
  })
  __resetTransportForTests()
  useHallWorkspaceStore.setState({
    enabled: true,
    tabs: [
      localTab("本机"),
      { key: "remote:7", remoteConnectionId: 7, name: "堂务", windowId: "w" },
    ],
    activeKey: "local",
  })
  switchTo("remote:7")
})

afterEach(() => {
  cleanup()
  resetConversationRuntimeStore()
  for (const backend of ["local", "remote:7"]) {
    const job = getHallSubmission(backend, surface)
    if (job) consumeHallSubmission(job)
  }
  __resetTransportForTests()
  useHallWorkspaceStore.setState({ enabled: false })
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__")
})

describe("submitted conversation survives a workspace switch", () => {
  it("restores actual user and completed assistant turns after the draft workspace remounts", async () => {
    parseHistory = true
    const origin = render(<HistoryProbe />)
    act(() => {
      submitHallConversation(input())
    })
    origin.unmount()
    switchTo("local")
    resetConversationRuntimeStore()
    render(<HistoryProbe />)
    await act(async () => {
      completeCreate(8)
      await flush()
    })
    expect(screen.getByTestId("history")).toHaveTextContent("empty")
    expect(
      calls.filter((call) => call.command === "get_folder_conversation")
    ).toHaveLength(0)

    cleanup()
    switchTo("remote:7")
    resetConversationRuntimeStore()
    render(<HistoryProbe />)
    await act(async () => {
      await flush()
    })
    expect(screen.getByTestId("history").textContent).toBe(`count\n${numbers}`)
    expect(getTimelineTurns(-800).map(({ turn }) => turn.role)).toEqual([
      "user",
      "assistant",
    ])
    expect(
      useConversationRuntimeStore.getState().byConversationId.get(-800)?.detail
        ?.summary.external_id
    ).toBe("original-execution")
    const reads = calls.filter(
      (call) => call.command === "get_folder_conversation"
    )
    expect(reads).toHaveLength(1)
    expect(reads[0]).toMatchObject({
      backend: "remote:7",
      args: { conversationId: 8 },
    })
    expect(calls.filter((call) => call.command === "acp_prompt")).toHaveLength(
      1
    )
  })
  it("finishes create → mode → prompt on the remote and restores its result only there", async () => {
    const view = render(<Probe />)
    act(() => {
      submitHallConversation(
        input({ modeId: "plan", currentModeId: "default" })
      )
    })
    expect(hasHallSubmission("original-connection")).toBe(true)
    view.unmount()
    switchTo("local")
    render(<Probe />)
    await act(async () => {
      completeCreate(7)
      await flush()
    })
    expect(screen.getByTestId("job")).toHaveTextContent("none")
    expect(
      calls
        .filter((c) =>
          ["create_conversation", "acp_set_mode", "acp_prompt"].includes(
            c.command
          )
        )
        .map((c) => [c.backend, c.command])
    ).toEqual([
      ["remote:7", "create_conversation"],
      ["remote:7", "acp_set_mode"],
      ["remote:7", "acp_prompt"],
    ])
    expect(calls.find((c) => c.command === "acp_prompt")?.args).toMatchObject({
      connectionId: "original-connection",
      conversationId: 7,
      folderId: 2,
      clientMessageId: "optimistic-g4",
    })
    expect(hasHallSubmission("original-connection")).toBe(false)
    cleanup()
    switchTo("remote:7")
    render(<Probe />)
    expect(screen.getByTestId("job")).toHaveTextContent("accepted:7")
    await expect(
      getTransport().call("get_folder_conversation", { conversationId: 7 })
    ).resolves.toEqual([[{ type: "text", text: "count" }]])
  })

  it("keeps an existing conversation's mode change and prompt on the local backend", async () => {
    switchTo("local")
    holdMode = true
    submitHallConversation(input({ conversationId: 7, modeId: "plan" }))
    await flush()
    switchTo("remote:7")
    completeMode()
    await flush()
    expect(
      calls
        .filter((c) => ["acp_set_mode", "acp_prompt"].includes(c.command))
        .map((c) => c.backend)
    ).toEqual(["local", "local"])
    expect(getHallSubmission("remote:7", surface)).toBeNull()
    expect(getHallSubmission("local", surface)?.status).toBe("accepted")
  })

  it("continues after closing the remote workspace tab and recovers when reopened", async () => {
    const lifetime = captureHallWorkspace()!
    submitHallConversation(input())
    useHallWorkspaceStore.getState().close("remote:7")
    expect(lifetime.isLeaving()).toBe(true)
    beginBackendSwitch()
    completeCreate(8)
    await flush()
    expect(messages.get("local")).toBeUndefined()
    useHallWorkspaceStore.getState().open(7, "堂务")
    beginBackendSwitch()
    configureRemoteDesktopTransport(remote)
    render(<Probe />)
    expect(screen.getByTestId("job")).toHaveTextContent("accepted:8")
  })

  it("uses the original chat directory and hidden folder after switching away", async () => {
    submitHallConversation(input({ chat: { existingDir: "/remote/chat" } }))
    switchTo("local")
    completeCreate(9)
    await flush()
    expect(
      calls.find((c) => c.command === "create_chat_conversation")?.args
    ).toMatchObject({ existingDir: "/remote/chat" })
    expect(calls.find((c) => c.command === "acp_prompt")?.args).toMatchObject({
      conversationId: 9,
      folderId: 2,
    })
    expect(
      getHallSubmission("remote:7", surface)?.result?.chat?.folder.path
    ).toBe("/remote/chat")
  })

  it("does not retry a failed write or publish its error into the new workspace", async () => {
    holdPrompt = true
    submitHallConversation(input({ conversationId: 7 }))
    switchTo("local")
    await flush()
    rejectPrompt!({
      code: "network_error",
      message: "Remote returned HTTP 502",
    })
    await flush()
    expect(calls.filter((c) => c.command === "acp_prompt")).toHaveLength(1)
    expect(getHallSubmission("local", surface)).toBeNull()
    expect(getHallSubmission("remote:7", surface)?.status).toBe("failed")
    expect(hasHallSubmission("original-connection")).toBe(false)
  })

  it("single-flights a new conversation while its create is pending", async () => {
    submitHallConversation(input())
    submitHallConversation(input())
    expect(
      calls.filter((c) => c.command === "create_conversation")
    ).toHaveLength(1)
    completeCreate(10)
    await flush()
    expect(calls.filter((c) => c.command === "acp_prompt")).toHaveLength(1)
  })

  it.each(["local", "remote:7"] as const)(
    "keeps %s image encoding after the backend changes during set-mode",
    async (origin) => {
      switchTo(origin)
      holdMode = true
      submitHallConversation(
        input({
          conversationId: 7,
          modeId: "plan",
          draft: {
            displayText: "image",
            blocks: [
              {
                type: "image",
                data: "fixture-bytes",
                mime_type: "image/png",
                uri: "file:///uploads/image.png",
              },
            ],
          },
        })
      )
      await flush()
      switchTo(origin === "local" ? "remote:7" : "local")
      completeMode()
      await flush()
      const prompt = calls.find((call) => call.command === "acp_prompt")!
      expect(prompt.backend).toBe(origin)
      expect(prompt.args.blocks).toEqual([
        {
          type: "image",
          data: origin === "local" ? "fixture-bytes" : "",
          mime_type: "image/png",
          uri: "file:///uploads/image.png",
        },
      ])
    }
  )
})
