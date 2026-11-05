"use client"

import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { toErrorMessage } from "@/lib/app-error"
import { ExecutionCapabilities } from "@/components/hall/members/execution-capabilities"
import { ExchangeManifestView } from "@/components/hall/tasks/exchange-manifest"
import { isExchangeManifest } from "@/lib/hall/exchange"
import {
  isTasksUnavailable,
  type HallTasksClient,
  type HallTaskDetail,
  type LaunchReview,
} from "@/lib/hall/tasks"

export function LaunchReviewPanel({
  client,
  task,
  memberName,
  onChanged,
}: {
  client: HallTasksClient
  task: HallTaskDetail
  memberName: string
  onChanged: () => Promise<void>
}) {
  const t = useTranslations("Hall.workflow")
  const [plan, setPlan] = useState<LaunchReview | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const latest = task.attempts[task.attempts.length - 1]
  const approved = task.launchReviews?.find(
    (review) => review.attemptId === latest?.attemptId
  )
  const manifestReady =
    plan?.exchangeManifestDigest === undefined
      ? plan?.exchangeManifest === undefined
      : isExchangeManifest(plan.exchangeManifest) &&
        plan.exchangeManifest.digest === plan.exchangeManifestDigest
  const submission = useRef<string | null>(null)
  const alive = useRef(false)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])
  async function act(action: "review" | "approve" | "cancel") {
    if (busy || (action === "approve" && !manifestReady)) return
    setBusy(true)
    setError(null)
    try {
      if (action === "review") {
        const current = await client.review(task.taskId)
        if (
          current.taskId !== task.taskId ||
          current.taskRevision !== task.revision ||
          current.memberId !== task.memberId ||
          current.snapshotId !== task.snapshot.snapshotId ||
          current.realExecutionAllowed !== false
        )
          throw new Error(t("planChanged"))
        if (alive.current) setPlan(current)
      } else {
        if (action === "approve" && plan) {
          submission.current ??= crypto.randomUUID()
          await client.approve(plan, submission.current)
        } else if (action === "cancel") await client.cancelQueued(task)
        if (alive.current) await onChanged()
      }
    } catch (failure) {
      if (alive.current)
        setError(
          isTasksUnavailable(failure)
            ? t("unsupported")
            : t("uncertain", { message: toErrorMessage(failure) })
        )
    } finally {
      if (alive.current) setBusy(false)
    }
  }
  if (task.status !== "draft" && task.status !== "queued") return null
  return (
    <section
      className="space-y-3 rounded-lg border bg-card p-4"
      aria-label={t("reviewTitle")}
    >
      <h3 className="text-sm font-medium">{t("reviewTitle")}</h3>
      <p className="text-sm text-muted-foreground">{t("closed")}</p>
      {approved && (
        <p className="text-sm text-muted-foreground">
          {t("approvedAt", { time: approved.approvedAt })}
        </p>
      )}
      {plan && (
        <div className="space-y-2 text-sm">
          <dl className="grid gap-2 sm:grid-cols-2">
            <div>
              <dt className="text-muted-foreground">{t("member")}</dt>
              <dd>{memberName}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">{t("cwd")}</dt>
              <dd className="break-all">{plan.cwd}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">{t("mode")}</dt>
              <dd>
                {plan.engine} · {plan.mode}
              </dd>
            </div>
            <div>
              <dt className="text-muted-foreground">{t("task")}</dt>
              <dd>{plan.title}</dd>
            </div>
          </dl>
          <p className="whitespace-pre-wrap break-words">{plan.description}</p>
          <div className="rounded-lg border p-3">
            <ExecutionCapabilities
              mode={plan.executionMode}
              capabilities={plan.capabilities}
            />
          </div>
          {plan.exchangeManifestDigest !== undefined && (
            <div className="space-y-2 rounded-lg border p-3">
              <ExchangeManifestView
                manifest={plan.exchangeManifest}
                digest={plan.exchangeManifestDigest}
              />
              <p className="break-all text-xs text-muted-foreground">
                {t("manifestBound")}{" "}
                <code dir="ltr">{plan.exchangeManifestDigest}</code>
              </p>
            </div>
          )}
          <details className="rounded-lg border p-3 text-xs">
            <summary className="cursor-pointer text-muted-foreground">
              {t("details")}
            </summary>
            <dl className="mt-2 space-y-2 break-all">
              <dt>{t("options")}</dt>
              <dd>
                <pre className="whitespace-pre-wrap">
                  {JSON.stringify(plan.options, null, 2)}
                </pre>
              </dd>
              <dt>{t("instructions")}</dt>
              <dd className="whitespace-pre-wrap">{plan.brief}</dd>
              <dt>{t("fingerprint")}</dt>
              <dd>{plan.planHash}</dd>
              <dt>{t("snapshot")}</dt>
              <dd>
                {plan.snapshotId} · {plan.snapshotVersion}
              </dd>
            </dl>
          </details>
        </div>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {!plan && !approved && (
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => void act("review")}
          >
            {t("review")}
          </Button>
        )}
        {plan && (
          <Button
            size="sm"
            disabled={busy || !manifestReady}
            onClick={() => void act("approve")}
          >
            {t(submission.current ? "retryApproval" : "approveQueue")}
          </Button>
        )}
        {task.status === "queued" && (
          <Button
            size="sm"
            variant="outline"
            disabled={busy || !!submission.current}
            onClick={() => void act("cancel")}
          >
            {t("cancelQueue")}
          </Button>
        )}
      </div>
    </section>
  )
}
