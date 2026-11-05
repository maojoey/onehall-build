import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  beginBackendSwitch,
  configureWebBranchTransport,
  configureRemoteDesktopTransport,
} from "./transport"
import { WebTransport } from "./transport/web-transport"
import {
  captureAttachmentUploader,
  uploadAttachment,
  downloadWorkspaceFile,
} from "./api"
import type { Transport } from "./transport/types"
import { getWebConnectionSnapshot } from "./transport/web-connection-store"
const branch = "11111111-1111-4111-8111-111111111111"
beforeEach(() => {
  __resetTransportForTests()
  __setTransportFactoriesForTests({ web: (url) => new WebTransport(url) })
  configureWebBranchTransport(branch)
})
afterEach(() => {
  __resetTransportForTests()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  localStorage.clear()
})
it("uploads attachment bytes to the selected branch prefix", async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValue(new Response('{"path":"/branch/upload.txt"}'))
  vi.stubGlobal("fetch", fetcher)
  await uploadAttachment(new File(["hello"], "upload.txt"))
  expect(fetcher.mock.calls[0][0]).toBe(
    `${location.origin}/branches/${branch}/api/upload_attachment`
  )
  expect(fetcher.mock.calls[0][1].body.get("file").name).toBe("upload.txt")
})
it("uses the captured branch for a download ticket received after switching", async () => {
  let finish!: (r: Response) => void
  vi.stubGlobal(
    "fetch",
    vi.fn(
      () =>
        new Promise((r) => {
          finish = r
        })
    )
  )
  let downloaded = ""
  vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
    this: HTMLAnchorElement
  ) {
    downloaded = this.href
  })
  const download = downloadWorkspaceFile("/work", "file.txt", "file.txt")
  beginBackendSwitch()
  finish(
    new Response('{"url":"/branch-download/test-ticket","filename":"file.txt"}')
  )
  await download
  expect(downloaded).toBe(
    `${location.origin}/branches/${branch}/branch-download/test-ticket`
  )
})
it("keeps captured attachment authority and credentials after changing branch and login", async () => {
  localStorage.setItem("codeg_token", "original-hq-token")
  const uploader = captureAttachmentUploader()
  beginBackendSwitch()
  configureWebBranchTransport("22222222-2222-4222-8222-222222222222")
  localStorage.setItem("codeg_token", "replacement-hq-token")
  const fetcher = vi.fn<
    (url: string, options: RequestInit) => Promise<Response>
  >(async () => new Response('{"path":"/branch/upload.txt"}'))
  vi.stubGlobal("fetch", fetcher)
  await uploader.upload(new File(["hello"], "upload.txt"))
  expect(fetcher.mock.calls[0]?.[0]).toBe(
    `${location.origin}/branches/${branch}/api/upload_attachment`
  )
  expect(
    (fetcher.mock.calls[0]?.[1].headers as Record<string, string>).Authorization
  ).toBe("Bearer original-hq-token")
  expect(uploader.isCurrent()).toBe(false)
})
it("keeps native path reads and subsequent uploads on their captured remote connection", async () => {
  __resetTransportForTests()
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: {},
  })
  try {
    let finishRead!: (file: unknown) => void
    const calls: { command: string; args?: Record<string, unknown> }[] = []
    const shell: Transport = {
      isDesktop: () => true,
      subscribe: async () => () => {},
      call: async <T>(command: string, args?: Record<string, unknown>) => {
        calls.push({ command, args })
        if (command === "read_local_file_for_upload") {
          return new Promise<T>((resolve) => {
            finishRead = resolve as (file: unknown) => void
          })
        }
        return { path: "/original/upload.txt" } as T
      },
    }
    __setTransportFactoriesForTests({ tauri: () => shell, remote: () => shell })
    configureRemoteDesktopTransport({
      id: 17,
      name: "A",
      baseUrl: "http://hq/branches/a",
      token: "hq",
      windowInstanceId: "test",
    })
    const uploader = captureAttachmentUploader()
    const first = uploader.uploadLocalPath("/local/a.txt", "same-tab")
    beginBackendSwitch()
    configureRemoteDesktopTransport({
      id: 29,
      name: "B",
      baseUrl: "http://hq/branches/b",
      token: "hq",
      windowInstanceId: "test",
    })
    const readResult = {
      fileName: "a.txt",
      mimeType: "text/plain",
      size: 5,
      dataBase64: "aGVsbG8=",
    }
    finishRead(readResult)
    await first
    const second = uploader.uploadLocalPath("/local/b.txt", "same-tab")
    finishRead({ ...readResult, fileName: "b.txt" })
    await second
    expect(
      calls
        .filter((call) => call.command === "remote_upload_attachment")
        .map((call) => call.args?.connectionId)
    ).toEqual([17, 17])
  } finally {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__")
  }
})
it("does not expire the incoming workspace from an outgoing upload's late 401", async () => {
  let finish!: (response: Response) => void
  vi.stubGlobal(
    "fetch",
    () =>
      new Promise<Response>((resolve) => {
        finish = resolve
      })
  )
  const uploading = uploadAttachment(new File(["hello"], "upload.txt"))
  beginBackendSwitch()
  configureWebBranchTransport("22222222-2222-4222-8222-222222222222")
  finish(new Response("Unauthorized", { status: 401 }))
  await expect(uploading).rejects.toThrow("Unauthorized")
  expect(getWebConnectionSnapshot()).toBe("connected")
})
