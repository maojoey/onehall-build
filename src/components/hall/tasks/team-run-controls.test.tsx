import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it, vi } from "vitest"
import messages from "@/i18n/messages/en.json"
import { TeamRunControls } from "./team-run-controls"
import type { TeamControlClient, TeamReadiness } from "@/lib/hall/control"
import type { HallTaskDetail } from "@/lib/hall/tasks"
const task = { taskId: "task", revision: 2, status: "queued" } as HallTaskDetail
const ready: TeamReadiness = {
  enabled: false,
  nativeVerified: false,
  runRoot: false,
  reason: "run_root_unavailable",
  platform: "macos-aarch64",
  nativeProbeSupported: false,
  transportReady: true,
}
function setup(row = task, state = ready, localRelease = true) {
  const c = {
    state: vi.fn().mockResolvedValue(state),
    dispatch: vi.fn().mockResolvedValue({ ...row, status: "running" }),
    refresh: vi.fn().mockResolvedValue(row),
    cancel: vi.fn().mockResolvedValue({ ...row, status: "running" }),
    recover: vi.fn().mockResolvedValue(row),
    probe: vi.fn().mockResolvedValue({}),
    resolveBestEffort: vi.fn().mockResolvedValue({ ...row, runtimes: [] }),
    resolveOrphan: vi
      .fn()
      .mockResolvedValue({ ...row, resolution: { kind: "operator_released" } }),
  }
  const changed = vi.fn()
  const view = render(
    <NextIntlClientProvider locale="en" messages={messages}>
      <TeamRunControls
        client={c as unknown as TeamControlClient}
        task={row}
        changed={changed}
        localRelease={localRelease}
      />
    </NextIntlClientProvider>
  )
  return { c, changed, ...view }
}
afterEach(() => vi.useRealTimers())
describe("private host runtime controls", () => {
  it("requires acknowledgement before closing a reviewed best-effort run", async () => {
    const row = {
      ...task,
      status: "interrupted",
      runtimes: [{ stage: "needs_review" }],
      exitChecks: [
        { coverage: "macos_process_group_best_effort", rootReaped: true },
      ],
    } as HallTaskDetail
    const { c } = setup(row, {
      ...ready,
      coverage: "macos_process_group_best_effort",
    } as TeamReadiness)
    const close = await screen.findByRole("button", {
      name: "Close reviewed run",
    })
    expect(close).toBeDisabled()
    expect(c.resolveBestEffort).not.toHaveBeenCalled()
    fireEvent.click(
      screen.getByRole("checkbox", { name: /I have checked the result/ })
    )
    expect(close).toBeEnabled()
    fireEvent.click(close)
    await waitFor(() => expect(c.resolveBestEffort).toHaveBeenCalledWith(row))
  })
  it("explains Mac cancellation limits before allowing a reviewed task", async () => {
    setup(task, {
      ...ready,
      enabled: true,
      nativeVerified: true,
      runRoot: true,
      reason: "ready",
      nativeProbeSupported: true,
      coverage: "macos_process_group_best_effort",
      completeDescendantCoverage: false,
    } as TeamReadiness)
    await screen.findByText(/macOS cancellation is best effort/)
    expect(screen.getByRole("button", { name: "Dispatch task" })).toBeEnabled()
  })
  it("keeps Mac real dispatch closed instead of treating Connected as B10", async () => {
    const { c } = setup()
    await screen.findByText(/This platform lacks complete descendant/)
    expect(screen.getByRole("button", { name: "Dispatch task" })).toBeDisabled()
    expect(c.dispatch).not.toHaveBeenCalled()
    expect(c.probe).not.toHaveBeenCalled()
    expect(
      screen.queryByRole("button", { name: "Run host check again" })
    ).not.toBeInTheDocument()
  })
  it("explains a failed host check and lets the user re-run it", async () => {
    const { c } = setup(task, {
      ...ready,
      runRoot: true,
      reason: "native_probe_failed",
      platform: "linux-aarch64",
      nativeProbeSupported: true,
    })
    await screen.findByText(/The host check failed/)
    expect(screen.getByRole("button", { name: "Dispatch task" })).toBeDisabled()
    fireEvent.click(
      screen.getByRole("button", { name: "Run host check again" })
    )
    await waitFor(() => expect(c.probe).toHaveBeenCalledWith())
  })
  it("dispatches the reviewed task and revision once the host is ready", async () => {
    const { c, changed } = setup(task, {
      ...ready,
      enabled: true,
      nativeVerified: true,
      runRoot: true,
      reason: "ready",
      platform: "linux-aarch64",
      nativeProbeSupported: true,
    })
    const button = screen.getByRole("button", { name: "Dispatch task" })
    await waitFor(() => expect(button).toBeEnabled())
    fireEvent.click(button)
    await waitFor(() => expect(changed).toHaveBeenCalled())
    expect(c.dispatch).toHaveBeenCalledWith(task)
  })
  it("cancellation remains available after dispatch admission is disabled", async () => {
    const running = { ...task, status: "running" } as HallTaskDetail
    const { c, changed } = setup(running)
    await screen.findByText(/This platform lacks/)
    fireEvent.click(
      screen.getByRole("button", { name: "Request cancellation" })
    )
    await waitFor(() => expect(changed).toHaveBeenCalled())
    expect(c.cancel).toHaveBeenCalledWith(running)
  })
  it("does not apply a late response after a backend unmount", async () => {
    let resolve!: (x: HallTaskDetail) => void
    const { c, changed, unmount } = setup()
    c.refresh.mockReturnValue(
      new Promise((r) => {
        resolve = r
      })
    )
    fireEvent.click(screen.getByRole("button", { name: "Update progress" }))
    unmount()
    await act(async () => resolve(task))
    expect(changed).not.toHaveBeenCalled()
  })
  it("polls only active work and stops on a backend unmount", async () => {
    vi.useFakeTimers()
    const idle = setup()
    await act(async () => {
      await Promise.resolve()
      vi.advanceTimersByTime(5000)
    })
    expect(idle.c.refresh).not.toHaveBeenCalled()
    idle.unmount()
    const active = setup({ ...task, status: "running" } as HallTaskDetail)
    await act(async () => {
      await Promise.resolve()
      vi.advanceTimersByTime(2000)
      await Promise.resolve()
    })
    expect(active.c.refresh).toHaveBeenCalledTimes(1)
    active.unmount()
    await act(async () => {
      vi.advanceTimersByTime(10000)
    })
    expect(active.c.refresh).toHaveBeenCalledTimes(1)
  })
})

const orphan = {
  ...task,
  revision: 7,
  status: "interrupted",
  attempts: [{ attemptId: "att-1", conversationId: null }],
  runtimes: [{ stage: "needs_review", exitConfirmedAt: null }],
  exitChecks: [],
} as unknown as HallTaskDetail
describe("pending-review orphan release", () => {
  it("needs a reason and the risk acknowledgement, then submits task, attempt and revision", async () => {
    const { c, changed } = setup(orphan)
    // no active run and no connection id: the saved evidence alone qualifies
    const release = await screen.findByRole("button", {
      name: "Release capacity",
    })
    expect(release).toBeDisabled()
    const box = screen.getByRole("checkbox", {
      name: /accept that the processes may still be running/,
    })
    expect(box).toBeDisabled()
    fireEvent.change(screen.getByLabelText(/Reason/), {
      target: { value: "Checked task list" },
    })
    expect(release).toBeDisabled()
    fireEvent.click(box)
    expect(release).toBeEnabled()
    fireEvent.click(release)
    await waitFor(() => expect(changed).toHaveBeenCalled())
    expect(c.resolveOrphan).toHaveBeenCalledWith(
      orphan,
      "att-1",
      "Checked task list"
    )
  })

  it("reloads but never resubmits after a conflict", async () => {
    const { c, changed } = setup(orphan)
    c.resolveOrphan.mockRejectedValue(new Error("stale revision"))
    fireEvent.change(await screen.findByLabelText(/Reason/), {
      target: { value: "ok" },
    })
    fireEvent.click(screen.getByRole("checkbox", { name: /accept that/ }))
    fireEvent.click(screen.getByRole("button", { name: "Release capacity" }))
    await screen.findByText(/Release failed: stale revision/)
    expect(c.refresh).toHaveBeenCalledWith("task")
    expect(changed).toHaveBeenCalled()
    expect(c.resolveOrphan).toHaveBeenCalledTimes(1)
    expect(
      screen.getByRole("button", { name: "Release capacity" })
    ).toBeDisabled()
  })

  it("is absent for a remote relay, a resolved run, or a confirmed exit", async () => {
    const view = setup(orphan, ready, false)
    await screen.findByText(/lacks complete descendant/)
    expect(
      screen.queryByRole("button", { name: "Release capacity" })
    ).not.toBeInTheDocument()
    view.unmount()
    const done = setup({
      ...orphan,
      resolution: { kind: "operator_released", reason: "r", resolvedAt: "t" },
    } as unknown as HallTaskDetail)
    await screen.findByText(/Released by the operator/)
    expect(
      screen.queryByRole("button", { name: "Release capacity" })
    ).not.toBeInTheDocument()
    done.unmount()
    setup({
      ...orphan,
      runtimes: [{ stage: "needs_review", exitConfirmedAt: "t" }],
    } as unknown as HallTaskDetail)
    await screen.findByText(/lacks complete descendant/)
    expect(
      screen.queryByRole("button", { name: "Release capacity" })
    ).not.toBeInTheDocument()
  })
})
