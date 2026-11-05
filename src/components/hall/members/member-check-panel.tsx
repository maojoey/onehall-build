"use client"

import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { ClipboardCheck } from "lucide-react"
import { SettingCard, SettingRow } from "@/components/shared/setting-card"
import { SettingsSection } from "@/components/shared/settings-section"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { extractAppCommandError } from "@/lib/app-error"
import {
  isMembersUnavailable,
  type HallMemberCheckReport,
  type HallMembersClient,
} from "@/lib/hall/members"
import { diagnosticMessage } from "./member-config-shared"
import { MemberExecutionPanel } from "./member-execution-panel"

const codes = [
  "directory_ok",
  "directory_unavailable",
  "declaration_valid",
  "declaration_invalid",
  "defaults_only",
  "engine_codex",
  "engine_claude_code",
  "engine_unsupported",
  "template_not_bound",
  "template_missing",
  "template_disabled",
  "template_engine_mismatch",
  "template_changed",
  "template_invalid",
  "template_ok",
  "project_not_bound",
  "project_unavailable",
  "project_ok",
  "runtime_unverified",
  "execution_pending",
] as const
const ids = [
  "directory",
  "declaration",
  "engine",
  "template",
  "project",
  "runtime",
  "execution",
] as const
const statuses = ["passed", "warning", "blocked", "pending"] as const

/** Keyed by the persisted observation; drafts never share this report. */
export function MemberCheckPanel({
  memberId,
  revision,
  checkVersion,
  client,
}: {
  memberId: string
  revision: number
  checkVersion: string
  client: HallMembersClient
}) {
  const t = useTranslations("Hall.members.check")
  const tm = useTranslations("Hall.members")
  const [result, setResult] = useState<HallMemberCheckReport | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [generation, setGeneration] = useState(0)
  const alive = useRef(false)
  const inFlight = useRef(false)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  async function check() {
    if (inFlight.current) return
    inFlight.current = true
    setBusy(true)
    setGeneration((current) => current + 1)
    setResult(null)
    setError(null)
    try {
      const response = await client.checkMember(
        memberId,
        revision,
        checkVersion
      )
      if (!alive.current) return
      if (
        response.memberId !== memberId ||
        response.revision !== revision ||
        response.checkVersion !== checkVersion ||
        response.activationAvailable !== false
      ) {
        setError(t("conflict"))
      } else setResult(response)
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

  return (
    <SettingsSection
      icon={ClipboardCheck}
      title={t("title")}
      description={t("description")}
    >
      <div className="space-y-3 p-3">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={busy}
          onClick={() => void check()}
        >
          {t(busy ? "checking" : "run")}
        </Button>
        {busy && (
          <p role="status" className="text-xs text-muted-foreground">
            {t("checking")}
          </p>
        )}
        {error && (
          <p role="alert" className="text-xs text-destructive">
            {error}
          </p>
        )}
        {result && (
          <>
            <p role="status" className="text-xs text-muted-foreground">
              {t(result.configurationValid ? "dataPassed" : "needsChanges")}
            </p>
            <SettingCard>
              {result.checks.map((row, i) => {
                const code =
                  codes.find((code) => code === row.code) ?? "unknown"
                const id = ids.find((id) => id === row.id) ?? "other"
                const status =
                  statuses.find((status) => status === row.status) ?? "pending"
                return (
                  <SettingRow
                    key={i}
                    title={t(`items.${id}`)}
                    control={
                      <Badge
                        variant={
                          status === "blocked" ? "destructive" : "secondary"
                        }
                      >
                        {t(`statuses.${status}`)}
                      </Badge>
                    }
                  >
                    <p className="text-xs leading-5 text-muted-foreground">
                      {t(`messages.${code}`, {
                        recorded: result.templateRecordedRevision ?? "—",
                        current: result.templateCurrentRevision ?? "—",
                      })}
                    </p>
                  </SettingRow>
                )
              })}
            </SettingCard>
            {!!result.diagnostics.length && (
              <ul role="alert" className="space-y-2 text-xs text-destructive">
                {result.diagnostics.map((diagnostic, i) => (
                  <li key={i}>
                    <p dir="ltr" className="break-all text-start font-mono">
                      {diagnostic.source} · {diagnostic.field}
                    </p>
                    <p>{tm(diagnosticMessage(diagnostic.code))}</p>
                  </li>
                ))}
              </ul>
            )}
            <p className="text-xs text-muted-foreground">{t("observedOnly")}</p>
            {result.executionPreviewSupported ? (
              <MemberExecutionPanel
                key={generation}
                memberId={memberId}
                revision={revision}
                checkVersion={checkVersion}
                client={client}
                dataValid={result.configurationValid}
                snapshotStorageSupported={result.snapshotStorageSupported}
                onChecked={setResult}
              />
            ) : (
              <p className="text-xs text-muted-foreground">
                {tm("execution.unsupported")}
              </p>
            )}
          </>
        )}
      </div>
    </SettingsSection>
  )
}
