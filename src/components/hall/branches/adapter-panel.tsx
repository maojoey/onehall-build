"use client"

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"

import { Button } from "@/components/ui/button"
import { toErrorMessage } from "@/lib/app-error"
import {
  adapterSyncClient,
  differsFrom,
  SYNCED_AGENTS,
} from "@/lib/hall/adapter-sync"
import type { AdapterStatus, AgentType } from "@/lib/types"

type Versions = Partial<Record<AgentType, AdapterStatus | { error: string }>>

async function readAll(connectionId: number | null): Promise<Versions> {
  const client = adapterSyncClient(connectionId)
  const out: Versions = {}
  await Promise.all(
    SYNCED_AGENTS.map(async (agent) => {
      try {
        out[agent] = await client.status(agent)
      } catch (error) {
        out[agent] = { error: toErrorMessage(error) }
      }
    })
  )
  return out
}

/**
 * Adapter versions on one branch next to headquarters, with a button that
 * installs headquarters' version on the branch. The branch tests the new
 * adapter before it switches, so a bad build leaves it on the old version.
 */
export function BranchAdapterPanel({
  headquartersConnectionId,
  connectBranch,
}: {
  headquartersConnectionId: number | null
  /** Resolves the connection id that reaches the branch. */
  connectBranch: () => Promise<number>
}) {
  const t = useTranslations("Hall.branches.adapters")
  // The parent builds this inline; reading it through a ref keeps the load
  // from rerunning on every render.
  const connect = useRef(connectBranch)
  useLayoutEffect(() => {
    connect.current = connectBranch
  })
  const [hq, setHq] = useState<Versions>({})
  const [branch, setBranch] = useState<Versions>({})
  const [branchConnection, setBranchConnection] = useState<number | null>(null)
  const [busy, setBusy] = useState<AgentType | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)

  const load = useCallback(async () => {
    setFailure(null)
    try {
      const id = await connect.current()
      setBranchConnection(id)
      const [reference, actual] = await Promise.all([
        readAll(headquartersConnectionId),
        readAll(id),
      ])
      setHq(reference)
      setBranch(actual)
    } catch (error) {
      setFailure(toErrorMessage(error))
    }
  }, [headquartersConnectionId])

  useEffect(() => {
    void load()
  }, [load])

  const rows = useMemo(
    () =>
      SYNCED_AGENTS.map((agent) => {
        const reference = hq[agent]
        const actual = branch[agent]
        return {
          agent,
          hqVersion:
            reference && "adapter_version" in reference
              ? reference.adapter_version
              : null,
          branchVersion:
            actual && "adapter_version" in actual
              ? actual.adapter_version
              : null,
          branchError: actual && "error" in actual ? actual.error : null,
        }
      }),
    [hq, branch]
  )

  async function match(agent: AgentType, version: string) {
    if (branchConnection === null) return
    setBusy(agent)
    setMessage(null)
    setFailure(null)
    try {
      await adapterSyncClient(branchConnection).upgrade(agent, version)
      setMessage(t("matched", { version }))
      setBranch(await readAll(branchConnection))
    } catch (error) {
      setFailure(t("failed", { message: toErrorMessage(error) }))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="space-y-2 text-xs">
      <div className="grid grid-cols-[1fr_auto_auto_auto] items-center gap-x-3 gap-y-1">
        <span className="font-medium">{t("agent")}</span>
        <span className="font-medium">{t("headquarters")}</span>
        <span className="font-medium">{t("branch")}</span>
        <span />
        {rows.map((row) => {
          const differs = differsFrom(row.hqVersion, row.branchVersion)
          return (
            <div key={row.agent} className="contents">
              <span className="font-mono">{row.agent}</span>
              <span className="font-mono">{row.hqVersion ?? "—"}</span>
              <span
                className={`font-mono ${differs ? "text-amber-500" : ""}`}
                title={row.branchError ?? undefined}
              >
                {row.branchError
                  ? t("unavailable")
                  : (row.branchVersion ?? "—")}
              </span>
              {differs && row.hqVersion ? (
                <Button
                  size="xs"
                  variant="outline"
                  disabled={busy !== null}
                  onClick={() => void match(row.agent, row.hqVersion!)}
                >
                  {busy === row.agent ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : null}
                  {t("match")}
                </Button>
              ) : (
                <span />
              )}
            </div>
          )
        })}
      </div>
      <p className="text-2xs text-muted-foreground">{t("safe")}</p>
      {message && <p role="status">{message}</p>}
      {failure && (
        <p role="alert" className="text-destructive">
          {failure}
        </p>
      )}
    </div>
  )
}
