"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { ArrowUpCircle, Loader2, RotateCcw } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  acpAdapterRollback,
  acpAdapterStatus,
  acpAdapterUpgrade,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import type { AdapterStatus, AgentType } from "@/lib/types"

const MANAGED_ADAPTER_AGENTS: ReadonlySet<AgentType> = new Set([
  "claude_code",
  "codex",
])

/** Claude Code and Codex run through an adapter that carries its own CLI. */
export function hasManagedAdapter(agentType: AgentType): boolean {
  return MANAGED_ADAPTER_AGENTS.has(agentType)
}

type Busy = "upgrade" | "rollback" | null

/**
 * Versions behind a Claude Code / Codex agent, with a safe upgrade: the new
 * adapter is installed beside the old one and started once before it takes
 * over, and the previous version stays available to roll back to.
 */
export function AcpAdapterRuntimeCard({ agentType }: { agentType: AgentType }) {
  const t = useTranslations("AcpAgentSettings.adapterRuntime")
  const [status, setStatus] = useState<AdapterStatus | null>(null)
  const [version, setVersion] = useState("")
  const [busy, setBusy] = useState<Busy>(null)

  const refresh = useCallback(async () => {
    try {
      setStatus(await acpAdapterStatus(agentType))
    } catch {
      setStatus(null)
    }
  }, [agentType])

  useEffect(() => {
    setStatus(null)
    void refresh()
  }, [refresh])

  const upgrade = async () => {
    const requested = version.trim() || null
    setBusy("upgrade")
    try {
      const outcome = await acpAdapterUpgrade(agentType, requested)
      toast.success(
        t(outcome.probe.needs_login ? "upgradedNeedsLogin" : "upgraded", {
          version: outcome.version,
        })
      )
      setVersion("")
    } catch (error) {
      toast.error(t("upgradeFailed", { message: toErrorMessage(error) }))
    } finally {
      setBusy(null)
      await refresh()
    }
  }

  const rollback = async () => {
    setBusy("rollback")
    try {
      const next = await acpAdapterRollback(agentType)
      setStatus(next)
      toast.success(t("rolledBack", { version: next.current ?? "" }))
    } catch (error) {
      toast.error(t("rollbackFailed", { message: toErrorMessage(error) }))
    } finally {
      setBusy(null)
    }
  }

  if (!status) return null
  const unknown = t("unknown")
  return (
    <div className="rounded-md border px-3 py-2 text-xs space-y-2">
      <div className="font-medium">{t("title")}</div>
      <div className="text-muted-foreground">
        {t("versions", {
          adapter: status.adapter_version ?? unknown,
          cli: status.cli_version ?? unknown,
        })}
      </div>
      <div className="text-2xs text-muted-foreground">
        {status.managed ? t("managed") : t("external")} {t("cliNote")}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Input
          value={version}
          onChange={(event) => setVersion(event.target.value)}
          placeholder={t("versionPlaceholder")}
          aria-label={t("versionLabel")}
          disabled={busy !== null}
          className="h-7 w-32 text-xs"
        />
        <Button
          type="button"
          size="xs"
          variant="outline"
          disabled={busy !== null}
          onClick={() => void upgrade()}
        >
          {busy === "upgrade" ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <ArrowUpCircle className="h-3.5 w-3.5" />
          )}
          {busy === "upgrade"
            ? t("upgrading")
            : version.trim()
              ? t("installVersion")
              : t("upgradeLatest")}
        </Button>
        {status.previous ? (
          <Button
            type="button"
            size="xs"
            variant="outline"
            disabled={busy !== null}
            onClick={() => void rollback()}
          >
            {busy === "rollback" ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <RotateCcw className="h-3.5 w-3.5" />
            )}
            {t("rollback", { version: status.previous })}
          </Button>
        ) : null}
      </div>
    </div>
  )
}
