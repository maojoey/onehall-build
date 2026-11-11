import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { NextIntlClientProvider } from "next-intl"
import en from "@/i18n/messages/en.json"
import { UpdateCredentialSettings } from "./update-credential-settings"
const mocks = vi.hoisted(() => ({ local: true, remote: false, call: vi.fn() }))
vi.mock("@/lib/transport", () => ({
  isDesktop: () => mocks.local,
  isRemoteDesktopMode: () => mocks.remote,
  getTransport: () => ({ call: mocks.call }),
}))
function mount() {
  return render(
    <NextIntlClientProvider locale="en" messages={en}>
      <UpdateCredentialSettings />
    </NextIntlClientProvider>
  )
}
beforeEach(() => {
  mocks.local = true
  mocks.remote = false
  mocks.call.mockReset()
  mocks.call.mockResolvedValue({ configured: false })
})
it.each([
  [false, false],
  [true, true],
])("hides device credentials for desktop=%s remote=%s", (desktop, remote) => {
  mocks.local = desktop
  mocks.remote = remote
  const { container } = mount()
  expect(container).toBeEmptyDOMElement()
  expect(mocks.call).not.toHaveBeenCalled()
})
it("imports only the file path and shows presence without showing the secret", async () => {
  mount()
  await screen.findByText("No device credential imported (optional)")
  fireEvent.change(screen.getByLabelText("Credential file path"), {
    target: { value: "/private/device.token" },
  })
  mocks.call.mockResolvedValue({ configured: true })
  fireEvent.click(
    screen.getByRole("button", { name: "Import credential file" })
  )
  await screen.findByText("Device credential stored in OS keyring")
  expect(mocks.call).toHaveBeenCalledWith("import_app_update_credential", {
    path: "/private/device.token",
  })
  expect(screen.getByLabelText("Credential file path")).toHaveValue("")
  fireEvent.click(
    screen.getByRole("button", { name: "Remove device credential" })
  )
  await waitFor(() =>
    expect(mocks.call).toHaveBeenCalledWith("remove_app_update_credential")
  )
})

it("hydrates a statically exported settings section on desktop without a mismatch", async () => {
  const { renderToString } = await import("react-dom/server")
  const { hydrateRoot } = await import("react-dom/client")
  const { act } = await import("@testing-library/react")
  const element = (
    <NextIntlClientProvider locale="en" messages={en}>
      <UpdateCredentialSettings />
    </NextIntlClientProvider>
  )
  mocks.local = false
  const html = renderToString(element)
  expect(html).toBe("")
  const container = document.createElement("div")
  container.innerHTML = html
  document.body.appendChild(container)
  const onRecoverableError = vi.fn()
  mocks.local = true
  let root!: ReturnType<typeof hydrateRoot>
  await act(async () => {
    root = hydrateRoot(container, element, { onRecoverableError })
  })
  await screen.findByText("No device credential imported (optional)")
  expect(onRecoverableError).not.toHaveBeenCalled()
  await act(async () => root.unmount())
  container.remove()
})
