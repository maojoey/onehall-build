import { getTransport, type Transport } from "@/lib/transport"
import type { HallTaskDetail } from "./tasks"
import type { HallEngine } from "./members"
export interface TeamReadiness {
  enabled: boolean
  nativeVerified: boolean
  runRoot: boolean
  reason:
    | "ready"
    | "run_root_unavailable"
    | "native_probe_pending"
    | "native_probe_failed"
    | "transport_unavailable"
  platform: string
  nativeProbeSupported: boolean
  transportReady: boolean
  coverage?: string
  completeDescendantCoverage?: boolean
}
export interface TeamSession {
  conversationId: number
  connectionId: string | null
  agentType: HallEngine
}
export function createTeamControlClient(transport: Transport = getTransport()) {
  const call = <T>(request: Record<string, unknown>) =>
    transport.call<T>("hall_team_control", { request })
  return {
    session: (taskId: string) =>
      call<TeamSession>({ action: "session", task_id: taskId }),
    state: () => call<TeamReadiness>({ action: "state" }),
    probe: () => call({ action: "native_probe" }),
    dispatch: (task: HallTaskDetail) =>
      call<HallTaskDetail>({
        action: "dispatch",
        task_id: task.taskId,
        expected_revision: task.revision,
      }),
    refresh: (taskId: string) =>
      call<HallTaskDetail>({ action: "refresh", task_id: taskId }),
    cancel: (task: HallTaskDetail) =>
      call<HallTaskDetail>({
        action: "cancel",
        task_id: task.taskId,
        expected_revision: task.revision,
      }),
    recover: (taskId: string) =>
      call<HallTaskDetail>({ action: "recover", task_id: taskId }),
    /** Local operator only; backend authority decides, not this client. */
    resolveOrphan: (task: HallTaskDetail, attemptId: string, reason: string) =>
      call<HallTaskDetail>({
        action: "resolve_orphan",
        task_id: task.taskId,
        attempt_id: attemptId,
        expected_revision: task.revision,
        acknowledge_unverified_processes: true,
        reason,
      }),
    resolveBestEffort: (task: HallTaskDetail) =>
      call<HallTaskDetail>({
        action: "resolve_best_effort",
        task_id: task.taskId,
        expected_revision: task.revision,
        acknowledge_unverified_descendants: true,
      }),
  }
}
export type TeamControlClient = ReturnType<typeof createTeamControlClient>
