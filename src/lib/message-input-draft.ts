"use client"

import type { JSONContent } from "@tiptap/core"
import { getActiveBackendKey } from "@/lib/transport" // onehall

import { sanitizeComposerDraftDoc } from "./composer-draft-sanitize"

interface PersistedDraftState {
  text: string
}

/** v2 draft payload: the composer's Tiptap document (preserves reference badges,
 *  which a Markdown round-trip would downgrade to plain links). */
interface PersistedDraftStateV2 {
  doc: JSONContent
}

const STORAGE_PREFIX = "codeg:message-input-draft:v1"
const STORAGE_PREFIX_V2 = "codeg:message-input-draft:v2"
const draftTextCache = new Map<string, string>()
const draftDocCache = new Map<string, JSONContent>()
const pendingPersistDrafts = new Map<string, string>()
const pendingPersistDocs = new Map<string, JSONContent>()
let idlePersistHandle: number | null = null
let persistenceListenersBound = false

// onehall: conversation ids and draft tab ids repeat across backends (local
// 1 vs remote 1), and in single-window mode they share one localStorage and
// these module caches — so every draft key is namespaced by the backend it
// belongs to, in memory as well as on disk. Local keys are unchanged. The
// exported functions take the caller's plain key; everything below them
// (caches, pending writes, storage) works on the scoped key, so a write
// flushed after a tab switch still lands under the backend it came from.
function backendScope(): string {
  const id = getActiveBackendKey()
  return id === "local" ? "" : `${id.replace(/^remote:/, "r")}:`
}

function scopedDraftKey(draftKey: string): string {
  return `${backendScope()}${draftKey}`
}

function storageKeyForDraftKey(scopedKey: string): string {
  return `${STORAGE_PREFIX}:${scopedKey}`
}

function storageKeyForDraftKeyV2(scopedKey: string): string {
  return `${STORAGE_PREFIX_V2}:${scopedKey}`
}

/** A persisted v2 payload's `doc` is only trusted when it is a ProseMirror doc
 *  root (a non-array object whose `type` is "doc"); anything else (corrupt or
 *  partial payload, array, …) is rejected so we fall back to v1 / null rather
 *  than hand garbage to the editor. */
function isTiptapDoc(value: unknown): value is JSONContent {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    (value as { type?: unknown }).type === "doc"
  )
}

function flushPendingDraftPersistence(): void {
  if (typeof window === "undefined") return
  if (pendingPersistDrafts.size === 0 && pendingPersistDocs.size === 0) {
    idlePersistHandle = null
    return
  }

  const textEntries = Array.from(pendingPersistDrafts.entries())
  pendingPersistDrafts.clear()
  const docEntries = Array.from(pendingPersistDocs.entries())
  pendingPersistDocs.clear()
  idlePersistHandle = null

  for (const [scopedKey, text] of textEntries) {
    try {
      localStorage.setItem(
        storageKeyForDraftKey(scopedKey),
        JSON.stringify({ text })
      )
    } catch {
      // Ignore storage quota/permission failures.
    }
  }
  for (const [scopedKey, doc] of docEntries) {
    let persisted = false
    try {
      localStorage.setItem(
        storageKeyForDraftKeyV2(scopedKey),
        JSON.stringify({ doc } satisfies PersistedDraftStateV2)
      )
      persisted = true
    } catch {
      // Keep the legacy v1 draft as a fallback when the v2 write fails
      // (quota / permission / serialization), so the draft is not lost.
    }
    // Only retire the legacy v1 draft once the v2 document is durably written.
    if (persisted) clearScopedDraftV1(scopedKey)
  }
}

function cancelScheduledDraftPersistence(): void {
  if (typeof window === "undefined") return
  if (idlePersistHandle == null) return
  if ("cancelIdleCallback" in window) {
    window.cancelIdleCallback(idlePersistHandle)
  }
  idlePersistHandle = null
}

function ensurePersistenceListeners(): void {
  if (typeof window === "undefined") return
  if (persistenceListenersBound) return
  persistenceListenersBound = true

  const flushNow = () => {
    cancelScheduledDraftPersistence()
    flushPendingDraftPersistence()
  }

  window.addEventListener("pagehide", flushNow)
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") {
      flushNow()
    }
  })
}

function scheduleDraftPersistence(): void {
  if (typeof window === "undefined") return
  if (idlePersistHandle != null) return

  ensurePersistenceListeners()
  if ("requestIdleCallback" in window) {
    idlePersistHandle = window.requestIdleCallback(() => {
      flushPendingDraftPersistence()
    })
    return
  }

  // Fallback for runtimes without requestIdleCallback.
  flushPendingDraftPersistence()
}

export function buildConversationDraftStorageKey(
  conversationId: number
): string {
  return `conv:${conversationId}`
}

/** Draft key prefix for an unbound (new-conversation) tab. Split groups each own
 *  their own draft tab, so the composer text is keyed PER TAB — the old shared
 *  `"new"` key made two groups' drafts overwrite each other. Draft tab ids
 *  survive a restart (they ride the group blob), so the text comes back with the
 *  tab. See {@link LEGACY_NEW_CONVERSATION_DRAFT_KEY} for the one-time handover. */
const NEW_CONVERSATION_DRAFT_PREFIX = "draft:"

/** The pre-per-tab shared key, adopted once by the first draft tab that finds
 *  no draft of its own (see {@link adoptLegacyNewConversationDraft}). */
export const LEGACY_NEW_CONVERSATION_DRAFT_KEY = "new"

export function buildNewConversationDraftStorageKey(tabId: string): string {
  return `${NEW_CONVERSATION_DRAFT_PREFIX}${tabId}`
}

/** Move a draft (v1 text + v2 doc + caches) from one key to another, e.g. when a
 *  closing draft tab hands its unsent text to the replacement draft. No-op when
 *  the source has nothing. */
export function moveMessageInputDraft(fromKey: string, toKey: string): void {
  if (fromKey === toKey) return
  const loaded = loadMessageInputDraftV2(fromKey)
  if (!loaded) return
  if (loaded.kind === "doc") {
    saveMessageInputDraftV2(toKey, loaded.doc)
  } else {
    saveMessageInputDraft(toKey, loaded.markdown)
  }
  clearMessageInputDraftV2(fromKey)
}

const legacyDraftAdopted = new Set<string>()

/** One-shot migration: hand the pre-per-tab shared draft to the first draft tab
 *  that has none of its own, so an in-flight draft is not stranded by the
 *  upgrade. A draft tab that already carries its own text passes without
 *  consuming the handover (the next empty one takes it); once there is nothing
 *  left to hand over, the whole path retires for the session. */
export function adoptLegacyNewConversationDraft(toKey: string): void {
  const scope = backendScope()
  if (legacyDraftAdopted.has(scope)) return
  if (!loadMessageInputDraftV2(LEGACY_NEW_CONVERSATION_DRAFT_KEY)) {
    legacyDraftAdopted.add(scope)
    return
  }
  if (loadMessageInputDraftV2(toKey)) return
  legacyDraftAdopted.add(scope)
  moveMessageInputDraft(LEGACY_NEW_CONVERSATION_DRAFT_KEY, toKey)
}

/** Test seam: re-arms the one-shot legacy adoption. */
export function resetLegacyDraftAdoptionForTests(): void {
  legacyDraftAdopted.clear()
}

/**
 * Drop persisted per-tab drafts whose tab no longer exists — a crash (or an
 * older build) can leave keys behind that nothing will ever clear. Scheduled on
 * idle and run once per launch by the tab store, right after hydration.
 *
 * The live set is a THUNK, read when the sweep actually runs: idle can land well
 * after the call, and a draft created in between (post-hydration recovery, the
 * user hitting "+") would otherwise look orphaned and lose its text.
 */
export function sweepOrphanDraftKeys(
  getLiveDraftTabIds: () => Set<string>,
  isCurrent: () => boolean = () => true
): void {
  if (typeof window === "undefined") return
  const scope = backendScope()
  const run = () => {
    // The live-set thunk reads a shared store. After a switch it belongs to
    // another backend and must never be used to prune this backend's drafts.
    // A → B → A can return to this scope before idle runs. The original
    // hydration must still be alive; a new A may not have loaded its tabs yet.
    if (!isCurrent() || backendScope() !== scope) return
    const liveDraftTabIds = getLiveDraftTabIds()
    // Track both halves: the storage key to remove and the draft key whose
    // caches must go with it (they can't be derived from each other by string
    // search — the storage prefix itself contains "draft:").
    const stale: { storageKey: string; draftKey: string }[] = []
    try {
      for (let i = 0; i < localStorage.length; i++) {
        const key = localStorage.key(i)
        if (!key) continue
        for (const prefix of [STORAGE_PREFIX, STORAGE_PREFIX_V2]) {
          const head = `${prefix}:${scope}${NEW_CONVERSATION_DRAFT_PREFIX}`
          if (!key.startsWith(head)) continue
          const tabId = key.slice(head.length)
          if (liveDraftTabIds.has(tabId)) continue
          stale.push({
            storageKey: key,
            draftKey: `${scope}${buildNewConversationDraftStorageKey(tabId)}`,
          })
        }
      }
      for (const { storageKey } of stale) localStorage.removeItem(storageKey)
    } catch {
      /* ignore storage access failures */
    }
    for (const { draftKey } of stale) {
      draftTextCache.delete(draftKey)
      draftDocCache.delete(draftKey)
      pendingPersistDrafts.delete(draftKey)
      pendingPersistDocs.delete(draftKey)
    }
  }
  if ("requestIdleCallback" in window) {
    window.requestIdleCallback(() => run())
    return
  }
  run()
}

export function loadMessageInputDraft(draftKey: string): string | null {
  const key = scopedDraftKey(draftKey)
  const cached = draftTextCache.get(key)
  if (typeof cached === "string") return cached
  if (typeof window === "undefined") return null

  try {
    const raw = localStorage.getItem(storageKeyForDraftKey(key))
    if (!raw) return null
    const parsed = JSON.parse(raw) as Partial<PersistedDraftState>
    if (typeof parsed.text !== "string") return null
    draftTextCache.set(key, parsed.text)
    return parsed.text
  } catch {
    return null
  }
}

export function saveMessageInputDraft(draftKey: string, text: string): void {
  if (text.length === 0) {
    clearMessageInputDraft(draftKey)
    return
  }

  const key = scopedDraftKey(draftKey)
  if (draftTextCache.get(key) === text) return
  draftTextCache.set(key, text)
  if (typeof window === "undefined") return

  pendingPersistDrafts.set(key, text)
  scheduleDraftPersistence()
}

export function clearMessageInputDraft(draftKey: string): void {
  clearScopedDraftV1(scopedDraftKey(draftKey))
}

function clearScopedDraftV1(scopedKey: string): void {
  draftTextCache.delete(scopedKey)
  pendingPersistDrafts.delete(scopedKey)
  if (typeof window === "undefined") return

  try {
    localStorage.removeItem(storageKeyForDraftKey(scopedKey))
  } catch {
    /* ignore */
  }
}

/**
 * Result of loading a v2 draft: a parsed composer document, a legacy v1 Markdown
 * string to hydrate via `setText` (migration), or null when no draft exists.
 */
export type LoadedDraftV2 =
  | { kind: "doc"; doc: JSONContent }
  | { kind: "legacyMarkdown"; markdown: string }
  | null

/**
 * Load the persisted composer draft for a key. Prefers the v2 document; falls
 * back to a v1 text draft (returned as `legacyMarkdown` for the host to hydrate
 * as Markdown), or null. The v1 draft is left in place on read — it is only
 * cleared once a v2 document is actually saved ({@link saveMessageInputDraftV2}),
 * so an unedited migration is never lost.
 */
export function loadMessageInputDraftV2(draftKey: string): LoadedDraftV2 {
  const key = scopedDraftKey(draftKey)
  const cached = draftDocCache.get(key)
  if (cached) return { kind: "doc", doc: cached }

  if (typeof window !== "undefined") {
    try {
      const raw = localStorage.getItem(storageKeyForDraftKeyV2(key))
      if (raw) {
        const parsed = JSON.parse(raw) as Partial<PersistedDraftStateV2>
        if (isTiptapDoc(parsed?.doc)) {
          // Down-migrate a draft saved by the old rich-text composer: a stale
          // heading/list/codeBlock/mark would otherwise make `setContent`
          // silently discard the WHOLE doc (losing the user's text and badges).
          // A draft already in the plain-text schema passes through untouched.
          const doc = sanitizeComposerDraftDoc(parsed.doc)
          draftDocCache.set(key, doc)
          return { kind: "doc", doc }
        }
      }
    } catch {
      // Fall through to the legacy v1 draft.
    }
  }

  const legacy = loadMessageInputDraft(draftKey)
  if (legacy != null && legacy.length > 0) {
    return { kind: "legacyMarkdown", markdown: legacy }
  }
  return null
}

/**
 * Persist the composer document for a key (v2). The host calls this only for a
 * non-empty document and {@link clearMessageInputDraftV2} otherwise. The in-memory
 * `draftDocCache` makes the v2 document win immediately; any legacy v1 text draft
 * is only removed once the v2 write is durably flushed (see the flush path), so a
 * deferred write that later fails cannot lose the draft.
 */
export function saveMessageInputDraftV2(
  draftKey: string,
  doc: JSONContent
): void {
  const key = scopedDraftKey(draftKey)
  draftDocCache.set(key, doc)
  if (typeof window === "undefined") return

  pendingPersistDocs.set(key, doc)
  scheduleDraftPersistence()
}

export function clearMessageInputDraftV2(draftKey: string): void {
  const key = scopedDraftKey(draftKey)
  draftDocCache.delete(key)
  pendingPersistDocs.delete(key)
  // Also drop any legacy v1 draft so a cleared composer stays cleared.
  clearScopedDraftV1(key)
  if (typeof window === "undefined") return

  try {
    localStorage.removeItem(storageKeyForDraftKeyV2(key))
  } catch {
    /* ignore */
  }
}
