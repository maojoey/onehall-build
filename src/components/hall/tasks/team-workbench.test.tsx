import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import messages from "@/i18n/messages/en.json"
const backend = vi.hoisted(() => ({ id: 1 }))
vi.mock("@/contexts/remote-connection-context", () => ({
  useRemoteConnection: () => ({ connection: { id: backend.id } }),
}))
vi.mock("./team-tasks", () => ({
  TeamTasks: () => (
    <input aria-label="project filter fixture" defaultValue="" />
  ),
}))
vi.mock("../branches/local-inbox", () => ({
  LocalBranchInbox: ({ onAccepted }: { onAccepted: () => void }) => (
    <button onClick={onAccepted}>Open accepted request in Tasks</button>
  ),
}))
vi.mock("../mailbox/team-mailbox", () => ({
  TeamMailbox: () => <p>Mailbox fixture</p>,
}))
import { TeamWorkbench } from "./team-workbench"
const page = () => (
  <NextIntlClientProvider locale="en" messages={messages}>
    <TeamWorkbench />
  </NextIntlClientProvider>
)
describe("team main workbench", () => {
  it("preserves the mounted task editor when opening Tasks from an accepted request", () => {
    backend.id = 1
    render(page())
    const editor = screen.getByLabelText("project filter fixture")
    fireEvent.change(editor, { target: { value: "Unsaved task edits" } })
    fireEvent.click(screen.getByRole("button", { name: "Branch requests" }))
    fireEvent.click(
      screen.getByRole("button", { name: "Open accepted request in Tasks" })
    )
    expect(screen.getByLabelText("project filter fixture")).toBe(editor)
    expect(editor).toHaveValue("Unsaved task edits")
    expect(screen.getByRole("status")).toHaveTextContent(/Refresh/)
  })
  it("preserves filters between task and inbox views, and resets them on backend switch", () => {
    backend.id = 1
    const { rerender } = render(page())
    fireEvent.change(screen.getByLabelText("project filter fixture"), {
      target: { value: "project one" },
    })
    expect(screen.queryByText("Mailbox fixture")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Supervisor inbox" }))
    expect(screen.getByText("Mailbox fixture")).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Tasks" }))
    expect(screen.getByLabelText("project filter fixture")).toHaveValue(
      "project one"
    )
    backend.id = 2
    rerender(page())
    expect(screen.getByLabelText("project filter fixture")).toHaveValue("")
    expect(screen.queryByText("Mailbox fixture")).not.toBeInTheDocument()
  })
})
