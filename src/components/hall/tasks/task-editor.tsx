"use client"

import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import Link from "next/link"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import type {
  HallMember,
  HallMemberProject,
  HallMemberSnapshot,
} from "@/lib/hall/members"
import type {
  HallTaskDetail,
  HallTasksClient,
  TaskDraft,
} from "@/lib/hall/tasks"
import { toErrorMessage } from "@/lib/app-error"

export const selectClass =
  "h-9 w-full rounded-md border border-input bg-background px-3 text-sm shadow-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"

export function TaskEditor({
  client,
  members,
  projects,
  task,
  membersHref,
  onSaved,
  onClose,
}: {
  client: HallTasksClient
  members: HallMember[]
  projects: HallMemberProject[]
  task: HallTaskDetail | null
  membersHref: string
  onSaved: () => void
  onClose: () => void
}) {
  const t = useTranslations("Hall.tasks")
  const [title, setTitle] = useState(task?.title ?? "")
  const [description, setDescription] = useState(task?.description ?? "")
  const [memberId, setMemberId] = useState(task?.memberId ?? "")
  const [snapshotId, setSnapshotId] = useState(task?.snapshot.snapshotId ?? "")
  const [purpose, setPurpose] = useState<"work" | "acceptance">(
    task?.purpose ?? "work"
  )
  const [snapshots, setSnapshots] = useState<HallMemberSnapshot[]>([])
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [attempted, setAttempted] = useState(false)
  const submissionId = useRef<string | null>(null)
  const originalRequest = useRef<TaskDraft | null>(null)
  const alive = useRef(false)
  const generation = useRef(0)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
      generation.current++
    }
  }, [])
  async function chooseMember(id: string) {
    const request = ++generation.current
    setMemberId(id)
    setSnapshotId("")
    setSnapshots([])
    setError(null)
    setLoading(false)
    if (!id) return
    setLoading(true)
    try {
      const rows = await client.snapshots(id)
      if (alive.current && generation.current === request) setSnapshots(rows)
    } catch (failure) {
      if (alive.current && generation.current === request)
        setError(t("failure", { message: toErrorMessage(failure) }))
    } finally {
      if (alive.current && generation.current === request) setLoading(false)
    }
  }
  const selected = snapshots.find((s) => s.snapshotId === snapshotId)
  const projectId = task?.projectId ?? selected?.projectId ?? null
  const project = projects.find((p) => p.id === projectId)
  const canSave =
    !!title.trim() &&
    (!!task || (!!selected && selected.projectId !== undefined)) &&
    (projectId === null || !!project) &&
    !loading &&
    !busy
  async function save() {
    if (!canSave) return
    setBusy(true)
    setError(null)
    setAttempted(true)
    try {
      if (task) await client.edit(task, { title, description })
      else {
        submissionId.current ??= crypto.randomUUID()
        originalRequest.current ??= {
          submissionId: submissionId.current,
          memberId,
          snapshotId,
          projectId,
          purpose,
          title,
          description,
        }
        await client.create(originalRequest.current)
      }
      if (alive.current) onSaved()
    } catch (failure) {
      if (alive.current)
        setError(t("saveUncertain", { message: toErrorMessage(failure) }))
    } finally {
      if (alive.current) setBusy(false)
    }
  }
  return (
    <section
      className="space-y-4 rounded-lg border bg-card p-4"
      aria-label={t(task ? "edit" : "create")}
    >
      <h3 className="text-sm font-medium">{t(task ? "edit" : "create")}</h3>
      <p className="text-xs text-muted-foreground">{t("draftOnly")}</p>
      {!task && (
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="space-y-1.5">
            <Label htmlFor="task-member">{t("member")}</Label>
            <select
              id="task-member"
              className={selectClass}
              value={memberId}
              disabled={attempted || busy}
              onChange={(e) => void chooseMember(e.target.value)}
            >
              <option value="">{t("chooseMember")}</option>
              {members.map((m) => (
                <option key={m.memberId} value={m.memberId}>
                  {m.name} · {m.role}
                </option>
              ))}
            </select>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="task-snapshot">{t("snapshot")}</Label>
            <select
              id="task-snapshot"
              className={selectClass}
              value={snapshotId}
              disabled={!memberId || loading || attempted || busy}
              onChange={(e) => setSnapshotId(e.target.value)}
            >
              <option value="">
                {t(loading ? "loading" : "chooseSnapshot")}
              </option>
              {snapshots.map((s, i) => (
                <option key={s.snapshotId} value={s.snapshotId}>
                  {t("savedVersion", {
                    number: snapshots.length - i,
                    revision: s.memberRevision,
                  })}
                </option>
              ))}
            </select>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="task-purpose">{t("purpose")}</Label>
            <select
              id="task-purpose"
              className={selectClass}
              value={purpose}
              disabled={attempted || busy}
              onChange={(e) =>
                setPurpose(e.target.value as "work" | "acceptance")
              }
            >
              <option value="work">{t("work")}</option>
              <option value="acceptance">{t("acceptance")}</option>
            </select>
          </div>
          <div className="space-y-1.5">
            <p className="text-xs text-muted-foreground">{t("project")}</p>
            <p className="text-sm">
              {project?.alias ||
                project?.name ||
                t(projectId === null ? "noProject" : "missingProject")}
            </p>
          </div>
        </div>
      )}
      {!task && (
        <p className="text-xs text-muted-foreground">
          {t("prepareHint")}{" "}
          <Link href={membersHref} className="underline">
            {t("manageMembers")}
          </Link>
        </p>
      )}
      <div className="space-y-1.5">
        <Label htmlFor="task-title">{t("taskTitle")}</Label>
        <Input
          id="task-title"
          value={title}
          maxLength={160}
          disabled={attempted || busy}
          onChange={(e) => setTitle(e.target.value)}
        />
      </div>
      <div className="space-y-1.5">
        <Label htmlFor="task-description">{t("description")}</Label>
        <Textarea
          id="task-description"
          value={description}
          maxLength={8000}
          rows={4}
          disabled={attempted || busy}
          onChange={(e) => setDescription(e.target.value)}
        />
      </div>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" onClick={() => void save()} disabled={!canSave}>
          {t(busy ? "saving" : attempted ? "retrySave" : "save")}
        </Button>
        <Button size="sm" variant="outline" onClick={onClose} disabled={busy}>
          {t("closeEditor")}
        </Button>
      </div>
    </section>
  )
}
