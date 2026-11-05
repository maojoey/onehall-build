"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { Eye, Pencil, Plus, RefreshCw, Users } from "lucide-react"
import { useTranslations } from "next-intl"
import { HallMark } from "@/components/hall/hall-mark"
import { FolderAliasLabel } from "@/components/conversations/folder-alias-label"
import {
  BackendBadge,
  useBackendLabel,
} from "@/components/layout/backend-badge"
import { SettingsSection } from "@/components/shared/settings-section"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Switch } from "@/components/ui/switch"
import { useRemoteConnection } from "@/contexts/remote-connection-context"
import { toErrorMessage } from "@/lib/app-error"
import { detectEnvironment } from "@/lib/transport/detect"
import {
  createHallMembersClient,
  HALL_ENGINE_LABELS,
  isMembersUnavailable,
  type HallMember,
  type HallMemberProject,
} from "@/lib/hall/members"
import type { AssistantInfo } from "@/lib/types"
import { MemberEditor } from "./member-editor"
import { MemberPreview } from "./member-preview"

export function MembersSettings() {
  const remote = useRemoteConnection()?.connection
  return <MembersSettingsBody key={remote?.id ?? "local"} />
}

function MembersSettingsBody() {
  const t = useTranslations("Hall.members")
  const localLabel = useBackendLabel()
  const web = detectEnvironment() === "web"
  const backendLabel = web ? t("currentServer") : localLabel
  // Client components are also prerendered during static export. The initial
  // loading UI is identical; bind the backend only in the browser realm.
  const [client] = useState(() =>
    typeof window === "undefined" ? null : createHallMembersClient()
  )
  const [members, setMembers] = useState<HallMember[] | null>(null)
  const [templates, setTemplates] = useState<AssistantInfo[] | null>(null)
  const [projects, setProjects] = useState<HallMemberProject[] | null>(null)
  const [projectLoading, setProjectLoading] = useState(false)
  const [projectError, setProjectError] = useState<string | null>(null)
  const [previewId, setPreviewId] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [templateLoading, setTemplateLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [templateError, setTemplateError] = useState<string | null>(null)
  const [editor, setEditor] = useState<{ member: HallMember | null } | null>(
    null
  )
  const [switching, setSwitching] = useState<string | null>(null)
  const alive = useRef(false)
  const memberRequest = useRef(0)
  const templateRequest = useRef(0)
  const projectRequest = useRef(0)

  const loadMembers = useCallback(async () => {
    if (!client) return
    const request = ++memberRequest.current
    setLoading(true)
    setError(null)
    try {
      const rows = await client.list()
      if (alive.current && request === memberRequest.current) setMembers(rows)
    } catch (failure) {
      if (alive.current && request === memberRequest.current) {
        setError(
          isMembersUnavailable(failure)
            ? t("unsupported")
            : t("loadFailed", { message: toErrorMessage(failure) })
        )
      }
    } finally {
      if (alive.current && request === memberRequest.current) setLoading(false)
    }
  }, [client, t])

  const loadTemplates = useCallback(async () => {
    if (!client) return
    const request = ++templateRequest.current
    setTemplateLoading(true)
    setTemplateError(null)
    try {
      const rows = await client.templates()
      if (alive.current && request === templateRequest.current)
        setTemplates(rows)
    } catch (failure) {
      if (alive.current && request === templateRequest.current)
        setTemplateError(
          t("templatesFailed", { message: toErrorMessage(failure) })
        )
    } finally {
      if (alive.current && request === templateRequest.current)
        setTemplateLoading(false)
    }
  }, [client, t])

  const loadProjects = useCallback(async () => {
    if (!client) return
    const request = ++projectRequest.current
    setProjectLoading(true)
    setProjectError(null)
    try {
      const rows = await client.projects()
      if (alive.current && request === projectRequest.current) setProjects(rows)
    } catch (failure) {
      if (alive.current && request === projectRequest.current)
        setProjectError(
          isMembersUnavailable(failure)
            ? t("projectsUnsupported")
            : t("projectsFailed", { message: toErrorMessage(failure) })
        )
    } finally {
      if (alive.current && request === projectRequest.current)
        setProjectLoading(false)
    }
  }, [client, t])

  useEffect(() => {
    alive.current = true
    void loadMembers()
    void loadTemplates()
    void loadProjects()
    return () => {
      alive.current = false
      memberRequest.current += 1
      templateRequest.current += 1
      projectRequest.current += 1
    }
  }, [loadMembers, loadTemplates, loadProjects])

  const canEdit =
    client !== null &&
    members !== null &&
    !error &&
    templates !== null &&
    !templateError &&
    !templateLoading &&
    projects !== null &&
    !projectError &&
    !projectLoading
  const previewMember = members?.find((row) => row.memberId === previewId)
  function saved(member: HallMember) {
    if (!alive.current) return
    // A refresh started before this write cannot erase it when it arrives.
    memberRequest.current += 1
    setLoading(false)
    setError(null)
    setMembers((current) => {
      if (!current?.some((row) => row.memberId === member.memberId)) {
        return [...(current ?? []), member]
      }
      return current.map((row) =>
        row.memberId === member.memberId
          ? row.revision > member.revision
            ? row
            : member
          : row
      )
    })
    setEditor(null)
  }

  async function switchDispatch(member: HallMember, enabled: boolean) {
    if (!client || switching) return
    setSwitching(member.memberId)
    try {
      saved(await client.setEnabled(member.memberId, enabled))
    } catch (failure) {
      if (alive.current)
        setError(t("switchFailed", { message: toErrorMessage(failure) }))
    } finally {
      if (alive.current) setSwitching(null)
    }
  }

  return (
    <ScrollArea className="h-full">
      <div className="w-full space-y-4 p-3 md:p-4">
        <header className="flex flex-wrap items-start justify-between gap-3">
          <div className="space-y-1">
            <h1 className="inline-flex items-center gap-1.5 text-sm font-semibold">
              {t("title")}
              <HallMark />
            </h1>
            <p className="text-xs text-muted-foreground">{t("description")}</p>
          </div>
          {web ? (
            <Badge variant="outline">{backendLabel}</Badge>
          ) : (
            <BackendBadge />
          )}
        </header>
        <SettingsSection
          icon={Users}
          title={t("registry")}
          description={t("inactiveNote")}
        >
          <div className="space-y-3 p-4">
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  void loadMembers()
                  void loadTemplates()
                  void loadProjects()
                }}
                disabled={loading}
              >
                <RefreshCw className={loading ? "animate-spin" : ""} />
                {t("refresh")}
              </Button>
              <Button
                size="sm"
                disabled={!canEdit}
                onClick={() => setEditor({ member: null })}
              >
                <Plus />
                {t("register")}
              </Button>
            </div>
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {error}
              </p>
            )}
            {templateError && (
              <div role="alert" className="space-y-2 text-sm text-destructive">
                <p>{templateError}</p>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void loadTemplates()}
                  disabled={templateLoading}
                >
                  {t("retryTemplates")}
                </Button>
              </div>
            )}
            {projectError && (
              <div role="alert" className="space-y-2 text-sm text-destructive">
                <p>{projectError}</p>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void loadProjects()}
                  disabled={projectLoading}
                >
                  {t("retryProjects")}
                </Button>
              </div>
            )}
            {members === null && !error && (
              <p role="status" className="text-sm text-muted-foreground">
                {t("loading")}
              </p>
            )}
            {members?.length === 0 && !error && (
              <p className="py-6 text-center text-sm text-muted-foreground">
                {t("empty")}
              </p>
            )}
            {!!members?.length && (
              <ul className="divide-y divide-border/60 overflow-hidden rounded-xl border border-border/70 bg-muted/40">
                {members.map((member) => {
                  const template = templates?.find(
                    (row) => row.uid === member.assistantUid
                  )
                  const project = projects?.find(
                    (row) => row.id === member.projectId
                  )
                  return (
                    <li key={member.memberId} className="space-y-2 p-3">
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0 space-y-1">
                          <p className="break-words text-sm font-medium">
                            {member.name}
                          </p>
                          <p className="break-words text-xs text-muted-foreground">
                            {member.role}
                          </p>
                        </div>
                        <div className="flex shrink-0 items-center gap-2">
                          <label className="flex items-center gap-2 text-xs text-muted-foreground">
                            <Switch
                              checked={member.enabled}
                              disabled={!client || switching !== null}
                              aria-label={t("dispatchAria", {
                                name: member.name,
                              })}
                              onCheckedChange={(enabled) =>
                                void switchDispatch(member, enabled)
                              }
                            />
                            {t(member.enabled ? "active" : "inactive")}
                          </label>
                          <Button
                            variant="ghost"
                            size="icon"
                            aria-label={t("previewAria", { name: member.name })}
                            onClick={() => setPreviewId(member.memberId)}
                          >
                            <Eye />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            aria-label={t("editAria", { name: member.name })}
                            disabled={!canEdit}
                            onClick={() => setEditor({ member })}
                          >
                            <Pencil />
                          </Button>
                        </div>
                      </div>
                      <p className="break-all font-mono text-xs text-muted-foreground">
                        {member.cwd}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {HALL_ENGINE_LABELS[member.engine] ?? member.engine} ·{" "}
                        {member.assistantUid
                          ? (template?.name ?? t("unavailableTemplate"))
                          : t("noTemplate")}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {t("project")}:{" "}
                        {member.projectId != null ? (
                          project ? (
                            <FolderAliasLabel
                              name={project.name}
                              alias={project.alias}
                            />
                          ) : (
                            `${t("unavailableProject")} · #${member.projectId}`
                          )
                        ) : (
                          t("noProject")
                        )}
                      </p>
                    </li>
                  )
                })}
              </ul>
            )}
          </div>
        </SettingsSection>
      </div>
      {editor && templates && projects && client && (
        <MemberEditor
          key={editor.member?.memberId ?? "new"}
          member={editor.member}
          templates={templates}
          projects={projects}
          client={client}
          backendLabel={backendLabel}
          onClose={() => setEditor(null)}
          onSaved={saved}
        />
      )}
      {previewMember && client && (
        <MemberPreview
          member={previewMember}
          client={client}
          projects={projectError ? null : projects}
          templates={templateError ? null : templates}
          backendLabel={backendLabel}
          onClose={() => setPreviewId(null)}
        />
      )}
    </ScrollArea>
  )
}
