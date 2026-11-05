"use client"

// onehall: the expandable part of a pet-panel row — latest progress, a one-line
// prompt box, stop, and "open in workspace". Lightweight on purpose: it polls
// the session snapshot while open instead of attaching to the event stream,
// so the panel stays a glance-and-act surface, not a second chat window.

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { ExternalLink, Loader2, Send, Square } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { acpCancel, acpGetSessionSnapshot, acpPrompt } from "@/lib/api"
import { focusConversation } from "@/lib/pet/api"
import { petRemoteConnectionId } from "@/lib/pet/remote"
import type { PetSessionEntry } from "@/lib/pet/types"
import type { LiveSessionSnapshot } from "@/lib/types"
import { randomUUID } from "@/lib/utils"
import { TurnBusyError } from "@/lib/turn-busy"

const POLL_MS = 2000
const PROGRESS_MAX_CHARS = 240

interface SessionDetailProps {
  session: PetSessionEntry
}

/** Last visible text of the in-flight message, plus any running tool titles. */
export function summarizeProgress(snapshot: LiveSessionSnapshot | null): {
  text: string
  tools: string[]
} {
  if (!snapshot) return { text: "", tools: [] }
  let text = ""
  for (const block of snapshot.live_message?.content ?? []) {
    if (block.kind === "text" && block.text.trim()) text = block.text
  }
  text = text.trim()
  if (text.length > PROGRESS_MAX_CHARS) {
    text = "…" + text.slice(text.length - PROGRESS_MAX_CHARS)
  }
  const tools = (snapshot.active_tool_calls ?? [])
    .filter(
      (call) => call.status === "pending" || call.status === "in_progress"
    )
    .map((call) => call.label)
    .filter((label) => label.length > 0)
  return { text, tools }
}

export function SessionDetail({ session }: SessionDetailProps) {
  const t = useTranslations("Pet")
  const [snapshot, setSnapshot] = useState<LiveSessionSnapshot | null>(null)
  const [draft, setDraft] = useState("")
  const [sending, setSending] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)
  // The prompt in flight, id tied to its TEXT. A resend of the same text after
  // a dropped response reuses the id, so the server answers "already
  // accepted" instead of running it twice (see `recent_client_message_ids`);
  // edited text gets a fresh id, so it is never mistaken for that retry.
  const inFlightRef = useRef<{ id: string; text: string } | null>(null)

  useEffect(() => {
    let cancelled = false
    let inFlight = false
    const tick = async () => {
      // No overlapping requests on a slow link.
      if (inFlight) return
      inFlight = true
      try {
        const next = await acpGetSessionSnapshot(session.connectionId)
        if (!cancelled) setSnapshot(next)
      } catch {
        // A dropped link shows as the offline banner; nothing to add here.
      } finally {
        inFlight = false
      }
    }
    void tick()
    const id = setInterval(() => void tick(), POLL_MS)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [session.connectionId])

  const send = useCallback(async () => {
    const text = draft.trim()
    if (!text || sending) return
    setSending(true)
    setFailure(null)
    const retry = inFlightRef.current
    const clientMessageId =
      retry && retry.text === text ? retry.id : `optimistic-${randomUUID()}`
    inFlightRef.current = { id: clientMessageId, text }
    try {
      await acpPrompt(
        session.connectionId,
        [{ type: "text", text }],
        session.folderId,
        session.conversationId,
        clientMessageId
      )
      inFlightRef.current = null
      setDraft("")
    } catch (err) {
      // Busy (turn in flight) or a dropped link: keep the draft and the id so
      // a later retry of the same text dedups server-side.
      setFailure(
        err instanceof TurnBusyError ? t("panel.busy") : t("panel.sendFailed")
      )
    } finally {
      setSending(false)
    }
  }, [draft, sending, session, t])

  const stop = useCallback(() => {
    void acpCancel(session.connectionId).catch((err) =>
      console.warn("[PetPanel] cancel failed:", err)
    )
  }, [session.connectionId])

  const open = useCallback(() => {
    const target = session.parent ?? session
    void focusConversation(
      target.folderId,
      target.conversationId,
      target.agentType,
      petRemoteConnectionId()
    ).catch((err) => console.warn("[PetPanel] focus conversation failed:", err))
  }, [session])

  const { text, tools } = summarizeProgress(snapshot)
  const running = snapshot?.status === "prompting"

  return (
    <div className="mx-2 mb-1 flex flex-col gap-1.5 rounded-md bg-muted/40 px-2 py-1.5 text-xs">
      <div className="text-2xs font-medium text-muted-foreground">
        {t("panel.progress")}
      </div>
      {text || tools.length > 0 ? (
        <div className="space-y-0.5">
          {text ? (
            <div className="whitespace-pre-wrap break-words">{text}</div>
          ) : null}
          {tools.map((title) => (
            <div
              key={title}
              className="flex items-center gap-1 text-muted-foreground"
            >
              <Loader2 className="h-3 w-3 animate-spin" aria-hidden />
              <span className="truncate">{title}</span>
            </div>
          ))}
        </div>
      ) : (
        <div className="text-muted-foreground">{t("panel.noProgress")}</div>
      )}

      <form
        className="flex items-center gap-1"
        onSubmit={(e) => {
          e.preventDefault()
          void send()
        }}
      >
        <Input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder={t("panel.sendPlaceholder")}
          className="h-7 text-xs"
          disabled={sending}
        />
        <Button
          type="submit"
          size="icon-xs"
          variant="default"
          disabled={sending || !draft.trim()}
          aria-label={t("panel.send")}
          title={t("panel.send")}
        >
          {sending ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <Send className="h-3 w-3" />
          )}
        </Button>
      </form>
      {failure ? <div className="text-destructive">{failure}</div> : null}

      <div className="flex items-center gap-1">
        <Button
          type="button"
          size="xs"
          variant="outline"
          onClick={stop}
          disabled={!running}
          title={t("panel.cancel")}
        >
          <Square className="mr-1 h-3 w-3" />
          {t("panel.cancel")}
        </Button>
        <Button
          type="button"
          size="xs"
          variant="ghost"
          onClick={open}
          title={t("panel.open")}
        >
          <ExternalLink className="mr-1 h-3 w-3" />
          {t("panel.open")}
        </Button>
      </div>
    </div>
  )
}
