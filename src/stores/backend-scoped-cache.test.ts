import { afterEach, expect, it } from "vitest"
import { useAppWorkspaceStore } from "./app-workspace-store"
import { useAssistantsStore } from "./assistants-store"
import {
  forgetBackendScopedStores,
  parkBackendScopedStores,
  resetBackendScopedStores,
  restoreBackendScopedStores,
} from "./backend-scoped-store-reset"
import type { AssistantInfo, FolderDetail } from "@/lib/types"

afterEach(() => {
  forgetBackendScopedStores()
  resetBackendScopedStores()
})

it("keeps real folder and assistant caches separated for equal IDs", () => {
  const set = (name: string) => {
    useAppWorkspaceStore.setState({
      folders: [{ id: 1, path: name } as FolderDetail],
    })
    useAssistantsStore.setState({
      byId: new Map([[1, { id: 1, name } as AssistantInfo]]),
    })
  }
  set("local")
  parkBackendScopedStores("local")
  resetBackendScopedStores()
  set("remote")
  parkBackendScopedStores("remote:7")
  resetBackendScopedStores()
  restoreBackendScopedStores("local")
  expect(useAppWorkspaceStore.getState().folders[0].path).toBe("local")
  expect(useAssistantsStore.getState().byId.get(1)?.name).toBe("local")
  resetBackendScopedStores()
  restoreBackendScopedStores("remote:7")
  expect(useAppWorkspaceStore.getState().folders[0].path).toBe("remote")
  expect(useAssistantsStore.getState().byId.get(1)?.name).toBe("remote")
})
