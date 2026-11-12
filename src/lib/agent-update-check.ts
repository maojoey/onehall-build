import {
  acpAdapterStatus,
  acpAdapterUpgrade,
  acpFetchAgentLatestRelease,
} from "@/lib/api"
import type { AgentType } from "@/lib/types"

/** Agents that run through an adapter OneHall can upgrade safely. */
export const CHECKED_AGENTS: readonly AgentType[] = ["claude_code", "codex"]

export const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000
const LAST_CHECK_KEY = "onehall.agentUpdates.lastCheck"
const AUTO_APPLY_KEY = "onehall.agentUpdates.autoApply"

export interface AgentUpdateOffer {
  agentType: AgentType
  installed: string
  latest: string
}

function parts(version: string): number[] | null {
  const core = version.trim().replace(/^v/, "").split(/[-+]/)[0]
  const nums = core.split(".").map((p) => (/^\d+$/.test(p) ? Number(p) : NaN))
  return nums.length > 0 && nums.every((n) => !Number.isNaN(n)) ? nums : null
}

/** Negative, zero or positive like a comparator; null when not comparable. */
export function compareVersions(a: string, b: string): number | null {
  const left = parts(a)
  const right = parts(b)
  if (!left || !right) return null
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const diff = (left[i] ?? 0) - (right[i] ?? 0)
    if (diff !== 0) return diff
  }
  return 0
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value)
  } catch {
    // Storage can be blocked; the check then simply runs each visit.
  }
}

export function isCheckDue(now = Date.now()): boolean {
  const last = Number(read(LAST_CHECK_KEY))
  return !Number.isFinite(last) || last <= 0 || now - last >= CHECK_INTERVAL_MS
}

export function markChecked(now = Date.now()) {
  write(LAST_CHECK_KEY, String(now))
}

export function getAutoApply(): boolean {
  return read(AUTO_APPLY_KEY) === "1"
}

export function setAutoApply(on: boolean) {
  write(AUTO_APPLY_KEY, on ? "1" : "0")
}

/** Which checked agents have a newer adapter than the one installed. */
export async function findAgentUpdates(): Promise<AgentUpdateOffer[]> {
  const offers: AgentUpdateOffer[] = []
  for (const agentType of CHECKED_AGENTS) {
    try {
      const [status, latest] = await Promise.all([
        acpAdapterStatus(agentType),
        acpFetchAgentLatestRelease(agentType),
      ])
      const installed = status.adapter_version
      if (!installed || !latest) continue
      const order = compareVersions(installed, latest.version)
      if (order !== null && order < 0) {
        offers.push({ agentType, installed, latest: latest.version })
      }
    } catch {
      // Offline or the registry is down: try again at the next check.
    }
  }
  return offers
}

/**
 * Apply an offer through the tested upgrade. A failed startup check keeps the
 * old version active, so this never leaves the agent worse off.
 */
export async function applyAgentUpdate(
  offer: AgentUpdateOffer
): Promise<{ ok: boolean; message?: string }> {
  try {
    await acpAdapterUpgrade(offer.agentType, offer.latest)
    return { ok: true }
  } catch (error) {
    return {
      ok: false,
      message: error instanceof Error ? error.message : String(error),
    }
  }
}
