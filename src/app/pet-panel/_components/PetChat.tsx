"use client"

import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2, Send, Square } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  acpCancel,
  acpFindConnectionForConversation,
  acpGetSessionSnapshot,
  acpPrompt,
  getFolderConversation,
  listAllConversations,
  listAllFolderDetails,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import {
  connectPetChat,
  lastAssistantText,
  petChatStorageKey,
} from "@/lib/pet/chat"
import { petRemoteConnectionId } from "@/lib/pet/remote"
import { getShellTransport } from "@/lib/transport"
import { randomUUID } from "@/lib/utils"
import type {
  DbConversationSummary,
  FolderDetail,
  LiveSessionSnapshot,
} from "@/lib/types"
import { PanelPermissionCard } from "./PanelPermissionCard"
import { summarizeProgress } from "./SessionDetail"

function read(key: string): string {
  try {
    return localStorage.getItem(key) ?? ""
  } catch {
    return ""
  }
}
function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value)
  } catch {
    /* storage unavailable */
  }
}

function savedRequest(key: string): { text: string; id: string } | null {
  try {
    const value = JSON.parse(read(key))
    return value &&
      typeof value.text === "string" &&
      typeof value.id === "string"
      ? value
      : null
  } catch {
    return null
  }
}

export function PetChat({
  remoteConnectionId = petRemoteConnectionId(),
}: {
  remoteConnectionId?: number | null
}) {
  const t = useTranslations("Pet.chat")
  // The pet gate deliberately leaves its children mounted on errors to show
  // the offline sprite. Its missing transport must never mean "use local".
  if (remoteConnectionId !== petRemoteConnectionId()) {
    return (
      <p role="alert" className="p-3 text-xs text-destructive">
        {t("unavailable")}
      </p>
    )
  }
  return <BoundPetChat remoteId={remoteConnectionId} />
}

function BoundPetChat({ remoteId }: { remoteId: number | null }) {
  const t = useTranslations("Pet.chat")
  const targetKey = petChatStorageKey(remoteId)
  const [targets, setTargets] = useState<DbConversationSummary[]>([])
  const [folders, setFolders] = useState<FolderDetail[]>([])
  const [targetId, setTargetId] = useState(() => read(targetKey))
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [revision, setRevision] = useState(0)
  useEffect(() => {
    let cancelled = false
    void Promise.all([listAllConversations(), listAllFolderDetails()])
      .then(([list, dirs]) => {
        if (cancelled) return
        const visible = list
          .filter(
            (item) =>
              !item.parent_id &&
              item.kind === "regular" &&
              dirs.some((dir) => dir.id === item.folder_id)
          )
          .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
        setTargets(visible)
        setFolders(dirs)
        setTargetId((saved) =>
          visible.some((item) => String(item.id) === saved)
            ? saved
            : String(visible[0]?.id ?? "")
        )
        setError(null)
      })
      .catch((err) => {
        if (!cancelled) setError(toErrorMessage(err))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [revision])
  const target = targets.find((item) => String(item.id) === targetId)
  const folder = folders.find((item) => item.id === target?.folder_id)
  useEffect(() => {
    if (target) write(targetKey, String(target.id))
  }, [targetKey, target])
  return (
    <div className="space-y-2 p-3 text-xs">
      <div className="space-y-1">
        <div>{t("target")}</div>
        <Select
          value={targetId}
          disabled={loading}
          onValueChange={(value) => {
            setTargetId(value)
            write(targetKey, value)
          }}
        >
          <SelectTrigger aria-label={t("target")} className="h-8 w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {targets.map((item) => (
              <SelectItem key={item.id} value={String(item.id)}>
                {item.title || `${item.agent_type} #${item.id}`}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
      {error ? (
        <div role="alert" className="space-y-1 text-destructive">
          {error}
          <Button
            size="xs"
            variant="outline"
            onClick={() => {
              setLoading(true)
              setRevision((value) => value + 1)
            }}
          >
            {t("refresh")}
          </Button>
        </div>
      ) : null}
      {!loading && !error && !target ? <p>{t("empty")}</p> : null}
      {target && folder ? (
        <Conversation
          key={target.id}
          target={target}
          folder={folder}
          remoteId={remoteId}
        />
      ) : null}
    </div>
  )
}

function Conversation({
  target,
  folder,
  remoteId,
}: {
  target: DbConversationSummary
  folder: FolderDetail
  remoteId: number | null
}) {
  const t = useTranslations("Pet.chat")
  const unavailable = t("unavailable")
  const draftKey = petChatStorageKey(remoteId, target.id)
  const requestKey = `${draftKey}:request`
  const [draft, setDraft] = useState(() => read(draftKey))
  const [reply, setReply] = useState("")
  const [snapshot, setSnapshot] = useState<LiveSessionSnapshot | null>(null)
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pollError, setPollError] = useState<string | null>(null)
  const input = useRef<HTMLInputElement>(null)
  const inFlight = useRef<{ text: string; id: string } | null>(null)
  const connection = useRef<string | null>(null)
  const sendingRef = useRef(false)
  const alive = useRef(true)

  useEffect(() => {
    if (!sending) input.current?.focus()
  }, [sending])

  useEffect(() => {
    alive.current = true
    input.current?.focus()
    const focus = () => input.current?.focus()
    const pending = getShellTransport()
      .subscribe("pet://focus-chat", focus)
      .catch(() => () => {})
    return () => {
      alive.current = false
      void pending.then((off) => off())
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    let polling = false
    const poll = async () => {
      if (polling) return
      polling = true
      try {
        if (petRemoteConnectionId() !== remoteId) throw new Error(unavailable)
        const live = await acpFindConnectionForConversation(
          target.id,
          target.external_id ?? undefined,
          target.agent_type
        )
        const [detail, state] = await Promise.all([
          getFolderConversation(target.id, { tailTurns: 4 }),
          live
            ? acpGetSessionSnapshot(live.connection_id)
            : Promise.resolve(null),
        ])
        if (cancelled || petRemoteConnectionId() !== remoteId) return
        connection.current = live?.connection_id ?? connection.current
        setSnapshot(state)
        setPollError(null)
        const progress = summarizeProgress(state).text
        setReply(progress || lastAssistantText(detail.turns))
      } catch (err) {
        if (!cancelled) setPollError(toErrorMessage(err))
      } finally {
        polling = false
      }
    }
    void poll()
    const timer = setInterval(() => void poll(), 2_000)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [target, remoteId, unavailable])

  const send = async () => {
    const text = draft.trim()
    if (!text || sendingRef.current) return
    sendingRef.current = true
    setSending(true)
    setError(null)
    const previous = inFlight.current ?? savedRequest(requestKey)
    const request =
      previous?.text === text
        ? previous
        : { text, id: `optimistic-${randomUUID()}` }
    inFlight.current = request
    write(requestKey, JSON.stringify(request))
    try {
      if (petRemoteConnectionId() !== remoteId) throw new Error(unavailable)
      const id = await connectPetChat(target, folder)
      if (petRemoteConnectionId() !== remoteId) throw new Error(unavailable)
      connection.current = id
      await acpPrompt(
        id,
        [{ type: "text", text }],
        target.folder_id,
        target.id,
        request.id
      )
      // The panel may have closed/reopened while a response was delayed.
      // Never erase a newer draft entered in that replacement window.
      if (read(draftKey).trim() === text) write(draftKey, "")
      if (savedRequest(requestKey)?.id === request.id) write(requestKey, "")
      inFlight.current = null
      if (alive.current) setDraft("")
    } catch (err) {
      if (alive.current) setError(toErrorMessage(err))
    } finally {
      sendingRef.current = false
      if (alive.current) {
        setSending(false)
        input.current?.focus()
      }
    }
  }
  const pending = snapshot?.pending_permission
  return (
    <div className="space-y-2">
      <div
        className="max-h-48 overflow-y-auto whitespace-pre-wrap break-words rounded-md bg-muted/40 p-2"
        aria-live="polite"
      >
        {reply || t("ready")}
      </div>
      {pending && connection.current ? (
        <PanelPermissionCard
          connectionId={connection.current}
          agentType={target.agent_type}
          permission={{
            requestId: pending.request_id,
            toolCall: pending.tool_call,
            options: pending.options,
          }}
        />
      ) : null}
      <form
        className="flex items-center gap-1"
        onSubmit={(event) => {
          event.preventDefault()
          void send()
        }}
      >
        <Input
          ref={input}
          value={draft}
          aria-label={t("input")}
          placeholder={t("input")}
          disabled={sending}
          onChange={(event) => {
            setDraft(event.target.value)
            write(draftKey, event.target.value)
          }}
          className="text-xs"
        />
        <Button
          type="submit"
          size="icon-xs"
          disabled={sending || !draft.trim()}
          aria-label={t("send")}
        >
          {sending ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <Send className="h-3 w-3" />
          )}
        </Button>
        {snapshot?.status === "prompting" && connection.current ? (
          <Button
            type="button"
            size="icon-xs"
            variant="outline"
            aria-label={t("stop")}
            onClick={() =>
              void acpCancel(connection.current!).catch((err) =>
                setError(toErrorMessage(err))
              )
            }
          >
            <Square className="h-3 w-3" />
          </Button>
        ) : null}
      </form>
      {error || pollError ? (
        <p role="alert" className="text-destructive">
          {error ?? pollError}
        </p>
      ) : null}
    </div>
  )
}
