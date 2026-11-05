import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import { ConnectionRecoveryNotice } from "./connection-recovery-notice"

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

function notice(overrides = {}) {
  return (
    <NextIntlClientProvider locale="en" messages={en}>
      <ConnectionRecoveryNotice
        contextKey="tab-1"
        connectionId="conn-1"
        waiting
        resync={async () => true}
        {...overrides}
      />
    </NextIntlClientProvider>
  )
}

it("reveals a stalled initialization without automatically restarting it", async () => {
  let calls = 0
  render(
    notice({
      resync: async () => {
        calls++
        return true
      },
    })
  )
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
  act(() => vi.advanceTimersByTime(30_000))
  expect(screen.getByRole("status")).toBeInTheDocument()
  expect(calls).toBe(0)
  await act(async () => fireEvent.click(screen.getByRole("button")))
  expect(calls).toBe(1)
})

it("keeps an unknown connection outcome visible without offering a duplicate spawn", () => {
  render(notice({ connectionId: null }))
  act(() => vi.advanceTimersByTime(30_000))
  expect(screen.getByRole("status")).toBeInTheDocument()
  expect(screen.queryByRole("button")).not.toBeInTheDocument()
})

it("clears on initialization completion and gives a replacement a fresh grace", () => {
  const view = render(notice())
  act(() => vi.advanceTimersByTime(30_000))
  expect(screen.getByRole("status")).toBeInTheDocument()
  view.rerender(notice({ waiting: false }))
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
  view.rerender(notice())
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
  view.rerender(notice({ connectionId: "conn-2" }))
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
  act(() => vi.advanceTimersByTime(29_999))
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
})

it("reports a failed synchronization without losing the recovery action", async () => {
  render(notice({ resync: async () => false }))
  act(() => vi.advanceTimersByTime(30_000))
  await act(async () => fireEvent.click(screen.getByRole("button")))
  expect(screen.getByRole("alert")).toBeInTheDocument()
  expect(screen.getByRole("button")).toBeEnabled()
})
