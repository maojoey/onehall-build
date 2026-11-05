import { act, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { RichComposerHandle } from "./rich-composer"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  beginBackendSwitch,
  configureWebBranchTransport,
} from "@/lib/transport"
import { WebTransport } from "@/lib/transport/web-transport"
import { useComposerAttachments } from "./use-composer-attachments"
const conversion = vi.hoisted(() => ({
  run: (): Promise<string> => Promise.resolve("aGVsbG8="),
}))
vi.mock("next-intl", () => {
  const t = (key: string) => key
  return { useTranslations: () => t }
})
vi.mock("./attachment-files", async (original) => ({
  ...(await original<typeof import("./attachment-files")>()),
  blobToBase64: () => conversion.run(),
}))
vi.mock("./composer-commands", () => ({ restoreBlocksIntoEditor: vi.fn() }))
const branchA = "11111111-1111-4111-8111-111111111111"
const branchB = "22222222-2222-4222-8222-222222222222"
function useAttachments() {
  return useComposerAttachments({
    editorRef: { current: null } as { current: RichComposerHandle | null },
    promptCapabilities: { image: true, embedded_context: true },
    attachmentTabId: "same-tab",
  })
}
beforeEach(() => {
  __resetTransportForTests()
  __setTransportFactoriesForTests({ web: (url) => new WebTransport(url) })
  configureWebBranchTransport(branchA)
  conversion.run = () => Promise.resolve("aGVsbG8=")
})
afterEach(() => {
  __resetTransportForTests()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})
it("keeps queued files on the backend selected before the first three uploads settle", async () => {
  const urls: string[] = []
  const pending: Array<() => void> = []
  vi.stubGlobal("fetch", (url: string) => {
    urls.push(url)
    return new Promise<Response>((resolve) => {
      pending.push(() => resolve(new Response('{"path":"/uploaded/file.txt"}')))
    })
  })
  const { result } = renderHook(useAttachments)
  let uploading!: Promise<void>
  act(() => {
    uploading = result.current.appendFilesFromInput(
      Array.from({ length: 4 }, (_, i) => new File(["hello"], `${i}.txt`))
    )
  })
  await waitFor(() => expect(urls).toHaveLength(3))
  act(() => {
    beginBackendSwitch()
    configureWebBranchTransport(branchB)
  })
  await act(async () => {
    pending.splice(0).forEach((finish) => finish())
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
  await waitFor(() => expect(urls).toHaveLength(4))
  await act(async () => {
    pending.splice(0).forEach((finish) => finish())
    await uploading
  })
  expect(urls).toEqual(
    Array(4).fill(
      `${location.origin}/branches/${branchA}/api/upload_attachment`
    )
  )
})
it("pins image uploads before thumbnail conversion and ignores late attachment state", async () => {
  let finishConversion!: (value: string) => void
  conversion.run = () =>
    new Promise((resolve) => {
      finishConversion = resolve
    })
  const urls: string[] = []
  vi.stubGlobal("fetch", async (url: string) => {
    urls.push(url)
    return new Response('{"path":"/uploaded/image.png"}')
  })
  const { result } = renderHook(useAttachments)
  let uploading!: Promise<void>
  act(() => {
    uploading = result.current.appendFilesFromInput([
      new File(["hello"], "image.png", { type: "image/png" }),
    ])
  })
  act(() => {
    beginBackendSwitch()
    configureWebBranchTransport(branchB)
  })
  await act(async () => {
    finishConversion("aGVsbG8=")
    await uploading
  })
  expect(urls).toEqual([
    `${location.origin}/branches/${branchA}/api/upload_attachment`,
  ])
  expect(result.current.attachments).toEqual([])
})
it("keeps local desktop images inline without uploading them", async () => {
  __resetTransportForTests()
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: {},
  })
  try {
    __setTransportFactoriesForTests({
      tauri: () => ({
        isDesktop: () => true,
        call: async <T,>() => undefined as T,
        subscribe: async () => () => {},
      }),
    })
    const fetcher = vi.fn()
    vi.stubGlobal("fetch", fetcher)
    const { result } = renderHook(useAttachments)
    await act(async () => {
      await result.current.appendFilesFromInput([
        new File(["hello"], "image.png", { type: "image/png" }),
      ])
    })
    expect(result.current.attachments).toEqual([
      expect.objectContaining({
        type: "image",
        data: "aGVsbG8=",
        uri: null,
      }),
    ])
    expect(result.current.hasUploadingImage).toBe(false)
    expect(fetcher).not.toHaveBeenCalled()
  } finally {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__")
  }
})
it("settles an active web image into its uploaded attachment", async () => {
  vi.stubGlobal(
    "fetch",
    async () => new Response('{"path":"/uploaded/image.png"}')
  )
  const { result } = renderHook(useAttachments)
  await act(async () => {
    await result.current.appendFilesFromInput([
      new File(["hello"], "image.png", { type: "image/png" }),
    ])
  })
  expect(result.current.attachments).toEqual([
    expect.objectContaining({
      uri: "file:///uploaded/image.png",
      uploading: false,
    }),
  ])
})
