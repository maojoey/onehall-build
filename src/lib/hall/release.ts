import policy from "../../../onehall.release.json"
import tauriConfig from "../../../src-tauri/tauri.conf.json"
import hallConfig from "../../../src-tauri/tauri.hall.conf.json"
import type { UpdateContextValue } from "@/components/providers/update-provider"

export const ONEHALL_RELEASES_URL = policy.releasesUrl
export const ONEHALL_BUILD_VERSION = policy.version
export const ONEHALL_UPSTREAM = policy.upstream

function publicKeyPacket(key: string): string | null {
  try {
    const lines = atob(key).trimEnd().split(/\r?\n/)
    if (lines.length !== 2 || !lines[0].startsWith("untrusted comment: ")) {
      return null
    }
    const packet = atob(lines[1])
    return packet.length === 42 && packet.startsWith("Ed")
      ? packet.slice(10)
      : null
  } catch {
    return null
  }
}

/** A release must opt in explicitly; unknown policies remain manual. */
export function signedDesktopUpdatesEnabled(): boolean {
  if (policy.product !== "onehall" || policy.updateMode !== "desktop-signed")
    return false
  const updater = hallConfig.plugins?.updater as {
    pubkey?: string
    endpoints?: unknown
  }
  const packet =
    typeof updater?.pubkey === "string" ? publicKeyPacket(updater.pubkey) : null
  const upstreamPacket = publicKeyPacket(tauriConfig.plugins.updater.pubkey)
  if (
    !packet ||
    !upstreamPacket ||
    packet === upstreamPacket ||
    !Array.isArray(updater.endpoints) ||
    updater.endpoints.length === 0
  ) {
    return false
  }
  let origin: string | null = null
  return updater.endpoints.every((endpoint: unknown) => {
    if (typeof endpoint !== "string") return false
    try {
      const url = new URL(endpoint)
      origin ??= url.origin
      return (
        url.origin === origin &&
        url.protocol === "https:" &&
        !!url.hostname &&
        !url.username &&
        !url.password &&
        !endpoint.includes("?") &&
        !endpoint.includes("#")
      )
    } catch {
      return false
    }
  })
}

/** No installer is offered until OneHall has its own signed release channel. */
export function manualUpdateContext(
  currentVersion: string
): UpdateContextValue {
  const blocked = async () => {
    throw new Error("OneHall uses manual updates")
  }
  return {
    manualOnly: true,
    state: { seq: 0, status: "idle" },
    isUpdating: false,
    restartCountdown: null,
    isRollingBack: false,
    isRestarting: false,
    hydrated: !!currentVersion,
    isBusy: false,
    available: null,
    currentVersion,
    checking: false,
    checkError: null,
    lastCheckedAt: null,
    selfUpdateSupported: false,
    liveProgress: false,
    runtime: undefined,
    rollbackAvailable: false,
    selfUpdateBlocker: null,
    canInstallInPlace: false,
    dismissedVersion: null,
    checkNow: async () => {},
    dismissAvailable: () => {},
    refreshLocalStatus: async () => {},
    startUpdate: blocked,
    restart: blocked,
    rollback: blocked,
  }
}
