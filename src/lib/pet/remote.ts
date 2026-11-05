"use client"

// onehall: the pet windows can be bound to a remote workspace connection (see
// `PetWindowConfig.remoteConnectionId` and `docs/onehall/phase1-plan.md`). When
// they are, their transport is a `RemoteDesktopTransport`, which knows whether
// the link to the server is up. This module exposes that as the pet's fifth
// ambient state ("offline") without touching the sprite-sheet state machine.

import { useSyncExternalStore } from "react"
import {
  getActiveRemoteConnectionId,
  getTransport,
  isRemoteDesktopMode,
} from "@/lib/transport"

/** `local` = not bound to a remote server (upstream behaviour). */
export type PetLinkState =
  | "local"
  | "connected"
  | "reconnecting"
  | "disconnected"

type LinkTransport = {
  getConnectionSnapshot(): "connected" | "reconnecting" | "disconnected"
  subscribeConnection(callback: () => void): () => void
  reconnectNow(): void
}

// Set by the pet variant of `RemoteConnectionGate` when the bound connection
// could not be loaded at all (record gone, token refused): there is no
// transport to ask, but the pet must still show as offline rather than idle.
let linkOverride: PetLinkState | null = null
const overrideListeners = new Set<() => void>()

export function setPetLinkOverride(next: PetLinkState | null): void {
  if (linkOverride === next) return
  linkOverride = next
  for (const cb of overrideListeners) cb()
}

function linkTransport(): LinkTransport | null {
  if (!isRemoteDesktopMode()) return null
  const candidate = getTransport() as unknown as Partial<LinkTransport>
  if (
    typeof candidate.getConnectionSnapshot !== "function" ||
    typeof candidate.subscribeConnection !== "function"
  ) {
    return null
  }
  return candidate as LinkTransport
}

function subscribeLink(callback: () => void): () => void {
  overrideListeners.add(callback)
  const offTransport = linkTransport()?.subscribeConnection(callback)
  return () => {
    overrideListeners.delete(callback)
    offTransport?.()
  }
}

function readLink(): PetLinkState {
  if (linkOverride) return linkOverride
  return linkTransport()?.getConnectionSnapshot() ?? "local"
}

function readLinkOnServer(): PetLinkState {
  return "local"
}

/** Live state of the pet's link to its server. Stable `"local"` when unbound. */
export function usePetLinkState(): PetLinkState {
  return useSyncExternalStore(subscribeLink, readLink, readLinkOnServer)
}

/** Kick a dropped remote link; no-op when unbound or not disconnected. */
export function reconnectPetLink(): void {
  linkTransport()?.reconnectNow()
}

/** The remote workspace connection this pet window is bound to, if any. */
export function petRemoteConnectionId(): number | null {
  return getActiveRemoteConnectionId()
}
