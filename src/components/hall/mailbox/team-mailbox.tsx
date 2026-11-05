"use client"
import { useEffect, useRef, useState, useCallback } from "react"
import { useTranslations } from "next-intl"
import { HallMark } from "@/components/hall/hall-mark"
import { Mail, RefreshCw } from "lucide-react"
import { BackendBadge } from "@/components/layout/backend-badge"
import { SettingsSection } from "@/components/shared/settings-section"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useRemoteConnection } from "@/contexts/remote-connection-context"
import {
  createHallMailboxClient,
  isMailboxUnavailable,
  type PmBinding,
  type HallMessage,
} from "@/lib/hall/mailbox"
import type { HallMember } from "@/lib/hall/members"
import { toErrorMessage } from "@/lib/app-error"
import { selectClass } from "../tasks/task-editor"
export function TeamMailbox() {
  const remote = useRemoteConnection()?.connection
  return <MailboxBody key={remote?.id ?? "local"} />
}
function MailboxBody() {
  const t = useTranslations("Hall.mailbox")
  const [client] = useState(() =>
    typeof window === "undefined" ? null : createHallMailboxClient()
  )
  const [members, setMembers] = useState<HallMember[]>([])
  const [binding, setBinding] = useState<PmBinding | null>(null)
  const [pm, setPm] = useState("")
  const [recipient, setRecipient] = useState("")
  const [messages, setMessages] = useState<HallMessage[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const alive = useRef(false)
  const inboxRequest = useRef(0)
  const loadRequest = useRef(0)
  useEffect(() => {
    const inboxCursor = inboxRequest
    const loadCursor = loadRequest
    alive.current = true
    return () => {
      alive.current = false
      inboxCursor.current++
      loadCursor.current++
    }
  }, [])
  const failure = useCallback(
    (e: unknown) =>
      isMailboxUnavailable(e)
        ? t("unsupported")
        : t("failure", { message: toErrorMessage(e) }),
    [t]
  )
  const load = useCallback(async () => {
    if (!client) return
    const current = ++loadRequest.current
    setLoading(true)
    setError(null)
    try {
      const [rows, role] = await Promise.all([client.members(), client.role()])
      if (alive.current && current === loadRequest.current) {
        setMembers(rows)
        setBinding(role)
        setPm(role.memberId ?? "")
      }
    } catch (e) {
      if (alive.current && current === loadRequest.current) {
        setError(failure(e))
        setBinding(null)
        setMembers([])
      }
    } finally {
      if (alive.current && current === loadRequest.current) setLoading(false)
    }
  }, [client, failure])
  useEffect(() => {
    void load()
  }, [load])
  const read = useCallback(
    async (id: string) => {
      const request = ++inboxRequest.current
      setMessages([])
      if (!client || !id) return
      setError(null)
      try {
        const rows = await client.inbox(id)
        if (alive.current && request === inboxRequest.current) setMessages(rows)
      } catch (e) {
        if (alive.current && request === inboxRequest.current)
          setError(failure(e))
      }
    },
    [client, failure]
  )
  useEffect(() => {
    void read(recipient)
  }, [recipient, read])
  async function savePm() {
    if (!client || !binding || busy) return
    setBusy(true)
    setError(null)
    try {
      const saved = await client.bind(pm || null, binding.revision)
      if (alive.current) {
        setBinding(saved)
        setPm(saved.memberId ?? "")
      }
    } catch (e) {
      if (alive.current) {
        setBinding(null)
        setError(t("saveUncertain", { message: toErrorMessage(e) }))
      }
    } finally {
      if (alive.current) setBusy(false)
    }
  }
  async function acknowledge(messageId: string) {
    if (!client || !recipient || busy) return
    const owner = recipient
    setBusy(true)
    setError(null)
    try {
      await client.acknowledge(owner, messageId)
      if (alive.current) await read(owner)
    } catch (e) {
      if (alive.current) setError(failure(e))
    } finally {
      if (alive.current) setBusy(false)
    }
  }
  const name = (id: string | null, fallback?: string | null) =>
    id === null
      ? t("human")
      : (members.find((m) => m.memberId === id)?.name ??
        fallback ??
        t("unknownMember"))
  return (
    <ScrollArea className="h-full">
      <div className="mx-auto max-w-5xl space-y-6 p-4 sm:p-6">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h1 className="inline-flex items-center gap-1.5 text-lg font-semibold">
            {t("title")}
            <HallMark />
          </h1>
          <BackendBadge />
        </div>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        <SettingsSection
          icon={Mail}
          title={t("pm")}
          description={t("admissionClosed")}
        >
          <div className="flex flex-wrap gap-2">
            <select
              aria-label={t("pm")}
              className={selectClass}
              disabled={loading || busy || !binding}
              value={pm}
              onChange={(e) => setPm(e.target.value)}
            >
              <option value="">{t("unbound")}</option>
              {members.map((m) => (
                <option key={m.memberId} value={m.memberId}>
                  {m.name}
                </option>
              ))}
            </select>
            <Button
              size="sm"
              variant="outline"
              disabled={
                loading || busy || !binding || (pm || null) === binding.memberId
              }
              onClick={() => void savePm()}
            >
              {t("savePm")}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={loading || busy}
              onClick={() => void load()}
            >
              <RefreshCw className={loading ? "animate-spin" : ""} />
              {t("refresh")}
            </Button>
          </div>
        </SettingsSection>
        <SettingsSection title={t("inbox")} description={t("noWakeup")}>
          <select
            aria-label={t("recipient")}
            className={selectClass}
            disabled={busy || loading || !binding}
            value={recipient}
            onChange={(e) => setRecipient(e.target.value)}
          >
            <option value="">{t("chooseMember")}</option>
            {members.map((m) => (
              <option key={m.memberId} value={m.memberId}>
                {m.name}
              </option>
            ))}
          </select>
          <Button
            size="sm"
            variant="outline"
            disabled={busy || loading || !recipient}
            onClick={() => void read(recipient)}
          >
            {t("read")}
          </Button>
          {loading && (
            <p role="status" className="text-sm text-muted-foreground">
              {t("loading")}
            </p>
          )}
          {recipient && !messages.length && (
            <p className="rounded-lg border border-dashed p-6 text-sm text-muted-foreground">
              {t("empty")}
            </p>
          )}
          <div className="space-y-3">
            {messages.map((m) => (
              <div
                key={m.messageId}
                className="space-y-2 rounded-lg border bg-card p-4 text-sm"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="font-medium">
                    {m.senderMemberId === null && m.kind === "report"
                      ? t("branchReport")
                      : name(m.senderMemberId, m.senderName)}{" "}
                    · {t(m.kind)}
                  </span>
                  <Badge variant="secondary">
                    {t(m.acknowledgedAt ? "readState" : "unread")}
                  </Badge>
                </div>
                <p className="text-xs text-muted-foreground">{m.createdAt}</p>
                <p className="whitespace-pre-wrap break-words">{m.body}</p>
                {m.taskId && (
                  <p className="break-all text-xs text-muted-foreground">
                    {t("taskReference", { id: m.taskId })}
                  </p>
                )}
                {!m.acknowledgedAt && (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy}
                    onClick={() => void acknowledge(m.messageId)}
                  >
                    {t("acknowledge")}
                  </Button>
                )}
              </div>
            ))}
          </div>
          {messages.length >= 100 && (
            <p className="text-xs text-muted-foreground">{t("bounded")}</p>
          )}
        </SettingsSection>
      </div>
    </ScrollArea>
  )
}
