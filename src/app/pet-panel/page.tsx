"use client"

import { Suspense } from "react"
import { useSearchParams } from "next/navigation"
import { PetPanel } from "./_components/PetPanel"
// onehall: the panel follows the sprite's remote binding (same URL params).
import { RemoteConnectionGate } from "@/contexts/remote-connection-context"

// Route loaded inside the desktop-only `pet-panel` window (opened by tapping
// the sprite). Renders the live active-session list from the `pet://sessions`
// stream of whichever server the pet is bound to.
export default function PetPanelPage() {
  return (
    <Suspense fallback={null}>
      <RemoteConnectionGate variant="pet">
        <Panel />
      </RemoteConnectionGate>
    </Suspense>
  )
}

function Panel() {
  const params = useSearchParams()
  const rawRemoteId = params.get("remoteConnectionId")
  return (
    <PetPanel
      chat={params.get("chat") === "1"}
      remoteConnectionId={rawRemoteId === null ? null : Number(rawRemoteId)}
    />
  )
}
