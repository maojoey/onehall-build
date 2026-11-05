import { sha256 } from "@noble/hashes/sha256"
import { bytesToHex } from "@noble/hashes/utils"
import type { DispatchDraft } from "./dispatch"

const databaseName = "hall.dispatch.submissions.v2"
const storeName = "submissions"
const maxRecords = 100
type Submission = { scope: string; digest: string; submissionId: string }
type Failure = "retryMismatch" | "retryStorageUnavailable" | "retryStorageFull"

export class DispatchSubmissionError extends Error {
  constructor(readonly code: Failure) {
    super(code)
  }
}

function validRecord(value: unknown): value is Submission {
  if (typeof value !== "object" || value === null) return false
  const record = value as Partial<Submission>
  return (
    typeof record.scope === "string" &&
    record.scope.length <= 512 &&
    typeof record.digest === "string" &&
    /^[a-f0-9]{64}$/.test(record.digest) &&
    typeof record.submissionId === "string" &&
    /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(
      record.submissionId
    )
  )
}

/** getRandomValues is available on private HTTP; subtle/randomUUID are not. */
function strongUuid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 0x0f) | 0x40
  bytes[8] = (bytes[8] & 0x3f) | 0x80
  const hex = bytesToHex(bytes)
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

/** Each operation opens its own connection, so the database transaction is the
 * cross-tab lock. Resolve only after commit, never after a request's success. */
async function transact<T>(
  work: (
    store: IDBObjectStore,
    done: (value: T) => void,
    fail: (code: Failure) => void
  ) => void
): Promise<T> {
  return new Promise((resolve, reject) => {
    const unavailable = () =>
      reject(new DispatchSubmissionError("retryStorageUnavailable"))
    try {
      // The earlier prototype was unreleased. Do not silently discard any
      // uncertain localStorage identity if a developer still has those records.
      const legacy = JSON.parse(
        localStorage.getItem("hall.dispatch.submissions.v1") ?? "[]"
      ) as unknown
      if (!Array.isArray(legacy) || legacy.length) {
        unavailable()
        return
      }
      const request = indexedDB.open(databaseName, 1)
      request.onupgradeneeded = () => {
        request.result.createObjectStore(storeName, { keyPath: "scope" })
      }
      request.onerror = unavailable
      request.onblocked = unavailable
      request.onsuccess = () => {
        const database = request.result
        try {
          const transaction = database.transaction(storeName, "readwrite", {
            durability: "strict",
          })
          let value: T
          let hasValue = false
          let failure: Failure = "retryStorageUnavailable"
          transaction.oncomplete = () => {
            database.close()
            if (hasValue) resolve(value)
            else unavailable()
          }
          transaction.onabort = () => {
            database.close()
            reject(new DispatchSubmissionError(failure))
          }
          const fail = (code: Failure) => {
            failure = code
            transaction.abort()
          }
          work(
            transaction.objectStore(storeName),
            (next) => {
              value = next
              hasValue = true
            },
            fail
          )
        } catch {
          database.close()
          unavailable()
        }
      }
    } catch {
      unavailable()
    }
  })
}

/** Store only scope, UUID and SHA256 of the full normalized payload. Keep the
 * identity until explicit New task, including after an uncertain create. */
export async function reserveDispatchSubmission(
  context: string,
  draft: Omit<DispatchDraft, "submissionId" | "exchangeManifestId">,
  /** Fingerprint of chosen files; omitted so plain drafts keep their digest. */
  attachmentKey?: string
): Promise<string> {
  const scope = JSON.stringify([context, draft.branchId])
  const digest = bytesToHex(
    sha256(
      new TextEncoder().encode(
        JSON.stringify({
          branchId: draft.branchId,
          memberRef: draft.memberRef,
          title: draft.title.trim(),
          description: draft.description,
          ...(attachmentKey ? { attachmentKey } : {}),
        })
      )
    )
  )
  return transact((store, done, fail) => {
    const read = store.get(scope)
    read.onsuccess = () => {
      const existing: unknown = read.result
      if (existing !== undefined) {
        if (!validRecord(existing)) fail("retryStorageUnavailable")
        else if (existing.digest !== digest) fail("retryMismatch")
        else done(existing.submissionId)
        return
      }
      const count = store.count()
      count.onsuccess = () => {
        // The count and insert share one write transaction. Never evict.
        if (count.result >= maxRecords) {
          fail("retryStorageFull")
          return
        }
        try {
          const submissionId = strongUuid()
          store.add({ scope, digest, submissionId } satisfies Submission)
          done(submissionId)
        } catch {
          fail("retryStorageUnavailable")
        }
      }
    }
  })
}

export async function newDispatchSubmission(context: string, branchId: string) {
  const scope = JSON.stringify([context, branchId])
  return transact<void>((store, done) => {
    store.delete(scope)
    done(undefined)
  })
}
