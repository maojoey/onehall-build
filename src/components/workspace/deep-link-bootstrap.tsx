"use client"

import { useCallback, useEffect, useRef } from "react"
import {
  LOCAL_BACKEND_KEY,
  backendKeyFor,
  useHallWorkspaceStore,
} from "@/stores/hall-workspace-store" // onehall
import { toast } from "sonner"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useTabStore, useTabActions } from "@/contexts/tab-context"
import { takePendingDeepLink } from "@/lib/deep-link"
import type { AgentType } from "@/lib/types"

/**
 * Handles `/workspace?folderId=X&conversationId=Y&agent=Z` URLs. Runs once,
 * after the workspace has loaded everything the link is checked against.
 *
 * On Windows and Linux this is also where a cold-start `codeg://session/<id>`
 * arrives: the URL comes in on argv, so the backend can resolve it and point
 * the window straight at this query string before the webview exists. (macOS
 * learns about the link too late for that and uses {@link PetFocusBridge}'s
 * parked-target drain instead.)
 */
export function DeepLinkBootstrap() {
  const foldersHydrated = useAppWorkspaceStore((s) => s.foldersHydrated)
  const tabsHydrated = useTabStore((s) => s.tabsHydrated)
  // The conversation check below is the only thing that can reject a link, and
  // `conversations` loads on its own request, fired in parallel with the folder
  // one (see `AppWorkspaceProvider`). Waiting for folders and tabs alone would
  // let that request lose the race and turn a perfectly good link into a
  // permanent "not found" — the URL is cleared on the way out, so there is no
  // second chance. `conversationsLoading` starts true, so this also covers the
  // very first load, not just refreshes.
  const conversationsLoading = useAppWorkspaceStore(
    (s) => s.conversationsLoading
  )
  const { openTab } = useTabActions()
  const ranRef = useRef(false)

  useEffect(() => {
    if (ranRef.current) return
    if (!foldersHydrated || !tabsHydrated || conversationsLoading) return
    ranRef.current = true

    if (typeof window === "undefined") return

    const params = new URLSearchParams(window.location.search)
    const rawFolderId = params.get("folderId")
    const rawConversationId = params.get("conversationId")
    const rawAgent = params.get("agent") as AgentType | null

    if (!rawFolderId && !rawConversationId) return

    const clearUrl = () => {
      try {
        window.history.replaceState({}, "", "/workspace")
      } catch {
        /* ignore */
      }
    }

    void (async () => {
      try {
        const folderId = rawFolderId ? Number(rawFolderId) : null
        const conversationId = rawConversationId
          ? Number(rawConversationId)
          : null

        if (folderId == null || !Number.isFinite(folderId)) return
        if (conversationId == null || !Number.isFinite(conversationId)) return
        if (!rawAgent) return

        // Read at run time: this effect fires once when hydration completes,
        // and getState() sees exactly the lists as of that moment.
        const { folders, addFolderToWorkspaceById } =
          useAppWorkspaceStore.getState()

        let folder = folders.find((f) => f.id === folderId)
        if (!folder) {
          try {
            folder = await addFolderToWorkspaceById(folderId)
          } catch (err) {
            console.error("[DeepLinkBootstrap] open folder failed:", err)
            toast.error("Unable to open linked folder")
            return
          }
        }

        // Re-read after the await: opening a folder refreshes the workspace,
        // and a snapshot taken before it is stale by definition.
        const hasConv = useAppWorkspaceStore
          .getState()
          .conversations.some(
            (c) =>
              c.id === conversationId &&
              c.folder_id === folderId &&
              c.agent_type === rawAgent
          )
        if (!hasConv) {
          toast.error("Linked conversation not found")
          return
        }

        openTab(folderId, conversationId, rawAgent, true)
      } finally {
        clearUrl()
      }
    })()
  }, [foldersHydrated, tabsHydrated, conversationsLoading, openTab])

  return null
}

type FocusRequest = {
  folderId: number
  conversationId: number
  agent: AgentType
}

/**
 * Live counterpart to {@link DeepLinkBootstrap}: listens for the pet panel's
 * `workspace://focus-conversation` request (emitted by the `focus_conversation`
 * command after bringing the main window forward) and opens the conversation
 * via `openTab` — no URL reload, so in-memory tab/session state survives.
 *
 * Also where an OS `codeg://session/<id>` deep link lands. That one does NOT
 * travel in an event payload: Tauri delivers an event only to webviews that
 * already registered a listener, so the emit for a cold-start link is dropped
 * on the floor. The backend parks the resolved target and sends a payload-less
 * `workspace://deep-link-pending` nudge instead; `takePendingDeepLink` is an
 * atomic take, so the mount drain and every nudge-driven drain compete for one
 * slot and exactly one of them opens the tab. Nothing can be replayed onto a
 * later mount, and nothing opens twice.
 *
 * Latest workspace state is held in a ref so the single subscription always
 * sees fresh state without re-subscribing on every change. A request that
 * arrives before folders/tabs hydrate is queued and replayed.
 */
export function PetFocusBridge() {
  const foldersHydrated = useAppWorkspaceStore((s) => s.foldersHydrated)
  const tabsHydrated = useTabStore((s) => s.tabsHydrated)
  const { openTab } = useTabActions()

  // Workspace state is read via getState() at attempt time; only the tab
  // half still needs a ref mirror (it lives in a context, not a store).
  const stateRef = useRef({ tabsHydrated, openTab })
  useEffect(() => {
    stateRef.current = { tabsHydrated, openTab }
  }, [tabsHydrated, openTab])

  // Focus requests waiting for the workspace to hydrate. Both producers are
  // one-shot — a pet-panel click is an event with no replay, and a drained
  // deep link has already been taken out of the backend slot — so a single
  // slot here would let whichever arrives second erase the first with no way
  // to get it back. Queue them; the last one still ends up focused.
  const pendingRef = useRef<FocusRequest[]>([])

  // Tail of the batches already running. Opening a tab can await a folder
  // load, so without this a request that arrives during that await would be
  // opened by its own batch first and leave the *earlier* conversation
  // focused. Chaining keeps batches strictly first-in-first-out.
  const runningRef = useRef<Promise<void>>(Promise.resolve())

  const attempt = useCallback(() => {
    if (pendingRef.current.length === 0) return
    if (
      !useAppWorkspaceStore.getState().foldersHydrated ||
      !stateRef.current.tabsHydrated
    ) {
      return // wait for hydration
    }
    // Take the whole queue before the async work (mirrors DeepLinkBootstrap)
    // so a later state change can't replay what is already being opened.
    const requests = pendingRef.current
    pendingRef.current = []
    runningRef.current = runningRef.current
      .then(async () => {
        for (const req of requests) {
          // Ensure the folder is in the workspace so the tab has a home. Read
          // the store per request: an earlier one may have just added it.
          const workspace = useAppWorkspaceStore.getState()
          if (!workspace.folders.some((f) => f.id === req.folderId)) {
            try {
              await workspace.addFolderToWorkspaceById(req.folderId)
            } catch (err) {
              console.error("[PetFocusBridge] open folder failed:", err)
              continue
            }
          }
          // Both producers name a live session, so the conversation exists;
          // open the tab directly and let its title/content hydrate. We do NOT
          // gate on the conversations list — it loads independently of folders,
          // and waiting on it (without a ready flag) would drop the request.
          stateRef.current.openTab(
            req.folderId,
            req.conversationId,
            req.agent,
            true
          )
        }
      })
      // Never leave the chain rejected: every later batch hangs off it.
      .catch((err) => {
        console.error("[PetFocusBridge] focus batch failed:", err)
      })
  }, [])

  // Replay queued requests once hydration flips ready.
  useEffect(() => {
    attempt()
  }, [foldersHydrated, tabsHydrated, attempt])

  useEffect(() => {
    const disposers: Array<() => void> = []
    let cancelled = false

    // Claim the parked deep link, if this call is the one that gets it.
    const drainDeepLink = async () => {
      const parked = await takePendingDeepLink()
      // Nothing to re-park on unmount: the workspace is tearing down, so
      // there is no tab left to open. Dropping it beats resurrecting it.
      if (cancelled || !parked) return
      pendingRef.current.push(parked)
      attempt()
    }

    void (async () => {
      try {
        const { getTransport } = await import("@/lib/transport")
        const transport = getTransport()

        // onehall: every window with an `Any`-target listener receives every
        // `focus_conversation` emit, whichever label it was addressed to; the
        // payload's `remoteConnectionId` (null = local) says whose ids these
        // are, and a window keeps only requests for its own backend.
        const { getActiveRemoteConnectionId, isRemoteDesktopMode } =
          await import("@/lib/transport")
        const acceptFocus = (payload: {
          folderId?: number
          conversationId?: number
          agent?: string
          remoteConnectionId?: number | null
          name?: string
        }) => {
          const folderId = Number(payload?.folderId)
          const conversationId = Number(payload?.conversationId)
          const agent = payload?.agent as AgentType | undefined
          if (
            !Number.isFinite(folderId) ||
            !Number.isFinite(conversationId) ||
            !agent
          ) {
            return
          }
          const forBackend = payload?.remoteConnectionId ?? null
          if (forBackend !== getActiveRemoteConnectionId()) {
            // onehall: single-window mode — the request is for another tab.
            // Park it for that backend's realm and switch to the tab; the
            // realm takes it on mount (below). Other windows just ignore it.
            const hall = useHallWorkspaceStore.getState()
            if (hall.enabled) {
              const key = backendKeyFor(forBackend)
              hall.setPendingFocus({
                backendKey: key,
                folderId,
                conversationId,
                agent,
              })
              if (forBackend === null) hall.activate(LOCAL_BACKEND_KEY)
              else {
                void import("@/lib/remote-workspace")
                  .then(({ listRemoteWorkspaceConnections }) =>
                    listRemoteWorkspaceConnections()
                  )
                  .then((list) =>
                    hall.open(
                      forBackend,
                      list.find((c) => c.id === forBackend)?.name ??
                        String(payload?.name ?? `#${forBackend}`)
                    )
                  )
                  .catch(() =>
                    hall.open(
                      forBackend,
                      String(payload?.name ?? `#${forBackend}`)
                    )
                  )
              }
            }
            return
          }
          pendingRef.current.push({ folderId, conversationId, agent })
          attempt()
        }

        // onehall: a request parked while this backend's tab was inactive.
        {
          const hall = useHallWorkspaceStore.getState()
          const parked = hall.enabled
            ? hall.takePendingFocus(
                backendKeyFor(getActiveRemoteConnectionId())
              )
            : null
          if (parked && !cancelled) {
            pendingRef.current.push({
              folderId: parked.folderId,
              conversationId: parked.conversationId,
              agent: parked.agent as AgentType,
            })
            attempt()
          }
        }

        const offFocus = await transport.subscribe<{
          folderId?: number
          conversationId?: number
          agent?: string
          remoteConnectionId?: number | null
        }>("workspace://focus-conversation", acceptFocus)
        if (cancelled) offFocus()
        else disposers.push(offFocus)

        const offPending = await transport.subscribe(
          "workspace://deep-link-pending",
          () => {
            void drainDeepLink()
          }
        )
        if (cancelled) offPending()
        else disposers.push(offPending)

        // onehall: in a remote-workspace window the transport above carries
        // only the REMOTE server's events. The pet's `focus_conversation` is a
        // LOCAL Tauri emit to this window, so also listen on the Tauri event
        // bridge — and first take any request parked while this window was
        // still being created (an emit to a window with no listener is lost).
        // Kept in its own try so a failure here never takes the
        // `deep-link-pending` subscription down with it.
        if (isRemoteDesktopMode()) {
          try {
            const { listen } = await import("@tauri-apps/api/event")
            const { invoke } = await import("@tauri-apps/api/core")
            const offLocal = await listen<Parameters<typeof acceptFocus>[0]>(
              "workspace://focus-conversation",
              (event) => acceptFocus(event.payload)
            )
            if (cancelled) offLocal()
            else disposers.push(offLocal)
            const parked = await invoke<
              Parameters<typeof acceptFocus>[0] | null
            >("take_pending_pet_focus")
            if (!cancelled && parked) acceptFocus(parked)
          } catch (err) {
            console.warn("[PetFocusBridge] local focus listener failed:", err)
          }
        }
      } catch (err) {
        console.warn("[PetFocusBridge] subscription failed:", err)
      }

      // Last, so a link resolved while the subscriptions were still being set
      // up cannot fall between the two: whatever the dropped nudge parked is
      // still sitting in the slot for this call to claim.
      await drainDeepLink()
    })()

    return () => {
      cancelled = true
      for (const off of disposers) off()
    }
  }, [attempt])

  return null
}
