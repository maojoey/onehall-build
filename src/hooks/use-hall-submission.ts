import { useCallback, useSyncExternalStore } from "react"
import {
  getHallSubmission,
  hallSubmissionBackendKey,
  subscribeHallSubmissions,
} from "@/lib/hall/submissions"

/** Each mounted workspace reads only its own submitted writes. */
export function useHallSubmission(surfaceKey: string) {
  const backendKey = hallSubmissionBackendKey()
  const snapshot = useCallback(
    () => getHallSubmission(backendKey, surfaceKey),
    [backendKey, surfaceKey]
  )
  return useSyncExternalStore(subscribeHallSubmissions, snapshot, () => null)
}
