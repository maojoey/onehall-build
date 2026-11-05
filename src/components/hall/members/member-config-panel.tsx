"use client"

import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { FileText } from "lucide-react"
import { SettingCard, SettingRow } from "@/components/shared/setting-card"
import { SettingsSection } from "@/components/shared/settings-section"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { extractAppCommandError, toErrorMessage } from "@/lib/app-error"
import {
  isMembersUnavailable,
  type HallMemberConfigPreview,
  type HallMembersClient,
} from "@/lib/hall/members"

import {
  labels,
  diagnosticMessage,
  fieldValue,
  ConfigValue,
} from "./member-config-shared"
import { MemberConfigEditor } from "./member-config-editor"
import { MemberCheckPanel } from "./member-check-panel"

/** The caller keys this panel by member + revision, within its backend realm. */
export function MemberConfigPanel({
  memberId,
  revision,
  client,
}: {
  memberId: string
  revision: number
  client: HallMembersClient
}) {
  const t = useTranslations("Hall.members")
  const [result, setResult] = useState<HallMemberConfigPreview | null>(null)
  const [editing, setEditing] = useState(false)
  const [saved, setSaved] = useState(false)
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

  async function read() {
    if (inFlight.current || editing) return
    inFlight.current = true
    setBusy(true)
    setError(null)
    setSaved(false)
    setResult(null)
    try {
      const response = await client.previewConfig(memberId, revision)
      if (response.memberId !== memberId || response.revision !== revision) {
        if (alive.current) setError(t("conflict"))
        return
      }
      if (alive.current) setResult(response)
    } catch (failure) {
      if (!alive.current) return
      const code = extractAppCommandError(failure)?.code
      setError(
        isMembersUnavailable(failure)
          ? t("config.unsupported")
          : code === "already_exists"
            ? t("conflict")
            : code === "not_found"
              ? t("notFound")
              : t("config.failed", { message: toErrorMessage(failure) })
      )
    } finally {
      inFlight.current = false
      if (alive.current) setBusy(false)
    }
  }

  return (
    <>
      <SettingsSection
        icon={FileText}
        title={t("config.title")}
        description={t("config.description")}
      >
        <div className="space-y-3 p-3">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busy || editing}
            onClick={() => void read()}
          >
            {t(busy ? "config.reading" : "config.read")}
          </Button>
          {busy && (
            <p role="status" className="text-xs text-muted-foreground">
              {t("config.reading")}
            </p>
          )}
          {error && (
            <p role="alert" className="text-xs text-destructive">
              {error}
            </p>
          )}
          {saved && (
            <p role="status" className="text-xs text-muted-foreground">
              {t("config.save.saved")}
            </p>
          )}
          {result && (
            <>
              <p className="text-xs text-muted-foreground">
                {t(result.valid ? "config.valid" : "config.invalid")}
              </p>
              {!!result.files.length && (
                <SettingCard>
                  {result.files.map((file) => (
                    <SettingRow
                      key={file.path}
                      title={
                        <span className="break-all font-mono text-xs">
                          {file.path}
                        </span>
                      }
                      control={
                        <Badge variant="secondary">
                          {t(`config.files.${file.status}`)}
                        </Badge>
                      }
                    />
                  ))}
                </SettingCard>
              )}
              {!!result.diagnostics.length && (
                <ul role="alert" className="space-y-2 text-xs text-destructive">
                  {result.diagnostics.map((diagnostic, i) => (
                    <li key={i} className="space-y-1">
                      <p dir="ltr" className="break-all text-start font-mono">
                        {diagnostic.source} · {diagnostic.field}
                      </p>
                      <p>{t(diagnosticMessage(diagnostic.code))}</p>
                    </li>
                  ))}
                </ul>
              )}
              {result.valid && result.effective && (
                <SettingCard>
                  {result.fields.map((field) => {
                    const value = fieldValue(result, field.field)
                    return (
                      <SettingRow
                        key={field.field}
                        title={t(`config.fields.${labels[field.field]}`)}
                        description={
                          field.overridden ? t("config.overridden") : undefined
                        }
                      >
                        <Badge
                          variant="outline"
                          className="max-w-full break-all whitespace-normal text-start font-mono text-xs"
                        >
                          {field.source === "defaults"
                            ? t("config.defaults")
                            : field.source}
                        </Badge>
                        <ConfigValue value={value} />
                      </SettingRow>
                    )
                  })}
                </SettingCard>
              )}
              {result.valid &&
                result.effective &&
                (result.fileVersion && result.overlay ? (
                  <MemberConfigEditor
                    baseline={result}
                    client={client}
                    onEditingChange={(next) => {
                      setEditing(next)
                      if (next) setSaved(false)
                    }}
                    onSaved={(response) => {
                      setResult(response)
                      setSaved(true)
                    }}
                  />
                ) : (
                  <p className="text-xs text-muted-foreground">
                    {t("config.draft.unsupported")}
                  </p>
                ))}
              {!editing && (!result.checkSupported || !result.checkVersion) && (
                <p className="text-xs text-muted-foreground">
                  {t("check.unsupported")}
                </p>
              )}
            </>
          )}
        </div>
      </SettingsSection>
      {!editing && result?.checkSupported && result.checkVersion && (
        <MemberCheckPanel
          key={result.checkVersion}
          memberId={memberId}
          revision={revision}
          checkVersion={result.checkVersion}
          client={client}
        />
      )}
    </>
  )
}
