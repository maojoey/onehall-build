import { getActiveBackendKey } from "@/lib/transport"

function key(folderId: number): string {
  const remoteId = getActiveBackendKey()
  const localKey = `lastCmd:${folderId}`
  return remoteId === "local"
    ? localKey
    : `${localKey}:${remoteId.replace(/^remote:/, "r")}`
}

/** Folder/command ids belong to their backend; negative global ids remain valid. */
export function loadCommandSelection(folderId: number): number | null {
  try {
    const raw = localStorage.getItem(key(folderId))
    if (raw === null) return null
    const id = Number(raw)
    return Number.isSafeInteger(id) && id !== 0 ? id : null
  } catch {
    return null
  }
}

export function saveCommandSelection(
  folderId: number,
  commandId: number
): void {
  try {
    localStorage.setItem(key(folderId), String(commandId))
  } catch {
    // Selection is only a convenience; storage failure must not run a command.
  }
}
