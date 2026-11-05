// @vitest-environment node
import { renderToString } from "react-dom/server"
import { NextIntlClientProvider } from "next-intl"
import { expect, it, vi } from "vitest"
import messages from "@/i18n/messages/en.json"
const mocks = vi.hoisted(() => ({ create: vi.fn() }))
vi.mock("@/lib/hall/tasks", async (original) => ({
  ...(await original<typeof import("@/lib/hall/tasks")>()),
  createHallTasksClient: mocks.create,
}))
vi.mock("@/contexts/remote-connection-context", () => ({
  useRemoteConnection: () => null,
}))
vi.mock("@/components/layout/backend-badge", () => ({
  BackendBadge: () => <span>Backend</span>,
}))
import { TeamTasks } from "./team-tasks"
it("statically renders the loading page without binding a browser backend", () => {
  const html = renderToString(
    <NextIntlClientProvider locale="en" messages={messages}>
      <TeamTasks />
    </NextIntlClientProvider>
  )
  expect(html).toContain("OneHall team")
  expect(html).toContain("Loading")
  expect(mocks.create).not.toHaveBeenCalled()
})
