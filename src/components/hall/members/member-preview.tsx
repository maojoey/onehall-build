"use client"

import { useTranslations } from "next-intl"
import { FolderAliasLabel } from "@/components/conversations/folder-alias-label"
import {
  SettingCard,
  SettingNote,
  SettingRow,
} from "@/components/shared/setting-card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  HALL_ENGINE_LABELS,
  type HallMember,
  type HallMemberProject,
  type HallMembersClient,
} from "@/lib/hall/members"
import type { AssistantInfo } from "@/lib/types"
import { MemberConfigPanel } from "./member-config-panel"

export function MemberPreview({
  member,
  projects,
  templates,
  backendLabel,
  onClose,
  client,
}: {
  member: HallMember
  projects: HallMemberProject[] | null
  templates: AssistantInfo[] | null
  backendLabel: string
  onClose: () => void
  client: HallMembersClient
}) {
  const t = useTranslations("Hall.members")
  const project = projects?.find((row) => row.id === member.projectId)
  const template = templates?.find((row) => row.uid === member.assistantUid)

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t("previewTitle", { name: member.name })}</DialogTitle>
          <DialogDescription>
            {t("saveTo", { backend: backendLabel })}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <SettingNote>{t("previewNote")}</SettingNote>
          <SettingCard>
            <SettingRow title={t("memberId")}>
              <p className="break-all font-mono text-xs text-muted-foreground">
                {member.memberId}
              </p>
            </SettingRow>
            <SettingRow title={t("role")} description={member.role} />
            <SettingRow title={t("cwd")}>
              <p
                dir="ltr"
                className="break-all text-start font-mono text-xs text-muted-foreground"
              >
                {member.cwd}
              </p>
            </SettingRow>
            <SettingRow title={t("project")}>
              <div className="space-y-1 text-xs text-muted-foreground">
                {project ? (
                  <>
                    <FolderAliasLabel
                      name={project.name}
                      alias={project.alias}
                    />
                    <p dir="ltr" className="break-all text-start font-mono">
                      {project.path}
                    </p>
                  </>
                ) : member.projectId != null ? (
                  <p>
                    {t("unavailableProject")} · #{member.projectId}
                  </p>
                ) : (
                  <p>{t("noProject")}</p>
                )}
              </div>
            </SettingRow>
            <SettingRow
              title={t("engine")}
              control={
                <Badge variant="secondary">
                  {HALL_ENGINE_LABELS[member.engine] ?? member.engine}
                </Badge>
              }
            />
            <SettingRow title={t("template")}>
              <div className="space-y-1 text-xs text-muted-foreground">
                <p>
                  {member.assistantUid
                    ? (template?.name ?? t("unavailableTemplate"))
                    : t("noTemplate")}
                </p>
                {member.assistantUid && (
                  <>
                    <p className="break-all font-mono">{member.assistantUid}</p>
                    <p>
                      {t("templateVersion", {
                        recorded: member.assistantRevision ?? "—",
                        current: template?.revision ?? "—",
                      })}
                    </p>
                  </>
                )}
              </div>
            </SettingRow>
            <SettingRow
              title={t("registryRevision")}
              control={<Badge variant="outline">{member.revision}</Badge>}
            />
            <SettingRow
              title={t("status")}
              description={t("activationPending")}
              control={
                <Badge variant="secondary">
                  {t(member.enabled ? "active" : "inactive")}
                </Badge>
              }
            />
          </SettingCard>
          <MemberConfigPanel
            key={`${member.memberId}:${member.revision}`}
            memberId={member.memberId}
            revision={member.revision}
            client={client}
          />
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            {t("done")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
