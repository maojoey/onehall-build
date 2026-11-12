"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { BranchDispatchPanel } from "@/components/hall/branches/dispatch-panel"
import { BranchAdapterPanel } from "@/components/hall/branches/adapter-panel"
import { BranchTargetInfo } from "@/components/hall/branches/target-info"
import { dispatchClient } from "@/lib/hall/dispatch"
import { MonitorCloud, RefreshCw } from "lucide-react"
import { branchClient, type HallBranchSummary } from "@/lib/hall/branches"
import { toErrorMessage } from "@/lib/app-error"
import { openRemoteWorkspace } from "@/lib/remote-workspace"
import { useHallWorkspaceStore } from "@/stores/hall-workspace-store"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

export function HallBranchDirectory({
  headquartersConnectionId,
  web,
  multiWindow = false,
}: {
  headquartersConnectionId: number | null
  web: boolean
  /** One window per machine (single-window off): a branch opens as its own
   *  window instead of a tab, and the trigger sits in the status bar. */
  multiWindow?: boolean
}) {
  const t = useTranslations("Hall.branches")
  const client = useMemo(
    () => branchClient(headquartersConnectionId),
    [headquartersConnectionId]
  )
  const dispatch = useMemo(
    () => dispatchClient(headquartersConnectionId),
    [headquartersConnectionId]
  )
  const [dispatchBranch, setDispatchBranch] =
    useState<HallBranchSummary | null>(null)
  const [adapterBranch, setAdapterBranch] = useState<HallBranchSummary | null>(
    null
  )
  const [open, setOpen] = useState(false)
  const [rows, setRows] = useState<HallBranchSummary[]>([])
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [name, setName] = useState("")
  const [baseUrl, setBaseUrl] = useState("")
  const [token, setToken] = useState("")
  const epoch = useRef(0)
  const active = useHallWorkspaceStore((s) =>
    s.tabs.find((tab) => tab.key === s.activeKey)
  )

  const refresh = useCallback(async () => {
    const generation = ++epoch.current
    setLoading(true)
    setError(null)
    try {
      const result = await client.list()
      if (generation !== epoch.current) return
      setRows(result)
      setLoading(false)
      await Promise.all(
        result
          .filter((row) => row.enabled)
          .map(async (row) => {
            try {
              const checked = await client.probe(row.id)
              if (generation !== epoch.current) return
              setRows((current) =>
                current.map((r) =>
                  r.id === checked.id && r.revision === checked.revision
                    ? checked
                    : r
                )
              )
            } catch {
              // A failed probe never removes a known branch or means it is off.
              if (generation === epoch.current)
                setRows((current) =>
                  current.map((r) =>
                    r.id === row.id && r.revision === row.revision
                      ? { ...r, status: "unreachable" }
                      : r
                  )
                )
            }
          })
      )
    } catch (failure) {
      if (generation === epoch.current) setError(toErrorMessage(failure))
    } finally {
      if (generation === epoch.current) setLoading(false)
    }
  }, [client])

  useEffect(() => {
    setRows([])
    setDispatchBranch(null)
    setAdapterBranch(null)
    setBusy(false)
    setToken("")
    if (open) void refresh()
    return () => {
      epoch.current += 1
    }
  }, [open, refresh, headquartersConnectionId])

  async function enter(row: HallBranchSummary) {
    const generation = epoch.current
    setBusy(true)
    setError(null)
    try {
      if (web) useHallWorkspaceStore.getState().openWebBranch(row.id, row.name)
      else if (multiWindow) {
        const connection = await client.connect(row.id)
        if (generation !== epoch.current) return
        await openRemoteWorkspace(connection.connectionId)
      } else {
        const connection = await client.connect(row.id)
        if (generation !== epoch.current) return
        useHallWorkspaceStore
          .getState()
          .openDerivedBranch(
            headquartersConnectionId!,
            connection.connectionId,
            row.id,
            connection.name
          )
      }
      setOpen(false)
    } catch (failure) {
      if (generation === epoch.current) setError(toErrorMessage(failure))
    } finally {
      if (generation === epoch.current) setBusy(false)
    }
  }

  async function toggle(row: HallBranchSummary) {
    const generation = epoch.current
    setBusy(true)
    try {
      const updated = await client.setEnabled(row, !row.enabled)
      if (generation === epoch.current) {
        setRows((current) =>
          current.map((r) => (r.id === updated.id ? updated : r))
        )
        if (!updated.enabled && dispatchBranch?.id === updated.id)
          setDispatchBranch(null)
      }
    } catch (failure) {
      if (generation === epoch.current) setError(toErrorMessage(failure))
    } finally {
      if (generation === epoch.current) setBusy(false)
    }
  }

  if (!web && headquartersConnectionId === null) return null
  return (
    <>
      <button
        type="button"
        className={
          multiWindow
            ? "flex items-center gap-1 rounded-md px-1.5 py-0.5 hover:bg-accent hover:text-foreground"
            : "my-1 ml-auto flex items-center gap-1 rounded-md bg-primary px-2.5 font-medium text-primary-foreground shadow-sm hover:bg-primary/90"
        }
        onClick={() => setOpen(true)}
      >
        <MonitorCloud className="h-3.5 w-3.5" aria-hidden />
        {t("directory")}
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{t("directory")}</DialogTitle>
            <DialogDescription>{t("description")}</DialogDescription>
          </DialogHeader>
          <div className="flex justify-end">
            <Button
              variant="outline"
              size="sm"
              disabled={loading || busy}
              onClick={() => void refresh()}
            >
              <RefreshCw />
              {t("refresh")}
            </Button>
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {t("loadFailed")} {error}
            </p>
          )}
          {loading && <p role="status">{t("loading")}</p>}
          {!loading && rows.length === 0 && !error && (
            <p className="text-sm text-muted-foreground">{t("empty")}</p>
          )}
          <div className="space-y-2">
            {rows.map((row) => (
              <div
                key={row.id}
                className="flex items-center justify-between gap-2 rounded-md border p-2"
              >
                <button
                  className="min-w-0 flex-1 text-left disabled:opacity-50"
                  type="button"
                  disabled={!row.enabled || busy}
                  aria-current={
                    active?.branchId === row.id ? "page" : undefined
                  }
                  onClick={() => void enter(row)}
                >
                  <span className="block truncate font-medium">{row.name}</span>
                  <span className="text-xs text-muted-foreground">
                    {t(row.enabled ? `status.${row.status}` : "disabled")}
                  </span>
                </button>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy || !row.enabled}
                  onClick={() => setDispatchBranch(row)}
                >
                  {t("dispatch")}
                </Button>
                {!web && (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy || !row.enabled}
                    onClick={() => setAdapterBranch(row)}
                  >
                    {t("adapters.open")}
                  </Button>
                )}
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  onClick={() => void toggle(row)}
                >
                  {t(row.enabled ? "disable" : "enable")}
                </Button>
              </div>
            ))}
          </div>
          {dispatchBranch && (
            <div className="space-y-2 border-t pt-3">
              <p className="font-medium">{dispatchBranch.name}</p>
              {!web && (
                <BranchTargetInfo
                  key={`target:${headquartersConnectionId}:${dispatchBranch.id}`}
                  connectBranch={async () =>
                    (await client.connect(dispatchBranch.id)).connectionId
                  }
                />
              )}
              <BranchDispatchPanel
                key={`${headquartersConnectionId ?? "web"}:${dispatchBranch.id}`}
                client={dispatch}
                branchId={dispatchBranch.id}
              />
            </div>
          )}
          {adapterBranch && !web && (
            <div className="space-y-2 border-t pt-3">
              <p className="font-medium">{adapterBranch.name}</p>
              <BranchAdapterPanel
                key={`${headquartersConnectionId}:${adapterBranch.id}`}
                headquartersConnectionId={headquartersConnectionId}
                connectBranch={async () =>
                  (await client.connect(adapterBranch.id)).connectionId
                }
              />
            </div>
          )}
          <details className="mt-2 border-t pt-3">
            <summary className="cursor-pointer text-sm">
              {t("register")}
            </summary>
            <form
              className="mt-3 space-y-3"
              onSubmit={(event) => {
                event.preventDefault()
                const generation = epoch.current
                const input = {
                  name: name.trim(),
                  baseUrl: baseUrl.trim(),
                  token,
                }
                setToken("")
                setBusy(true)
                setError(null)
                void client
                  .register(input)
                  .then(() => {
                    if (generation !== epoch.current) return
                    setName("")
                    setBaseUrl("")
                    setBusy(false)
                    void refresh()
                  })
                  .catch((failure) => {
                    if (generation === epoch.current)
                      setError(toErrorMessage(failure))
                  })
                  .finally(() => {
                    if (generation === epoch.current) setBusy(false)
                  })
              }}
            >
              <Label htmlFor="hall-branch-name">{t("name")}</Label>
              <Input
                id="hall-branch-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                maxLength={120}
              />
              <Label htmlFor="hall-branch-address">{t("address")}</Label>
              <Input
                id="hall-branch-address"
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                required
                type="url"
              />
              <Label htmlFor="hall-branch-token">{t("credential")}</Label>
              <Input
                id="hall-branch-token"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                required
                type="password"
                autoComplete="off"
              />
              <p className="text-xs text-muted-foreground">
                {t("credentialHint")}
              </p>
              <Button type="submit" disabled={busy}>
                {t("register")}
              </Button>
            </form>
          </details>
        </DialogContent>
      </Dialog>
    </>
  )
}
