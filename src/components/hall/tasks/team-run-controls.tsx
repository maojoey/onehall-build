"use client"
import dynamic from "next/dynamic"
import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import type {
  TeamControlClient,
  TeamReadiness,
  TeamSession,
} from "@/lib/hall/control"
import { isTasksUnavailable, type HallTaskDetail } from "@/lib/hall/tasks"
import { toErrorMessage } from "@/lib/app-error"
function status(state: TeamReadiness | null) {
  if (!state) return "closed"
  if (!state.nativeProbeSupported) return "nativeMissing"
  switch (state.reason) {
    case "ready":
      return "ready"
    case "run_root_unavailable":
      return "runRootMissing"
    case "native_probe_pending":
      return "probePending"
    case "native_probe_failed":
      return "probeFailed"
    case "transport_unavailable":
      return "transportMissing"
    default:
      return "closed"
  }
}
const TeamTranscriptDialog = dynamic(
  () => import("./team-transcript-dialog").then((m) => m.TeamTranscriptDialog),
  { ssr: false }
)
export function TeamRunControls({
  client,
  task,
  changed,
  localRelease = false,
}: {
  client: TeamControlClient
  task: HallTaskDetail
  changed: (detail: HallTaskDetail) => void
  /** False for a remote workspace relay: it must not offer this machine's release. */
  localRelease?: boolean
}) {
  const t = useTranslations("Hall.control")
  const [session, setSession] = useState<TeamSession | null>(null)
  const [state, setState] = useState<TeamReadiness | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [acknowledgedRun, setAcknowledgedRun] = useState<string | null>(null)
  const [orphanReason, setOrphanReason] = useState("")
  const [orphanAck, setOrphanAck] = useState<string | null>(null)
  const resolutionKey = `${task.taskId}:${task.revision}`
  const alive = useRef(false)
  useEffect(() => {
    alive.current = true
    void client
      .state()
      .then((s) => {
        if (alive.current) setState(s)
      })
      .catch((f) => {
        if (alive.current)
          setError(isTasksUnavailable(f) ? t("unsupported") : toErrorMessage(f))
      })
    return () => {
      alive.current = false
    }
  }, [client, t])
  const changedRef = useRef(changed)
  changedRef.current = changed
  const request = useRef(0)
  useEffect(() => {
    if (!["running", "awaiting_permission"].includes(task.status)) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout>
    async function update() {
      const current = ++request.current
      try {
        const next = await client.refresh(task.taskId)
        if (!cancelled && alive.current && current === request.current)
          changedRef.current(next)
      } catch (failure) {
        if (!cancelled && alive.current && current === request.current)
          setError(toErrorMessage(failure))
      }
      if (!cancelled) timer = setTimeout(() => void update(), 2000)
    }
    timer = setTimeout(() => void update(), 2000)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [client, task.taskId, task.status])
  async function act(
    action:
      | "dispatch"
      | "refresh"
      | "cancel"
      | "recover"
      | "probe"
      | "resolveBestEffort"
  ) {
    if (busy) return
    const current = ++request.current
    setBusy(true)
    setError(null)
    try {
      if (action === "probe") await client.probe()
      else if (
        action === "dispatch" ||
        action === "cancel" ||
        action === "resolveBestEffort"
      ) {
        const detail = await client[action](task)
        if (alive.current && request.current === current) changed(detail)
      } else if (action === "refresh" || action === "recover") {
        const detail = await client[action](task.taskId)
        if (alive.current && request.current === current) changed(detail)
      }
      const next = await client.state()
      if (alive.current) setState(next)
    } catch (failure) {
      if (alive.current)
        setError(
          isTasksUnavailable(failure)
            ? t("unsupported")
            : toErrorMessage(failure)
        )
    } finally {
      if (alive.current) setBusy(false)
    }
  }
  const attempt = task.attempts?.[task.attempts.length - 1]
  const orphanKey = `${task.taskId}:${task.revision}:${attempt?.attemptId ?? ""}`
  const orphanText = orphanReason.trim()
  // Eligible without any active or connection id: only the saved evidence counts.
  const canReleaseOrphan =
    localRelease &&
    !!attempt &&
    !task.resolution &&
    task.status === "interrupted" &&
    task.runtimes?.[task.runtimes.length - 1]?.stage === "needs_review" &&
    !task.runtimes[task.runtimes.length - 1].exitConfirmedAt
  async function releaseOrphan() {
    if (
      busy ||
      !attempt ||
      orphanAck !== orphanKey ||
      !orphanText ||
      orphanText.length > 1000
    )
      return
    const current = ++request.current
    setBusy(true)
    setError(null)
    try {
      const detail = await client.resolveOrphan(
        task,
        attempt.attemptId,
        orphanText
      )
      if (alive.current && request.current === current) {
        setOrphanReason("")
        setOrphanAck(null)
        changed(detail)
      }
    } catch (failure) {
      if (!alive.current) return
      setError(t("orphanFailed", { message: toErrorMessage(failure) }))
      setOrphanAck(null)
      // Reload only; the operator must review and confirm again.
      try {
        const next = await client.refresh(task.taskId)
        if (alive.current && request.current === current) changed(next)
      } catch {
        /* the visible error already tells the operator to check again */
      }
    } finally {
      if (alive.current) setBusy(false)
    }
  }
  async function viewSession() {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      const row = await client.session(task.taskId)
      if (alive.current) setSession(row)
    } catch (failure) {
      if (alive.current) setError(toErrorMessage(failure))
    } finally {
      if (alive.current) setBusy(false)
    }
  }
  const running = ["running", "awaiting_permission"].includes(task.status)
  const latestRuntime = task.runtimes?.[task.runtimes.length - 1]
  const latestExit = task.exitChecks?.[task.exitChecks.length - 1]
  const canResolve =
    task.status === "interrupted" &&
    state?.coverage === "macos_process_group_best_effort" &&
    latestRuntime?.stage === "needs_review" &&
    latestExit?.coverage === "macos_process_group_best_effort" &&
    latestExit.rootReaped
  return (
    <section
      className="space-y-3 rounded-lg border bg-card p-4"
      aria-label={t("title")}
    >
      <h3 className="text-sm font-medium">{t("title")}</h3>
      <p className="text-sm text-muted-foreground">{t(status(state))}</p>
      {state?.coverage === "macos_process_group_best_effort" && (
        <p className="text-sm text-muted-foreground">{t("bestEffort")}</p>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {session && (
        <TeamTranscriptDialog
          session={session}
          title={task.title}
          close={() => setSession(null)}
        />
      )}
      <div className="flex flex-wrap gap-2">
        {task.attempts?.some((a) => a.conversationId != null) && (
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => void viewSession()}
          >
            {t("viewSession")}
          </Button>
        )}
        {task.status === "queued" && (
          <Button
            size="sm"
            disabled={busy || !state?.enabled}
            onClick={() => void act("dispatch")}
          >
            {t("dispatch")}
          </Button>
        )}
        <Button
          size="sm"
          variant="outline"
          disabled={busy}
          onClick={() => void act("refresh")}
        >
          {t("refresh")}
        </Button>
        {running && (
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => void act("cancel")}
          >
            {t("cancel")}
          </Button>
        )}
        {task.status === "interrupted" &&
          latestRuntime?.stage !== "terminal" && (
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void act("recover")}
            >
              {t("recover")}
            </Button>
          )}
      </div>
      {canResolve && (
        <div className="space-y-2 rounded-lg border p-3">
          <label className="flex items-start gap-2 text-sm">
            <input
              type="checkbox"
              checked={acknowledgedRun === resolutionKey}
              disabled={busy}
              onChange={(event) =>
                setAcknowledgedRun(event.target.checked ? resolutionKey : null)
              }
            />
            {t("resolveAcknowledge")}
          </label>
          <Button
            size="sm"
            variant="outline"
            disabled={busy || acknowledgedRun !== resolutionKey}
            onClick={() => void act("resolveBestEffort")}
          >
            {t("resolveBestEffort")}
          </Button>
        </div>
      )}
      {canReleaseOrphan && (
        <div className="space-y-2 rounded-lg border p-3">
          <p className="text-sm font-medium">{t("orphanTitle")}</p>
          <p className="text-xs text-muted-foreground">{t("orphanNote")}</p>
          <label className="block space-y-1 text-sm">
            <span>{t("orphanReason")}</span>
            <Input
              value={orphanReason}
              maxLength={1000}
              disabled={busy}
              onChange={(e) => {
                setOrphanReason(
                  e.target.value.replace(/[\u0000-\u001f\u007f]/g, " ")
                )
                setOrphanAck(null)
              }}
            />
          </label>
          <label className="flex items-start gap-2 text-sm">
            <input
              type="checkbox"
              checked={orphanAck === orphanKey}
              disabled={busy || !orphanText}
              onChange={(e) =>
                setOrphanAck(e.target.checked ? orphanKey : null)
              }
            />
            {t("orphanAcknowledge")}
          </label>
          <Button
            size="sm"
            variant="outline"
            disabled={busy || !orphanText || orphanAck !== orphanKey}
            onClick={() => void releaseOrphan()}
          >
            {t("orphanRelease")}
          </Button>
        </div>
      )}
      {task.resolution && (
        <p role="status" className="text-xs text-muted-foreground">
          {t("orphanReleased", { time: task.resolution.resolvedAt })}
        </p>
      )}
      {task.status === "interrupted" && (
        <p className="text-xs text-muted-foreground">{t("recoveryNote")}</p>
      )}
      {state && (
        <details className="space-y-3 rounded-lg border p-3 text-sm">
          <summary className="cursor-pointer text-muted-foreground">
            {t("admission")}
          </summary>
          <p className="mt-3 text-xs text-muted-foreground">
            {t("admissionNote")}
          </p>
          <p className="text-xs text-muted-foreground">
            {state.platform} · {state.reason}
          </p>
          {state.runRoot && (
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void act("probe")}
            >
              {t("probe")}
            </Button>
          )}
        </details>
      )}
    </section>
  )
}
