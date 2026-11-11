"use client"

import { useEffect, useLayoutEffect, useRef, type RefObject } from "react"
import type { QueuedMessage, UseMessageQueueReturn } from "./use-message-queue"
import { flushRetryDelayMs } from "@/lib/queue-flush"

export interface QueuedSendCallbacks {
  fromQueueFlush: true
  queueId: string
  onAccepted: () => void
  onNotSent: () => void
  onUnknown: () => void
}

/** Same flush for foreground and kept-alive background conversation tabs. */
export function useQueueAutoFlush({
  queue,
  ready,
  blocked,
  bounceAt,
  send,
  sendTimeMode,
}: {
  queue: UseMessageQueueReturn
  ready: boolean
  sendTimeMode?: string | null
  blocked: boolean
  bounceAt: RefObject<number>
  send: (item: QueuedMessage, callbacks: QueuedSendCallbacks) => void
}) {
  const latest = useRef({ queue, ready, blocked, send, sendTimeMode })
  useLayoutEffect(() => {
    latest.current = { queue, ready, blocked, send, sendTimeMode }
  })
  const alive = useRef(true)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])
  const headId = queue.queue[0]?.id
  const headStatus = queue.queue[0]?.status
  const pending = queue.queue.some(
    (item) => item.status && item.status !== "queued"
  )
  useEffect(() => {
    if (
      !ready ||
      blocked ||
      !headId ||
      pending ||
      (headStatus && headStatus !== "queued") ||
      queue.persistenceError
    )
      return
    const timer = setTimeout(
      () => {
        const origin = latest.current
        if (!origin.ready || origin.blocked) return
        void (async () => {
          const item = await origin.queue.beginSend(
            undefined,
            origin.sendTimeMode
          )
          if (!item) return
          const settle = origin.queue.settle
          if (
            !alive.current ||
            latest.current.queue.beginSend !== origin.queue.beginSend ||
            !latest.current.ready ||
            latest.current.blocked
          ) {
            settle(item.id, "busy", item.deliveryToken)
            return
          }
          try {
            origin.send(item, {
              fromQueueFlush: true,
              queueId: item.id,
              onAccepted: () => settle(item.id, "accepted", item.deliveryToken),
              onNotSent: () => {
                bounceAt.current = Date.now()
                settle(item.id, "busy", item.deliveryToken)
              },
              onUnknown: () => settle(item.id, "unknown", item.deliveryToken),
            })
          } catch {
            settle(item.id, "unknown", item.deliveryToken)
          }
        })()
      },
      flushRetryDelayMs(Date.now(), bounceAt.current)
    )
    return () => clearTimeout(timer)
  }, [
    ready,
    blocked,
    headId,
    headStatus,
    pending,
    queue.persistenceError,
    bounceAt,
  ])
}
