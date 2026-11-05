import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { CONNECTION_KEEPALIVE_INTERVAL_MS } from "@/lib/constants"
import type { Transport } from "@/lib/transport/types"
import { localTab, useHallWorkspaceStore } from "@/stores/hall-workspace-store"
import {
  forgetParkedOwnerships,
  parkOwnedConnection,
  releaseParkedWorkspaceKeepalives,
  retainParkedWorkspaceConnection,
  takeParkedOwnership,
} from "./workspace-lifetime"

beforeEach(() => {
  vi.useFakeTimers()
  useHallWorkspaceStore.setState({
    enabled: true,
    tabs: [localTab("Local")],
    activeKey: "local",
  })
  useHallWorkspaceStore.getState().open(7, "Seven")
  useHallWorkspaceStore.getState().open(8, "Eight")
  useHallWorkspaceStore.getState().activate("local")
})

afterEach(() => {
  forgetParkedOwnerships()
  useHallWorkspaceStore.setState({ enabled: false })
  vi.useRealTimers()
})

function hold(backend: string, request = vi.fn(async () => true)) {
  retainParkedWorkspaceConnection(
    backend,
    "same-context",
    "same-id",
    request as unknown as Transport["call"]
  )
  return request
}

async function tick(count = 1) {
  await vi.advanceTimersByTimeAsync(CONNECTION_KEEPALIVE_INTERVAL_MS * count)
}

describe("parked workspace engine keepalive", () => {
  it("pins equal connection ids to their own backends and stops only the closed backend", async () => {
    const seven = hold("remote:7")
    const eight = hold("remote:8")
    await tick()
    expect(seven).toHaveBeenCalledWith("acp_touch_connection", {
      connectionId: "same-id",
    })
    expect(eight).toHaveBeenCalledWith("acp_touch_connection", {
      connectionId: "same-id",
    })
    forgetParkedOwnerships("remote:7")
    seven.mockClear()
    eight.mockClear()
    await tick()
    expect(seven).not.toHaveBeenCalled()
    expect(eight).toHaveBeenCalledTimes(1)
  })

  it("hands the heartbeat back to a mounted provider and releases its timer", async () => {
    const request = hold("remote:7")
    releaseParkedWorkspaceKeepalives("remote:7")
    await tick(4)
    expect(request).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
  })

  it("does not send a queued heartbeat after the workspace tab was closed", async () => {
    const request = hold("remote:7")
    useHallWorkspaceStore.getState().close("remote:7")
    await tick(3)
    expect(request).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
  })

  it("does not multiply outstanding probes during a slow request", async () => {
    let finish!: (live: boolean) => void
    const request = hold(
      "remote:7",
      vi.fn(
        () =>
          new Promise<boolean>((resolve) => {
            finish = resolve
          })
      )
    )
    await tick(5)
    expect(request).toHaveBeenCalledTimes(1)
    finish(true)
    await tick()
    expect(request).toHaveBeenCalledTimes(2)
  })

  it("ignores an old target's late answer after the same backend key is reopened", async () => {
    let finish!: (live: boolean) => void
    hold(
      "remote:7",
      vi.fn(
        () =>
          new Promise<boolean>((resolve) => {
            finish = resolve
          })
      )
    )
    await tick()
    forgetParkedOwnerships("remote:7")
    const replacement = hold("remote:7")
    parkOwnedConnection("remote:7", "same-context", "same-id")
    finish(false)
    await tick()
    expect(replacement).toHaveBeenCalledTimes(1)
    expect(takeParkedOwnership("remote:7", "same-context")).toBe("same-id")
  })

  it("stops probing a definitively gone engine without creating a new one", async () => {
    const request = hold(
      "remote:7",
      vi.fn(async () => false)
    )
    parkOwnedConnection("remote:7", "same-context", "same-id")
    await tick(4)
    expect(request).toHaveBeenCalledTimes(1)
    expect(takeParkedOwnership("remote:7", "same-context")).toBeNull()
    expect(vi.getTimerCount()).toBe(0)
  })

  it("keeps an offline engine parked and clears all timers on shell shutdown", async () => {
    const request = hold(
      "remote:7",
      vi.fn(async () => {
        throw Error("offline")
      })
    )
    await tick(3)
    expect(request).toHaveBeenCalledTimes(3)
    forgetParkedOwnerships()
    request.mockClear()
    await tick(4)
    expect(request).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
  })
})
