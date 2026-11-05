import { getTransport, type Transport } from "@/lib/transport"
import { isTasksUnavailable } from "./tasks"
import { toErrorMessage } from "@/lib/app-error"
import { createHallMembersClient } from "./members"

export interface PmBinding {
  memberId: string | null
  revision: number
}
export interface HallMessage {
  messageId: string
  senderMemberId: string | null
  recipientMemberId: string
  taskId: string | null
  attemptId: string | null
  kind: "message" | "report"
  body: string
  createdAt: string
  acknowledgedAt: string | null
  /** Current display names from the server (older servers omit them). */
  senderName?: string | null
  recipientName?: string | null
}
export function createHallMailboxClient(transport: Transport = getTransport()) {
  return {
    members: createHallMembersClient(transport).list,
    role: () => transport.call<PmBinding>("hall_get_pm_binding", {}),
    bind: (memberId: string | null, revision: number) =>
      transport.call<PmBinding>("hall_bind_pm", {
        memberId,
        expectedRevision: revision,
      }),
    inbox: (memberId: string) =>
      transport.call<HallMessage[]>("hall_read_mailbox", { memberId }),
    acknowledge: (memberId: string, messageId: string) =>
      transport.call<void>("hall_ack_message", { memberId, messageId }),
  }
}
export function isMailboxUnavailable(error: unknown) {
  return (
    isTasksUnavailable(error) ||
    /command\s+['"`]?hall_(?:get_pm_binding|bind_pm|read_mailbox|ack_message|send_message)['"`]?\s+not found/i.test(
      toErrorMessage(error)
    )
  )
}
