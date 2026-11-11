import { isNoActiveTurnRejection } from "@/lib/turn-busy"

export async function deliverQueuedSteer(
  steer: () => Promise<unknown>,
  prioritize: () => void
): Promise<boolean> {
  try {
    await steer()
    return true
  } catch (error) {
    if (!isNoActiveTurnRejection(error)) throw error
    // Keep the row queued. The existing flush owns readiness, mode selection,
    // optimistic messages and busy retries; this path must not send a prompt.
    prioritize()
    return false
  }
}

/** Durable row lifecycle around the engine's native/pull steering IO. */
export async function deliverQueuedRowSteer(
  queue: Pick<
    import("@/hooks/use-message-queue").UseMessageQueueReturn,
    "beginSend" | "settle" | "moveToFront"
  >,
  id: string,
  steer: (draft: import("@/lib/types").PromptDraft) => Promise<unknown>,
  canSend: () => boolean
): Promise<boolean | undefined> {
  const claimed = await queue.beginSend(id)
  if (!claimed) return
  const settle = (outcome: "accepted" | "busy" | "unknown") =>
    queue.settle(id, outcome, claimed.deliveryToken)
  if (!canSend()) {
    settle("busy")
    return
  }
  try {
    const delivered = await deliverQueuedSteer(
      () => steer(claimed.draft),
      () => {
        settle("busy")
        queue.moveToFront(id)
      }
    )
    if (delivered) settle("accepted")
    return delivered
  } catch (error) {
    settle("unknown")
    throw error
  }
}
