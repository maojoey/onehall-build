"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"

interface Props {
  contextKey: string
  connectionId: string | null
  waiting: boolean
  resync: (contextKey: string) => Promise<boolean>
}

/** Initialization can stall after Connected, before session/selectors arrive.
 * This notice never restarts a process, even when a connect outcome is unknown. */
export function ConnectionRecoveryNotice({
  contextKey,
  connectionId,
  waiting,
  resync,
}: Props) {
  const t = useTranslations("Folder.statusBar.connection")
  const key = waiting ? `${contextKey}:${connectionId ?? "pending"}` : null
  const [elapsedKey, setElapsedKey] = useState<string | null>(null)
  const [busyKey, setBusyKey] = useState<string | null>(null)
  const [failedKey, setFailedKey] = useState<string | null>(null)
  useEffect(() => {
    if (!key) return
    const timer = setTimeout(() => setElapsedKey(key), 30_000)
    return () => {
      clearTimeout(timer)
      setElapsedKey(null)
    }
  }, [key])

  if (!key || key !== elapsedKey) return null
  const pending = busyKey === key
  const description = connectionId
    ? t("slowConnectionDescription")
    : t("unknownConnectionOutcome")
  const synchronize = async () => {
    if (pending) return
    setBusyKey(key)
    setFailedKey(null)
    try {
      if (!(await resync(contextKey))) setFailedKey(key)
    } catch {
      setFailedKey(key)
    } finally {
      setBusyKey(null)
    }
  }
  return (
    <span
      role="status"
      className="inline-flex items-center gap-1 text-xs text-amber-700 dark:text-amber-400"
      title={description}
    >
      <span className="max-w-64 truncate">
        {connectionId ? t("slowConnection") : description}
      </span>
      {connectionId && (
        <Button
          size="xs"
          variant="ghost"
          disabled={pending}
          onClick={() => void synchronize()}
        >
          {pending ? t("reconnecting") : t("resync")}
        </Button>
      )}
      {failedKey === key && <span role="alert">{t("resyncFailed")}</span>}
    </span>
  )
}
