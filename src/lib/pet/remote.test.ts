import { describe, expect, it, vi, beforeEach } from "vitest"
import { renderHook, act } from "@testing-library/react"

const transportMock = vi.hoisted(() => ({
  remote: false,
  state: "connected" as "connected" | "reconnecting" | "disconnected",
  listeners: new Set<() => void>(),
  reconnectNow: vi.fn(),
  activeId: null as number | null,
}))

vi.mock("@/lib/transport", () => ({
  isRemoteDesktopMode: () => transportMock.remote,
  getActiveRemoteConnectionId: () => transportMock.activeId,
  getTransport: () => ({
    getConnectionSnapshot: () => transportMock.state,
    subscribeConnection: (cb: () => void) => {
      transportMock.listeners.add(cb)
      return () => transportMock.listeners.delete(cb)
    },
    reconnectNow: transportMock.reconnectNow,
  }),
}))

import {
  petRemoteConnectionId,
  reconnectPetLink,
  usePetLinkState,
} from "./remote"

describe("pet remote link", () => {
  beforeEach(() => {
    transportMock.remote = false
    transportMock.state = "connected"
    transportMock.listeners.clear()
    transportMock.reconnectNow.mockClear()
    transportMock.activeId = null
  })

  it("reports local when the pet is not bound to a remote server", () => {
    const { result } = renderHook(() => usePetLinkState())
    expect(result.current).toBe("local")
    expect(petRemoteConnectionId()).toBeNull()
    reconnectPetLink()
    expect(transportMock.reconnectNow).not.toHaveBeenCalled()
  })

  it("follows the remote transport's connection state", () => {
    transportMock.remote = true
    transportMock.activeId = 7
    const { result } = renderHook(() => usePetLinkState())
    expect(result.current).toBe("connected")
    expect(petRemoteConnectionId()).toBe(7)

    act(() => {
      transportMock.state = "disconnected"
      for (const cb of transportMock.listeners) cb()
    })
    expect(result.current).toBe("disconnected")

    reconnectPetLink()
    expect(transportMock.reconnectNow).toHaveBeenCalledTimes(1)
  })
})
