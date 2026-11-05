import { extractAppCommandError, toErrorMessage } from "@/lib/app-error"
import { getTransport, type Transport } from "@/lib/transport"
import type { AssistantInfo } from "@/lib/types"

/** Engines a member can run on: Codex (default) and Claude Code. */
export type HallEngine = "codex" | "claude_code"
export const HALL_ENGINES: readonly HallEngine[] = ["codex", "claude_code"]
export const HALL_ENGINE_LABELS: Record<HallEngine, string> = {
  codex: "Codex",
  claude_code: "Claude Code",
}
/** The two session modes the server derives from a member's scope. */
export function hallSafeModes(engine: HallEngine): readonly string[] {
  return engine === "claude_code"
    ? ["default", "acceptEdits"]
    : ["read-only", "workspace-write"]
}
export function isHallEngine(value: unknown): value is HallEngine {
  return HALL_ENGINES.includes(value as HallEngine)
}

/** How a member's agent runs. Absent on members that never chose one. */
export type HallExecutionMode = "restricted" | "local_assistant"
export const HALL_EXECUTION_MODES: readonly HallExecutionMode[] = [
  "restricted",
  "local_assistant",
]
export type HallEnforcement = "verified" | "not_enforced" | "unknown"
export interface HallExecutionCapabilities {
  readBoundary: HallEnforcement
  writeBoundary: HallEnforcement
  networkBoundary: HallEnforcement
  permissionRequests: "supported" | "unsupported" | "unknown"
  hallIdentity: boolean
  hallReport: boolean
  exitCoverage:
    | "linux_cgroup_v2"
    | "windows_job_object"
    | "macos_process_group_best_effort"
    | "unknown"
  engineVersion: string | null
  adapterVersion: string | null
  evidenceId: string | null
}
const ENFORCEMENTS: readonly string[] = ["verified", "not_enforced", "unknown"]
/**
 * Defensive read of host-generated capabilities. Anything malformed is
 * treated as absent so the UI never invents a guarantee.
 */
export function readCapabilities(
  value: unknown
): HallExecutionCapabilities | null {
  const v = value as HallExecutionCapabilities | null | undefined
  if (
    !v ||
    typeof v !== "object" ||
    !ENFORCEMENTS.includes(v.readBoundary) ||
    !ENFORCEMENTS.includes(v.writeBoundary) ||
    !ENFORCEMENTS.includes(v.networkBoundary) ||
    typeof v.hallIdentity !== "boolean" ||
    typeof v.hallReport !== "boolean"
  )
    return null
  return v
}
export function isHallExecutionMode(
  value: unknown
): value is HallExecutionMode {
  return HALL_EXECUTION_MODES.includes(value as HallExecutionMode)
}

export interface HallMember {
  memberId: string
  name: string
  role: string
  cwd: string
  projectId: number | null
  engine: HallEngine
  assistantUid: string | null
  assistantRevision: number | null
  enabled: boolean
  revision: number
  createdAt: string
  updatedAt: string
}

export interface HallMemberEdit {
  name: string
  role: string
  assistantUid: string | null
  /** Omit to preserve the binding when using an older client. */
  projectId?: number | null
  /** A bound preset decides the engine; omit to keep the current one. */
  engine?: HallEngine
}

export interface HallMemberProject {
  id: number
  name: string
  alias: string | null
  path: string
}

export interface HallMemberDraft extends HallMemberEdit {
  cwd: string
}

export type HallMemberConfigField =
  | "model"
  | "scope.read"
  | "scope.write"
  | "scope.shell"
  | "scope.network"
  | "skills.shared"
  | "skills.own"
  | "onedesk_key"
  | "execution_mode"

/** `local` = loopback only (e.g. OneDesk on the hall's own host). */
export type HallMemberNetwork = "none" | "local" | "default"

export interface HallMemberConfigPreview {
  memberId: string
  revision: number
  valid: boolean
  files: { path: string; status: "loaded" | "missing" | "error" }[]
  effective: {
    model: string | null
    scope: {
      read: string[]
      write: string[]
      shell: string[]
      network: HallMemberNetwork
    }
    skills: { shared: string[]; own: string[] }
    /** Older servers omit it (same as `none`). */
    onedesk_key?: "none" | "read"
    /** Null or absent: the member never chose a mode (older servers omit it). */
    execution_mode?: HallExecutionMode | null
  } | null
  fields: {
    field: HallMemberConfigField
    source: string
    overridden: boolean
  }[]
  diagnostics: { code: string; source: string; field: string }[]
  /** Optional for compatibility with backends that only support reading. */
  fileVersion?: string
  overlay?: HallMemberConfigOverlay
  saveSupported?: boolean
  checkSupported?: boolean
  /** Read-only observation token, including invalid/unavailable files. */
  checkVersion?: string
}

export type HallMemberCheckId =
  | "directory"
  | "declaration"
  | "engine"
  | "template"
  | "project"
  | "runtime"
  | "execution"
export interface HallMemberCheckReport {
  memberId: string
  revision: number
  checkVersion: string
  configurationValid: boolean
  activationAvailable: false
  checks: {
    id: HallMemberCheckId
    status: "passed" | "warning" | "blocked" | "pending"
    code: string
  }[]
  diagnostics: HallMemberConfigPreview["diagnostics"]
  templateRecordedRevision: number | null
  templateCurrentRevision: number | null
  executionPreviewSupported?: boolean
  snapshotStorageSupported?: boolean
}

export interface HallMemberExecutionSummary {
  snapshotVersion: string
  engine: HallEngine
  requestedMode: "read-only" | "workspace-write" | "default" | "acceptEdits"
  modeSource: "scope" | "template"
  modelSource: "member" | "template" | "engine_default"
  permissionPolicy: "wait"
  optionCount: number
  templateRevision: number | null
  instructionsIncluded: boolean
  readRuleCount: number
  writeRuleCount: number
  shellRuleCount: number
  network: "none" | "default"
  runtimeVerified: false
  /** Optional: servers before the execution-mode contract omit both. */
  executionMode?: HallExecutionMode | null
  capabilities?: HallExecutionCapabilities
}

export interface HallMemberSnapshot {
  projectId?: number | null
  snapshotId: string
  memberId: string
  memberRevision: number
  checkVersion: string
  createdAt: string
  launchAllowed: false
  summary: HallMemberExecutionSummary
}

export interface HallMemberExecutionPreview {
  memberId: string
  revision: number
  checkVersion: string
  valid: boolean
  launchAllowed: false
  summary: HallMemberExecutionSummary | null
  issues: { code: string; field: string; index: number | null }[]
  preflight: HallMemberCheckReport
}

export interface HallMemberConfigOverlay {
  schema_version: 1
  model?: string
  scope?: {
    read?: string[]
    write?: string[]
    shell?: string[]
    network?: HallMemberNetwork
  }
  skills?: { shared?: string[]; own?: string[] }
  onedesk_key?: "none" | "read"
  execution_mode?: HallExecutionMode
}

export type HallMemberConfigValue = string | string[] | null

export interface HallMemberConfigDraftPreview {
  memberId: string
  revision: number
  fileVersion: string
  valid: boolean
  effective: HallMemberConfigPreview["effective"]
  fields: HallMemberConfigPreview["fields"]
  diagnostics: HallMemberConfigPreview["diagnostics"]
  changes: {
    field: HallMemberConfigField
    before: HallMemberConfigValue
    after: HallMemberConfigValue
    beforeSource: string
    afterSource: string
  }[]
}

/** One page, one backend. Never fall back to the shell on a missing command. */
export function createHallMembersClient(transport: Transport = getTransport()) {
  return {
    list: () => transport.call<HallMember[]>("hall_list_members", {}),
    projects: () =>
      transport.call<HallMemberProject[]>("hall_list_member_projects", {}),
    saveSnapshot: (
      memberId: string,
      expectedRevision: number,
      expectedCheckVersion: string,
      expectedSnapshotVersion: string
    ) =>
      transport.call<HallMemberSnapshot>("hall_save_member_snapshot", {
        memberId,
        expectedRevision,
        expectedCheckVersion,
        expectedSnapshotVersion,
      }),
    listSnapshots: (memberId: string) =>
      transport.call<HallMemberSnapshot[]>("hall_list_member_snapshots", {
        memberId,
      }),
    previewExecution: (
      memberId: string,
      expectedRevision: number,
      expectedCheckVersion: string
    ) =>
      transport.call<HallMemberExecutionPreview>(
        "hall_preview_member_execution",
        { memberId, expectedRevision, expectedCheckVersion }
      ),
    checkMember: (
      memberId: string,
      expectedRevision: number,
      expectedCheckVersion: string
    ) =>
      transport.call<HallMemberCheckReport>("hall_check_member", {
        memberId,
        expectedRevision,
        expectedCheckVersion,
      }),
    saveConfig: (
      memberId: string,
      expectedRevision: number,
      expectedFileVersion: string,
      draft: HallMemberConfigOverlay
    ) =>
      transport.call<HallMemberConfigPreview>("hall_save_member_config", {
        memberId,
        expectedRevision,
        expectedFileVersion,
        draft,
      }),
    previewDraft: (
      memberId: string,
      expectedRevision: number,
      expectedFileVersion: string,
      draft: HallMemberConfigOverlay
    ) =>
      transport.call<HallMemberConfigDraftPreview>(
        "hall_preview_member_config_draft",
        { memberId, expectedRevision, expectedFileVersion, draft }
      ),
    previewConfig: (memberId: string, expectedRevision: number) =>
      transport.call<HallMemberConfigPreview>("hall_preview_member_config", {
        memberId,
        expectedRevision,
      }),
    templates: async () => {
      const rows = await transport.call<AssistantInfo[]>("assistant_list", {})
      return rows.filter(
        (row) => isHallEngine(row.agent_type) && row.enabled && !row.deleted_at
      )
    },
    register: (draft: HallMemberDraft) =>
      transport.call<HallMember>("hall_register_member", { draft }),
    update: (
      memberId: string,
      expectedRevision: number,
      draft: HallMemberEdit
    ) =>
      transport.call<HallMember>("hall_update_member", {
        memberId,
        expectedRevision,
        draft,
      }),
    setEnabled: (memberId: string, enabled: boolean) =>
      transport.call<HallMember>("hall_set_member_enabled", {
        memberId,
        enabled,
      }),
  }
}

export type HallMembersClient = ReturnType<typeof createHallMembersClient>

export function isMembersUnavailable(error: unknown): boolean {
  return (
    extractAppCommandError(error)?.code === "not_implemented" ||
    /command\s+['"`]?hall_(?:list_members|list_member_projects|register_member|update_member|save_member_config|save_member_snapshot|list_member_snapshots|check_member|preview_member_execution|preview_member_config(?:_draft)?)['"`]?\s+not found/i.test(
      toErrorMessage(error)
    )
  )
}
