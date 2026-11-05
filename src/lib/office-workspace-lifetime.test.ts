import { afterEach, expect, it, vi } from "vitest"
import { captureOfficeWatchClient } from "./api"
import {
  __resetTransportForTests,
  __setTransportFactoriesForTests,
  beginBackendSwitch,
  configureWebBranchTransport,
} from "./transport"
import { WebTransport } from "./transport/web-transport"
afterEach(() => {
  __resetTransportForTests()
  vi.unstubAllGlobals()
})
it("releases a late headquarters watch on headquarters after switching to a branch", async () => {
  __setTransportFactoriesForTests({ web: (url) => new WebTransport(url) })
  let finish!: (r: Response) => void
  const fetcher = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r
        })
    )
    .mockResolvedValue(new Response("null"))
  vi.stubGlobal("fetch", fetcher)
  const client = captureOfficeWatchClient()
  const start = client.start("/work", "report.docx")
  beginBackendSwitch()
  configureWebBranchTransport("11111111-1111-4111-8111-111111111111")
  finish(new Response('{"port":1234,"cap":"cap","hold":"hold"}'))
  const acquired = await start
  await client.stop("/work", "report.docx", acquired.cap, acquired.hold)
  expect(fetcher.mock.calls[1][0]).toBe(
    `${location.origin}/api/stop_office_watch`
  )
})
