"use client"
import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { toErrorMessage } from "@/lib/app-error"
import {
  isExchangeUnavailable,
  type ArtifactRef,
  type ExchangeClient,
} from "@/lib/hall/exchange"

/** `null` = the host did not say; never read as a negative or positive. */
export type ArtifactCollection =
  | { state: "loading" }
  | { state: "unsupported" }
  | { state: "failed" }
  | { state: "ready"; artifacts: ArtifactRef[] }

const TERMINAL = ["succeeded", "failed", "interrupted", "cancelled"] as const
type Terminal = (typeof TERMINAL)[number]
const execKeys = [
  "pending",
  "running",
  "succeeded",
  "failed",
  "interrupted",
  "cancelled",
] as const
type ExecKey = (typeof execKeys)[number]

function executionKey(status: string | null | undefined): ExecKey | "unknown" {
  if (!status) return "unknown"
  if (status === "awaiting_permission") return "running"
  if (status === "outcome_unknown") return "interrupted"
  if (
    [
      "queued",
      "draft",
      "pending_delivery",
      "awaiting_acceptance",
      "accepted",
    ].includes(status)
  )
    return "pending"
  return (execKeys as readonly string[]).includes(status)
    ? (status as ExecKey)
    : "unknown"
}

/**
 * Exit is confirmed only by positive evidence: a host timestamp, or every
 * collected file sealed with exitVerified. Missing evidence is "unknown".
 */
export function exitState(
  exitConfirmedAt: string | null | undefined,
  artifacts: ArtifactRef[] | null,
  finished = true
): "confirmed" | "unconfirmed" | "unknown" {
  if (artifacts?.some((a) => !a.exitVerified)) return "unconfirmed"
  if (exitConfirmedAt) return "confirmed"
  // A null timestamp only means "not confirmed" once the task has ended.
  if (exitConfirmedAt === null) return finished ? "unconfirmed" : "unknown"
  if (artifacts && artifacts.length > 0) return "confirmed"
  return "unknown"
}

export function TaskArtifacts({
  taskStatus,
  exitConfirmedAt,
  reported,
  collection,
  download,
}: {
  taskStatus: string | null | undefined
  exitConfirmedAt?: string | null
  reported?: boolean
  collection: ArtifactCollection
  /** Absent when this shell cannot save files; downloads stay disabled. */
  download?: (artifact: ArtifactRef) => Promise<void>
}) {
  const t = useTranslations("Hall.artifacts")
  const [saving, setSaving] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const alive = useRef(true)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])
  const artifacts = collection.state === "ready" ? collection.artifacts : null
  const exec = executionKey(taskStatus)
  const finished = (TERMINAL as readonly string[]).includes(exec as Terminal)
  const exit = exitState(exitConfirmedAt, artifacts, finished)
  // A finished task is only described as a plain success with confirmed exit.
  const execLabel =
    exec === "succeeded" && exit !== "confirmed" ? "succeededUnconfirmed" : exec
  const collectionKey =
    collection.state === "ready"
      ? collection.artifacts.length
        ? "collected"
        : "none"
      : collection.state
  async function save(artifact: ArtifactRef) {
    if (!download || saving) return
    setSaving(artifact.objectId)
    setError(null)
    try {
      await download(artifact)
    } catch (failure) {
      if (alive.current)
        setError(t("downloadFailed", { message: toErrorMessage(failure) }))
    } finally {
      if (alive.current) setSaving(null)
    }
  }
  return (
    <section
      className="space-y-2 rounded-lg border p-3 text-sm"
      aria-label={t("title")}
    >
      <h4 className="font-medium">{t("title")}</h4>
      <dl className="grid gap-2 sm:grid-cols-2">
        <div>
          <dt className="text-muted-foreground">{t("execution")}</dt>
          <dd data-state={execLabel}>{t(`exec.${execLabel}`)}</dd>
        </div>
        <div>
          <dt className="text-muted-foreground">{t("exit")}</dt>
          <dd data-state={exit}>{t(`exitState.${exit}`)}</dd>
        </div>
        <div>
          <dt className="text-muted-foreground">{t("report")}</dt>
          <dd>
            {t(
              reported === undefined
                ? "reportState.unknown"
                : reported
                  ? "reportState.received"
                  : "reportState.missing"
            )}
          </dd>
        </div>
        <div>
          <dt className="text-muted-foreground">{t("collection")}</dt>
          <dd data-state={collectionKey}>
            {t(`collectionState.${collectionKey}`)}
          </dd>
        </div>
      </dl>
      {artifacts && artifacts.length > 0 && exit !== "confirmed" && (
        <p role="alert" className="text-xs text-destructive">
          {t("unverifiedNotice")}
        </p>
      )}
      {finished &&
        exec !== "succeeded" &&
        artifacts &&
        artifacts.length > 0 && (
          <p className="text-xs text-muted-foreground">
            {t("filesNotSuccess")}
          </p>
        )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      {artifacts && artifacts.length > 0 && (
        <ul className="space-y-1">
          {artifacts.map((a) => (
            <li
              key={a.objectId}
              className="flex flex-wrap items-center gap-2 rounded border p-2"
            >
              <span className="break-all font-medium">{a.displayName}</span>
              <span className="text-xs text-muted-foreground">
                {t("size", { bytes: a.size })}
              </span>
              <code
                dir="ltr"
                className="break-all text-xs text-muted-foreground"
              >
                {a.sha256.slice(0, 12)}
              </code>
              {!a.exitVerified && (
                <span className="text-xs text-destructive">
                  {t("fileUnverified")}
                </span>
              )}
              <Button
                size="sm"
                variant="outline"
                disabled={!download || saving !== null}
                onClick={() => void save(a)}
              >
                {t(saving === a.objectId ? "saving" : "download")}
              </Button>
            </li>
          ))}
        </ul>
      )}
      {artifacts && artifacts.length > 0 && !download && (
        <p className="text-xs text-muted-foreground">
          {t("downloadUnavailable")}
        </p>
      )}
    </section>
  )
}

/** Loads the collected files of one dispatch and renders the four states. */
export function DispatchTaskArtifacts({
  exchange,
  dispatchId,
  taskStatus,
  exitConfirmedAt,
  reported,
  download,
  refreshKey,
}: {
  exchange?: ExchangeClient
  dispatchId: string
  taskStatus: string | null | undefined
  exitConfirmedAt?: string | null
  reported?: boolean
  download?: (artifact: ArtifactRef) => Promise<void>
  /** Changes when the parent row changed, e.g. after a sync. */
  refreshKey?: string | number
}) {
  const key = `${dispatchId}:${refreshKey ?? ""}`
  const [fetched, setFetched] = useState<{
    exchange: ExchangeClient
    key: string
    value: ArtifactCollection
  } | null>(null)
  useEffect(() => {
    if (!exchange) return
    let current = true
    const done = (value: ArtifactCollection) => {
      if (current) setFetched({ exchange, key, value })
    }
    exchange
      .artifacts(dispatchId)
      .then((artifacts) => done({ state: "ready", artifacts }))
      .catch((failure: unknown) =>
        done({
          state: isExchangeUnavailable(failure) ? "unsupported" : "failed",
        })
      )
    return () => {
      current = false
    }
  }, [exchange, dispatchId, key])
  // A different exchange, dispatch or revision shows "loading" until its own
  // answer arrives; an older answer is never reused.
  const collection: ArtifactCollection = !exchange
    ? { state: "unsupported" }
    : fetched?.exchange === exchange && fetched.key === key
      ? fetched.value
      : { state: "loading" }
  return (
    <TaskArtifacts
      taskStatus={taskStatus}
      exitConfirmedAt={exitConfirmedAt}
      reported={reported}
      collection={collection}
      download={download}
    />
  )
}
