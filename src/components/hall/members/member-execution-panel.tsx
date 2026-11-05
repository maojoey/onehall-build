"use client"

import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { SettingCard, SettingRow } from "@/components/shared/setting-card"
import { Button } from "@/components/ui/button"
import { extractAppCommandError } from "@/lib/app-error"
import {
  isMembersUnavailable,
  type HallMemberCheckReport,
  type HallMemberExecutionPreview,
  type HallMembersClient,
  isHallEngine,
  hallSafeModes,
} from "@/lib/hall/members"

import { ExecutionCapabilities } from "./execution-capabilities"
import { MemberSnapshotPanel } from "./member-snapshot-panel"

const issues = [
  "member_data_invalid",
  "invalid_path_rule",
  "conflicting_mode",
  "unsafe_mode",
  "mode_exceeds_scope",
  "template_content_invalid",
] as const
const fields = {
  member: "member",
  "scope.read": "read",
  "scope.write": "write",
  mode: "mode",
  template: "template",
} as const

export function MemberExecutionPanel({
  memberId,
  revision,
  checkVersion,
  client,
  dataValid,
  onChecked,
  snapshotStorageSupported = false,
}: {
  memberId: string
  revision: number
  checkVersion: string
  client: HallMembersClient
  dataValid: boolean
  snapshotStorageSupported?: boolean
  onChecked: (report: HallMemberCheckReport) => void
}) {
  const t = useTranslations("Hall.members.execution")
  const [result, setResult] = useState<HallMemberExecutionPreview | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const alive = useRef(false)
  const inFlight = useRef(false)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  async function prepare() {
    if (inFlight.current || !dataValid) return
    inFlight.current = true
    setBusy(true)
    setResult(null)
    setError(null)
    try {
      const response = await client.previewExecution(
        memberId,
        revision,
        checkVersion
      )
      if (!alive.current) return
      if (
        response.memberId !== memberId ||
        response.revision !== revision ||
        response.checkVersion !== checkVersion ||
        response.launchAllowed !== false ||
        response.preflight.memberId !== memberId ||
        response.preflight.revision !== revision ||
        response.preflight.checkVersion !== checkVersion ||
        response.preflight.activationAvailable !== false
      ) {
        setError(t("conflict"))
        return
      }
      if (
        response.valid &&
        (!response.summary ||
          response.summary.runtimeVerified !== false ||
          response.summary.permissionPolicy !== "wait" ||
          !isHallEngine(response.summary.engine) ||
          !hallSafeModes(response.summary.engine).includes(
            response.summary.requestedMode
          ) ||
          !["scope", "template"].includes(response.summary.modeSource) ||
          !["member", "template", "engine_default"].includes(
            response.summary.modelSource
          ) ||
          !["none", "default"].includes(response.summary.network) ||
          !/^[a-f0-9]{64}$/i.test(response.summary.snapshotVersion))
      ) {
        setError(t("failed"))
        return
      }
      setResult(response)
      onChecked(response.preflight)
    } catch (failure) {
      if (!alive.current) return
      const code = extractAppCommandError(failure)?.code
      setError(
        isMembersUnavailable(failure)
          ? t("unsupported")
          : code === "already_exists" || code === "not_found"
            ? t("conflict")
            : t("failed")
      )
    } finally {
      inFlight.current = false
      if (alive.current) setBusy(false)
    }
  }
  const summary = result?.valid ? result.summary : null

  return (
    <div className="space-y-3">
      <p className="text-sm font-medium">{t("title")}</p>
      <p className="text-xs leading-5 text-muted-foreground">
        {t("description")}
      </p>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={busy || !dataValid}
        onClick={() => void prepare()}
      >
        {t(busy ? "preparing" : "prepare")}
      </Button>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      {result && (
        <p role="status" className="text-xs text-muted-foreground">
          {t(result.valid ? "generated" : "blocked")}
        </p>
      )}
      {!!result?.issues.length && (
        <ul role="alert" className="space-y-2 text-xs text-destructive">
          {result.issues.map((issue, i) => {
            const code = issues.find((code) => code === issue.code) ?? "unknown"
            const field = Object.prototype.hasOwnProperty.call(
              fields,
              issue.field
            )
              ? fields[issue.field as keyof typeof fields]
              : "other"
            return (
              <li key={i}>
                <p>
                  {t(`fields.${field}`)}
                  {issue.index !== null &&
                  Number.isInteger(issue.index) &&
                  issue.index >= 0 &&
                  issue.index < 64
                    ? ` · ${issue.index + 1}`
                    : ""}
                </p>
                <p>{t(`issues.${code}`)}</p>
              </li>
            )
          })}
        </ul>
      )}
      {summary && (
        <>
          <SettingCard>
            <SettingRow title={t("version")}>
              <p
                dir="ltr"
                className="break-all text-start font-mono text-xs text-muted-foreground"
              >
                {summary.snapshotVersion}
              </p>
            </SettingRow>
            <SettingRow title={t("mode")}>
              <p className="text-xs text-muted-foreground">
                {t(
                  summary.requestedMode === "read-only"
                    ? "readOnly"
                    : "workspaceWrite"
                )}{" "}
                ·{" "}
                {t(
                  summary.modeSource === "template"
                    ? "templateSource"
                    : "scopeSource"
                )}
              </p>
            </SettingRow>
            <SettingRow title={t("executionMode")}>
              <ExecutionCapabilities
                mode={summary.executionMode}
                capabilities={summary.capabilities}
              />
            </SettingRow>
            <SettingRow title={t("model")}>
              <p className="text-xs text-muted-foreground">
                {t(
                  summary.modelSource === "member"
                    ? "memberSource"
                    : summary.modelSource === "template"
                      ? "templateSource"
                      : "engineDefault"
                )}
              </p>
            </SettingRow>
            <SettingRow title={t("approval")}>
              <p className="text-xs text-muted-foreground">{t("wait")}</p>
            </SettingRow>
            <SettingRow title={t("content")}>
              <p className="text-xs text-muted-foreground">
                {t("options", {
                  count: summary.optionCount,
                  revision: summary.templateRevision ?? "—",
                })}
              </p>
              <p className="text-xs text-muted-foreground">
                {t(
                  summary.instructionsIncluded
                    ? "instructionsIncluded"
                    : "noInstructions"
                )}
              </p>
            </SettingRow>
            <SettingRow title={t("paths")}>
              <p className="text-xs text-muted-foreground">
                {t("pathCounts", {
                  read: summary.readRuleCount,
                  write: summary.writeRuleCount,
                })}
              </p>
            </SettingRow>
            <SettingRow title={t("otherRules")}>
              <p className="text-xs text-muted-foreground">
                {t("limits", {
                  shell: summary.shellRuleCount,
                  network: summary.network,
                })}
              </p>
            </SettingRow>
          </SettingCard>
          <p className="text-xs leading-5 text-muted-foreground">
            {t("pending")}
          </p>
        </>
      )}
      {snapshotStorageSupported && (
        <MemberSnapshotPanel
          key={summary?.snapshotVersion ?? "unprepared"}
          memberId={memberId}
          revision={revision}
          checkVersion={checkVersion}
          expectedSnapshotVersion={summary?.snapshotVersion ?? null}
          client={client}
        />
      )}
    </div>
  )
}
