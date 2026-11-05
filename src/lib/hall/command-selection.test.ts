import { beforeEach, describe, expect, it, vi } from "vitest"

const backend = vi.hoisted(() => ({ id: null as number | null }))
vi.mock("@/lib/transport", () => ({
  getActiveRemoteConnectionId: () => backend.id,
  getActiveBackendKey: () =>
    backend.id === null ? "local" : `remote:${backend.id}`,
}))

import { loadCommandSelection, saveCommandSelection } from "./command-selection"

beforeEach(() => {
  localStorage.clear()
  backend.id = null
})

describe("OneHall command selection", () => {
  it("keeps the local legacy selection and two remote selections apart", () => {
    localStorage.setItem("lastCmd:7", "3")
    expect(loadCommandSelection(7)).toBe(3)
    backend.id = 10
    expect(loadCommandSelection(7)).toBeNull()
    saveCommandSelection(7, -1)
    backend.id = 11
    expect(loadCommandSelection(7)).toBeNull()
    saveCommandSelection(7, 9)
    backend.id = 10
    expect(loadCommandSelection(7)).toBe(-1)
    expect(loadCommandSelection(8)).toBeNull()
    backend.id = null
    expect(loadCommandSelection(7)).toBe(3)
    backend.id = 11
    expect(loadCommandSelection(7)).toBe(9)
  })

  it("ignores a malformed stored selection", () => {
    localStorage.setItem("lastCmd:7", "not-an-id")
    expect(loadCommandSelection(7)).toBeNull()
  })
})
