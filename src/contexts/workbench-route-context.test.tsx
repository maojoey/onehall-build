import { fireEvent, render } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import * as lifetime from "@/lib/hall/workspace-lifetime"

import {
  WorkbenchRouteProvider,
  useWorkbenchRoute,
} from "./workbench-route-context"

function Probe() {
  const { routeId, isConversations, setRoute, openConversations } =
    useWorkbenchRoute()
  return (
    <div>
      <span data-testid="route">{routeId}</span>
      <span data-testid="isConv">{String(isConversations)}</span>
      <button onClick={() => setRoute("automations")}>go</button>
      <button onClick={openConversations}>back</button>
    </div>
  )
}

describe("WorkbenchRouteProvider", () => {
  it("defaults to the conversation workspace and switches routes", () => {
    const { getByTestId, getByText } = render(
      <WorkbenchRouteProvider>
        <Probe />
      </WorkbenchRouteProvider>
    )
    expect(getByTestId("route").textContent).toBe("conversations")
    expect(getByTestId("isConv").textContent).toBe("true")

    fireEvent.click(getByText("go"))
    expect(getByTestId("route").textContent).toBe("automations")
    expect(getByTestId("isConv").textContent).toBe("false")

    fireEvent.click(getByText("back"))
    expect(getByTestId("route").textContent).toBe("conversations")
    expect(getByTestId("isConv").textContent).toBe("true")
  })

  it("throws when used outside the provider", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {})
    expect(() => render(<Probe />)).toThrow(/WorkbenchRouteProvider/)
    spy.mockRestore()
  })

  it("restores each open backend's route and forgets it on close", () => {
    let key = "remote:7"
    const capture = vi
      .spyOn(lifetime, "captureHallWorkspace")
      .mockImplementation(() => ({ key, isLeaving: () => false }))
    const first = render(
      <WorkbenchRouteProvider>
        <Probe />
      </WorkbenchRouteProvider>
    )
    fireEvent.click(first.getByText("go"))
    first.unmount()
    key = "remote:8"
    const second = render(
      <WorkbenchRouteProvider>
        <Probe />
      </WorkbenchRouteProvider>
    )
    expect(second.getByTestId("route").textContent).toBe("conversations")
    second.unmount()
    key = "remote:7"
    const again = render(
      <WorkbenchRouteProvider>
        <Probe />
      </WorkbenchRouteProvider>
    )
    expect(again.getByTestId("route").textContent).toBe("automations")
    again.unmount()
    lifetime.forgetParkedOwnerships(key)
    const closed = render(
      <WorkbenchRouteProvider>
        <Probe />
      </WorkbenchRouteProvider>
    )
    expect(closed.getByTestId("route").textContent).toBe("conversations")
    closed.unmount()
    capture.mockRestore()
  })
})
