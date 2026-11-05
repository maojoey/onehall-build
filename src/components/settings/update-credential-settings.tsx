"use client"

import { useEffect, useId, useState, useSyncExternalStore } from "react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  getUpdateCredentialStatus,
  importUpdateCredential,
  removeUpdateCredential,
  usesTauriUpdater,
} from "@/lib/updater"

const subscribeEnvironment = () => () => {}
const serverEnvironment = () => false

export function UpdateCredentialSettings() {
  const localDesktop = useSyncExternalStore(
    subscribeEnvironment,
    usesTauriUpdater,
    serverEnvironment
  )
  return localDesktop ? <LocalUpdateCredentialSettings /> : null
}

function LocalUpdateCredentialSettings() {
  const t = useTranslations("SystemSettings")
  const id = useId()
  const [configured, setConfigured] = useState<boolean | null>(null)
  const [path, setPath] = useState("")
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    let active = true
    getUpdateCredentialStatus()
      .then((result) => {
        if (active) setConfigured(result.configured)
      })
      .catch(() => {
        if (active) setFailed(true)
      })
    return () => {
      active = false
    }
  }, [])
  async function change(remove: boolean) {
    setBusy(true)
    setFailed(false)
    try {
      const result = await (remove
        ? removeUpdateCredential()
        : importUpdateCredential(path.trim()))
      setConfigured(result.configured)
      setPath("")
    } catch {
      setFailed(true)
    } finally {
      setBusy(false)
    }
  }
  return (
    <section className="space-y-3 rounded-lg border p-3">
      <h3 className="text-sm font-medium">{t("updateCredentialTitle")}</h3>
      <p className="text-xs text-muted-foreground">
        {t("updateCredentialDescription")}
      </p>
      <p role="status" className="text-xs">
        {configured === null
          ? t("updateCredentialLoading")
          : configured
            ? t("updateCredentialPresent")
            : t("updateCredentialAbsent")}
      </p>
      <label htmlFor={id} className="text-xs">
        {t("updateCredentialPath")}
      </label>
      <Input
        id={id}
        value={path}
        onChange={(event) => setPath(event.target.value)}
        autoComplete="off"
        disabled={busy}
      />
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={busy || !path.trim()}
          onClick={() => void change(false)}
        >
          {t("updateCredentialImport")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={busy || !configured}
          onClick={() => void change(true)}
        >
          {t("updateCredentialRemove")}
        </Button>
      </div>
      {failed && (
        <p role="alert" className="text-xs text-destructive">
          {t("updateCredentialError")}
        </p>
      )}
    </section>
  )
}
