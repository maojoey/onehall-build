"use client"

import { useCallback, useEffect, useState } from "react"
import { ArrowUpCircle, Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import {
  type AgentUpdateOffer,
  applyAgentUpdate,
  findAgentUpdates,
  getAutoApply,
  isCheckDue,
  markChecked,
  setAutoApply,
} from "@/lib/agent-update-check"

/**
 * Once a day, looks for a newer Claude Code / Codex adapter and shows a chip
 * when there is one. Updating goes through the tested upgrade, so a bad
 * release is refused and the old version stays active. "Apply automatically"
 * is off until the user turns it on.
 */
export function StatusBarAgentUpdates() {
  const t = useTranslations("AgentUpdates")
  const [offers, setOffers] = useState<AgentUpdateOffer[]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [auto, setAuto] = useState(false)

  const apply = useCallback(
    async (offer: AgentUpdateOffer, quiet = false) => {
      setBusy(offer.agentType)
      const result = await applyAgentUpdate(offer)
      setBusy(null)
      if (result.ok) {
        setOffers((held) => held.filter((o) => o.agentType !== offer.agentType))
        toast.success(t("updated", { version: offer.latest }))
      } else if (!quiet) {
        toast.error(t("failed", { message: result.message ?? "" }))
      }
    },
    [t]
  )

  useEffect(() => {
    const autoApply = getAutoApply()
    setAuto(autoApply)
    if (!isCheckDue()) return
    let cancelled = false
    void (async () => {
      const found = await findAgentUpdates()
      if (cancelled) return
      markChecked()
      setOffers(found)
      if (autoApply) for (const offer of found) await apply(offer, true)
    })()
    return () => {
      cancelled = true
    }
    // The daily check runs once per mount; `apply` only changes with locale.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  if (offers.length === 0) return null
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button className="flex items-center gap-1.5 text-primary hover:text-primary/80">
          <ArrowUpCircle className="h-3.5 w-3.5" />
          <span>{t("chip", { count: offers.length })}</span>
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="end" className="w-80 gap-3 p-3">
        <div className="text-xs font-medium">{t("title")}</div>
        {offers.map((offer) => (
          <div
            key={offer.agentType}
            className="flex items-center justify-between gap-2 text-xs"
          >
            <span className="font-mono">
              {offer.agentType} {offer.installed} → {offer.latest}
            </span>
            <Button
              size="xs"
              variant="outline"
              disabled={busy !== null}
              onClick={() => void apply(offer)}
            >
              {busy === offer.agentType ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : null}
              {t("update")}
            </Button>
          </div>
        ))}
        <p className="text-2xs text-muted-foreground">{t("safe")}</p>
        <label className="flex items-center gap-2 text-2xs">
          <input
            type="checkbox"
            checked={auto}
            onChange={(event) => {
              setAuto(event.target.checked)
              setAutoApply(event.target.checked)
            }}
          />
          {t("auto")}
        </label>
      </PopoverContent>
    </Popover>
  )
}
