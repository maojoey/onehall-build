import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { webcrypto } from "node:crypto"
import { IDBFactory } from "fake-indexeddb"
import type { DispatchClient } from "@/lib/hall/dispatch"
vi.mock("next-intl", () => ({ useTranslations: () => (key: string) => key }))
import { BranchDispatchPanel } from "./dispatch-panel"
const row = {
  dispatchId: "saved",
  branchId: "win",
  memberRef: "worker",
  title: "Sum",
  status: "awaiting_acceptance",
  revision: 2,
  createdAt: "",
  updatedAt: "",
}
beforeEach(() => {
  localStorage.clear()
  vi.stubGlobal("crypto", webcrypto)
  vi.stubGlobal("indexedDB", new IDBFactory())
})
afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})
function setup(context = "hq:1", branchId = "win") {
  const c = {
    submissionContext: context,
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    sync: vi.fn().mockResolvedValue(row),
  }
  const view = render(
    <BranchDispatchPanel client={c as DispatchClient} branchId={branchId} />
  )
  return { c, ...view }
}
it("keeps one submission identity after a lost response and never claims execution from delivery", async () => {
  const { c } = setup()
  c.create
    .mockRejectedValueOnce(new Error("Connection lost"))
    .mockResolvedValueOnce(row)
  fireEvent.change(screen.getByLabelText("title"), { target: { value: "Sum" } })
  fireEvent.change(screen.getByLabelText("description"), {
    target: { value: "Add the input numbers" },
  })
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await screen.findByRole("alert")
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await waitFor(() => expect(c.create).toHaveBeenCalledTimes(2))
  expect(c.create.mock.calls[0]).toEqual(c.create.mock.calls[1])
  expect(c.create.mock.calls[0][0]).toMatchObject({
    branchId: "win",
    memberRef: "worker",
  })
  expect(
    await screen.findByText("status.awaiting_acceptance")
  ).toBeInTheDocument()
  expect(screen.queryByText("status.succeeded")).not.toBeInTheDocument()
})
it("does not display another branch's late receipt when switched", async () => {
  const { c, rerender } = setup()
  let finish!: (value: typeof row) => void
  c.create.mockReturnValue(
    new Promise((r) => {
      finish = r
    })
  )
  fireEvent.change(screen.getByLabelText("title"), { target: { value: "Sum" } })
  fireEvent.change(screen.getByLabelText("description"), {
    target: { value: "Add" },
  })
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await waitFor(() => expect(c.create).toHaveBeenCalledOnce())
  rerender(
    <BranchDispatchPanel client={c as DispatchClient} branchId="intel" />
  )
  await act(async () => finish(row))
  expect(
    screen.queryByText("status.awaiting_acceptance")
  ).not.toBeInTheDocument()
})

function fillDraft(description = "Add the input numbers") {
  fireEvent.change(screen.getByLabelText("title"), { target: { value: "Sum" } })
  fireEvent.change(screen.getByLabelText("description"), {
    target: { value: description },
  })
}
it("recovers the same submission after unmount and a fresh client, without storing the task body", async () => {
  const first = setup()
  first.c.create.mockRejectedValue(new Error("Connection lost"))
  fillDraft()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await screen.findByRole("alert")
  const submitted = first.c.create.mock.calls[0][0]
  expect(JSON.stringify(localStorage)).not.toContain("Add the input numbers")
  expect(JSON.stringify(localStorage)).not.toContain('"title"')
  first.unmount()
  const second = setup()
  second.c.create.mockResolvedValue(row)
  fillDraft()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await waitFor(() => expect(second.c.create).toHaveBeenCalledOnce())
  expect(second.c.create.mock.calls[0][0]).toEqual(submitted)
})
it("keeps retry identities separate for headquarters and branch contexts", async () => {
  const ids: string[] = []
  for (const [hq, branch] of [
    ["hq:1", "win"],
    ["hq:2", "win"],
    ["hq:1", "intel"],
  ]) {
    const view = setup(hq, branch)
    view.c.create.mockRejectedValue(new Error("Connection lost"))
    fillDraft()
    fireEvent.click(screen.getByRole("button", { name: "send" }))
    await screen.findByRole("alert")
    ids.push(view.c.create.mock.calls[0][0].submissionId)
    view.unmount()
  }
  expect(new Set(ids).size).toBe(3)
  const retry = setup("hq:1", "win")
  retry.c.create.mockResolvedValue(row)
  fillDraft()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await waitFor(() => expect(retry.c.create).toHaveBeenCalledOnce())
  expect(retry.c.create.mock.calls[0][0].submissionId).toBe(ids[0])
})
it("requires an explicit new task to replace an uncertain payload or intentionally send identical work", async () => {
  const { c } = setup()
  c.create.mockRejectedValue(new Error("Connection lost"))
  fillDraft()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await screen.findByRole("alert")
  const originalId = c.create.mock.calls[0][0].submissionId
  fillDraft("Changed instructions")
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  expect(await screen.findByText("retryMismatch")).toBeInTheDocument()
  expect(c.create).toHaveBeenCalledOnce()
  fireEvent.click(screen.getByRole("button", { name: "newTask" }))
  await waitFor(() => expect(screen.getByLabelText("title")).toHaveValue(""))
  fillDraft()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await waitFor(() => expect(c.create).toHaveBeenCalledTimes(2))
  expect(c.create.mock.calls[1][0].submissionId).not.toBe(originalId)
})
it("does not send when durable retry identity cannot be saved", async () => {
  const { c } = setup()
  c.create.mockRejectedValue(new Error("Connection lost"))
  vi.stubGlobal("indexedDB", undefined)
  fillDraft()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "retryStorageUnavailable"
  )
  expect(c.create).not.toHaveBeenCalled()
})

it("fails closed on corrupt legacy retry storage without dropping uncertain records", async () => {
  localStorage.setItem("hall.dispatch.submissions.v1", "invalid")
  const { c } = setup()
  fillDraft()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "retryStorageUnavailable"
  )
  expect(c.create).not.toHaveBeenCalled()
  expect(localStorage.getItem("hall.dispatch.submissions.v1")).toBe("invalid")
})
it("fails closed on full retry storage without dropping uncertain records", async () => {
  const records = Array.from({ length: 100 }, (_, i) => ({
    scope: JSON.stringify([`other:${i}`, "win"]),
    digest: "a".repeat(64),
    submissionId: crypto.randomUUID(),
  }))
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open("hall.dispatch.submissions.v2", 1)
    request.onupgradeneeded = () =>
      request.result.createObjectStore("submissions", { keyPath: "scope" })
    request.onsuccess = () => resolve(request.result)
    request.onerror = () => reject(request.error)
  })
  await new Promise<void>((resolve, reject) => {
    const transaction = db.transaction("submissions", "readwrite")
    for (const record of records)
      transaction.objectStore("submissions").add(record)
    transaction.oncomplete = () => resolve()
    transaction.onabort = () => reject(transaction.error)
  })
  const { c } = setup()
  fillDraft()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  expect(await screen.findByRole("alert")).toHaveTextContent("retryStorageFull")
  expect(c.create).not.toHaveBeenCalled()
  const retained = await new Promise<unknown[]>((resolve) => {
    const transaction = db.transaction("submissions", "readonly")
    const request = transaction.objectStore("submissions").getAll()
    transaction.oncomplete = () => resolve(request.result)
  })
  expect(retained).toHaveLength(100)
  expect(retained).toEqual(expect.arrayContaining(records))
  db.close()
})

function exchangeClient(upload?: unknown, pick?: unknown) {
  const exchange = {
    prepareInput: vi
      .fn()
      .mockResolvedValue({ objectId: "obj", transferId: "tr" }),
    confirmInput: vi.fn().mockResolvedValue({ objectId: "obj" }),
    prepareManifest: vi.fn().mockResolvedValue({ manifestId: "man" }),
    artifacts: vi.fn().mockResolvedValue([]),
  }
  const c = {
    submissionContext: "shell",
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn().mockResolvedValue(row),
    sync: vi.fn().mockResolvedValue(row),
    exchange,
    bytes: { upload, download: vi.fn(), ...(pick ? { pick } : {}) },
  }
  render(
    <BranchDispatchPanel
      client={c as unknown as DispatchClient}
      branchId="win"
    />
  )
  return { c, exchange }
}
function fill() {
  fireEvent.change(screen.getByLabelText("title"), { target: { value: "Sum" } })
  fireEvent.change(screen.getByLabelText("description"), {
    target: { value: "Add" },
  })
}

it("hides attachments when the backend has no file exchange", () => {
  setup()
  expect(screen.queryByText("attachments")).not.toBeInTheDocument()
})

it("explains attachments are unavailable where bytes cannot be sent", () => {
  exchangeClient(undefined)
  expect(screen.getByText("attachUnavailable")).toBeInTheDocument()
  expect(screen.queryByLabelText("attachInputs")).not.toBeInTheDocument()
})

it("stages files under the submission, then creates the dispatch with the frozen manifest", async () => {
  const upload = vi.fn().mockResolvedValue({})
  const { c, exchange } = exchangeClient(upload)
  fill()
  const input = screen.getByLabelText("attachInputs") as HTMLInputElement
  fireEvent.change(input, { target: { files: [new File(["abc"], "in.txt")] } })
  await screen.findByText("in.txt")
  fireEvent.change(screen.getByLabelText("outputSlots"), {
    target: { value: "out.txt" },
  })
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await waitFor(() => expect(c.create).toHaveBeenCalledOnce())
  const submissionId = exchange.prepareInput.mock.calls[0][0]
  expect(exchange.prepareInput).toHaveBeenCalledWith(
    submissionId,
    "in.txt",
    3,
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
  )
  expect(upload).toHaveBeenCalledWith(
    "tr",
    expect.objectContaining({ name: "in.txt", source: expect.any(File) })
  )
  expect(exchange.prepareManifest).toHaveBeenCalledWith(
    submissionId,
    ["obj"],
    ["out.txt"]
  )
  expect(c.create.mock.calls[0][0]).toMatchObject({
    submissionId,
    exchangeManifestId: "man",
  })
})

it("does not create a dispatch when an upload fails", async () => {
  const { c } = exchangeClient(vi.fn().mockRejectedValue(new Error("offline")))
  fill()
  fireEvent.change(screen.getByLabelText("attachInputs"), {
    target: { files: [new File(["abc"], "in.txt")] },
  })
  await screen.findByText("in.txt")
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await screen.findByText("offline")
  expect(c.create).not.toHaveBeenCalled()
})

it("desktop: the host picks and inspects files, and only its path is uploaded", async () => {
  const sha = "c".repeat(64)
  const pick = vi.fn().mockResolvedValue([
    {
      name: "報告.json",
      size: 42,
      sha256: sha,
      source: { path: "/r/報告.json" },
    },
  ])
  const upload = vi.fn().mockResolvedValue({})
  const { c, exchange } = exchangeClient(upload, pick)
  fill()
  // No page-side file input: the host dialog is the only way to choose.
  expect(screen.queryByLabelText("attachInputs")).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "chooseInputs" }))
  await screen.findByText("報告.json")
  expect(screen.getByText(sha.slice(0, 12))).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await waitFor(() => expect(c.create).toHaveBeenCalledOnce())
  const submissionId = exchange.prepareInput.mock.calls[0][0]
  expect(exchange.prepareInput).toHaveBeenCalledWith(
    submissionId,
    "報告.json",
    42,
    sha
  )
  expect(upload).toHaveBeenCalledWith(
    "tr",
    expect.objectContaining({ source: { path: "/r/報告.json" } })
  )
})

it("desktop: an inspection failure is shown and nothing is staged", async () => {
  const pick = vi.fn().mockRejectedValue(new Error("Choose a regular file"))
  const { exchange } = exchangeClient(vi.fn(), pick)
  fireEvent.click(screen.getByRole("button", { name: "chooseInputs" }))
  expect(await screen.findByText("Choose a regular file")).toBeInTheDocument()
  expect(exchange.prepareInput).not.toHaveBeenCalled()
})

it("sends the member the sender asked for", async () => {
  const c = {
    submissionContext: "hq:1",
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn().mockResolvedValue(row),
    sync: vi.fn().mockResolvedValue(row),
  }
  render(
    <BranchDispatchPanel
      client={c as DispatchClient}
      branchId="win"
      memberRef="m-reviewer"
    />
  )
  fireEvent.change(screen.getByLabelText("title"), { target: { value: "Sum" } })
  fireEvent.change(screen.getByLabelText("description"), {
    target: { value: "Add" },
  })
  fireEvent.click(screen.getByRole("button", { name: "send" }))
  await waitFor(() =>
    expect(c.create.mock.calls[0][0]).toMatchObject({ memberRef: "m-reviewer" })
  )
})
