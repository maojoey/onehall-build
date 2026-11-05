"use client"

import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { AnimatePresence, motion } from "motion/react"
import { Inbox, Pin, X } from "lucide-react"
import {
  closePetPanel,
  getPetPanelPinned,
  resizePetPanel,
  setPetPanelPinned,
} from "@/lib/pet/api"
import { getShellTransport, isDesktop } from "@/lib/transport"
import { usePetSessions } from "../../pet/_hooks/usePetSessions"
import { sessionSortRank } from "@/lib/pet/session-display"
import { SessionRow } from "./SessionRow"
import {
  petRemoteConnectionId,
  reconnectPetLink,
  usePetLinkState,
} from "@/lib/pet/remote"
import { WifiOff } from "lucide-react"
import { PetChat } from "./PetChat"

// Cap the scrollable list so a long session list doesn't grow the window past a
// sane height; beyond this the list scrolls. The window itself is sized to fit
// content (header + this list), so few/no sessions leave no dead space.
const LIST_MAX_HEIGHT_PX = 320

export function PetPanel({
  chat = false,
  remoteConnectionId,
}: {
  chat?: boolean
  remoteConnectionId?: number | null
}) {
  const t = useTranslations("Pet")
  const backendReady =
    remoteConnectionId === undefined ||
    remoteConnectionId === petRemoteConnectionId()
  const { sessions } = usePetSessions(backendReady)
  const wrapperRef = useRef<HTMLDivElement>(null)
  // onehall: link to the bound remote server ("local" when unbound).
  const link = usePetLinkState()
  const [chatRequested, setChatRequested] = useState(chat)
  // Native windows start pinned before remote permission/draft requests run.
  const [pinned, setPinned] = useState(true)
  const [pinBusy, setPinBusy] = useState(false)
  const [pinError, setPinError] = useState(false)
  const pinRevision = useRef(0)

  useEffect(() => {
    if (!isDesktop()) return
    let disposed = false
    void getPetPanelPinned()
      .then((value) => {
        if (!disposed && pinRevision.current === 0) setPinned(value)
      })
      .catch(() => {
        if (!disposed && pinRevision.current === 0) setPinError(true)
      })
    return () => {
      disposed = true
    }
  }, [])

  const togglePin = async () => {
    if (pinBusy) return
    pinRevision.current += 1
    setPinBusy(true)
    setPinError(false)
    try {
      await setPetPanelPinned(!pinned)
      setPinned(!pinned)
    } catch {
      setPinError(true)
    } finally {
      setPinBusy(false)
    }
  }

  useEffect(() => {
    if (!isDesktop()) return
    let disposed = false
    const listener = getShellTransport()
      .subscribe("pet://focus-chat", () => {
        if (!disposed) setChatRequested(true)
      })
      .catch(() => () => {})
    return () => {
      disposed = true
      void listener.then((off) => off())
    }
  }, [])

  const sorted = useMemo(
    () => [...sessions].sort((a, b) => sessionSortRank(a) - sessionSortRank(b)),
    [sessions]
  )

  // Explicit dismissal always works, including when the panel is pinned.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        void closePetPanel().catch(() => {})
      }
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [])

  // Fit the OS window to the rendered content. The wrapper sizes to its content
  // (no forced fill), so its box height — including the `p-2` chrome — is the
  // height the window should be. A ResizeObserver catches every content change
  // (sessions loading, a permission card expanding/collapsing), rAF-coalesced
  // and integer-deduped so a burst nets one resize and we never re-fire on our
  // own resize (which only changes window height, not the content's). 1 CSS px
  // == 1 logical px, exactly what the backend `set_size` expects — so we send
  // the measured value directly, with NO devicePixelRatio multiply.
  useEffect(() => {
    if (!isDesktop()) return
    const el = wrapperRef.current
    if (!el) return

    let raf = 0
    let lastSent = -1

    const measure = () => {
      raf = 0
      const h = Math.ceil(el.getBoundingClientRect().height)
      if (h <= 0 || h === lastSent) return
      lastSent = h
      void resizePetPanel(h).catch(() => {})
    }
    const schedule = () => {
      if (raf) return
      raf = requestAnimationFrame(measure)
    }

    schedule()
    const ro = new ResizeObserver(schedule)
    ro.observe(el)

    return () => {
      ro.disconnect()
      if (raf) cancelAnimationFrame(raf)
    }
  }, [])

  return (
    <div
      ref={wrapperRef}
      className="flex w-screen flex-col border border-border"
    >
      <header className="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
        <div className="min-w-0 flex-1 truncate text-sm font-semibold">
          {chatRequested ? t("chat.title") : t("panel.title")}
          {backendReady && !chatRequested && sorted.length > 0 ? (
            <span className="ml-1 font-normal text-muted-foreground">
              ({sorted.length})
            </span>
          ) : null}
        </div>
        <button
          type="button"
          aria-label={t("panel.pin")}
          aria-pressed={pinned}
          disabled={pinBusy}
          title={pinned ? t("panel.unpin") : t("panel.pinHint")}
          onClick={() => void togglePin()}
          className="flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent aria-pressed:bg-accent aria-pressed:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
        >
          <Pin className="h-3.5 w-3.5" aria-hidden />
          {pinned ? t("panel.pinned") : t("panel.pin")}
        </button>
        <button
          type="button"
          aria-label={t("menu.close")}
          onClick={() => void closePetPanel().catch(() => {})}
          className="rounded p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </header>

      {pinError ? (
        <p role="alert" className="px-3 py-1.5 text-xs text-destructive">
          {t("panel.pinFailed")}
        </p>
      ) : null}

      {link === "disconnected" || link === "reconnecting" ? (
        <div className="flex items-center gap-2 border-b border-border bg-muted/50 px-3 py-1.5 text-xs text-muted-foreground">
          <WifiOff className="h-3.5 w-3.5 shrink-0" aria-hidden />
          <span className="min-w-0 flex-1 truncate">
            {link === "disconnected" ? t("panel.linkDown") : t("panel.offline")}
          </span>
          {link === "disconnected" ? (
            <button
              type="button"
              onClick={() => reconnectPetLink()}
              className="shrink-0 rounded px-1.5 py-0.5 text-foreground hover:bg-accent"
            >
              {t("panel.reconnect")}
            </button>
          ) : null}
        </div>
      ) : null}

      {!backendReady ? (
        <p role="alert" className="p-3 text-xs text-destructive">
          {t("chat.unavailable")}
        </p>
      ) : chatRequested ? (
        <PetChat remoteConnectionId={remoteConnectionId} />
      ) : sorted.length === 0 ? (
        <div className="flex flex-col items-center justify-center gap-2 px-6 py-8 text-center">
          <Inbox className="h-8 w-8 text-muted-foreground/50" />
          <div className="text-sm font-medium">{t("panel.empty")}</div>
          <div className="text-xs text-muted-foreground">
            {t("panel.emptyHint")}
          </div>
        </div>
      ) : (
        <ul
          className="overflow-y-auto py-1"
          style={{ maxHeight: LIST_MAX_HEIGHT_PX }}
        >
          <AnimatePresence initial={false}>
            {sorted.map((session) => (
              <motion.li
                key={session.connectionId}
                layout
                exit={{ opacity: 0, height: 0 }}
                transition={{ duration: 0.2, ease: "easeOut" }}
                style={{ overflow: "hidden" }}
              >
                <SessionRow session={session} />
              </motion.li>
            ))}
          </AnimatePresence>
        </ul>
      )}
    </div>
  )
}
