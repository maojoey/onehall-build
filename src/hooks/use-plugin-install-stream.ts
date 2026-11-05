import { useCallback, useRef, useState } from "react"
import { subscribe } from "@/lib/platform"
import type { PluginInstallEvent, PluginInstallEventKind } from "@/lib/types"

const PLUGIN_INSTALL_EVENT = "app://opencode-plugin-install"

export type PluginInstallStatus = "idle" | "running" | "success" | "failed"

interface PluginInstallStreamState {
  status: PluginInstallStatus
  logs: string[]
  error: string | null
}

export function usePluginInstallStream() {
  const [state, setState] = useState<PluginInstallStreamState>({
    status: "idle",
    logs: [],
    error: null,
  })
  const unsubRef = useRef<(() => void) | null>(null)

  const start = useCallback(async (taskId: string) => {
    setState({ status: "running", logs: [], error: null })

    unsubRef.current?.()

    const unsub = await subscribe<PluginInstallEvent>(
      PLUGIN_INSTALL_EVENT,
      (event) => {
        if (event.task_id !== taskId) return

        switch (event.kind as PluginInstallEventKind) {
          case "started":
            setState((prev) => ({ ...prev, status: "running" }))
            break
          case "log":
            setState((prev) => ({
              ...prev,
              logs: [...prev.logs, event.payload],
            }))
            break
          case "completed":
            setState((prev) => ({
              ...prev,
              status: "success",
              logs: [...prev.logs, event.payload],
            }))
            unsubRef.current?.()
            break
          case "failed":
            setState((prev) => ({
              ...prev,
              status: "failed",
              error: event.payload,
              logs: [...prev.logs, `ERROR: ${event.payload}`],
            }))
            unsubRef.current?.()
            break
        }
      }
    )

    unsubRef.current = unsub
  }, [])

  const reset = useCallback(() => {
    unsubRef.current?.()
    unsubRef.current = null
    setState({ status: "idle", logs: [], error: null })
  }, [])

  return { ...state, start, reset }
}
