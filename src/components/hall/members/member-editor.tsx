"use client"

import { useEffect, useId, useRef, useState, type FormEvent } from "react"
import { useTranslations } from "next-intl"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"
import { FolderSelect } from "@/components/shared/folder-select"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { extractAppCommandError, toErrorMessage } from "@/lib/app-error"
import {
  isMembersUnavailable,
  type HallMember,
  type HallMemberProject,
  type HallMembersClient,
  HALL_ENGINE_LABELS,
  HALL_ENGINES,
  type HallEngine,
} from "@/lib/hall/members"
import type { AssistantInfo } from "@/lib/types"

interface MemberEditorProps {
  member: HallMember | null
  templates: AssistantInfo[]
  projects: HallMemberProject[]
  client: HallMembersClient
  backendLabel: string
  onClose: () => void
  onSaved: (member: HallMember) => void
}

export function MemberEditor({
  member,
  templates,
  projects,
  client,
  backendLabel,
  onClose,
  onSaved,
}: MemberEditorProps) {
  const t = useTranslations("Hall.members")
  const id = useId()
  const [name, setName] = useState(member?.name ?? "")
  const [role, setRole] = useState(member?.role ?? "")
  const [cwd, setCwd] = useState(member?.cwd ?? "")
  const [engine, setEngine] = useState<HallEngine>(member?.engine ?? "codex")
  const [assistantUid, setAssistantUid] = useState(member?.assistantUid ?? null)
  // A preset runs on one engine, so only that engine's presets are offered.
  const engineTemplates = templates.filter((row) => row.agent_type === engine)
  const [projectId, setProjectId] = useState(member?.projectId ?? null)
  const [browsing, setBrowsing] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const inFlight = useRef(false)
  const alive = useRef(false)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  const unavailableTemplate =
    assistantUid !== null &&
    !engineTemplates.some((row) => row.uid === assistantUid)
  const unavailableProject =
    projectId !== null && !projects.some((row) => row.id === projectId)

  async function submit(event: FormEvent) {
    event.preventDefault()
    if (inFlight.current) return
    if (
      !name.trim() ||
      !role.trim() ||
      !cwd.trim() ||
      Array.from(name.trim()).length > 80 ||
      Array.from(role.trim()).length > 200 ||
      unavailableTemplate ||
      unavailableProject
    ) {
      setError(t("invalidForm"))
      return
    }
    inFlight.current = true
    setBusy(true)
    setError(null)
    const edit = {
      name: name.trim(),
      role: role.trim(),
      assistantUid,
      projectId,
      engine,
    }
    try {
      const saved = member
        ? await client.update(member.memberId, member.revision, edit)
        : await client.register({ ...edit, cwd })
      if (alive.current) onSaved(saved)
    } catch (failure) {
      if (!alive.current) return
      const code = extractAppCommandError(failure)?.code
      setError(
        isMembersUnavailable(failure)
          ? t("unsupported")
          : code === "already_exists"
            ? t(member ? "conflict" : "duplicate")
            : code === "not_found"
              ? t("notFound")
              : toErrorMessage(failure)
      )
    } finally {
      inFlight.current = false
      if (alive.current) setBusy(false)
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !inFlight.current) onClose()
      }}
    >
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t(member ? "editTitle" : "register")}</DialogTitle>
          <DialogDescription>
            {t("saveTo", { backend: backendLabel })}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor={`${id}-name`}>{t("name")}</Label>
            <Input
              id={`${id}-name`}
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={busy}
              autoFocus
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor={`${id}-role`}>{t("role")}</Label>
            <Input
              id={`${id}-role`}
              value={role}
              onChange={(e) => setRole(e.target.value)}
              disabled={busy}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor={`${id}-cwd`}>{t("cwd")}</Label>
            <div className="flex gap-2">
              <Input
                id={`${id}-cwd`}
                value={cwd}
                onChange={(e) => setCwd(e.target.value)}
                readOnly={member !== null}
                disabled={busy}
                className="font-mono text-xs"
              />
              {!member && (
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={() => setBrowsing(true)}
                >
                  {t("browse")}
                </Button>
              )}
            </div>
            <p className="text-xs text-muted-foreground">
              {t(member ? "fixedDirectory" : "directoryHint", {
                backend: backendLabel,
              })}
            </p>
          </div>
          <div className="space-y-2">
            <p className="text-sm font-medium">{t("project")}</p>
            <FolderSelect
              folders={projects}
              value={projectId}
              onChange={setProjectId}
              allLabel={t("noProject")}
              onSelectAll={() => setProjectId(null)}
              variant="field"
              disabled={busy}
              title={t("project")}
              className="w-full max-w-none"
            />
            <p className="text-xs text-muted-foreground">{t("projectHint")}</p>
            {unavailableProject && (
              <p className="text-xs text-destructive">
                {t("unavailableProject")} · #{projectId}
              </p>
            )}
          </div>
          <div className="space-y-2">
            <Label htmlFor={`${id}-engine`}>{t("engine")}</Label>
            <Select
              value={engine}
              onValueChange={(value) => {
                const next = value as HallEngine
                setEngine(next)
                // A preset of the other engine no longer fits.
                if (
                  !templates.some(
                    (row) => row.uid === assistantUid && row.agent_type === next
                  )
                )
                  setAssistantUid(null)
              }}
              disabled={busy}
            >
              <SelectTrigger id={`${id}-engine`} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {HALL_ENGINES.map((value) => (
                  <SelectItem key={value} value={value}>
                    {HALL_ENGINE_LABELS[value]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {member && engine !== member.engine && (
              <p className="text-xs text-muted-foreground">
                {t("engineChanged")}
              </p>
            )}
          </div>
          <div className="space-y-2">
            <Label htmlFor={`${id}-template`}>{t("template")}</Label>
            <Select
              value={assistantUid ?? "none"}
              onValueChange={(value) =>
                setAssistantUid(value === "none" ? null : value)
              }
              disabled={busy}
            >
              <SelectTrigger id={`${id}-template`} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">{t("noTemplate")}</SelectItem>
                {unavailableTemplate && (
                  <SelectItem value={assistantUid!} disabled>
                    {t("unavailableTemplate")}
                  </SelectItem>
                )}
                {engineTemplates.map((template) => (
                  <SelectItem key={template.uid} value={template.uid}>
                    {template.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {member && (
            <p className="break-all font-mono text-xs text-muted-foreground">
              {t("memberId")}: {member.memberId}
            </p>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={onClose}
              disabled={busy}
            >
              {t("cancel")}
            </Button>
            <Button
              type="submit"
              disabled={busy || unavailableTemplate || unavailableProject}
            >
              {t(busy ? "saving" : member ? "save" : "register")}
            </Button>
          </DialogFooter>
        </form>
        <DirectoryBrowserDialog
          open={browsing}
          onOpenChange={setBrowsing}
          onSelect={setCwd}
          initialPath={cwd || undefined}
          title={t("cwd")}
        />
      </DialogContent>
    </Dialog>
  )
}
