"use client"

// onehall: "which server does the pet watch" — binds the pet windows to one of
// the saved remote workspace connections (or back to this computer). Stored
// in the local pet config; takes effect on the next summon.

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  closePetWindow,
  getPetSettings,
  openPetWindow,
  setPetRemoteBinding,
} from "@/lib/pet/api"
import { listRemoteWorkspaceConnections } from "@/lib/remote-workspace"
import { isDesktop } from "@/lib/transport"
import type { RemoteWorkspaceConnection } from "@/lib/types"

const LOCAL = "local"

export function PetRemoteBinding() {
  const t = useTranslations("Pet.manager")
  const [connections, setConnections] = useState<RemoteWorkspaceConnection[]>(
    []
  )
  const [value, setValue] = useState<string>(LOCAL)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!isDesktop()) return
    let cancelled = false
    void Promise.all([
      listRemoteWorkspaceConnections().catch(() => []),
      getPetSettings().catch(() => null),
    ]).then(([list, settings]) => {
      if (cancelled) return
      setConnections(list)
      const bound = settings?.remoteConnectionId
      setValue(
        bound !== null &&
          bound !== undefined &&
          list.some((c) => c.id === bound)
          ? String(bound)
          : LOCAL
      )
    })
    return () => {
      cancelled = true
    }
  }, [])

  const onChange = useCallback(
    async (next: string) => {
      const previous = value
      setValue(next)
      setSaving(true)
      try {
        const config = await setPetRemoteBinding(
          next === LOCAL ? null : Number(next)
        )
        // A pet that is already on screen was opened with the OLD binding
        // (it is baked into the window URL): close and re-summon so it
        // follows the new one right away.
        if (config.enabled && config.activePetId) {
          await closePetWindow().catch(() => {})
          await openPetWindow()
        }
      } catch (err) {
        setValue(previous)
        toast.error(err instanceof Error ? err.message : String(err))
      } finally {
        setSaving(false)
      }
    },
    [value]
  )

  if (!isDesktop()) return null

  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <span className="text-muted-foreground">{t("remoteBinding")}</span>
      <Select value={value} onValueChange={onChange} disabled={saving}>
        <SelectTrigger className="h-8 w-56" size="sm">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={LOCAL}>{t("remoteLocal")}</SelectItem>
          {connections.map((c) => (
            <SelectItem key={c.id} value={String(c.id)}>
              {c.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <span className="text-xs text-muted-foreground">
        {t("remoteBindingHint")}
      </span>
    </div>
  )
}
