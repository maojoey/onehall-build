"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { HallMark } from "@/components/hall/hall-mark"
import { ClipboardList, Plus, RefreshCw } from "lucide-react"
import Link from "next/link"
import { BackendBadge } from "@/components/layout/backend-badge"
import { SettingsSection } from "@/components/shared/settings-section"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useRemoteConnection } from "@/contexts/remote-connection-context"
import {
  createHallTasksClient,
  isTasksUnavailable,
  type HallTask,
  type HallTaskDetail,
} from "@/lib/hall/tasks"
import type { HallMember, HallMemberProject } from "@/lib/hall/members"
import { toErrorMessage } from "@/lib/app-error"
import { openSettingsWindow } from "@/lib/api"
import { isBranchWorkspace, isRemoteDesktopMode } from "@/lib/transport"
import { TeamRunControls } from "./team-run-controls"
import { LaunchReviewPanel } from "./launch-review-panel"
import { TaskEditor, selectClass } from "./task-editor"

/** In the workspace (`onOpenMailbox` given) members open in the settings
 *  window and the mailbox in the workbench; on the settings page both stay
 *  links, so the workspace window is never navigated into settings. */
export function TeamTasks({ onOpenMailbox }: { onOpenMailbox?: () => void }) {
  const remote = useRemoteConnection()?.connection
  return (
    <TeamTasksBody
      key={remote?.id ?? "local"}
      membersHref={`/settings/hall-members${remote ? `?remoteConnectionId=${remote.id}` : ""}`}
      onOpenMailbox={onOpenMailbox}
    />
  )
}
function TeamTasksBody({
  membersHref,
  onOpenMailbox,
}: {
  membersHref: string
  onOpenMailbox?: () => void
}) {
  const t = useTranslations("Hall.tasks")
  const [client] = useState(() =>
    typeof window === "undefined" ? null : createHallTasksClient()
  )
  const [tasks, setTasks] = useState<HallTask[]>([])
  const [members, setMembers] = useState<HallMember[]>([])
  const [projects, setProjects] = useState<HallMemberProject[]>([])
  const [projectFilter, setProjectFilter] = useState("")
  const [memberFilter, setMemberFilter] = useState("")
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [detail, setDetail] = useState<HallTaskDetail | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)
  const [editor, setEditor] = useState<{ task: HallTaskDetail | null } | null>(
    null
  )
  const [busy, setBusy] = useState(false)
  const alive = useRef(false)
  const request = useRef(0)
  const detailRequest = useRef(0)
  useEffect(() => {
    const lists = request
    const details = detailRequest
    alive.current = true
    return () => {
      alive.current = false
      lists.current++
      details.current++
    }
  }, [])
  const load = useCallback(async () => {
    if (!client) return
    const current = ++request.current
    detailRequest.current++
    setDetail(null)
    setDetailLoading(false)
    setLoading(true)
    setError(null)
    setTasks([])
    try {
      const [rows, people, folders] = await Promise.all([
        client.list({
          ...(memberFilter ? { memberId: memberFilter } : {}),
          ...(projectFilter === "none"
            ? { unassignedProject: true }
            : projectFilter
              ? { projectId: Number(projectFilter) }
              : {}),
        }),
        client.members(),
        client.projects(),
      ])
      if (alive.current && request.current === current) {
        setTasks(rows)
        setMembers(people)
        setProjects(folders)
      }
    } catch (failure) {
      if (alive.current && request.current === current)
        setError(
          isTasksUnavailable(failure)
            ? t("unsupported")
            : t("failure", { message: toErrorMessage(failure) })
        )
    } finally {
      if (alive.current && request.current === current) setLoading(false)
    }
  }, [client, memberFilter, projectFilter, t])
  useEffect(() => {
    void load()
  }, [load])
  async function openDetail(taskId: string) {
    if (!client) return
    const current = ++detailRequest.current
    setDetail(null)
    setDetailLoading(true)
    setError(null)
    try {
      const row = await client.get(taskId)
      if (alive.current && detailRequest.current === current) setDetail(row)
    } catch (failure) {
      if (alive.current && detailRequest.current === current)
        setError(
          isTasksUnavailable(failure)
            ? t("unsupported")
            : t("failure", { message: toErrorMessage(failure) })
        )
    } finally {
      if (alive.current && detailRequest.current === current)
        setDetailLoading(false)
    }
  }
  async function cancel() {
    if (!client || !detail || busy) return
    setBusy(true)
    setError(null)
    try {
      await client.cancel(detail)
      if (alive.current) {
        await load()
        if (alive.current) await openDetail(detail.taskId)
      }
    } catch (failure) {
      if (alive.current)
        setError(t("saveUncertain", { message: toErrorMessage(failure) }))
    } finally {
      if (alive.current) setBusy(false)
    }
  }
  const memberName = (id: string) =>
    members.find((m) => m.memberId === id)?.name ?? t("missingMember")
  const projectName = (id: number | null) =>
    id === null
      ? t("noProject")
      : projects.find((p) => p.id === id)?.alias ||
        projects.find((p) => p.id === id)?.name ||
        t("missingProject")
  const locked = busy || !!editor
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
        <SettingsSection
          icon={ClipboardList}
          title={t("tasks")}
          description={t("draftOnly")}
        >
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={loading || locked}
              onClick={() => void load()}
            >
              <RefreshCw className={loading ? "animate-spin" : ""} />
              {t("refresh")}
            </Button>
            <Button
              size="sm"
              disabled={
                !client || loading || !!error || !members.length || locked
              }
              onClick={() => {
                detailRequest.current++
                setDetailLoading(false)
                setEditor({ task: null })
              }}
            >
              <Plus />
              {t("create")}
            </Button>
            {onOpenMailbox ? (
              <>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() =>
                    void openSettingsWindow("hall-members").catch((f) =>
                      setError(toErrorMessage(f))
                    )
                  }
                >
                  {t("manageMembers")}
                </Button>
                <Button size="sm" variant="ghost" onClick={onOpenMailbox}>
                  {t("mailbox")}
                </Button>
              </>
            ) : (
              <>
                <Button size="sm" variant="ghost" asChild>
                  <Link href={membersHref}>{t("manageMembers")}</Link>
                </Button>
                <Button size="sm" variant="ghost" asChild>
                  <Link
                    href={membersHref.replace("hall-members", "hall-mailbox")}
                  >
                    {t("mailbox")}
                  </Link>
                </Button>
              </>
            )}
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <select
              aria-label={t("projectFilter")}
              className={selectClass}
              disabled={locked}
              value={projectFilter}
              onChange={(e) => setProjectFilter(e.target.value)}
            >
              <option value="">{t("allProjects")}</option>
              <option value="none">{t("noProject")}</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.alias || p.name}
                </option>
              ))}
            </select>
            <select
              aria-label={t("memberFilter")}
              className={selectClass}
              disabled={locked}
              value={memberFilter}
              onChange={(e) => setMemberFilter(e.target.value)}
            >
              <option value="">{t("allMembers")}</option>
              {members.map((m) => (
                <option key={m.memberId} value={m.memberId}>
                  {m.name}
                </option>
              ))}
            </select>
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          {loading ? (
            <p role="status" className="text-sm text-muted-foreground">
              {t("loading")}
            </p>
          ) : !tasks.length && !error ? (
            <p className="rounded-lg border border-dashed p-6 text-sm text-muted-foreground">
              {t(members.length ? "empty" : "noMembers")}
            </p>
          ) : null}
          <div className="grid gap-3 sm:grid-cols-2">
            {tasks.map((task) => (
              <button
                key={task.taskId}
                type="button"
                disabled={locked}
                onClick={() => void openDetail(task.taskId)}
                className="space-y-2 rounded-lg border bg-card p-4 text-left transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
              >
                <div className="flex items-start justify-between gap-2">
                  <span className="break-words text-sm font-medium">
                    {task.title}
                  </span>
                  <Badge variant="secondary">{t(task.status)}</Badge>
                </div>
                <p className="text-xs text-muted-foreground">
                  {memberName(task.memberId)} · {projectName(task.projectId)}
                </p>
                <p className="text-xs text-muted-foreground">
                  {t(task.purpose)} ·{" "}
                  {t(
                    task.status === "draft"
                      ? "notExecuted"
                      : "executionUnverified"
                  )}
                </p>
              </button>
            ))}
          </div>
          {tasks.length >= 100 && (
            <p className="text-xs text-muted-foreground">{t("bounded")}</p>
          )}
        </SettingsSection>
        {editor && client && (
          <TaskEditor
            client={client}
            members={members}
            projects={projects}
            task={editor.task}
            membersHref={membersHref}
            onClose={() => setEditor(null)}
            onSaved={() => {
              setEditor(null)
              void load()
            }}
          />
        )}
        {detailLoading && (
          <p role="status" className="text-sm text-muted-foreground">
            {t("loading")}
          </p>
        )}
        {detail && !editor && (
          <SettingsSection
            title={detail.title}
            description={`${memberName(detail.memberId)} · ${projectName(detail.projectId)}`}
          >
            <div className="flex flex-wrap gap-2">
              <Badge variant="secondary">{t(detail.status)}</Badge>
              <Badge variant="outline">
                {t(
                  detail.status === "draft"
                    ? "notExecuted"
                    : "executionUnverified"
                )}
              </Badge>
            </div>
            <p className="text-sm text-muted-foreground">{t("draftOnly")}</p>
            {members.find((m) => m.memberId === detail.memberId)?.revision !==
              detail.snapshot.memberRevision && (
              <p className="text-sm text-amber-600 dark:text-amber-400">
                {t("memberChanged")}
              </p>
            )}
            <h3 className="text-sm font-medium">{t("description")}</h3>
            <p className="whitespace-pre-wrap break-words text-sm">
              {detail.description || t("noDescription")}
            </p>
            {detail.status === "draft" && (
              <div className="flex flex-wrap gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => setEditor({ task: detail })}
                >
                  {t("edit")}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void cancel()}
                >
                  {t("cancelDraft")}
                </Button>
              </div>
            )}
            {client && (
              <LaunchReviewPanel
                key={`${detail.taskId}:${detail.revision}`}
                client={client}
                task={detail}
                memberName={memberName(detail.memberId)}
                onChanged={async () => {
                  await load()
                  await openDetail(detail.taskId)
                }}
              />
            )}
            {client?.control && (
              <TeamRunControls
                key={`control:${detail.taskId}`}
                client={client.control}
                task={detail}
                localRelease={!isRemoteDesktopMode() && !isBranchWorkspace()}
                changed={(next) => {
                  setDetail(next)
                  setTasks((rows) =>
                    rows.map((row) => (row.taskId === next.taskId ? next : row))
                  )
                }}
              />
            )}
            <h3 className="text-sm font-medium">{t("history")}</h3>
            <ol className="space-y-1 text-xs text-muted-foreground">
              {detail.history.map((h) => (
                <li key={h.revision}>
                  {t(h.action)} · {h.createdAt} ·{" "}
                  {t("revision", { revision: h.revision })}
                </li>
              ))}
            </ol>
            <p className="text-sm text-muted-foreground">
              {detail.attempts.length ? t("queueNote") : t("noAttempts")}
            </p>
            {!!detail.attempts.length && (
              <div className="space-y-2">
                <h3 className="text-sm font-medium">{t("attempts")}</h3>
                <ol className="space-y-1 text-xs text-muted-foreground">
                  {detail.attempts.map((a) => (
                    <li key={a.attemptId}>
                      #{a.attemptNumber} · {t(a.status)}
                    </li>
                  ))}
                </ol>
              </div>
            )}
            {!!detail.runtimes?.length && (
              <div className="space-y-2 text-sm">
                <h3 className="font-medium">{t("runtimeHistory")}</h3>
                {detail.runtimes.map((r) => (
                  <div
                    key={r.attemptId}
                    className="space-y-1 rounded-lg border p-3"
                  >
                    <p className="text-muted-foreground">
                      {t("runtimeStage", { stage: r.stage })}
                    </p>
                    {r.runtimeKind === "fixture" && <p>{t("fixtureOnly")}</p>}
                    {r.stage === "needs_review" && (
                      <p className="text-amber-600 dark:text-amber-400">
                        {t("recoveryRequired")}
                      </p>
                    )}
                    {r.cancelRequestedAt && (
                      <p>
                        {t("cancelRequestTime", { time: r.cancelRequestedAt })}
                      </p>
                    )}
                    {r.cancelAckAt && (
                      <p>{t("cancelAckTime", { time: r.cancelAckAt })}</p>
                    )}
                    <p>
                      {r.exitConfirmedAt
                        ? t("exitTime", { time: r.exitConfirmedAt })
                        : t("noExitProof")}
                    </p>
                  </div>
                ))}
              </div>
            )}
            {!!detail.exitChecks?.length && (
              <section className="space-y-2 text-sm">
                <h3 className="font-medium">{t("exitChecks")}</h3>
                {detail.exitChecks.map((check) => (
                  <div
                    key={check.attemptId}
                    className="space-y-1 rounded-lg border p-3"
                  >
                    <p>
                      {t(
                        check.complete
                          ? "exitEvidenceComplete"
                          : check.coverage ===
                                "macos_process_group_best_effort" &&
                              detail.runtimes?.some(
                                (runtime) =>
                                  runtime.attemptId === check.attemptId &&
                                  runtime.stage === "terminal" &&
                                  !runtime.exitConfirmedAt
                              ) &&
                              detail.attempts.some(
                                (attempt) =>
                                  attempt.attemptId === check.attemptId &&
                                  attempt.status === "interrupted"
                              )
                            ? "exitEvidenceHumanResolved"
                            : "exitEvidenceIncomplete"
                      )}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {check.observedAt}
                    </p>
                    <details className="text-xs">
                      <summary className="cursor-pointer text-muted-foreground">
                        {t("configurationDetails")}
                      </summary>
                      <p>
                        {check.coverage} · {check.reason}
                      </p>
                      <p>
                        {check.knownReaped}/{check.knownProcesses} ·{" "}
                        {check.pendingSpawns}
                      </p>
                    </details>
                  </div>
                ))}
              </section>
            )}
            {!!detail.issues?.length && (
              <div
                role="alert"
                className="space-y-1 rounded-lg border border-amber-500/50 bg-amber-500/10 p-3 text-sm"
              >
                <p className="font-medium">{t("issuesTitle")}</p>
                <ul className="space-y-1 text-xs">
                  {detail.issues.map((issue) => (
                    <li key={`${issue.attemptId}:${issue.createdAt}`}>
                      {t(`issue_${issue.tool}`)} · {issue.detail} ·{" "}
                      {issue.createdAt}
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {!!detail.results?.length && (
              <div className="space-y-2">
                <h3 className="text-sm font-medium">{t("results")}</h3>
                {detail.results.map((r) => (
                  <details
                    key={r.resultId}
                    className="rounded-lg border p-3 text-sm"
                  >
                    <summary className="cursor-pointer">
                      {t("recordedResult", { outcome: r.outcome })} ·{" "}
                      {r.createdAt}
                    </summary>
                    <p className="mt-2 whitespace-pre-wrap break-words">
                      {r.body}
                    </p>
                  </details>
                ))}
              </div>
            )}
            <details className="rounded-lg border p-3 text-xs">
              <summary className="cursor-pointer text-muted-foreground">
                {t("configurationDetails")}
              </summary>
              <dl className="mt-2 space-y-2 break-all">
                <dt>{t("snapshot")}</dt>
                <dd>{detail.snapshot.snapshotId}</dd>
                <dt>{t("fingerprint")}</dt>
                <dd>{detail.snapshot.snapshotVersion}</dd>
                <dt>
                  {t("revision", { revision: detail.snapshot.memberRevision })}
                </dt>
              </dl>
            </details>
          </SettingsSection>
        )}
      </div>
    </ScrollArea>
  )
}
