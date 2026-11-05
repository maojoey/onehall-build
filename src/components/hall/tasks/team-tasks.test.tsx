import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import messages from "@/i18n/messages/en.json"

const mocks = vi.hoisted(() => ({
  backend: 1,
  clients: new Map<number, ReturnType<typeof makeClient>>(),
}))
function makeClient() {
  return {
    list: vi.fn(),
    members: vi.fn(),
    projects: vi.fn(),
    snapshots: vi.fn(),
    get: vi.fn(),
    create: vi.fn(),
    edit: vi.fn(),
    cancel: vi.fn(),
  }
}
vi.mock("@/lib/hall/tasks", async (original) => ({
  ...(await original<typeof import("@/lib/hall/tasks")>()),
  createHallTasksClient: () => mocks.clients.get(mocks.backend),
}))
vi.mock("@/contexts/remote-connection-context", () => ({
  useRemoteConnection: () => ({ connection: { id: mocks.backend } }),
}))
vi.mock("@/components/layout/backend-badge", () => ({
  BackendBadge: () => <span>Remote backend</span>,
}))
import { TeamTasks } from "./team-tasks"

const member = (id = "m1", name = "Reviewer") => ({
  memberId: id,
  name,
  role: "Review",
  revision: 1,
  projectId: null,
})
const snapshot = (id = "s1", owner = "m1") => ({
  snapshotId: id,
  memberId: owner,
  memberRevision: 1,
  projectId: null,
})
const task = (id = "t1", title = "Review code") => ({
  taskId: id,
  memberId: "m1",
  projectId: null,
  snapshot: { ...snapshot(), snapshotVersion: "a".repeat(64) },
  title,
  status: "draft",
  purpose: "work",
  revision: 1,
  launchAllowed: false,
  runtimeVerified: false,
})
const detail = (id = "t1", title = "Review code") => ({
  ...task(id, title),
  description: "Expected review delivery",
  attempts: [],
  history: [
    { revision: 1, action: "draft_created", createdAt: "2026-10-25T00:00:00Z" },
  ],
})
function page() {
  return (
    <NextIntlClientProvider locale="en" messages={messages}>
      <TeamTasks />
    </NextIntlClientProvider>
  )
}
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((r) => {
    resolve = r
  })
  return { promise, resolve }
}
function configure(id: number) {
  const c = makeClient()
  c.list.mockResolvedValue([])
  c.members.mockResolvedValue([member()])
  c.projects.mockResolvedValue([])
  c.snapshots.mockResolvedValue([snapshot()])
  c.get.mockResolvedValue(detail())
  c.create.mockResolvedValue(task())
  c.edit.mockResolvedValue(task())
  c.cancel.mockResolvedValue({ ...task(), status: "cancelled" })
  mocks.clients.set(id, c)
  return c
}
beforeEach(() => {
  mocks.backend = 1
  mocks.clients.clear()
  configure(1)
  configure(2)
})
async function startDraft() {
  const create = await screen.findByRole("button", { name: "New draft task" })
  await waitFor(() => expect(create).toBeEnabled())
  fireEvent.click(create)
  fireEvent.change(screen.getByLabelText("Owner"), { target: { value: "m1" } })
  await screen.findByRole("option", {
    name: "Saved version 1 · member revision 1",
  })
  fireEvent.change(screen.getByLabelText("Saved configuration"), {
    target: { value: "s1" },
  })
  fireEvent.change(screen.getByLabelText("Task title"), {
    target: { value: "Explicit draft" },
  })
}

describe("team task page", () => {
  it.each([
    ["terminal", "interrupted", true],
    ["needs_review", "interrupted", false],
    ["running", "running", false],
  ])(
    "reports incomplete exit evidence for %s / %s with the correct capacity state",
    async (stage, status, released) => {
      const c = mocks.clients.get(1)!
      c.list.mockResolvedValue([{ ...task(), status }])
      c.get.mockResolvedValue({
        ...detail(),
        status,
        attempts: [
          { attemptId: "a1", attemptNumber: 1, status: "interrupted" },
        ],
        runtimes: [
          {
            attemptId: "a1",
            stage,
            runtimeKind: "native",
            exitConfirmedAt: null,
          },
        ],
        exitChecks: [
          {
            attemptId: "a1",
            coverage: "macos_process_group_best_effort",
            complete: false,
            rootReaped: true,
            knownProcesses: 1,
            knownReaped: 1,
            pendingSpawns: 0,
            reason: "descendant_coverage_unconfirmed",
            observedAt: "now",
          },
        ],
        history: [
          {
            revision: 2,
            action: "human_best_effort_resolution",
            createdAt: "now",
          },
        ],
      })
      render(page())
      fireEvent.click(
        await screen.findByRole("button", { name: /Review code/ })
      )
      await screen.findByText("Exit observations")
      if (released) {
        expect(
          screen.getByText(/Capacity was released by human review/)
        ).toHaveTextContent(/incomplete exit evidence/)
        expect(
          screen.queryByText("Exit remains unverified; capacity stays occupied")
        ).not.toBeInTheDocument()
      } else {
        expect(
          screen.getByText("Exit remains unverified; capacity stays occupied")
        ).toBeInTheDocument()
        expect(
          screen.queryByText(/Capacity was released by human review/)
        ).not.toBeInTheDocument()
      }
    }
  )
  it("does not borrow a terminal runtime from another attempt to label incomplete evidence released", async () => {
    const c = mocks.clients.get(1)!
    c.list.mockResolvedValue([{ ...task(), status: "interrupted" }])
    c.get.mockResolvedValue({
      ...detail(),
      status: "interrupted",
      attempts: [{ attemptId: "a2", attemptNumber: 2, status: "interrupted" }],
      runtimes: [
        {
          attemptId: "a1",
          stage: "terminal",
          runtimeKind: "native",
          exitConfirmedAt: null,
        },
        {
          attemptId: "a2",
          stage: "needs_review",
          runtimeKind: "native",
          exitConfirmedAt: null,
        },
      ],
      exitChecks: [
        {
          attemptId: "a2",
          coverage: "macos_process_group_best_effort",
          complete: false,
        },
      ],
    })
    render(page())
    fireEvent.click(await screen.findByRole("button", { name: /Review code/ }))
    expect(
      await screen.findByText(
        "Exit remains unverified; capacity stays occupied"
      )
    ).toBeInTheDocument()
    expect(
      screen.queryByText(/Capacity was released by human review/)
    ).not.toBeInTheDocument()
  })

  it("loads an empty draft workspace without implicit saves or engines", async () => {
    const c = mocks.clients.get(1)!
    render(page())
    await screen.findByText(/No tasks in this view/)
    expect(c.create).not.toHaveBeenCalled()
    expect(c.edit).not.toHaveBeenCalled()
    expect(c.cancel).not.toHaveBeenCalled()
    expect(
      screen.getByText(/Approved tasks can be dispatched/)
    ).toBeInTheDocument()
    expect(
      screen.getByRole("link", { name: "Manage members" })
    ).toHaveAttribute("href", "/settings/hall-members?remoteConnectionId=1")
  })
  it("shows unsupported explicitly and never opens a local save", async () => {
    const c = mocks.clients.get(1)!
    c.list.mockRejectedValue({
      code: "network_error",
      message: "Remote returned HTTP 501 Not Implemented",
      detail: JSON.stringify({
        code: "not_implemented",
        message: "API endpoint 'hall_list_tasks' is not available in web mode",
      }),
    })
    render(page())
    await screen.findByText(/This backend does not support team tasks/)
    expect(
      screen.getByRole("button", { name: "New draft task" })
    ).toBeDisabled()
    expect(c.create).not.toHaveBeenCalled()
    expect(mocks.clients.get(2)!.list).not.toHaveBeenCalled()
  })
  it("submits only on a click and reuses the exact create request after uncertain failure", async () => {
    const c = mocks.clients.get(1)!
    c.create.mockRejectedValueOnce(new Error("connection lost"))
    render(page())
    await startDraft()
    expect(c.create).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Save draft" }))
    await screen.findByText(/Save was not confirmed/)
    expect(screen.getByLabelText("Task title")).toBeDisabled()
    fireEvent.click(
      screen.getByRole("button", { name: "Retry same submission" })
    )
    await waitFor(() => expect(c.create).toHaveBeenCalledTimes(2))
    expect(c.create.mock.calls[1][0]).toBe(c.create.mock.calls[0][0])
    expect(c.create.mock.calls[0][0]).toMatchObject({
      memberId: "m1",
      snapshotId: "s1",
      projectId: null,
      purpose: "work",
      title: "Explicit draft",
    })
  })
  it("ignores late snapshot results after choosing a different member", async () => {
    const c = mocks.clients.get(1)!
    c.members.mockResolvedValue([member(), member("m2", "Writer")])
    const old = deferred<unknown[]>()
    c.snapshots
      .mockReturnValueOnce(old.promise)
      .mockResolvedValueOnce([snapshot("s2", "m2")])
    render(page())
    fireEvent.click(
      await screen.findByRole("button", { name: "New draft task" })
    )
    fireEvent.change(screen.getByLabelText("Owner"), {
      target: { value: "m1" },
    })
    fireEvent.change(screen.getByLabelText("Owner"), {
      target: { value: "m2" },
    })
    await screen.findByRole("option", {
      name: "Saved version 1 · member revision 1",
    })
    await act(async () => old.resolve([snapshot("old")]))
    const options = Array.from(
      (screen.getByLabelText("Saved configuration") as HTMLSelectElement)
        .options
    ).map((o) => o.value)
    expect(options).toContain("s2")
    expect(options).not.toContain("old")
  })
  it("ignores outgoing backend reads and successful saves after a switch", async () => {
    const first = mocks.clients.get(1)!
    const pending = deferred<unknown>()
    first.create.mockReturnValue(pending.promise)
    const view = render(page())
    await startDraft()
    fireEvent.click(screen.getByRole("button", { name: "Save draft" }))
    await waitFor(() => expect(first.create).toHaveBeenCalledOnce())
    mocks.backend = 2
    view.rerender(page())
    await screen.findByText(/No tasks in this view/)
    const second = mocks.clients.get(2)!
    const before = second.list.mock.calls.length
    await act(async () => pending.resolve(task()))
    expect(second.create).not.toHaveBeenCalled()
    expect(second.list).toHaveBeenCalledTimes(before)
    expect(screen.queryByLabelText("Task title")).not.toBeInTheDocument()
  })
  it("ignores a late task list from the outgoing backend", async () => {
    const first = mocks.clients.get(1)!
    const old = deferred<unknown[]>()
    first.list.mockReturnValue(old.promise)
    const view = render(page())
    await waitFor(() => expect(first.list).toHaveBeenCalled())
    mocks.backend = 2
    view.rerender(page())
    await screen.findByText(/No tasks in this view/)
    await act(async () => old.resolve([task("old", "Wrong backend task")]))
    expect(screen.queryByText("Wrong backend task")).not.toBeInTheDocument()
  })
  it("shows host queue preparation without pretending execution or an exit", async () => {
    const c = mocks.clients.get(1)!
    c.list.mockResolvedValue([{ ...task(), status: "interrupted" }])
    c.get.mockResolvedValue({
      ...detail(),
      status: "interrupted",
      attempts: [{ attemptId: "a1", attemptNumber: 1, status: "interrupted" }],
      history: [
        {
          revision: 4,
          action: "attempt_interrupted",
          createdAt: "2026-10-25T00:00:00Z",
        },
      ],
    })
    render(page())
    fireEvent.click(await screen.findByRole("button", { name: /Review code/ }))
    await screen.findByText(/Unknown prior preparation keeps its capacity/)
    expect(
      screen.queryByRole("button", { name: "Cancel draft" })
    ).not.toBeInTheDocument()
    expect(screen.getAllByText(/Execution unverified/)).toHaveLength(2)
    expect(screen.queryByText(/No attempts yet/)).not.toBeInTheDocument()
    expect(c.create).not.toHaveBeenCalled()
    expect(c.cancel).not.toHaveBeenCalled()
  })
  it("keeps fixture results separate from cancellation acknowledgment and verified exit", async () => {
    const c = mocks.clients.get(1)!
    c.list.mockResolvedValue([{ ...task(), status: "running" }])
    c.get.mockResolvedValue({
      ...detail(),
      status: "running",
      attempts: [
        {
          attemptId: "a1",
          attemptNumber: 1,
          status: "cancel_requested",
          startedAt: "2026-10-25T00:00:00Z",
          finishedAt: null,
        },
      ],
      runtimes: [
        {
          attemptId: "a1",
          stage: "cancel_requested",
          runtimeKind: "fixture",
          intentAt: "2026-10-25T00:00:00Z",
          cancelRequestedAt: "2026-10-25T00:01:00Z",
          cancelAckAt: "2026-10-25T00:01:01Z",
          exitConfirmedAt: null,
        },
      ],
      results: [
        {
          resultId: "r1",
          attemptId: "a1",
          outcome: "succeeded",
          body: "<script>fixture only</script>",
          createdAt: "2026-10-25T00:01:02Z",
        },
      ],
    })
    render(page())
    fireEvent.click(await screen.findByRole("button", { name: /Review code/ }))
    await screen.findByText(/Isolated fixture record/)
    expect(screen.getByText(/Cancellation acknowledged:/)).toBeInTheDocument()
    expect(screen.getByText(/No verified exit evidence/)).toBeInTheDocument()
    expect(screen.getByText(/Reported outcome: succeeded/)).toBeInTheDocument()
    expect(
      screen.getByText("<script>fixture only</script>")
    ).toBeInTheDocument()
    expect(document.querySelector("script")).toBeNull()
    expect(c.cancel).not.toHaveBeenCalled()
  })
  it("flags a succeeded task whose report never reached the PM", async () => {
    const c = mocks.clients.get(1)!
    c.list.mockResolvedValue([{ ...task(), status: "succeeded" }])
    c.get.mockResolvedValue({
      ...detail(),
      status: "succeeded",
      issues: [
        {
          attemptId: "a1",
          tool: "hall_report",
          detail: "Team mailbox request or role binding changed",
          createdAt: "2026-10-28T00:00:00Z",
        },
      ],
    })
    render(page())
    fireEvent.click(await screen.findByRole("button", { name: /Review code/ }))
    const alert = await screen.findByRole("alert")
    expect(alert).toHaveTextContent(/key actions did not go through/)
    expect(alert).toHaveTextContent(/Report to the PM/)
    expect(alert).toHaveTextContent(/role binding changed/)
  })
  it("guards details against out-of-order replies and only cancels a selected draft", async () => {
    const c = mocks.clients.get(1)!
    c.list.mockResolvedValue([
      task("t1", "First task"),
      task("t2", "Second task"),
    ])
    const old = deferred<unknown>()
    c.get
      .mockReturnValueOnce(old.promise)
      .mockResolvedValueOnce(detail("t2", "Second details"))
    render(page())
    fireEvent.click(await screen.findByRole("button", { name: /First task/ }))
    fireEvent.click(screen.getByRole("button", { name: /Second task/ }))
    await screen.findByText("Second details")
    await act(async () => old.resolve(detail("t1", "Obsolete details")))
    expect(screen.queryByText("Obsolete details")).not.toBeInTheDocument()
    expect(c.cancel).not.toHaveBeenCalled()
    expect(screen.getByText(/No attempts yet/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Cancel draft" }))
    await waitFor(() =>
      expect(c.cancel).toHaveBeenCalledWith(
        expect.objectContaining({ taskId: "t2", revision: 1 })
      )
    )
  })
})
