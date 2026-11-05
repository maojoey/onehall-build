import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import { WebBranchStatus } from "./web-branch-status"

const shell = vi.hoisted(() => ({
  state: "connected",
  unauthorized: vi.fn(),
  listeners: new Set<() => void>(),
}))
vi.mock("@/lib/transport/web-connection-store", () => ({
  subscribeWebConnection: (fn: () => void) => {
    shell.listeners.add(fn)
    return () => shell.listeners.delete(fn)
  },
  getWebConnectionSnapshot: () => shell.state,
  getWebConnectionServerSnapshot: () => "connected",
  notifyWebUnauthorized: shell.unauthorized,
}))
function health() {
  let state = "reconnecting"
  let failure: string | null = null
  const listeners = new Set<() => void>()
  return {
    getConnectionSnapshot: () => state,
    getBranchFailure: () => failure,
    subscribeConnection: (fn: () => void) => {
      listeners.add(fn)
      return () => listeners.delete(fn)
    },
    reconnectNow: vi.fn(),
    eventStream: vi.fn(),
    set(next: string, code: string | null = null) {
      state = next
      failure = code
      listeners.forEach((fn) => fn())
    },
  }
}
beforeEach(() => {
  vi.useFakeTimers()
  shell.state = "connected"
  shell.unauthorized.mockClear()
  shell.listeners.clear()
})
afterEach(() => vi.useRealTimers())
function mount() {
  const transport = health()
  const leave = vi.fn()
  const view = render(
    <NextIntlClientProvider locale="en" messages={en}>
      <WebBranchStatus
        name="Windows"
        transport={transport as never}
        onHeadquarters={leave}
      />
    </NextIntlClientProvider>
  )
  return { transport, leave, ...view }
}

it("shows scoped retry and an escape to headquarters after a grace period", () => {
  const { transport, leave } = mount()
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
  act(() => vi.advanceTimersByTime(4_000))
  expect(screen.getByRole("status")).toHaveTextContent("Windows")
  fireEvent.click(
    screen.getByRole("button", { name: en.WebConnection.reconnectNow })
  )
  expect(transport.reconnectNow).toHaveBeenCalledOnce()
  fireEvent.click(
    screen.getByRole("button", { name: en.Hall.branches.headquarters })
  )
  expect(leave).toHaveBeenCalledOnce()
  act(() => transport.set("connected"))
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
})

it("escalates a headquarters 401 observed through a branch to the shell", () => {
  const { transport } = mount()
  act(() => transport.set("unauthorized"))
  expect(shell.unauthorized).toHaveBeenCalledWith("codeg")
  act(() => transport.set("proxyUnauthorized"))
  expect(shell.unauthorized).toHaveBeenCalledWith("proxy")
})

it("keeps branch credential failures distinct from headquarters login", () => {
  const { transport } = mount()
  act(() => transport.set("branchUnavailable", "branch_auth_failed"))
  expect(screen.getByRole("status")).toHaveTextContent(
    en.Hall.branches.status.auth_failed
  )
  expect(shell.unauthorized).not.toHaveBeenCalled()
})

it("yields to the shell guard and detaches from the outgoing branch", () => {
  const { unmount, transport } = mount()
  act(() => vi.advanceTimersByTime(4_000))
  act(() => {
    shell.state = "reconnecting"
    shell.listeners.forEach((fn) => fn())
  })
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
  unmount()
  act(() => transport.set("unauthorized"))
  expect(shell.unauthorized).not.toHaveBeenCalled()
})
