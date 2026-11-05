import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import messages from "@/i18n/messages/en.json"
const mocks = vi.hoisted(() => ({
  backend: 1,
  clients: new Map<number, ReturnType<typeof client>>(),
}))
function client() {
  return {
    members: vi.fn(),
    role: vi.fn(),
    bind: vi.fn(),
    inbox: vi.fn(),
    acknowledge: vi.fn(),
  }
}
vi.mock("@/lib/hall/mailbox", async (original) => ({
  ...(await original<typeof import("@/lib/hall/mailbox")>()),
  createHallMailboxClient: () => mocks.clients.get(mocks.backend),
}))
vi.mock("@/contexts/remote-connection-context", () => ({
  useRemoteConnection: () => ({ connection: { id: mocks.backend } }),
}))
vi.mock("@/components/layout/backend-badge", () => ({
  BackendBadge: () => <span>Remote backend</span>,
}))
import { TeamMailbox } from "./team-mailbox"
function page() {
  return (
    <NextIntlClientProvider locale="en" messages={messages}>
      <TeamMailbox />
    </NextIntlClientProvider>
  )
}
function deferred<T>() {
  let resolve!: (v: T) => void
  const promise = new Promise<T>((r) => {
    resolve = r
  })
  return { promise, resolve }
}
const message = {
  messageId: "msg",
  senderMemberId: "a",
  recipientMemberId: "b",
  taskId: "task",
  attemptId: "attempt",
  kind: "report",
  body: "<script>fixture only</script>",
  createdAt: "2026-10-25T00:00:00Z",
  acknowledgedAt: null,
}
beforeEach(() => {
  mocks.backend = 1
  mocks.clients.clear()
  for (const id of [1, 2]) {
    const c = client()
    c.members.mockResolvedValue([
      { memberId: "a", name: "Coordinator" },
      { memberId: "b", name: "Worker" },
    ])
    c.role.mockResolvedValue({ memberId: "a", revision: 2 })
    c.inbox.mockResolvedValue([message])
    c.bind.mockResolvedValue({ memberId: "b", revision: 3 })
    c.acknowledge.mockResolvedValue(undefined)
    mocks.clients.set(id, c)
  }
})
async function chooseMember(id = "b") {
  await waitFor(() =>
    expect(screen.getByLabelText("Mailbox owner")).toBeEnabled()
  )
  fireEvent.change(screen.getByLabelText("Mailbox owner"), {
    target: { value: id },
  })
}
describe("team mailbox", () => {
  it("does not bind or consume mail on mount and only saves an explicit stable member selection", async () => {
    const c = mocks.clients.get(1)!
    render(page())
    await screen.findByRole("option", { name: "Not assigned" })
    await waitFor(() =>
      expect(screen.getByLabelText("Duty coordinator")).toBeEnabled()
    )
    expect(c.bind).not.toHaveBeenCalled()
    expect(c.inbox).not.toHaveBeenCalled()
    expect(c.acknowledge).not.toHaveBeenCalled()
    fireEvent.change(screen.getByLabelText("Duty coordinator"), {
      target: { value: "b" },
    })
    expect(c.bind).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Save coordinator" }))
    await waitFor(() => expect(c.bind).toHaveBeenCalledWith("b", 2))
    expect(
      screen.getByText(/Saving the binding starts nothing/)
    ).toBeInTheDocument()
  })
  it("keeps reading separate from acknowledgment and escapes report body", async () => {
    const c = mocks.clients.get(1)!
    render(page())
    await chooseMember()
    await screen.findByText(message.body)
    expect(document.querySelector("script")).toBeNull()
    expect(c.inbox).toHaveBeenCalledWith("b")
    expect(c.acknowledge).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Mark as read" }))
    await waitFor(() => expect(c.acknowledge).toHaveBeenCalledWith("b", "msg"))
  })
  it("shows an old backend explicitly without local fallback or role writes", async () => {
    const c = mocks.clients.get(1)!
    c.role.mockRejectedValue(new Error("command hall_get_pm_binding not found"))
    render(page())
    await screen.findByText(/This backend does not support the team mailbox/)
    expect(
      screen.getByRole("button", { name: "Save coordinator" })
    ).toBeDisabled()
    expect(c.bind).not.toHaveBeenCalled()
    expect(mocks.clients.get(2)!.role).not.toHaveBeenCalled()
  })
  it("ignores outgoing backend mailbox results", async () => {
    const c = mocks.clients.get(1)!
    const old = deferred<unknown[]>()
    c.inbox.mockReturnValue(old.promise)
    const view = render(page())
    await chooseMember()
    await waitFor(() => expect(c.inbox).toHaveBeenCalled())
    mocks.backend = 2
    view.rerender(page())
    await waitFor(() => expect(mocks.clients.get(2)!.role).toHaveBeenCalled())
    await act(async () => old.resolve([message]))
    expect(screen.queryByText(message.body)).not.toBeInTheDocument()
    expect(mocks.clients.get(2)!.acknowledge).not.toHaveBeenCalled()
  })
  it("ignores a late mailbox response after choosing another member", async () => {
    const c = mocks.clients.get(1)!
    const old = deferred<unknown[]>()
    c.inbox.mockReturnValueOnce(old.promise).mockResolvedValueOnce([])
    render(page())
    await chooseMember("b")
    await chooseMember("a")
    await waitFor(() => expect(c.inbox).toHaveBeenCalledWith("a"))
    await act(async () => old.resolve([message]))
    expect(screen.queryByText(message.body)).not.toBeInTheDocument()
  })
  it("requires a refresh after an uncertain binding save instead of blindly retrying", async () => {
    const c = mocks.clients.get(1)!
    c.bind.mockRejectedValue(new Error("network lost"))
    render(page())
    await waitFor(() =>
      expect(screen.getByLabelText("Duty coordinator")).toBeEnabled()
    )
    fireEvent.change(screen.getByLabelText("Duty coordinator"), {
      target: { value: "b" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Save coordinator" }))
    await screen.findByText(/Coordinator save was not confirmed/)
    expect(
      screen.getByRole("button", { name: "Save coordinator" })
    ).toBeDisabled()
    expect(c.bind).toHaveBeenCalledOnce()
  })
})

it("labels a host-received branch report instead of attributing it to the user", async () => {
  mocks.clients.get(1)!.inbox.mockResolvedValue([
    {
      ...message,
      senderMemberId: null,
      senderName: "Branch report",
      taskId: null,
      attemptId: null,
    },
  ])
  render(page())
  await chooseMember()
  expect(await screen.findByText(/Branch report · Report/)).toBeInTheDocument()
})
