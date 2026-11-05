"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { isDesktop } from "@/lib/transport"
import { toErrorMessage } from "@/lib/app-error"
import {
  getPetChatSettings,
  setPetChatShortcut,
  type PetChatSettings,
} from "@/lib/pet/chat"
import {
  spellStopShortcut,
  stopShortcutFromEvent,
  stopShortcutProblem,
  stopShortcutLabel,
} from "@/lib/computer/stop-shortcut"
import { usePlatform } from "@/hooks/use-platform"

export function PetChatShortcut() {
  const t = useTranslations("Pet.chat")
  const { isMac } = usePlatform()
  const [settings, setSettings] = useState<PetChatSettings | null>(null)
  const [shortcut, setShortcut] = useState("")
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  useEffect(() => {
    if (!isDesktop()) return
    let cancelled = false
    void getPetChatSettings()
      .then((value) => {
        if (!cancelled) {
          setSettings(value)
          setShortcut(value.shortcut)
        }
      })
      .catch((err) => {
        if (!cancelled) setError(toErrorMessage(err))
      })
    return () => {
      cancelled = true
    }
  }, [])
  if (!isDesktop()) return null

  const save = async () => {
    setSaving(true)
    setError(null)
    try {
      const value = await setPetChatShortcut(shortcut)
      setSettings(value)
      setShortcut(value.shortcut)
    } catch (err) {
      setError(toErrorMessage(err))
    } finally {
      setSaving(false)
    }
  }
  return (
    <div className="space-y-2 text-sm">
      <label htmlFor="pet-chat-shortcut">{t("shortcut")}</label>
      <div className="flex items-center gap-2">
        <Input
          id="pet-chat-shortcut"
          value={shortcut}
          disabled={saving || !settings}
          onKeyDown={(event) => {
            const parts = stopShortcutFromEvent(event.nativeEvent)
            if (parts && !stopShortcutProblem(parts, isMac)) {
              event.preventDefault()
              setShortcut(spellStopShortcut(parts))
            }
          }}
          onChange={(event) => setShortcut(event.target.value)}
          placeholder="Control+Shift+KeyH"
          className="max-w-xs"
        />
        <Button
          size="sm"
          disabled={saving || !settings}
          onClick={() => void save()}
        >
          {t("save")}
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">{t("shortcutHint")}</p>
      <p className="text-xs text-muted-foreground">
        {settings?.status.active
          ? t("active", {
              shortcut: stopShortcutLabel(settings.status.active, isMac),
            })
          : t("inactive")}
      </p>
      {error || settings?.status.detail ? (
        <p role="alert" className="text-xs text-destructive">
          {error ?? settings?.status.detail}
        </p>
      ) : null}
    </div>
  )
}
