"use client"
import { useCallback, useEffect, useId, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  localInboxClient,
  type BranchInboxItem,
  type LocalInboxClient,
} from "@/lib/hall/branch-inbox"
import type { HallMember, HallMemberSnapshot } from "@/lib/hall/members"
import { toErrorMessage } from "@/lib/app-error"
import { Button } from "@/components/ui/button"
import { ExchangeManifestView } from "@/components/hall/tasks/exchange-manifest"
import { isExchangeManifest } from "@/lib/hall/exchange"
import { Label } from "@/components/ui/label"
import { selectClass } from "../tasks/task-editor"

export function LocalBranchInbox({
  client: provided,
  onAccepted,
}: {
  client?: LocalInboxClient | null
  onAccepted?: () => void
}) {
  const t = useTranslations("Hall.branchInbox")
  const [client] = useState(() =>
    provided === undefined ? localInboxClient() : provided
  )
  if (!client)
    return <p className="p-4 text-sm text-muted-foreground">{t("localOnly")}</p>
  return <InboxBody client={client} onAccepted={onAccepted} />
}
function InboxBody({
  client,
  onAccepted,
}: {
  client: LocalInboxClient
  onAccepted?: () => void
}) {
  const t = useTranslations("Hall.branchInbox")
  const [rows, setRows] = useState<BranchInboxItem[]>([])
  const [members, setMembers] = useState<HallMember[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const epoch = useRef(0)
  const load = useCallback(async () => {
    const generation = ++epoch.current
    setLoading(true)
    setError(null)
    try {
      const [items, people] = await Promise.all([
        client.list(),
        client.members(),
      ])
      if (generation === epoch.current) {
        setRows(items)
        setMembers(people.filter((m) => m.enabled))
      }
    } catch (e) {
      if (generation === epoch.current) setError(toErrorMessage(e))
    } finally {
      if (generation === epoch.current) setLoading(false)
    }
  }, [client])
  useEffect(() => {
    const generation = epoch
    void load()
    return () => {
      generation.current++
    }
  }, [load])
  return (
    <section className="h-full space-y-3 overflow-auto p-4">
      <p className="text-sm text-muted-foreground">{t("hint")}</p>
      <Button variant="outline" disabled={loading} onClick={() => void load()}>
        {t("refresh")}
      </Button>
      {error && <p role="alert">{error}</p>}
      {!loading && rows.length === 0 && <p>{t("empty")}</p>}
      {rows.map((item) => (
        <InboxCard
          key={`${item.issuerId}:${item.dispatchId}:${item.revision}`}
          item={item}
          members={members}
          client={client}
          onAccepted={onAccepted}
        />
      ))}
    </section>
  )
}
function InboxCard({
  item,
  members,
  client,
  onAccepted,
}: {
  item: BranchInboxItem
  members: HallMember[]
  client: LocalInboxClient
  onAccepted?: () => void
}) {
  const t = useTranslations("Hall.branchInbox")
  const id = useId()
  const [member, setMember] = useState("")
  const [snapshots, setSnapshots] = useState<HallMemberSnapshot[]>([])
  const [snapshot, setSnapshot] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [accepted, setAccepted] = useState(!!item.taskId)
  const [justAccepted, setJustAccepted] = useState(false)
  const [rejected, setRejected] = useState(item.status === "rejected")
  const epoch = useRef(0)
  const locked = useRef(false)
  const alive = useRef(true)
  useEffect(() => {
    const generation = epoch
    alive.current = true
    return () => {
      alive.current = false
      generation.current++
    }
  }, [])
  const selected = snapshots.find(
    (s) => s.snapshotId === snapshot && s.memberId === member
  )
  const manifestReady =
    item.exchangeManifest === undefined ||
    isExchangeManifest(item.exchangeManifest)
  const pending =
    item.status === "awaiting_confirmation" && !accepted && !rejected
  // The sender may name one of this machine's members; preselect it and its
  // newest snapshot. Accepting is still this person's click.
  const requested = members.find((m) => m.memberId === item.memberRef)
  const requestedId = pending ? (requested?.memberId ?? null) : null
  async function choose(value: string, pickNewest = false) {
    const generation = ++epoch.current
    setMember(value)
    setSnapshot("")
    setSnapshots([])
    setError(null)
    if (!value) return
    try {
      const result = await client.snapshots(value)
      if (generation !== epoch.current) return
      setSnapshots(result)
      if (pickNewest) {
        const newest = result
          .filter((s) => s.projectId !== undefined)
          .reduce<HallMemberSnapshot | null>(
            (best, s) => (!best || s.createdAt > best.createdAt ? s : best),
            null
          )
        if (newest) setSnapshot(newest.snapshotId)
      }
    } catch (e) {
      if (generation === epoch.current) setError(toErrorMessage(e))
    }
  }
  const preselected = useRef(false)
  useEffect(() => {
    if (!requestedId || preselected.current) return
    preselected.current = true
    void choose(requestedId, true)
    // choose is recreated each render; preselection runs once per card.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestedId])
  async function decide(accept: boolean) {
    if (
      locked.current ||
      !pending ||
      (accept &&
        (!manifestReady || !selected || selected.projectId === undefined))
    )
      return
    locked.current = true
    setBusy(true)
    setError(null)
    try {
      if (accept) {
        await client.accept({
          issuerId: item.issuerId,
          dispatchId: item.dispatchId,
          expectedRevision: item.revision,
          memberId: member,
          snapshotId: selected!.snapshotId,
          projectId: selected!.projectId!,
        })
        if (alive.current) {
          setAccepted(true)
          setJustAccepted(true)
        }
      } else {
        await client.reject(item)
        if (alive.current) setRejected(true)
      }
    } catch (e) {
      if (alive.current) setError(toErrorMessage(e))
    } finally {
      if (alive.current) {
        locked.current = false
        setBusy(false)
      }
    }
  }
  return (
    <article className="space-y-2 rounded-md border p-3">
      <p className="font-medium">{item.title}</p>
      <p className="whitespace-pre-wrap text-sm">{item.description}</p>
      {item.exchangeManifest !== undefined && (
        <div className="rounded-md border p-2">
          <ExchangeManifestView manifest={item.exchangeManifest} />
        </div>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {pending && (
        <>
          {requested && (
            <p className="text-sm text-muted-foreground">
              {t("requested", { name: requested.name })}
            </p>
          )}
          <Label htmlFor={`${id}-member`}>{t("member")}</Label>
          <select
            id={`${id}-member`}
            className={selectClass}
            value={member}
            disabled={busy}
            onChange={(e) => void choose(e.target.value)}
          >
            <option value="">{t("choose")}</option>
            {members.map((m) => (
              <option key={m.memberId} value={m.memberId}>
                {m.name}
              </option>
            ))}
          </select>
          <Label htmlFor={`${id}-snapshot`}>{t("snapshot")}</Label>
          <select
            id={`${id}-snapshot`}
            className={selectClass}
            value={snapshot}
            disabled={busy || !member}
            onChange={(e) => setSnapshot(e.target.value)}
          >
            <option value="">{t("choose")}</option>
            {snapshots.map((s) => (
              <option key={s.snapshotId} value={s.snapshotId}>
                {s.createdAt}
              </option>
            ))}
          </select>
          <div className="flex gap-2">
            <Button
              disabled={
                busy ||
                !manifestReady ||
                !selected ||
                selected.projectId === undefined
              }
              onClick={() => void decide(true)}
            >
              {t("accept")}
            </Button>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => void decide(false)}
            >
              {t("reject")}
            </Button>
          </div>
        </>
      )}
      {accepted && (
        <>
          <p role="status">
            {t(justAccepted ? "acceptedHint" : "linkedTaskHint")}
          </p>
          <Button variant="outline" onClick={onAccepted}>
            {t("openTasks")}
          </Button>
        </>
      )}
      {rejected && <p role="status">{t("rejected")}</p>}
    </article>
  )
}
