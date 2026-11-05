import { describe, expect, it, vi } from "vitest"
import {
  attachmentKey,
  checkAttachments,
  hashFile,
  pickBrowserFile,
  stageAttachments,
  type PickedFile,
} from "./dispatch-attachments"

const file = (name: string, text: string) => new File([text], name)
const pick = (name: string, text: string, sha256: string): PickedFile => ({
  name,
  size: text.length,
  sha256,
  source: file(name, text),
})

describe("attachment staging", () => {
  it("hashes a file in the standard way", async () => {
    expect(await hashFile(file("a.txt", "abc"))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    )
  })

  it("stages every file under the one submission before freezing the manifest", async () => {
    const order: string[] = []
    const exchange = {
      prepareInput: vi.fn(async (_s: string, name: string) => {
        order.push(`prepare:${name}`)
        return { objectId: `o-${name}`, transferId: `t-${name}` }
      }),
      confirmInput: vi.fn(async (_s: string, id: string) => {
        order.push(`confirm:${id}`)
        return { objectId: id }
      }),
      prepareManifest: vi.fn(async () => ({ manifestId: "m1" })),
    }
    const upload = vi.fn(async (t: string) => {
      order.push(`upload:${t}`)
      return {}
    })
    const picked: PickedFile[] = [pick("a.txt", "abc", "a".repeat(64))]
    const id = await stageAttachments(
      { exchange, bytes: { upload, download: vi.fn() } } as never,
      "sub",
      picked,
      ["out.txt"]
    )
    expect(id).toBe("m1")
    expect(order).toEqual([
      "prepare:a.txt",
      "upload:t-a.txt",
      "confirm:o-a.txt",
    ])
    // The whole chosen item goes to the byte channel (Blob or desktop path).
    expect(upload).toHaveBeenCalledWith("t-a.txt", picked[0])
    expect(exchange.prepareManifest).toHaveBeenCalledWith(
      "sub",
      ["o-a.txt"],
      ["out.txt"]
    )
  })

  it("stops without a manifest when an upload fails", async () => {
    const exchange = {
      prepareInput: vi
        .fn()
        .mockResolvedValue({ objectId: "o", transferId: "t" }),
      confirmInput: vi.fn(),
      prepareManifest: vi.fn(),
    }
    const upload = vi.fn().mockRejectedValue(new Error("offline"))
    await expect(
      stageAttachments(
        { exchange, bytes: { upload } } as never,
        "sub",
        [pick("a", "x", "a".repeat(64))],
        []
      )
    ).rejects.toThrow("offline")
    expect(exchange.confirmInput).not.toHaveBeenCalled()
    expect(exchange.prepareManifest).not.toHaveBeenCalled()
  })

  it("refuses to run without upload support", async () => {
    await expect(
      stageAttachments(
        { exchange: {} as never, bytes: { download: vi.fn() } },
        "s",
        [],
        []
      )
    ).rejects.toThrow()
  })

  it("checks counts, sizes and case-insensitive duplicates", () => {
    expect(checkAttachments([{ name: "a", size: 1 }], ["out"])).toBeNull()
    expect(checkAttachments([{ name: "a", size: 513 * 1024 * 1024 }], [])).toBe(
      "tooLarge"
    )
    expect(
      checkAttachments(
        [
          { name: "a", size: 500 * 1024 * 1024 },
          { name: "b", size: 500 * 1024 * 1024 },
          { name: "c", size: 500 * 1024 * 1024 },
          { name: "d", size: 500 * 1024 * 1024 },
          { name: "e", size: 100 * 1024 * 1024 },
        ],
        []
      )
    ).toBe("totalTooLarge")
    expect(
      checkAttachments(
        [
          { name: "A", size: 1 },
          { name: "a", size: 1 },
        ],
        []
      )
    ).toBe("duplicate")
    expect(checkAttachments([], ["Out", "out"])).toBe("duplicate")
    expect(
      checkAttachments(
        Array.from({ length: 201 }, (_, i) => ({ name: `f${i}`, size: 1 })),
        []
      )
    ).toBe("tooMany")
  })

  it("keeps a browser file's own name, size and bytes", async () => {
    const chosen = await pickBrowserFile(file("a.txt", "abc"))
    expect(chosen).toMatchObject({ name: "a.txt", size: 3 })
    expect(chosen.sha256).toBe(await hashFile(file("a.txt", "abc")))
    expect(chosen.source).toBeInstanceOf(Blob)
  })

  it("derives a stable key independent of selection order", () => {
    const a = pick("a", "1", "a".repeat(64))
    const b = pick("b", "22", "b".repeat(64))
    expect(attachmentKey([a, b], ["x", "y"])).toBe(
      attachmentKey([b, a], ["y", "x"])
    )
    expect(attachmentKey([a], [])).not.toBe(attachmentKey([b], []))
  })
})
