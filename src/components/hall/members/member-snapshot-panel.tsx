"use client"

import { useEffect, useRef, useState } from "react"
import { useLocale, useTranslations } from "next-intl"
import { SettingCard, SettingRow } from "@/components/shared/setting-card"
import { Button } from "@/components/ui/button"
import { extractAppCommandError } from "@/lib/app-error"
import {
  isMembersUnavailable,
  type HallMemberSnapshot,
  type HallMembersClient,
  isHallEngine,
  hallSafeModes,
} from "@/lib/hall/members"

function valid(record: HallMemberSnapshot, memberId: string) {
  return (
    record?.memberId === memberId &&
    /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(
      record.snapshotId
    ) &&
    Number.isInteger(record.memberRevision) &&
    record.memberRevision > 0 &&
    /^[a-f0-9]{64}$/i.test(record.checkVersion) &&
    Number.isFinite(Date.parse(record.createdAt)) &&
    record.launchAllowed === false &&
    record.summary?.runtimeVerified === false &&
    record.summary.permissionPolicy === "wait" &&
    isHallEngine(record.summary.engine) &&
    hallSafeModes(record.summary.engine).includes(
      record.summary.requestedMode
    ) &&
    /^[a-f0-9]{64}$/i.test(record.summary.snapshotVersion)
  )
}

export function MemberSnapshotPanel({
  memberId,
  revision,
  checkVersion,
  expectedSnapshotVersion,
  client,
}: {
  memberId: string
  revision: number
  checkVersion: string
  expectedSnapshotVersion: string | null
  client: HallMembersClient
}) {
  const t = useTranslations("Hall.members.snapshots")
  const te = useTranslations("Hall.members.execution")
  const locale = useLocale()
  const [records, setRecords] = useState<HallMemberSnapshot[] | null>(null)
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<"save" | "list" | null>(null)
  const [stale, setStale] = useState(false)
  const alive = useRef(false)
  const inFlight = useRef(false)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  async function perform(action: "save" | "list") {
    if (
      inFlight.current ||
      (action === "save" && (!expectedSnapshotVersion || stale))
    )
      return
    inFlight.current = true
    setBusy(action)
    setSaved(false)
    setError(null)
    if (action === "list") setRecords(null)
    try {
      if (action === "save") {
        const record = await client.saveSnapshot(
          memberId,
          revision,
          checkVersion,
          expectedSnapshotVersion!
        )
        if (!alive.current) return
        if (
          !valid(record, memberId) ||
          record.memberRevision !== revision ||
          record.checkVersion !== checkVersion ||
          record.summary.snapshotVersion !== expectedSnapshotVersion
        ) {
          setStale(true)
          setError(t("conflict"))
          return
        }
        setRecords((current) =>
          [
            record,
            ...(current ?? []).filter(
              (row) => row.snapshotId !== record.snapshotId
            ),
          ]
            .sort(
              (a, b) =>
                Date.parse(b.createdAt) - Date.parse(a.createdAt) ||
                b.snapshotId.localeCompare(a.snapshotId)
            )
            .slice(0, 20)
        )
        setSaved(true)
      } else {
        const rows = await client.listSnapshots(memberId)
        if (!alive.current) return
        if (
          !Array.isArray(rows) ||
          rows.length > 20 ||
          rows.some((row) => !valid(row, memberId)) ||
          new Set(rows.map((row) => row.snapshotId)).size !== rows.length
        ) {
          setError(t("failed"))
          return
        }
        setRecords(rows)
      }
    } catch (failure) {
      if (!alive.current) return
      const code = extractAppCommandError(failure)?.code
      const conflict = code === "already_exists" || code === "not_found"
      if (action === "save" && conflict) setStale(true)
      setError(
        isMembersUnavailable(failure)
          ? t("unsupported")
          : conflict
            ? t("conflict")
            : t(action === "save" ? "unconfirmed" : "failed")
      )
    } finally {
      inFlight.current = false
      if (alive.current) setBusy(null)
    }
  }

  return (
    <div className="space-y-3">
      <p className="text-sm font-medium">{t("title")}</p>
      <p className="text-xs leading-5 text-muted-foreground">
        {t("description")}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!!busy || !expectedSnapshotVersion || stale}
          onClick={() => void perform("save")}
        >
          {t(busy === "save" ? "saving" : "save")}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!!busy}
          onClick={() => void perform("list")}
        >
          {t(busy === "list" ? "loading" : "load")}
        </Button>
      </div>
      {saved && (
        <p role="status" className="text-xs text-muted-foreground">
          {t("saved")}
        </p>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      {records?.length === 0 && (
        <p role="status" className="text-xs text-muted-foreground">
          {t("empty")}
        </p>
      )}
      {records && records.length > 0 && (
        <>
          <p className="text-xs text-muted-foreground">
            {t("history", { count: records.length })}
          </p>
          {records.map((record) => (
            <SettingCard key={record.snapshotId}>
              <SettingRow title={t("record")}>
                <p
                  dir="ltr"
                  className="break-all text-start font-mono text-xs text-muted-foreground"
                >
                  {record.snapshotId}
                </p>
                <p className="text-xs text-muted-foreground">
                  <time dateTime={record.createdAt}>
                    {new Date(record.createdAt).toLocaleString(locale)}
                  </time>
                </p>
                <p className="text-xs text-muted-foreground">
                  {t("revision", { revision: record.memberRevision })} ·{" "}
                  {te(
                    record.summary.requestedMode === "read-only"
                      ? "readOnly"
                      : "workspaceWrite"
                  )}
                </p>
              </SettingRow>
              <SettingRow title={te("version")}>
                <p
                  dir="ltr"
                  className="break-all text-start font-mono text-xs text-muted-foreground"
                >
                  {record.summary.snapshotVersion}
                </p>
              </SettingRow>
            </SettingCard>
          ))}
        </>
      )}
      <p className="text-xs leading-5 text-muted-foreground">{t("pending")}</p>
    </div>
  )
}
