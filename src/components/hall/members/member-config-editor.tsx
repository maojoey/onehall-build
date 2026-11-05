"use client"

import { useEffect, useId, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  SettingCard,
  SettingNote,
  SettingRow,
} from "@/components/shared/setting-card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import { extractAppCommandError } from "@/lib/app-error"
import {
  isHallExecutionMode,
  isMembersUnavailable,
  type HallMemberConfigDraftPreview,
  type HallMemberConfigField,
  type HallMemberConfigOverlay,
  type HallMemberConfigPreview,
  type HallExecutionMode,
  type HallMembersClient,
} from "@/lib/hall/members"
import {
  ConfigValue,
  diagnosticMessage,
  fieldValue,
  labels,
} from "./member-config-shared"

type Entries = Record<
  HallMemberConfigField,
  { override: boolean; text: string }
>
type Props = {
  baseline: HallMemberConfigPreview
  client: HallMembersClient
  onEditingChange: (editing: boolean) => void
  onSaved: (result: HallMemberConfigPreview) => void
}
const fields = Object.keys(labels) as HallMemberConfigField[]

function initialEntries(baseline: HallMemberConfigPreview): Entries {
  const overlay = baseline.overlay
  const flat = {
    model: overlay?.model,
    "scope.read": overlay?.scope?.read,
    "scope.write": overlay?.scope?.write,
    "scope.shell": overlay?.scope?.shell,
    "scope.network": overlay?.scope?.network,
    "skills.shared": overlay?.skills?.shared,
    "skills.own": overlay?.skills?.own,
    onedesk_key: overlay?.onedesk_key,
    execution_mode: overlay?.execution_mode,
  }
  return Object.fromEntries(
    fields.map((field) => {
      const value = flat[field] ?? fieldValue(baseline, field)
      return [
        field,
        {
          override: flat[field] !== undefined,
          text: Array.isArray(value) ? value.join("\n") : (value ?? ""),
        },
      ]
    })
  ) as Entries
}

function buildOverlay(entries: Entries): HallMemberConfigOverlay {
  const draft: HallMemberConfigOverlay = { schema_version: 1 }
  if (entries.model.override) draft.model = entries.model.text
  for (const field of fields) {
    if (!entries[field].override || field === "model") continue
    const lines = entries[field].text
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
    switch (field) {
      case "scope.network": {
        const text = entries[field].text
        draft.scope = {
          ...draft.scope,
          network: text === "default" || text === "local" ? text : "none",
        }
        break
      }
      case "onedesk_key":
        draft.onedesk_key = entries[field].text === "read" ? "read" : "none"
        break
      case "execution_mode":
        // Never defaulted: an unchosen mode is left out, not guessed.
        if (isHallExecutionMode(entries[field].text))
          draft.execution_mode = entries[field].text as HallExecutionMode
        break
      case "scope.read":
      case "scope.write":
      case "scope.shell":
        draft.scope = { ...draft.scope, [field.slice(6)]: lines }
        break
      case "skills.shared":
      case "skills.own":
        draft.skills = { ...draft.skills, [field.slice(7)]: lines }
    }
  }
  return draft
}

export function MemberConfigEditor(props: Props) {
  const t = useTranslations("Hall.members.config.draft")
  const [open, setOpen] = useState(false)
  const change = (editing: boolean) => {
    setOpen(editing)
    props.onEditingChange(editing)
  }
  return open ? (
    <DraftForm
      {...props}
      cancel={() => change(false)}
      onSaved={(result) => {
        props.onSaved(result)
        change(false)
      }}
    />
  ) : (
    <Button
      variant="outline"
      size="sm"
      type="button"
      onClick={() => change(true)}
    >
      {t("edit")}
    </Button>
  )
}

function DraftForm({
  baseline,
  client,
  cancel,
  onSaved,
}: Props & { cancel: () => void }) {
  const t = useTranslations("Hall.members")
  const id = useId()
  const [entries, setEntries] = useState(() => initialEntries(baseline))
  const [result, setResult] = useState<HallMemberConfigDraftPreview | null>(
    null
  )
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [saving, setSaving] = useState(false)
  const [saveUncertain, setSaveUncertain] = useState(false)
  const alive = useRef(false)
  const inFlight = useRef(false)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])
  const modeEntry = entries.execution_mode
  const modeIncomplete =
    modeEntry.override && !isHallExecutionMode(modeEntry.text)

  function update(
    field: HallMemberConfigField,
    patch: Partial<Entries[HallMemberConfigField]>
  ) {
    if (saving || saveUncertain) return
    setEntries((current) => ({
      ...current,
      [field]: { ...current[field], ...patch },
    }))
    setResult(null)
    setError(null)
  }
  async function preview() {
    if (
      inFlight.current ||
      saveUncertain ||
      modeIncomplete ||
      !baseline.fileVersion
    )
      return
    inFlight.current = true
    setBusy(true)
    setResult(null)
    setError(null)
    try {
      const response = await client.previewDraft(
        baseline.memberId,
        baseline.revision,
        baseline.fileVersion,
        buildOverlay(entries)
      )
      if (!alive.current) return
      if (
        response.memberId !== baseline.memberId ||
        response.revision !== baseline.revision ||
        response.fileVersion !== baseline.fileVersion
      ) {
        setError(t("config.draft.conflict"))
      } else setResult(response)
    } catch (failure) {
      if (!alive.current) return
      const code = extractAppCommandError(failure)?.code
      setError(
        isMembersUnavailable(failure)
          ? t("config.draft.unsupported")
          : code === "already_exists" || code === "not_found"
            ? t("config.draft.conflict")
            : t("config.draft.failed")
      )
    } finally {
      inFlight.current = false
      if (alive.current) setBusy(false)
    }
  }
  async function save() {
    if (
      inFlight.current ||
      saveUncertain ||
      modeIncomplete ||
      !baseline.saveSupported ||
      !baseline.fileVersion ||
      !result?.valid ||
      !result.changes.length
    )
      return
    inFlight.current = true
    setSaving(true)
    setError(null)
    try {
      const response = await client.saveConfig(
        baseline.memberId,
        baseline.revision,
        baseline.fileVersion,
        buildOverlay(entries)
      )
      if (!alive.current) return
      if (
        response.memberId !== baseline.memberId ||
        response.revision !== baseline.revision ||
        !response.valid ||
        !response.effective ||
        !response.overlay ||
        !/^[a-f0-9]{64}$/i.test(response.fileVersion ?? "")
      ) {
        throw new Error("Unconfirmed save result")
      }
      onSaved(response)
    } catch (failure) {
      if (!alive.current) return
      setSaveUncertain(true)
      setResult(null)
      const code = extractAppCommandError(failure)?.code
      setError(
        isMembersUnavailable(failure)
          ? t("config.save.unsupported")
          : code === "already_exists" || code === "not_found"
            ? t("config.save.conflict")
            : t("config.save.failed")
      )
    } finally {
      inFlight.current = false
      if (alive.current) setSaving(false)
    }
  }
  function source(value: string, side: "before" | "after") {
    return value === "defaults"
      ? t("config.defaults")
      : value === ".onehall/agent.ui.yaml" && side === "after"
        ? t("config.draft.overlay")
        : value
  }

  return (
    <div className="space-y-3">
      <SettingNote>
        {t(
          baseline.saveSupported
            ? "config.save.description"
            : "config.draft.description"
        )}
      </SettingNote>
      <p className="break-all text-xs text-muted-foreground">
        {t("config.draft.target")} <code>.onehall/agent.ui.yaml</code>
      </p>
      <SettingCard>
        {fields.map((field) => {
          const label = t(`config.fields.${labels[field]}`)
          const entry = entries[field]
          const controlId = `${id}-${field}`
          return (
            <SettingRow
              key={field}
              title={label}
              htmlFor={entry.override ? controlId : undefined}
              description={t(
                entry.override
                  ? "config.draft.overriding"
                  : "config.draft.inheriting"
              )}
              control={
                <Switch
                  aria-label={t("config.draft.overrideField", { field: label })}
                  checked={entry.override}
                  disabled={busy || saving || saveUncertain}
                  onCheckedChange={(checked) =>
                    update(field, { override: checked })
                  }
                />
              }
            >
              {!entry.override ? null : field === "model" ? (
                <Input
                  id={controlId}
                  value={entry.text}
                  disabled={busy || saving || saveUncertain || !entry.override}
                  onChange={(event) =>
                    update(field, { text: event.target.value })
                  }
                  maxLength={160}
                />
              ) : field === "scope.network" ||
                field === "onedesk_key" ||
                field === "execution_mode" ? (
                <Select
                  value={entry.text}
                  disabled={busy || saving || saveUncertain || !entry.override}
                  onValueChange={(text) => update(field, { text })}
                >
                  <SelectTrigger id={controlId} className="w-full">
                    <SelectValue
                      placeholder={
                        field === "execution_mode"
                          ? t("executionMode.choose")
                          : undefined
                      }
                    />
                  </SelectTrigger>
                  <SelectContent>
                    {(field === "onedesk_key"
                      ? ["none", "read"]
                      : field === "execution_mode"
                        ? ["restricted", "local_assistant"]
                        : ["none", "local", "default"]
                    ).map((value) => (
                      <SelectItem key={value} value={value}>
                        {field === "execution_mode"
                          ? t(
                              `executionMode.label.${value as HallExecutionMode}`
                            )
                          : value}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <Textarea
                  id={controlId}
                  value={entry.text}
                  disabled={busy || saving || saveUncertain || !entry.override}
                  onChange={(event) =>
                    update(field, { text: event.target.value })
                  }
                  rows={3}
                  className="font-mono text-xs"
                />
              )}
            </SettingRow>
          )
        })}
      </SettingCard>
      <p className="text-xs text-muted-foreground">
        {t("config.draft.arrays")}
      </p>
      {modeIncomplete && (
        <p role="alert" className="text-xs text-destructive">
          {t("executionMode.chooseRequired")}
        </p>
      )}
      {modeEntry.override && (
        <p className="text-xs text-muted-foreground">
          {t("executionMode.newSnapshotNote")}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          type="button"
          disabled={busy || saving || saveUncertain || modeIncomplete}
          onClick={() => void preview()}
        >
          {t(busy ? "config.draft.previewing" : "config.draft.preview")}
        </Button>
        {baseline.saveSupported && (
          <Button
            size="sm"
            type="button"
            disabled={
              busy ||
              saving ||
              saveUncertain ||
              !result?.valid ||
              !result.changes.length
            }
            onClick={() => void save()}
          >
            {t(saving ? "config.save.saving" : "config.save.save")}
          </Button>
        )}
        <Button
          size="sm"
          type="button"
          variant="outline"
          disabled={saving}
          onClick={cancel}
        >
          {t("config.draft.cancel")}
        </Button>
      </div>
      {!baseline.saveSupported && (
        <p className="text-xs text-muted-foreground">
          {t("config.save.unsupported")}
        </p>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      {result && (
        <div className="space-y-3" role="status">
          <p className="text-xs text-muted-foreground">
            {t(
              !result.valid
                ? "config.invalid"
                : result.changes.length
                  ? "config.draft.changes"
                  : "config.draft.unchanged"
            )}
          </p>
          {result.diagnostics.map((diagnostic, i) => (
            <p key={i} role="alert" className="text-xs text-destructive">
              {t(diagnosticMessage(diagnostic.code))}
            </p>
          ))}
          {result.valid && result.changes.length > 0 && (
            <SettingCard>
              {result.changes.map((change) => (
                <SettingRow
                  key={change.field}
                  title={t(`config.fields.${labels[change.field]}`)}
                >
                  <div className="grid min-w-0 gap-3 sm:grid-cols-2">
                    {(["before", "after"] as const).map((side) => (
                      <div key={side} className="min-w-0 space-y-2">
                        <p className="text-xs font-medium">
                          {t(`config.draft.${side}`)}
                        </p>
                        <Badge
                          variant="outline"
                          className="max-w-full break-all whitespace-normal text-start text-xs"
                        >
                          {source(change[`${side}Source`], side)}
                        </Badge>
                        <ConfigValue value={change[side]} />
                      </div>
                    ))}
                  </div>
                </SettingRow>
              ))}
            </SettingCard>
          )}
        </div>
      )}
    </div>
  )
}
