import { stripUploadedImagePayloads } from "@/lib/api"
import { captureSubmissionBackend, getActiveBackendKey } from "@/lib/transport"
import { isTurnInProgressRejection, TurnBusyError } from "@/lib/turn-busy"
import type {
  AgentType,
  CreateChatConversationResult,
  PromptDraft,
} from "@/lib/types"
import { useHallWorkspaceStore } from "@/stores/hall-workspace-store"
import { retainHallSubmission } from "./workspace-lifetime"

export interface HallSubmissionInput {
  surfaceKey: string
  connectionId: string
  folderId: number
  conversationId: number | null
  agentType: AgentType
  title: string
  draft: PromptDraft
  clientMessageId: string
  modeId?: string | null
  currentModeId?: string | null
  chat?: { existingDir?: string }
  queueId?: string
  fromQueueFlush?: boolean
}

export interface HallSubmission {
  input: HallSubmissionInput
  backendKey: string
  status: "pending" | "accepted" | "failed"
  result?: {
    conversationId: number
    folderId: number
    chat?: CreateChatConversationResult
  }
  error?: unknown
}

// This is a window-local handoff, NOT a transcript cache. Only a view in the
// originating backend/surface can consume its result after remounting. Nothing
// here mutates the active workspace's stores, tabs, drafts, or notifications.
const jobs = new Map<string, HallSubmission>()
const listeners = new Set<() => void>()
const jobKey = (backend: string, surface: string) =>
  JSON.stringify([backend, surface])

export function subscribeHallSubmissions(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function hallSubmissionBackendKey(): string {
  return getActiveBackendKey()
}

export function getHallSubmission(backend: string, surface: string) {
  return jobs.get(jobKey(backend, surface)) ?? null
}

export function isHallSubmissionPending(surface: string): boolean {
  return (
    getHallSubmission(hallSubmissionBackendKey(), surface)?.status === "pending"
  )
}

export function consumeHallSubmission(job: HallSubmission) {
  const key = jobKey(job.backendKey, job.input.surfaceKey)
  if (jobs.get(key) !== job || job.status === "pending") return
  jobs.delete(key)
  listeners.forEach((listener) => listener())
}

/** Starts synchronously in the send gesture, before the shell can unmount.
 * No write is retried. A lost response remains a failed/uncertain submission. */
export function submitHallConversation(
  input: HallSubmissionInput,
  onOutcome?: (outcome: "accepted" | "busy" | "unknown") => void
): boolean {
  if (!useHallWorkspaceStore.getState().enabled) return false
  const backend = captureSubmissionBackend()
  const key = jobKey(backend.key, input.surfaceKey)
  if (jobs.get(key)?.status === "pending") return true
  const release = retainHallSubmission(input.connectionId, backend.key)
  const job: HallSubmission = {
    input,
    backendKey: backend.key,
    status: "pending",
  }
  jobs.set(key, job)
  listeners.forEach((listener) => listener())

  void (async () => {
    let conversationId = input.conversationId
    let folderId = input.folderId
    let chat: CreateChatConversationResult | undefined
    try {
      if (conversationId === null) {
        if (input.chat) {
          chat = await backend.call<CreateChatConversationResult>(
            "create_chat_conversation",
            {
              agentType: input.agentType,
              title: input.title,
              existingDir: input.chat.existingDir ?? null,
            }
          )
          conversationId = chat.conversationId
          folderId = chat.folderId
        } else {
          conversationId = await backend.call<number>("create_conversation", {
            folderId,
            agentType: input.agentType,
            title: input.title,
          })
        }
      }
      if (input.modeId && input.modeId !== input.currentModeId) {
        await backend.call("acp_set_mode", {
          connectionId: input.connectionId,
          modeId: input.modeId,
        })
      }
      await backend.call("acp_prompt", {
        connectionId: input.connectionId,
        blocks: stripUploadedImagePayloads(
          input.draft.blocks,
          backend.stripImages
        ),
        folderId,
        conversationId,
        clientMessageId: input.clientMessageId,
      })
      onOutcome?.("accepted")
      jobs.set(key, {
        ...job,
        status: "accepted",
        result: { conversationId, folderId, chat },
      })
    } catch (error) {
      onOutcome?.(isTurnInProgressRejection(error) ? "busy" : "unknown")
      jobs.set(key, {
        ...job,
        status: "failed",
        result:
          conversationId === null
            ? undefined
            : { conversationId, folderId, chat },
        error: isTurnInProgressRejection(error) ? new TurnBusyError() : error,
      })
    } finally {
      release()
      listeners.forEach((listener) => listener())
    }
  })()
  return true
}
