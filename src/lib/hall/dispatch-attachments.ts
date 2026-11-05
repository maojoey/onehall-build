import { sha256 } from "@noble/hashes/sha256"
import { bytesToHex } from "@noble/hashes/utils"
import type { DispatchClient } from "./dispatch"
import type { PickedFile } from "./exchange-transfer"

export type { PickedFile } from "./exchange-transfer"

/** Courtesy limits mirroring the host (contract §3); the host still enforces. */
export const MAX_FILE_BYTES = 512 * 1024 * 1024
export const MAX_TOTAL_BYTES = 2 * 1024 * 1024 * 1024
export const MAX_ITEMS = 200
const CHUNK = 4 * 1024 * 1024

export type AttachmentProblem =
  | "tooMany"
  | "tooLarge"
  | "totalTooLarge"
  | "duplicate"

function readChunk(blob: Blob): Promise<ArrayBuffer> {
  if (typeof blob.arrayBuffer === "function") return blob.arrayBuffer()
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result as ArrayBuffer)
    reader.onerror = () => reject(reader.error ?? new Error("Read failed"))
    reader.readAsArrayBuffer(blob)
  })
}

/** Streams the file in chunks so a large input is not copied whole in memory. */
export async function hashFile(file: Blob): Promise<string> {
  const hash = sha256.create()
  for (let at = 0; at < file.size; at += CHUNK)
    hash.update(new Uint8Array(await readChunk(file.slice(at, at + CHUNK))))
  return bytesToHex(hash.digest())
}

/** Browser selection: the page hashes the bytes it will send. */
export async function pickBrowserFile(file: File): Promise<PickedFile> {
  return {
    name: file.name,
    size: file.size,
    sha256: await hashFile(file),
    source: file,
  }
}

export function checkAttachments(
  files: { name: string; size: number }[],
  outputs: string[]
): AttachmentProblem | null {
  if (files.length + outputs.length > MAX_ITEMS) return "tooMany"
  if (files.some((f) => f.size > MAX_FILE_BYTES)) return "tooLarge"
  if (files.reduce((sum, f) => sum + f.size, 0) > MAX_TOTAL_BYTES)
    return "totalTooLarge"
  const inputs = files.map((f) => f.name.toLowerCase())
  const slots = outputs.map((n) => n.toLowerCase())
  if (
    new Set(inputs).size !== inputs.length ||
    new Set(slots).size !== slots.length
  )
    return "duplicate"
  return null
}

/** Stable across retries of the same selection; goes into the submission key. */
export function attachmentKey(picked: PickedFile[], outputs: string[]): string {
  return bytesToHex(
    sha256(
      new TextEncoder().encode(
        JSON.stringify([
          picked.map((p) => [p.name, p.size, p.sha256]).sort(),
          [...outputs].sort(),
        ])
      )
    )
  )
}

/**
 * Stage every file under the one dispatch submission, then freeze a manifest.
 * Each step is idempotent on the host, so a retry after an uncertain failure
 * repeats the same submission instead of creating a second set of objects.
 */
export async function stageAttachments(
  client: Pick<DispatchClient, "exchange" | "bytes">,
  submissionId: string,
  picked: PickedFile[],
  outputs: string[]
): Promise<string> {
  const { exchange, bytes } = client
  if (!exchange || !bytes?.upload) throw new Error("File exchange unavailable")
  const ids: string[] = []
  for (const item of picked) {
    const ticket = await exchange.prepareInput(
      submissionId,
      item.name,
      item.size,
      item.sha256
    )
    await bytes.upload(ticket.transferId, item)
    const ref = await exchange.confirmInput(submissionId, ticket.objectId)
    ids.push(ref.objectId)
  }
  return (await exchange.prepareManifest(submissionId, ids, outputs)).manifestId
}
