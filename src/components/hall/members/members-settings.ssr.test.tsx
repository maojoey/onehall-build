// @vitest-environment node
import { renderToString } from "react-dom/server"
import { NextIntlClientProvider } from "next-intl"
import { expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"

const mocks = vi.hoisted(() => ({
  getTransport: vi.fn(() => {
    throw new Error("A static export must not access a desktop transport")
  }),
}))
vi.mock("@/lib/transport", () => ({ getTransport: mocks.getTransport }))
vi.mock("@/contexts/remote-connection-context", () => ({
  useRemoteConnection: () => null,
}))
vi.mock("@/components/layout/backend-badge", () => ({
  BackendBadge: () => null,
  useBackendLabel: () => "Local",
}))

import { MembersSettings } from "./members-settings"

it("exports the member page without a browser or a desktop backend", () => {
  const html = renderToString(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <MembersSettings />
    </NextIntlClientProvider>
  )
  expect(html).toContain("OneHall members")
  expect(mocks.getTransport).not.toHaveBeenCalled()
})
