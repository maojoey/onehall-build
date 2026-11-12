"use client"

// onehall: with "single window" off the bottom workspace tabs (and their
// Branches button) do not exist, so a machine opened in its own window gets the
// branch directory here instead: link machines, send them tasks, compare agent
// versions. Single-window mode keeps its button in the tab bar.

import { useEffect, useState } from "react"

import { HallBranchDirectory } from "@/components/layout/hall-branch-directory"
import { useRemoteConnection } from "@/contexts/remote-connection-context"
import { hallGetSettings } from "@/lib/hall/api"
import { isDesktop } from "@/lib/platform"

export function StatusBarBranches() {
  const remote = useRemoteConnection()
  const [multiWindow, setMultiWindow] = useState(false)

  useEffect(() => {
    if (!isDesktop()) return
    let cancelled = false
    hallGetSettings()
      .then((settings) => {
        if (!cancelled) setMultiWindow(!settings.singleWindow)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  const id = remote?.connection?.id ?? null
  if (!multiWindow || id === null) return null
  return (
    <HallBranchDirectory
      headquartersConnectionId={id}
      web={false}
      multiWindow
    />
  )
}
