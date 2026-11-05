import { createTeamControlClient } from "./control"
import { extractAppCommandError, toErrorMessage } from "@/lib/app-error"
import { getTransport, type Transport } from "@/lib/transport"
import {
  createHallMembersClient,
  isMembersUnavailable,
  type HallExecutionCapabilities,
  type HallExecutionMode,
} from "./members"
import type { ExchangeManifest } from "./exchange"

export interface TaskSnapshot {
  snapshotId: string
  memberId: string
  memberRevision: number
  projectId: number | null
  snapshotVersion: string
}
export interface HallTask {
  taskId: string
  memberId: string
  projectId: number | null
  snapshot: TaskSnapshot
  purpose: "work" | "acceptance"
  title: string
  status:
    | "draft"
    | "cancelled"
    | "queued"
    | "interrupted"
    | "running"
    | "awaiting_permission"
    | "succeeded"
    | "failed"
  revision: number
  createdAt: string
  updatedAt: string
  launchAllowed: false
  runtimeVerified: false
}
export interface HallTaskDetail extends HallTask {
  description: string
  attempts: {
    attemptId: string
    attemptNumber: number
    status:
      | "queued"
      | "starting"
      | "interrupted"
      | "running"
      | "awaiting_permission"
      | "cancel_requested"
      | "cancelled"
      | "succeeded"
      | "failed"
    conversationId?: number | null
    startedAt?: string | null
    finishedAt?: string | null
  }[]
  runtimes?: {
    attemptId: string
    stage: string
    runtimeKind: string
    intentAt: string
    cancelRequestedAt: string | null
    cancelAckAt: string | null
    exitConfirmedAt: string | null
  }[]
  results?: {
    resultId: string
    attemptId: string
    outcome: string
    body: string
    createdAt: string
  }[]
  /** Key team actions that failed and were not retried successfully. */
  issues?: {
    attemptId: string
    tool:
      | "hall_report"
      | "hall_assign"
      | "hall_send_message"
      | "hall_dispatch_branch"
    detail: string
    createdAt: string
  }[]
  launchReviews?: { attemptId: string; planHash: string; approvedAt: string }[]
  exitChecks?: {
    attemptId: string
    rootReaped: boolean
    knownProcesses: number
    knownReaped: number
    pendingSpawns: number
    coverage: string
    complete: boolean
    reason: string
    observedAt: string
  }[]
  /** Present once an operator released a pending-review run (01). */
  resolution?: {
    kind: "operator_released"
    reason: string
    resolvedAt: string
  }
  /** Persisted start phases; used to tell an orphan from a plain failure. */
  startRecovery?: {
    attemptId: string
    phase: string
    failure: string | null
    failureCode: string | null
  }[]
  history: {
    revision: number
    action:
      | "draft_created"
      | "draft_updated"
      | "draft_cancelled"
      | "task_queued"
      | "attempt_claimed"
      | "attempt_interrupted"
      | "dispatch_intent"
      | "attempt_running"
      | "attempt_rejected"
      | "runtime_waiting"
      | "runtime_running"
      | "result_received"
      | "cancel_requested"
      | "cancel_acknowledged"
      | "exit_confirmed"
      | "human_best_effort_resolution"
      | "queued_cancelled"
      | "start_failed"
      | "startup_recovered"
      | "human_orphan_resolution"
    createdAt: string
  }[]
}
export interface LaunchReview {
  taskId: string
  taskRevision: number
  memberId: string
  memberRevision: number
  snapshotId: string
  snapshotVersion: string
  projectId: number | null
  title: string
  description: string
  cwd: string
  engine: string
  mode: string
  options: Record<string, string>
  brief: string
  /** Omitted for snapshots that never chose a mode (older servers too). */
  executionMode?: HallExecutionMode
  capabilities?: HallExecutionCapabilities
  /** Omitted when the task has no attachments. */
  exchangeManifestDigest?: string
  /** Display copy of the bound manifest; older servers omit it. */
  exchangeManifest?: ExchangeManifest
  planHash: string
  realExecutionAllowed: false
  admission: string
}
export interface TaskDraft {
  submissionId: string
  memberId: string
  projectId: number | null
  snapshotId: string
  purpose: "work" | "acceptance"
  title: string
  description: string
}
export interface TaskFilter {
  memberId?: string
  projectId?: number
  unassignedProject?: boolean
}
/** Bind once. Missing commands never fall back to the local shell. */
export function createHallTasksClient(transport: Transport = getTransport()) {
  const members = createHallMembersClient(transport)
  return {
    control: createTeamControlClient(transport),
    members: members.list,
    projects: members.projects,
    snapshots: members.listSnapshots,
    list: (filter: TaskFilter = {}) =>
      transport.call<HallTask[]>("hall_list_tasks", { filter }),
    get: (taskId: string) =>
      transport.call<HallTaskDetail>("hall_get_task", { taskId }),
    create: (draft: TaskDraft) =>
      transport.call<HallTask>("hall_create_task", { draft }),
    edit: (task: HallTask, draft: { title: string; description: string }) =>
      transport.call<HallTask>("hall_edit_task", {
        taskId: task.taskId,
        expectedRevision: task.revision,
        draft,
      }),
    review: (taskId: string) =>
      transport.call<LaunchReview>("hall_review_task", { taskId }),
    approve: (plan: LaunchReview, submissionId: string) =>
      transport.call<{
        attemptId: string
        taskId: string
        taskRevision: number
      }>("hall_approve_task", {
        approval: {
          taskId: plan.taskId,
          expectedRevision: plan.taskRevision,
          planHash: plan.planHash,
          submissionId,
        },
      }),
    cancelQueued: (task: HallTask) =>
      transport.call<void>("hall_cancel_queued_task", {
        taskId: task.taskId,
        expectedRevision: task.revision,
      }),
    cancel: (task: HallTask) =>
      transport.call<HallTask>("hall_cancel_task", {
        taskId: task.taskId,
        expectedRevision: task.revision,
      }),
  }
}
export type HallTasksClient = ReturnType<typeof createHallTasksClient>
export function isTasksUnavailable(error: unknown): boolean {
  const parsed = extractAppCommandError(error)
  if (isMembersUnavailable(error) || parsed?.code === "not_implemented")
    return true
  // Older desktop proxies cannot deserialize the server-only not_implemented
  // code and wrap its JSON in network_error.detail. Recognize that exact
  // shape, without treating an arbitrary 404/offline error as compatibility.
  if (parsed?.code === "network_error" && parsed.detail) {
    try {
      const nested: unknown = JSON.parse(parsed.detail)
      if (
        typeof nested === "object" &&
        nested !== null &&
        "code" in nested &&
        nested.code === "not_implemented"
      )
        return true
    } catch {
      /* Plain network/proxy errors remain normal errors. */
    }
  }
  return /command\s+['"`]?hall_(?:create_task|list_tasks|get_task|edit_task|cancel_task|review_task|approve_task|cancel_queued_task|team_control)['"`]?\s+not found/i.test(
    toErrorMessage(error)
  )
}
