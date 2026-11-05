import { describe, expect, it, vi } from "vitest"
import samples from "../../../src-tauri/src/hall/exchange/contract_samples/hall_exchange_requests.json"
import report from "../../../src-tauri/src/hall/exchange/contract_samples/report_v2.json"
import { createExchangeClient, isArtifactRef } from "./exchange"

describe("hall_exchange client", () => {
  it("sends exactly the published request shapes", async () => {
    const sent: unknown[] = []
    const client = createExchangeClient((async (request: unknown) => {
      sent.push(request)
      return []
    }) as never)
    const [a, b, c, d] = samples as unknown as Record<string, never>[]
    await client.prepareInput(a.submissionId, a.displayName, a.size, a.sha256)
    await client.confirmInput(b.submissionId, b.objectId)
    await client.prepareManifest(
      c.submissionId,
      c.inputObjectIds as unknown as string[],
      c.outputNames as unknown as string[]
    )
    await client.artifacts(d.dispatchId)
    expect(sent).toEqual(samples)
  })

  it("accepts the published artifact and rejects missing fields", async () => {
    const [artifact] = report.artifacts
    expect(isArtifactRef(artifact)).toBe(true)
    const { exitVerified: _omit, ...partial } = artifact
    void _omit
    expect(isArtifactRef(partial)).toBe(false)
    const call = vi.fn().mockResolvedValue([partial])
    await expect(
      createExchangeClient(call as never).artifacts("d")
    ).rejects.toThrow("Malformed")
  })
})
