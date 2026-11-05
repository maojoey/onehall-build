import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import type { HallMembersClient } from "@/lib/hall/members"
import { MemberConfigPanel } from "./member-config-panel"

const config = {
  memberId: "member-a",
  revision: 4,
  valid: true,
  files: [{ path: "agent.yaml", status: "loaded" }],
  effective: {
    model: null,
    scope: { read: ["**"], write: [], shell: [], network: "none" },
    skills: { shared: [], own: ["review"] },
  },
  fields: [
    {
      field: "scope.write",
      source: ".onehall/agent.ui.yaml",
      overridden: true,
    },
    { field: "skills.own", source: "agent.yaml", overridden: false },
  ],
  diagnostics: [],
}
function setup(
  previewConfig = vi.fn().mockResolvedValue(config),
  revision = 4
) {
  const client = { previewConfig } as unknown as HallMembersClient
  const view = render(
    <NextIntlClientProvider locale="en" messages={en}>
      <MemberConfigPanel
        memberId="member-a"
        revision={revision}
        client={client}
      />
    </NextIntlClientProvider>
  )
  return { ...view, previewConfig }
}

describe("member declaration preview", () => {
  it("reads only on request, keeps arrays empty, and shows origins without writing", async () => {
    const { previewConfig } = setup()
    expect(previewConfig).not.toHaveBeenCalled()
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Read configuration" })
      )
    })
    expect(previewConfig).toHaveBeenCalledWith("member-a", 4)
    expect(screen.getByText("Overrides handwritten value")).toBeInTheDocument()
    expect(screen.getByText("review")).toBeInTheDocument()
    expect(screen.getByText("Empty")).toBeInTheDocument()
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /save/i })
    ).not.toBeInTheDocument()
  })

  it("does not display partial configuration on validation failure", async () => {
    setup(
      vi.fn().mockResolvedValue({
        ...config,
        valid: false,
        diagnostics: [
          { code: "identity_conflict", source: "agent.yaml", field: "cwd" },
        ],
      })
    )
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Read configuration" })
      )
    })
    expect(screen.getByRole("alert")).toHaveTextContent(
      "does not match the member registry"
    )
    expect(screen.queryByText("review")).not.toBeInTheDocument()
  })

  it("rejects a response for a different registry revision", async () => {
    setup(vi.fn().mockResolvedValue({ ...config, revision: 3 }))
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Read configuration" })
      )
    })
    expect(screen.getByRole("alert")).toHaveTextContent("changed elsewhere")
    expect(screen.queryByText("review")).not.toBeInTheDocument()
  })

  it("uses a fixed generic message for an unknown diagnostic code", async () => {
    const result = {
      ...config,
      valid: false,
      diagnostics: [
        { code: "__proto__", source: "agent.yaml", field: "document" },
      ],
    }
    setup(vi.fn().mockResolvedValue(result))
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Read configuration" })
      )
    })
    expect(screen.getByRole("alert")).toHaveTextContent(
      "A field value or YAML tag is unsupported"
    )
    expect(screen.queryByText("review")).not.toBeInTheDocument()
  })

  it("retries failures explicitly and explains an old backend without fallback", async () => {
    const call = vi
      .fn()
      .mockRejectedValueOnce({ code: "not_implemented", message: "missing" })
      .mockResolvedValue(config)
    setup(call)
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Read configuration" })
      )
    })
    expect(screen.getByRole("alert")).toHaveTextContent(
      "does not support configuration preview"
    )
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Read configuration" })
      )
    })
    expect(call).toHaveBeenCalledTimes(2)
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it("suppresses duplicate reads and discards a late reply after unmount", async () => {
    let finish!: (value: unknown) => void
    const pending = new Promise((resolve) => {
      finish = resolve
    })
    const call = vi.fn().mockReturnValue(pending)
    const { unmount } = setup(call)
    const button = screen.getByRole("button", { name: "Read configuration" })
    fireEvent.click(button)
    fireEvent.click(button)
    expect(call).toHaveBeenCalledTimes(1)
    expect(button).toBeDisabled()
    unmount()
    setup(vi.fn(), 5)
    await act(async () => {
      finish(config)
    })
    expect(screen.queryByText("review")).not.toBeInTheDocument()
  })
})
