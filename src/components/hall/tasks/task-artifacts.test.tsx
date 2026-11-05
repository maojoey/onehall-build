import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import report from "../../../../src-tauri/src/hall/exchange/contract_samples/report_v2.json"
import type { ArtifactRef, ExchangeClient } from "@/lib/hall/exchange"
import {
  DispatchTaskArtifacts,
  TaskArtifacts,
  exitState,
} from "./task-artifacts"

const sample = report.artifacts[0] as ArtifactRef
const verified: ArtifactRef = { ...sample, exitVerified: true }
function wrap(node: React.ReactNode) {
  return render(
    <NextIntlClientProvider locale="en" messages={en}>
      {node}
    </NextIntlClientProvider>
  )
}

describe("four separate states", () => {
  it("shows files collected while the task is interrupted and exit unconfirmed", () => {
    wrap(
      <TaskArtifacts
        taskStatus="interrupted"
        exitConfirmedAt={null}
        reported
        collection={{ state: "ready", artifacts: [sample] }}
      />
    )
    expect(screen.getByText("Task interrupted")).toBeInTheDocument()
    expect(screen.getByText("Exit not confirmed")).toBeInTheDocument()
    expect(screen.getByText("Files collected")).toBeInTheDocument()
    expect(screen.getByText("Report received")).toBeInTheDocument()
    expect(screen.queryByText("Task succeeded")).not.toBeInTheDocument()
    expect(screen.getByRole("alert")).toHaveTextContent(/not a success/)
  })

  it("never calls a finished task a success while exit is unverified", () => {
    wrap(
      <TaskArtifacts
        taskStatus="succeeded"
        reported
        collection={{ state: "ready", artifacts: [sample] }}
      />
    )
    expect(screen.queryByText("Task succeeded")).not.toBeInTheDocument()
    expect(
      screen.getByText("Task finished, but exit is not confirmed")
    ).toBeInTheDocument()
  })

  it("says success only with a verified exit", () => {
    wrap(
      <TaskArtifacts
        taskStatus="succeeded"
        reported
        collection={{ state: "ready", artifacts: [verified] }}
      />
    )
    expect(screen.getByText("Task succeeded")).toBeInTheDocument()
    expect(screen.getByText("Exit confirmed")).toBeInTheDocument()
  })

  it("treats missing evidence as unknown, not as good news", () => {
    expect(exitState(undefined, null)).toBe("unknown")
    expect(exitState(undefined, [])).toBe("unknown")
    expect(exitState(null, null)).toBe("unconfirmed")
    expect(exitState(null, null, false)).toBe("unknown")
    expect(exitState("2026-11-02T00:00:00Z", [sample])).toBe("unconfirmed")
    wrap(
      <TaskArtifacts
        taskStatus="succeeded"
        collection={{ state: "unsupported" }}
      />
    )
    expect(screen.getByText("Report unknown")).toBeInTheDocument()
    expect(screen.getByText("Not supported by this server")).toBeInTheDocument()
  })

  it("renders an empty list and a failed collection", () => {
    const view = wrap(
      <TaskArtifacts
        taskStatus="failed"
        collection={{ state: "ready", artifacts: [] }}
      />
    )
    expect(screen.getByText("No files collected")).toBeInTheDocument()
    view.unmount()
    wrap(<TaskArtifacts taskStatus="failed" collection={{ state: "failed" }} />)
    expect(screen.getByText("Collection failed")).toBeInTheDocument()
  })

  it("renders on the server without effects", async () => {
    const { renderToString } = await import("react-dom/server")
    const html = renderToString(
      <NextIntlClientProvider locale="en" messages={en}>
        <TaskArtifacts taskStatus="running" collection={{ state: "loading" }} />
      </NextIntlClientProvider>
    )
    expect(html).toContain("Running")
  })
})

describe("saving a collected file", () => {
  it("is keyboard operable and reports a failed save", async () => {
    const download = vi.fn().mockRejectedValue(new Error("exists"))
    wrap(
      <TaskArtifacts
        taskStatus="interrupted"
        exitConfirmedAt={null}
        collection={{ state: "ready", artifacts: [sample] }}
        download={download}
      />
    )
    const button = screen.getByRole("button", { name: "Save file" })
    button.focus()
    expect(button).toHaveFocus()
    fireEvent.click(button)
    await waitFor(() => expect(download).toHaveBeenCalledWith(sample))
    expect(
      await screen.findByText(/Could not save the file: exists/)
    ).toBeInTheDocument()
  })

  it("disables saving where the shell cannot save", () => {
    wrap(
      <TaskArtifacts
        taskStatus="interrupted"
        collection={{ state: "ready", artifacts: [sample] }}
      />
    )
    expect(screen.getByRole("button", { name: "Save file" })).toBeDisabled()
    expect(screen.getByText(/cannot save files/)).toBeInTheDocument()
  })
})

describe("loading from the host", () => {
  it("loads the published artifact fixture", async () => {
    const exchange = {
      artifacts: vi.fn().mockResolvedValue([sample]),
    } as unknown as ExchangeClient
    wrap(
      <DispatchTaskArtifacts
        exchange={exchange}
        dispatchId={sample.dispatchId}
        taskStatus="interrupted"
        exitConfirmedAt={null}
      />
    )
    expect(await screen.findByText("Files collected")).toBeInTheDocument()
    expect(screen.getByText("out.txt")).toBeInTheDocument()
  })

  it("shows unsupported for an old server and failed for a sync error", async () => {
    const view = wrap(
      <DispatchTaskArtifacts dispatchId="d" taskStatus="succeeded" />
    )
    expect(screen.getByText("Not supported by this server")).toBeInTheDocument()
    view.unmount()
    const exchange = {
      artifacts: vi.fn().mockRejectedValue(new Error("401")),
    } as unknown as ExchangeClient
    wrap(
      <DispatchTaskArtifacts
        exchange={exchange}
        dispatchId="d"
        taskStatus="succeeded"
      />
    )
    expect(await screen.findByText("Collection failed")).toBeInTheDocument()
  })

  it("ignores a late answer after unmount", async () => {
    let finish!: (rows: ArtifactRef[]) => void
    const exchange = {
      artifacts: vi.fn().mockReturnValue(new Promise((r) => (finish = r))),
    } as unknown as ExchangeClient
    const view = wrap(
      <DispatchTaskArtifacts
        exchange={exchange}
        dispatchId="d"
        taskStatus="failed"
      />
    )
    view.unmount()
    await act(async () => finish([sample]))
    expect(screen.queryByText("out.txt")).not.toBeInTheDocument()
  })
})

it("does not call a running task's exit unconfirmed", () => {
  wrap(
    <TaskArtifacts
      taskStatus="running"
      exitConfirmedAt={null}
      collection={{ state: "loading" }}
    />
  )
  expect(screen.getByText("Exit unknown")).toBeInTheDocument()
})
