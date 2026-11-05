import { extractAppCommandError, toErrorMessage } from "@/lib/app-error"
import { getServerBaseUrl, getTransport, type Transport } from "@/lib/transport"
import { codegAuthHeaders, getCodegToken } from "@/lib/transport/web-auth"

/** Mirrors of `src-tauri/src/hall/exchange` (contract §3–4). */
export interface ObjectRef {
  objectId: string
  displayName: string
  size: number
  sha256: string
}
export interface ExchangeManifest {
  schemaVersion: 1
  manifestId: string
  inputs: ObjectRef[]
  outputs: { slotId: string; displayName: string }[]
  digest: string
}
export interface ArtifactRef extends ObjectRef {
  slotId: string
  dispatchId: string
  branchId: string
  taskId: string
  attemptId: string
  runId: string
  sealedAt: string
  exitVerified: boolean
}
export interface InputTicket {
  objectId: string
  transferId: string
}

const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i
const SHA = /^[a-f0-9]{64}$/i

export function isObjectRef(value: unknown): value is ObjectRef {
  const v = value as ObjectRef | null
  return (
    !!v &&
    typeof v.objectId === "string" &&
    UUID.test(v.objectId) &&
    typeof v.displayName === "string" &&
    v.displayName.length > 0 &&
    Number.isSafeInteger(v.size) &&
    v.size >= 0 &&
    typeof v.sha256 === "string" &&
    SHA.test(v.sha256)
  )
}

/** Display check for a frozen manifest; the host has already verified it. */
export function isExchangeManifest(value: unknown): value is ExchangeManifest {
  const v = value as ExchangeManifest | null
  return (
    !!v &&
    v.schemaVersion === 1 &&
    typeof v.manifestId === "string" &&
    typeof v.digest === "string" &&
    SHA.test(v.digest) &&
    Array.isArray(v.inputs) &&
    v.inputs.every(isObjectRef) &&
    Array.isArray(v.outputs) &&
    v.outputs.every(
      (o) =>
        !!o &&
        typeof o.slotId === "string" &&
        typeof o.displayName === "string" &&
        o.displayName.length > 0
    )
  )
}

/** Strict check: a missing field is a protocol problem, never coerced. */
export function isArtifactRef(value: unknown): value is ArtifactRef {
  const v = value as ArtifactRef | null
  return (
    isObjectRef(v) &&
    typeof v.slotId === "string" &&
    typeof v.dispatchId === "string" &&
    typeof v.taskId === "string" &&
    typeof v.attemptId === "string" &&
    typeof v.runId === "string" &&
    typeof v.sealedAt === "string" &&
    typeof v.exitVerified === "boolean"
  )
}

export async function sha256Hex(bytes: ArrayBuffer): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", bytes)
  return Array.from(new Uint8Array(digest), (b) =>
    b.toString(16).padStart(2, "0")
  ).join("")
}

export type ExchangeCall = <T>(request: Record<string, unknown>) => Promise<T>

/** `hall_exchange` is an HQ command; one page talks to one backend. */
export function transportExchangeCall(
  transport: Transport = getTransport()
): ExchangeCall {
  return (request) => transport.call("hall_exchange", { request })
}

export function createExchangeClient(
  call: ExchangeCall = transportExchangeCall()
) {
  return {
    prepareInput: (
      submissionId: string,
      displayName: string,
      size: number,
      sha256: string
    ) =>
      call<InputTicket>({
        action: "prepare_input",
        submissionId,
        displayName,
        size,
        sha256,
      }),
    confirmInput: (submissionId: string, objectId: string) =>
      call<ObjectRef>({ action: "confirm_input", submissionId, objectId }),
    prepareManifest: (
      submissionId: string,
      inputObjectIds: string[],
      outputNames: string[]
    ) =>
      call<ExchangeManifest>({
        action: "prepare_manifest",
        submissionId,
        inputObjectIds,
        outputNames,
      }),
    /** Throws on a malformed row instead of showing a partial list. */
    artifacts: async (dispatchId: string): Promise<ArtifactRef[]> => {
      const rows = await call<unknown>({ action: "artifacts", dispatchId })
      if (!Array.isArray(rows) || !rows.every(isArtifactRef))
        throw new Error("Malformed artifact list")
      return rows
    },
  }
}
export type ExchangeClient = ReturnType<typeof createExchangeClient>

export function isExchangeUnavailable(error: unknown): boolean {
  return (
    extractAppCommandError(error)?.code === "configuration_missing" ||
    /command\s+['"`]?hall_exchange['"`]?\s+not found/i.test(
      toErrorMessage(error)
    )
  )
}

function authHeaders(): Record<string, string> {
  const token = getCodegToken()
  return token ? codegAuthHeaders(token) : {}
}

/** Browser upload of the bytes announced by `prepareInput`; the host verifies. */
export async function uploadInputBytes(
  transferId: string,
  body: Blob
): Promise<ObjectRef> {
  const response = await fetch(
    `${getServerBaseUrl()}/api/hall-exchange/transfers/${encodeURIComponent(transferId)}`,
    { method: "PUT", body, headers: authHeaders(), credentials: "include" }
  )
  if (!response.ok) throw new Error(`Upload failed (${response.status})`)
  const ref: unknown = await response.json()
  if (!isObjectRef(ref)) throw new Error("Malformed upload receipt")
  return ref
}

/** Browser download; the user's browser decides where the file is saved. */
export async function fetchArtifactBlob(objectId: string): Promise<Blob> {
  const response = await fetch(
    `${getServerBaseUrl()}/api/hall-exchange/artifacts/${encodeURIComponent(objectId)}`,
    { headers: authHeaders(), credentials: "include" }
  )
  if (!response.ok) throw new Error(`Download failed (${response.status})`)
  return response.blob()
}
