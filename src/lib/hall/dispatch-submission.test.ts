import { webcrypto } from "node:crypto"
import { IDBFactory, IDBObjectStore as FakeObjectStore } from "fake-indexeddb"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  newDispatchSubmission,
  reserveDispatchSubmission,
} from "./dispatch-submission"

const databaseName = "hall.dispatch.submissions.v2"
const draft = {
  branchId: "win",
  memberRef: "worker",
  title: " Sum ",
  description: "Add the input numbers",
}

beforeEach(() => {
  localStorage.clear()
  vi.stubGlobal("indexedDB", new IDBFactory())
  vi.stubGlobal("crypto", webcrypto)
})
afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

async function database() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(databaseName, 1)
    request.onupgradeneeded = () =>
      request.result.createObjectStore("submissions", { keyPath: "scope" })
    request.onsuccess = () => resolve(request.result)
    request.onerror = () => reject(request.error)
  })
}
async function records() {
  const db = await database()
  try {
    return await new Promise<unknown[]>((resolve, reject) => {
      const transaction = db.transaction("submissions", "readonly")
      const request = transaction.objectStore("submissions").getAll()
      transaction.oncomplete = () => resolve(request.result)
      transaction.onabort = () => reject(transaction.error)
    })
  } finally {
    db.close()
  }
}

it("uses strong HTTP-available randomness and SHA256 without secure-context crypto methods", async () => {
  vi.stubGlobal("crypto", {
    getRandomValues: webcrypto.getRandomValues.bind(webcrypto),
  })
  const id = await reserveDispatchSubmission("hq:1", draft)
  expect(id).toMatch(
    /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/
  )
  expect(
    await reserveDispatchSubmission("hq:1", { ...draft, title: "Sum" })
  ).toBe(id)
  expect(await records()).toEqual([
    {
      scope: JSON.stringify(["hq:1", "win"]),
      submissionId: id,
      digest:
        "fefc7b0286f6e3a54893cfd27a21d24856a2cf5af27a0fa35c197e3bbcef5a10",
    },
  ])
  const stored = JSON.stringify(await records())
  expect(stored).toContain(id)
  expect(stored).not.toContain(draft.description)
  expect(stored).not.toContain('"title"')
})

it("atomically reserves one durable identity across independent tab clients", async () => {
  vi.resetModules()
  const tabA = await import("./dispatch-submission")
  vi.resetModules()
  const tabB = await import("./dispatch-submission")
  const ids = await Promise.all(
    Array.from({ length: 12 }, (_, i) =>
      (i % 2 ? tabA : tabB).reserveDispatchSubmission("hq:1", draft)
    )
  )
  expect(new Set(ids).size).toBe(1)
  expect(await records()).toEqual([
    {
      scope: JSON.stringify(["hq:1", "win"]),
      submissionId: ids[0],
      digest: expect.stringMatching(/^[a-f0-9]{64}$/),
    },
  ])
})

it("keeps every durable record when independent contexts reserve concurrently", async () => {
  const ids = await Promise.all(
    Array.from({ length: 12 }, (_, i) =>
      reserveDispatchSubmission(`hq:${i}`, draft)
    )
  )
  expect(new Set(ids).size).toBe(12)
  expect(await records()).toHaveLength(12)
  await Promise.all(
    ids.map(async (id, i) => {
      expect(await reserveDispatchSubmission(`hq:${i}`, draft)).toBe(id)
    })
  )
})

it("fails closed when the durable store cannot be opened", async () => {
  const abort = vi
    .spyOn(IDBFactory.prototype, "open")
    .mockImplementation(() => {
      throw new Error("Storage denied")
    })
  await expect(reserveDispatchSubmission("hq:1", draft)).rejects.toMatchObject({
    code: "retryStorageUnavailable",
  })
  abort.mockRestore()
})

it("preserves unreleased legacy retry records and fails closed", async () => {
  const legacy = JSON.stringify([
    { scope: "existing", submissionId: "old", digest: "opaque" },
  ])
  localStorage.setItem("hall.dispatch.submissions.v1", legacy)
  await expect(reserveDispatchSubmission("hq:1", draft)).rejects.toMatchObject({
    code: "retryStorageUnavailable",
  })
  expect(localStorage.getItem("hall.dispatch.submissions.v1")).toBe(legacy)
})

it("changes a durable identity only through an explicit new-task action", async () => {
  const old = await reserveDispatchSubmission("hq:1", draft)
  await expect(
    reserveDispatchSubmission("hq:1", { ...draft, description: "Different" })
  ).rejects.toMatchObject({ code: "retryMismatch" })
  await newDispatchSubmission("hq:1", "win")
  const next = await reserveDispatchSubmission("hq:1", draft)
  expect(next).not.toBe(old)
})

it("rejects an aborted write even after the insert request succeeds", async () => {
  const add = FakeObjectStore.prototype.add
  vi.spyOn(FakeObjectStore.prototype, "add").mockImplementation(function (
    this: IDBObjectStore,
    ...args: Parameters<IDBObjectStore["add"]>
  ) {
    const request = add.apply(this, args)
    request.addEventListener("success", () => this.transaction.abort())
    return request
  })
  await expect(reserveDispatchSubmission("hq:1", draft)).rejects.toMatchObject({
    code: "retryStorageUnavailable",
  })
  expect(await records()).toEqual([])
})

it("keeps the old identity if explicit New task cannot commit", async () => {
  const old = await reserveDispatchSubmission("hq:1", draft)
  const remove = FakeObjectStore.prototype.delete
  const failure = vi
    .spyOn(FakeObjectStore.prototype, "delete")
    .mockImplementation(function (
      this: IDBObjectStore,
      ...args: Parameters<IDBObjectStore["delete"]>
    ) {
      const request = remove.apply(this, args)
      request.addEventListener("success", () => this.transaction.abort())
      return request
    })
  await expect(newDispatchSubmission("hq:1", "win")).rejects.toMatchObject({
    code: "retryStorageUnavailable",
  })
  failure.mockRestore()
  expect(await reserveDispatchSubmission("hq:1", draft)).toBe(old)
})

it("serializes capacity checks without eviction when tabs reserve the last available slot", async () => {
  const db = await database()
  const existing = Array.from({ length: 99 }, (_, i) => ({
    scope: JSON.stringify([`existing:${i}`, "win"]),
    digest: "a".repeat(64),
    submissionId: crypto.randomUUID(),
  }))
  await new Promise<void>((resolve, reject) => {
    const transaction = db.transaction("submissions", "readwrite")
    for (const record of existing)
      transaction.objectStore("submissions").add(record)
    transaction.oncomplete = () => resolve()
    transaction.onabort = () => reject(transaction.error)
  })
  db.close()
  const results = await Promise.allSettled([
    reserveDispatchSubmission("last:1", draft),
    reserveDispatchSubmission("last:2", draft),
  ])
  expect(
    results.filter((result) => result.status === "fulfilled")
  ).toHaveLength(1)
  expect(results.filter((result) => result.status === "rejected")).toEqual([
    {
      status: "rejected",
      reason: expect.objectContaining({ code: "retryStorageFull" }),
    },
  ])
  const retained = await records()
  expect(retained).toHaveLength(100)
  expect(retained).toEqual(expect.arrayContaining(existing))
})

it("fails closed when strong random generation is unavailable", async () => {
  vi.stubGlobal("crypto", {})
  await expect(reserveDispatchSubmission("hq:1", draft)).rejects.toMatchObject({
    code: "retryStorageUnavailable",
  })
  expect(await records()).toEqual([])
})
