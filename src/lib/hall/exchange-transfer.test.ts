import { describe, expect, it, vi } from "vitest"
import sample from "../../../src-tauri/src/hall/exchange/contract_samples/desktop_transfer.json"
import envelope from "../../../src-tauri/src/hall/exchange/contract_samples/envelope_v2.json"
import report from "../../../src-tauri/src/hall/exchange/contract_samples/report_v2.json"
import { isExchangeManifest, type ArtifactRef } from "./exchange"
import {
  browserTransfer,
  desktopTransfer,
  type HostDialogs,
  type PickedFile,
} from "./exchange-transfer"

const reply = sample.inspectReply
const artifact = report.artifacts[0] as unknown as ArtifactRef
const receipt = {
  objectId: "44444444-4444-4444-8444-444444444444",
  displayName: reply.displayName,
  size: reply.size,
  sha256: reply.sha256,
}
const chosen: PickedFile = {
  name: reply.displayName,
  size: reply.size,
  sha256: reply.sha256,
  source: { path: "/Users/op/in.json" },
}
function host(result: unknown = receipt) {
  const invoke = vi.fn(async (command: string) =>
    command === "hall_exchange_inspect_file" ? reply : result
  )
  const dialogs: HostDialogs = {
    pickFiles: vi.fn(async () => ["/Users/op/in.json"]),
    saveFile: vi.fn(async () => "/Users/op/out.txt"),
  }
  return { invoke, dialogs }
}
/** Argument names must be exactly the published command signatures. */
function expectSignature(call: unknown[]) {
  const [command, args] = call as [string, Record<string, unknown>]
  const names = (sample.commands as Record<string, string[]>)[command]
  expect(names, command).toBeDefined()
  expect(Object.keys(args).sort()).toEqual([...names].sort())
}

describe("desktop byte transfer", () => {
  it("lets the host inspect chosen paths; the page never computes the hash", async () => {
    const { invoke, dialogs } = host()
    const picked = await desktopTransfer(null, invoke as never, dialogs).pick!()
    expect(picked).toEqual([chosen])
    expect(invoke).toHaveBeenCalledWith("hall_exchange_inspect_file", {
      path: "/Users/op/in.json",
    })
    invoke.mock.calls.forEach(expectSignature)
  })

  it("rejects a malformed inspection instead of guessing size or hash", async () => {
    const { dialogs } = host()
    const invoke = vi.fn(async () => ({ displayName: "in.json", size: -1 }))
    await expect(
      desktopTransfer(null, invoke as never, dialogs).pick!()
    ).rejects.toThrow("Malformed")
  })

  it("uploads to the local HQ by path and to a remote HQ by connection id only", async () => {
    const local = host()
    await desktopTransfer(null, local.invoke as never, local.dialogs).upload!(
      "tr",
      chosen
    )
    expect(local.invoke).toHaveBeenCalledWith("hall_exchange_upload_file", {
      transferId: "tr",
      path: "/Users/op/in.json",
    })
    const remote = host()
    await desktopTransfer(7, remote.invoke as never, remote.dialogs).upload!(
      "tr",
      chosen
    )
    expect(remote.invoke).toHaveBeenCalledWith(
      "hall_exchange_remote_upload_file",
      { connectionId: 7, transferId: "tr", path: "/Users/op/in.json" }
    )
    // No token, base URL or header ever travels through the page.
    for (const call of [
      ...local.invoke.mock.calls,
      ...remote.invoke.mock.calls,
    ])
      expectSignature(call)
  })

  it("refuses a receipt for different bytes than were announced", async () => {
    const { invoke, dialogs } = host({ ...receipt, sha256: "f".repeat(64) })
    await expect(
      desktopTransfer(7, invoke as never, dialogs).upload!("tr", chosen)
    ).rejects.toThrow("Malformed upload receipt")
  })

  it("saves only to a new operator-chosen file, with the recorded size and hash for a remote HQ", async () => {
    const local = host(null)
    await desktopTransfer(null, local.invoke as never, local.dialogs).download(
      artifact
    )
    expect(local.dialogs.saveFile).toHaveBeenCalledWith(artifact.displayName)
    expect(local.invoke).toHaveBeenCalledWith("hall_exchange_save_artifact", {
      objectId: artifact.objectId,
      destination: "/Users/op/out.txt",
    })
    const remote = host(null)
    await desktopTransfer(3, remote.invoke as never, remote.dialogs).download(
      artifact
    )
    expect(remote.invoke).toHaveBeenCalledWith(
      "hall_exchange_remote_save_artifact",
      {
        connectionId: 3,
        objectId: artifact.objectId,
        size: artifact.size,
        sha256: artifact.sha256,
        destination: "/Users/op/out.txt",
      }
    )
    for (const call of [
      ...local.invoke.mock.calls,
      ...remote.invoke.mock.calls,
    ])
      expectSignature(call)
  })

  it("does nothing when the save dialog is cancelled", async () => {
    const { invoke, dialogs } = host(null)
    dialogs.saveFile = vi.fn(async () => null)
    await desktopTransfer(3, invoke as never, dialogs).download(artifact)
    expect(invoke).not.toHaveBeenCalled()
  })

  it("never mixes a browser Blob into the desktop path channel or vice versa", async () => {
    const { invoke, dialogs } = host()
    await expect(
      desktopTransfer(null, invoke as never, dialogs).upload!("tr", {
        ...chosen,
        source: new Blob(["x"]),
      })
    ).rejects.toThrow()
    await expect(browserTransfer().upload!("tr", chosen)).rejects.toThrow()
    expect(invoke).not.toHaveBeenCalled()
  })
})

describe("manifest display check", () => {
  it("accepts the published v2 manifest shape and rejects partial ones", () => {
    expect(isExchangeManifest(envelope.manifest)).toBe(true)
    expect(isExchangeManifest({ ...envelope.manifest, inputs: [{}] })).toBe(
      false
    )
    expect(isExchangeManifest({ ...envelope.manifest, schemaVersion: 2 })).toBe(
      false
    )
    expect(isExchangeManifest(null)).toBe(false)
  })
})
