import {
  fetchArtifactBlob,
  isObjectRef,
  uploadInputBytes,
  type ArtifactRef,
  type ObjectRef,
} from "./exchange"

/**
 * One input chosen by the operator. In a browser the page holds the bytes;
 * on the desktop it holds only the chosen path, and the host computes size
 * and hash and later re-reads the same file (rejecting links and changes).
 */
export interface PickedFile {
  name: string
  size: number
  sha256: string
  source: Blob | { path: string }
}

/** Byte channel for one HQ backend; credentials never reach this layer. */
export interface ByteTransfer {
  /** Host-side selection (desktop). Absent where the page picks a Blob. */
  pick?: () => Promise<PickedFile[]>
  upload?: (transferId: string, item: PickedFile) => Promise<ObjectRef>
  download: (artifact: ArtifactRef) => Promise<void>
}

export type HostInvoke = <T>(
  command: string,
  args: Record<string, unknown>
) => Promise<T>
export interface HostDialogs {
  pickFiles: () => Promise<string[]>
  /** `null` when the operator cancels. */
  saveFile: (suggestedName: string) => Promise<string | null>
}

interface InspectedFile {
  displayName: string
  size: number
  sha256: string
}
function isInspected(value: unknown): value is InspectedFile {
  const v = value as InspectedFile | null
  return (
    !!v &&
    typeof v.displayName === "string" &&
    v.displayName.length > 0 &&
    Number.isSafeInteger(v.size) &&
    v.size >= 0 &&
    typeof v.sha256 === "string" &&
    /^[a-f0-9]{64}$/.test(v.sha256)
  )
}

/** The receipt must describe exactly what was announced for this file. */
function checkedReceipt(ref: unknown, item: PickedFile): ObjectRef {
  if (!isObjectRef(ref) || ref.size !== item.size || ref.sha256 !== item.sha256)
    throw new Error("Malformed upload receipt")
  return ref
}

/**
 * Desktop shell. `headquartersConnectionId === null` is this computer's own
 * HQ; otherwise the saved remote HQ connection, whose token stays in Rust.
 */
export function desktopTransfer(
  headquartersConnectionId: number | null,
  invoke: HostInvoke,
  dialogs: HostDialogs
): ByteTransfer {
  const remote = headquartersConnectionId
  return {
    pick: async () => {
      const picked: PickedFile[] = []
      for (const path of await dialogs.pickFiles()) {
        const seen = await invoke<unknown>("hall_exchange_inspect_file", {
          path,
        })
        if (!isInspected(seen)) throw new Error("Malformed file inspection")
        picked.push({
          name: seen.displayName,
          size: seen.size,
          sha256: seen.sha256,
          source: { path },
        })
      }
      return picked
    },
    upload: async (transferId, item) => {
      if (item.source instanceof Blob)
        throw new Error("Desktop upload needs a chosen file")
      const path = item.source.path
      const ref =
        remote === null
          ? await invoke<unknown>("hall_exchange_upload_file", {
              transferId,
              path,
            })
          : await invoke<unknown>("hall_exchange_remote_upload_file", {
              connectionId: remote,
              transferId,
              path,
            })
      return checkedReceipt(ref, item)
    },
    download: async (artifact) => {
      // The host creates a NEW file only; an existing one is never replaced.
      const destination = await dialogs.saveFile(artifact.displayName)
      if (!destination) return
      if (remote === null)
        await invoke("hall_exchange_save_artifact", {
          objectId: artifact.objectId,
          destination,
        })
      else
        await invoke("hall_exchange_remote_save_artifact", {
          connectionId: remote,
          objectId: artifact.objectId,
          size: artifact.size,
          sha256: artifact.sha256,
          destination,
        })
    },
  }
}

/** Save through the browser's own download; never overwrites a chosen path. */
async function browserDownload(artifact: ArtifactRef) {
  const url = URL.createObjectURL(await fetchArtifactBlob(artifact.objectId))
  try {
    const link = document.createElement("a")
    link.href = url
    link.download = artifact.displayName
    link.click()
  } finally {
    URL.revokeObjectURL(url)
  }
}

/** Same-origin browser page served by the HQ itself. */
export function browserTransfer(): ByteTransfer {
  return {
    upload: async (transferId, item) => {
      if (!(item.source instanceof Blob))
        throw new Error("Browser upload needs file bytes")
      return checkedReceipt(
        await uploadInputBytes(transferId, item.source),
        item
      )
    },
    download: browserDownload,
  }
}

/** Real Tauri bindings, loaded only inside the desktop shell. */
export const tauriHost: { invoke: HostInvoke; dialogs: HostDialogs } = {
  invoke: async (command, args) => {
    const { invoke } = await import("@tauri-apps/api/core")
    return invoke(command, args)
  },
  dialogs: {
    pickFiles: async () => {
      const { open } = await import("@tauri-apps/plugin-dialog")
      const chosen = await open({ multiple: true, directory: false })
      if (!chosen) return []
      return Array.isArray(chosen) ? chosen : [chosen]
    },
    saveFile: async (suggestedName) => {
      const { save } = await import("@tauri-apps/plugin-dialog")
      return (await save({ defaultPath: suggestedName })) ?? null
    },
  },
}
