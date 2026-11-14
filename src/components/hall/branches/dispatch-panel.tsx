"use client"
import { useCallback, useEffect, useId, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import type { BranchDispatch, DispatchClient } from "@/lib/hall/dispatch"
import {
  DispatchSubmissionError,
  newDispatchSubmission,
  reserveDispatchSubmission,
} from "@/lib/hall/dispatch-submission"
import {
  attachmentKey,
  checkAttachments,
  pickBrowserFile,
  stageAttachments,
  type PickedFile,
} from "@/lib/hall/dispatch-attachments"
import { DispatchTaskArtifacts } from "@/components/hall/tasks/task-artifacts"
import { toErrorMessage } from "@/lib/app-error"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"

export function BranchDispatchPanel({
  client,
  branchId,
  memberRef = "worker",
}: {
  client: DispatchClient
  branchId: string
  /** The branch member asked for, or "worker" to let the branch choose. */
  memberRef?: string
}) {
  const t = useTranslations("Hall.dispatch")
  const id = useId()
  const [title, setTitle] = useState("")
  const [description, setDescription] = useState("")
  const [rows, setRows] = useState<BranchDispatch[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [picked, setPicked] = useState<PickedFile[]>([])
  const [outputText, setOutputText] = useState("")
  const [hashing, setHashing] = useState(false)
  const epoch = useRef(0)
  const locked = useRef(false)
  const listing = useRef(0)
  const submissionContext = client.submissionContext ?? "shell"
  const load = useCallback(async () => {
    const generation = epoch.current
    const read = ++listing.current
    try {
      const all = await client.list()
      if (generation === epoch.current && read === listing.current)
        setRows(all.filter((r) => r.branchId === branchId))
    } catch (failure) {
      if (generation === epoch.current && read === listing.current)
        setError(toErrorMessage(failure))
    }
  }, [client, branchId])
  useEffect(() => {
    const generation = epoch
    generation.current++
    locked.current = false
    setTitle("")
    setDescription("")
    setPicked([])
    setOutputText("")
    setHashing(false)
    setRows([])
    setBusy(false)
    setError(null)
    void load()
    return () => {
      generation.current++
    }
  }, [load])
  async function run(action: () => Promise<BranchDispatch | null>) {
    if (locked.current) return
    locked.current = true
    listing.current++
    const generation = epoch.current
    setBusy(true)
    setError(null)
    try {
      const row = await action()
      if (row && generation === epoch.current)
        setRows((current) => [
          row,
          ...current.filter((r) => r.dispatchId !== row.dispatchId),
        ])
    } catch (failure) {
      if (generation === epoch.current)
        setError(
          failure instanceof DispatchSubmissionError
            ? t(failure.code)
            : toErrorMessage(failure)
        )
    } finally {
      if (generation === epoch.current) {
        locked.current = false
        setBusy(false)
      }
    }
  }
  const outputs = [
    ...new Set(
      outputText
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean)
    ),
  ]
  const canAttach = !!client.exchange && !!client.bytes?.upload
  const problem =
    picked.length || outputs.length ? checkAttachments(picked, outputs) : null
  const pickOnHost = client.bytes?.pick
  /** Browser: hash the chosen bytes here. Desktop: the host inspects paths. */
  async function choose(list: FileList | null) {
    if (!list?.length) return
    const files = Array.from(list)
    await add(async () => {
      const next: PickedFile[] = []
      for (const file of files) next.push(await pickBrowserFile(file))
      return next
    })
  }
  async function add(select: () => Promise<PickedFile[]>) {
    const generation = epoch.current
    setHashing(true)
    setError(null)
    try {
      const next = await select()
      if (generation === epoch.current && next.length)
        setPicked((current) => [...current, ...next])
    } catch (failure) {
      if (generation === epoch.current) setError(toErrorMessage(failure))
    } finally {
      if (generation === epoch.current) setHashing(false)
    }
  }
  async function startNewTask() {
    if (locked.current) return
    locked.current = true
    const generation = epoch.current
    setBusy(true)
    setError(null)
    try {
      await newDispatchSubmission(submissionContext, branchId)
      if (generation === epoch.current) {
        setTitle("")
        setDescription("")
        setPicked([])
        setOutputText("")
      }
    } catch {
      if (generation === epoch.current) setError(t("retryStorageUnavailable"))
    } finally {
      if (generation === epoch.current) {
        locked.current = false
        setBusy(false)
      }
    }
  }
  return (
    <section className="space-y-3">
      <p className="text-sm text-muted-foreground">{t("hint")}</p>
      <p className="text-sm text-muted-foreground">{t("retryHint")}</p>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      <form
        className="space-y-2"
        onSubmit={(event) => {
          event.preventDefault()
          if (
            !title.trim() ||
            !description.trim() ||
            locked.current ||
            hashing ||
            problem
          )
            return
          const draft = {
            branchId,
            memberRef,
            title: title.trim(),
            description,
          }
          const generation = epoch.current
          void run(async () => {
            const attach = picked.length > 0 || outputs.length > 0
            const submissionId = await reserveDispatchSubmission(
              submissionContext,
              draft,
              attach ? attachmentKey(picked, outputs) : undefined
            )
            if (generation !== epoch.current) return null
            const exchangeManifestId = attach
              ? await stageAttachments(client, submissionId, picked, outputs)
              : undefined
            if (generation !== epoch.current) return null
            const row = await client.create({
              ...draft,
              submissionId,
              ...(exchangeManifestId ? { exchangeManifestId } : {}),
            })
            if (generation === epoch.current)
              setRows((current) => [
                row,
                ...current.filter((r) => r.dispatchId !== row.dispatchId),
              ])
            // Once durably saved, deliver/query only this identity. A lost
            // delivery response does not turn the next click into another task.
            return client.sync(row.dispatchId)
          })
        }}
      >
        <Label htmlFor={`${id}-title`}>{t("title")}</Label>
        <Input
          id={`${id}-title`}
          value={title}
          maxLength={160}
          required
          disabled={busy}
          onChange={(e) => setTitle(e.target.value)}
        />
        <Label htmlFor={`${id}-description`}>{t("description")}</Label>
        <Textarea
          id={`${id}-description`}
          value={description}
          maxLength={8000}
          required
          disabled={busy}
          onChange={(e) => setDescription(e.target.value)}
        />
        {client.exchange && (
          <fieldset className="space-y-2 rounded-md border p-2">
            <legend className="px-1 text-sm">{t("attachments")}</legend>
            {canAttach ? (
              <>
                {pickOnHost ? (
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-sm">{t("attachInputs")}</span>
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={busy || hashing}
                      onClick={() => void add(pickOnHost)}
                    >
                      {t("chooseInputs")}
                    </Button>
                  </div>
                ) : (
                  <>
                    <Label htmlFor={`${id}-files`}>{t("attachInputs")}</Label>
                    <input
                      id={`${id}-files`}
                      type="file"
                      multiple
                      disabled={busy || hashing}
                      onChange={(e) => {
                        void choose(e.target.files)
                        e.target.value = ""
                      }}
                    />
                  </>
                )}
                {hashing && (
                  <p role="status" className="text-xs text-muted-foreground">
                    {t("hashing")}
                  </p>
                )}
                {picked.length > 0 && (
                  <ul className="space-y-1 text-sm">
                    {picked.map((p, i) => (
                      <li
                        key={`${p.name}:${p.sha256}`}
                        className="flex flex-wrap items-center gap-2"
                      >
                        <span className="break-all">{p.name}</span>
                        <span className="text-xs text-muted-foreground">
                          {t("fileSize", { bytes: p.size })}
                        </span>
                        <code
                          dir="ltr"
                          className="text-xs text-muted-foreground"
                          title={p.sha256}
                        >
                          {p.sha256.slice(0, 12)}
                        </code>
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          disabled={busy}
                          onClick={() =>
                            setPicked((c) => c.filter((_, j) => j !== i))
                          }
                        >
                          {t("removeFile")}
                        </Button>
                      </li>
                    ))}
                  </ul>
                )}
                <Label htmlFor={`${id}-outputs`}>{t("outputSlots")}</Label>
                <Textarea
                  id={`${id}-outputs`}
                  rows={2}
                  value={outputText}
                  disabled={busy}
                  onChange={(e) => setOutputText(e.target.value)}
                />
                <p className="text-xs text-muted-foreground">
                  {t("manifestNote")}
                </p>
                {problem && (
                  <p role="alert" className="text-sm text-destructive">
                    {t(`problem.${problem}`)}
                  </p>
                )}
              </>
            ) : (
              <p className="text-sm text-muted-foreground">
                {t("attachUnavailable")}
              </p>
            )}
          </fieldset>
        )}
        <Button
          type="submit"
          disabled={
            busy || hashing || !!problem || !title.trim() || !description.trim()
          }
        >
          {t("send")}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={busy}
          onClick={() => void load()}
        >
          {t("refresh")}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={busy}
          onClick={() => void startNewTask()}
        >
          {t("newTask")}
        </Button>
      </form>
      <div className="space-y-2">
        {rows.map((row) => (
          <div key={row.dispatchId} className="space-y-1 rounded-md border p-2">
            <p className="font-medium">{row.title}</p>
            <p role="status" className="text-sm">
              {t(`status.${row.status}`)}
            </p>
            {row.syncError && (
              <p className="text-sm text-destructive">{t("syncFailed")}</p>
            )}
            {row.resultText && (
              <pre className="max-h-60 overflow-auto whitespace-pre-wrap text-sm">
                {row.resultText}
              </pre>
            )}
            {client.exchange && row.taskStatus && (
              <DispatchTaskArtifacts
                exchange={client.exchange}
                dispatchId={row.dispatchId}
                taskStatus={row.taskStatus}
                exitConfirmedAt={row.exitConfirmedAt}
                reported={
                  row.reported ?? (row.resultText != null ? true : undefined)
                }
                refreshKey={`${row.revision}:${row.updatedAt}`}
                download={
                  client.bytes ? (a) => client.bytes!.download(a) : undefined
                }
              />
            )}
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void run(() => client.sync(row.dispatchId))}
            >
              {t("sync")}
            </Button>
          </div>
        ))}
      </div>
    </section>
  )
}
