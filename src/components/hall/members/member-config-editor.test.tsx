import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import type {
  HallMemberConfigDraftPreview,
  HallMemberConfigPreview,
  HallMembersClient,
} from "@/lib/hall/members"
import { MemberConfigPanel } from "./member-config-panel"

const baseline: HallMemberConfigPreview = {
  memberId: "member-a",
  revision: 4,
  valid: true,
  fileVersion: "a".repeat(64),
  overlay: { schema_version: 1, skills: { own: ["review"] } },
  files: [],
  effective: {
    model: "base",
    scope: { read: ["**"], write: ["src/**"], shell: [], network: "none" },
    skills: { shared: [], own: ["review"] },
  },
  fields: [],
  diagnostics: [],
}
const unchanged: HallMemberConfigDraftPreview = {
  memberId: "member-a",
  revision: 4,
  fileVersion: "a".repeat(64),
  valid: true,
  effective: baseline.effective,
  fields: [],
  diagnostics: [],
  changes: [],
}

async function setup(
  previewDraft = vi.fn().mockResolvedValue(unchanged),
  value = baseline,
  saveConfig = vi.fn()
) {
  const previewConfig = vi.fn().mockResolvedValue(value)
  const view = render(
    <NextIntlClientProvider locale="en" messages={en}>
      <MemberConfigPanel
        memberId="member-a"
        revision={4}
        client={
          {
            previewConfig,
            previewDraft,
            saveConfig,
          } as unknown as HallMembersClient
        }
      />
    </NextIntlClientProvider>
  )
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "Read configuration" }))
  )
  return { ...view, previewDraft, previewConfig, saveConfig }
}
function edit() {
  fireEvent.click(screen.getByRole("button", { name: "Edit draft" }))
}
async function preview() {
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "Preview changes" }))
  )
}

describe("member configuration draft", () => {
  const changed = {
    ...unchanged,
    changes: [
      {
        field: "skills.own",
        before: ["review"],
        after: [],
        beforeSource: ".onehall/agent.ui.yaml",
        afterSource: ".onehall/agent.ui.yaml",
      },
    ],
  }
  const saved: HallMemberConfigPreview = {
    ...baseline,
    saveSupported: true,
    fileVersion: "b".repeat(64),
    overlay: { schema_version: 1, skills: { own: [] } },
    effective: { ...baseline.effective!, skills: { shared: [], own: [] } },
  }

  it("saves explicitly after a valid preview and adopts the server's new baseline", async () => {
    const call = vi.fn().mockResolvedValue(saved)
    await setup(
      vi.fn().mockResolvedValue(changed),
      { ...baseline, saveSupported: true },
      call
    )
    edit()
    expect(screen.getByRole("button", { name: "Save overlay" })).toBeDisabled()
    fireEvent.change(screen.getByLabelText("Own skills"), {
      target: { value: "" },
    })
    await preview()
    expect(call).not.toHaveBeenCalled()
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Save overlay" }))
    )
    expect(call).toHaveBeenCalledExactlyOnceWith(
      "member-a",
      4,
      "a".repeat(64),
      { schema_version: 1, skills: { own: [] } }
    )
    expect(
      screen.getByText(
        "Overlay saved. Activation and access checks remain separate."
      )
    ).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Read configuration" })
    ).toBeEnabled()
    edit()
    expect(screen.queryByText(/Overlay saved/)).not.toBeInTheDocument()
    expect(screen.getByLabelText("Own skills")).toHaveValue("")
    expect(screen.getByRole("button", { name: "Save overlay" })).toBeDisabled()
  })

  it("preserves local networking and a key-only override through preview and save", async () => {
    const value: HallMemberConfigPreview = {
      ...baseline,
      saveSupported: true,
      overlay: { schema_version: 1, onedesk_key: "read" },
      effective: {
        ...baseline.effective!,
        scope: { ...baseline.effective!.scope, network: "local" },
        onedesk_key: "read",
      },
    }
    const changedKey: HallMemberConfigDraftPreview = {
      ...unchanged,
      changes: [
        {
          field: "onedesk_key",
          before: "read",
          after: "none",
          beforeSource: ".onehall/agent.ui.yaml",
          afterSource: "default",
        },
      ],
    }
    const saveConfig = vi
      .fn()
      .mockResolvedValue({ ...value, overlay: { schema_version: 1 } })
    const { previewDraft } = await setup(
      vi.fn().mockResolvedValue(changedKey),
      value,
      saveConfig
    )
    edit()
    expect(screen.getByLabelText("OneDesk key")).toHaveTextContent("read")
    fireEvent.click(
      screen.getByRole("switch", { name: "Override OneDesk key" })
    )
    fireEvent.click(
      screen.getByRole("switch", { name: "Override Network profile" })
    )
    expect(screen.getByLabelText("Network profile")).toHaveTextContent("local")
    await preview()
    const draft = { schema_version: 1, scope: { network: "local" } }
    expect(previewDraft).toHaveBeenLastCalledWith(
      "member-a",
      4,
      "a".repeat(64),
      draft
    )
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Save overlay" }))
    )
    expect(saveConfig).toHaveBeenCalledExactlyOnceWith(
      "member-a",
      4,
      "a".repeat(64),
      draft
    )
  })

  it("editing after preview revokes save eligibility", async () => {
    const { saveConfig } = await setup(vi.fn().mockResolvedValue(changed), {
      ...baseline,
      saveSupported: true,
    })
    edit()
    await preview()
    expect(screen.getByRole("button", { name: "Save overlay" })).toBeEnabled()
    fireEvent.change(screen.getByLabelText("Own skills"), {
      target: { value: "different" },
    })
    expect(screen.getByRole("button", { name: "Save overlay" })).toBeDisabled()
    fireEvent.click(screen.getByRole("button", { name: "Save overlay" }))
    expect(saveConfig).not.toHaveBeenCalled()
  })

  it("keeps an unchanged or invalid preview unsaveable", async () => {
    const call = vi
      .fn()
      .mockResolvedValueOnce(unchanged)
      .mockResolvedValueOnce({ ...changed, valid: false, diagnostics: [] })
    await setup(call, { ...baseline, saveSupported: true })
    edit()
    await preview()
    expect(screen.getByRole("button", { name: "Save overlay" })).toBeDisabled()
    await preview()
    expect(screen.getByRole("button", { name: "Save overlay" })).toBeDisabled()
  })

  it("blocks duplicate saves, field changes and discard while saving", async () => {
    let resolve!: (value: HallMemberConfigPreview) => void
    const call = vi.fn().mockReturnValue(
      new Promise<HallMemberConfigPreview>((done) => {
        resolve = done
      })
    )
    await setup(
      vi.fn().mockResolvedValue(changed),
      { ...baseline, saveSupported: true },
      call
    )
    edit()
    await preview()
    fireEvent.click(screen.getByRole("button", { name: "Save overlay" }))
    fireEvent.click(screen.getByRole("button", { name: "Saving…" }))
    expect(call).toHaveBeenCalledTimes(1)
    expect(screen.getByRole("button", { name: "Discard draft" })).toBeDisabled()
    expect(screen.getByLabelText("Own skills")).toBeDisabled()
    await act(async () => resolve(saved))
    expect(
      screen.getByRole("button", { name: "Edit draft" })
    ).toBeInTheDocument()
  })

  it.each(["already_exists", "io_error", "not_implemented"])(
    "requires a fresh read after save %s",
    async (code) => {
      await setup(
        vi.fn().mockResolvedValue(changed),
        { ...baseline, saveSupported: true },
        vi.fn().mockRejectedValue({ code, message: "private details" })
      )
      edit()
      await preview()
      await act(async () =>
        fireEvent.click(screen.getByRole("button", { name: "Save overlay" }))
      )
      expect(screen.getByRole("alert")).not.toHaveTextContent("private details")
      expect(
        screen.getByRole("button", { name: "Preview changes" })
      ).toBeDisabled()
      expect(
        screen.getByRole("button", { name: "Save overlay" })
      ).toBeDisabled()
      expect(
        screen.getByRole("button", { name: "Discard draft" })
      ).toBeEnabled()
      fireEvent.click(screen.getByRole("button", { name: "Discard draft" }))
      expect(
        screen.getByRole("button", { name: "Read configuration" })
      ).toBeEnabled()
    }
  )

  it("rejects an unconfirmed save response from another member", async () => {
    await setup(
      vi.fn().mockResolvedValue(changed),
      { ...baseline, saveSupported: true },
      vi.fn().mockResolvedValue({ ...saved, memberId: "other" })
    )
    edit()
    await preview()
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Save overlay" }))
    )
    expect(screen.getByRole("alert")).toHaveTextContent(
      "could not be confirmed"
    )
    expect(screen.queryByText(/Overlay saved/)).not.toBeInTheDocument()
  })

  it("does not apply a late save reply to a closed backend panel", async () => {
    let resolve!: (value: HallMemberConfigPreview) => void
    const view = await setup(
      vi.fn().mockResolvedValue(changed),
      { ...baseline, saveSupported: true },
      vi.fn().mockReturnValue(
        new Promise<HallMemberConfigPreview>((done) => {
          resolve = done
        })
      )
    )
    edit()
    await preview()
    fireEvent.click(screen.getByRole("button", { name: "Save overlay" }))
    view.unmount()
    await act(async () => resolve(saved))
    expect(screen.queryByText(/Overlay saved/)).not.toBeInTheDocument()
  })

  it("opens explicitly and preserves sparse overrides, inheritance and explicit empty arrays", async () => {
    const { previewDraft, previewConfig } = await setup()
    expect(previewDraft).not.toHaveBeenCalled()
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument()
    edit()
    expect(
      screen.getByRole("button", { name: "Read configuration" })
    ).toBeDisabled()
    expect(screen.getByLabelText("Own skills")).toHaveValue("review")
    fireEvent.click(
      screen.getByRole("switch", { name: "Override Model declaration" })
    )
    expect(screen.getByLabelText("Model declaration")).toHaveValue("base")
    fireEvent.change(screen.getByLabelText("Model declaration"), {
      target: { value: "new-model" },
    })
    fireEvent.click(
      screen.getByRole("switch", { name: "Override Write scope" })
    )
    expect(screen.getByLabelText("Write scope")).toHaveValue("src/**")
    fireEvent.change(screen.getByLabelText("Write scope"), {
      target: { value: "\n " },
    })
    await preview()
    expect(previewDraft).toHaveBeenCalledWith("member-a", 4, "a".repeat(64), {
      schema_version: 1,
      model: "new-model",
      scope: { write: [] },
      skills: { own: ["review"] },
    })
    expect(screen.getByText("No value or source changes")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("switch", { name: "Override Own skills" }))
    expect(
      screen.queryByText("No value or source changes")
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText("Own skills")).not.toBeInTheDocument()
    await preview()
    expect(previewDraft).toHaveBeenLastCalledWith(
      "member-a",
      4,
      "a".repeat(64),
      {
        schema_version: 1,
        model: "new-model",
        scope: { write: [] },
      }
    )
    expect(previewConfig).toHaveBeenCalledTimes(1)
    expect(
      screen.queryByRole("button", { name: /save/i })
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Discard draft" }))
    expect(
      screen.getByRole("button", { name: "Read configuration" })
    ).toBeEnabled()
    edit()
    expect(
      screen.getByRole("switch", { name: "Override Model declaration" })
    ).not.toBeChecked()
  })

  it("shows before and after values and sources, then clears them on editing", async () => {
    const previewDraft = vi.fn().mockResolvedValue({
      ...unchanged,
      changes: [
        {
          field: "model",
          before: "base",
          after: "new-model",
          beforeSource: "agent.yaml",
          afterSource: ".onehall/agent.ui.yaml",
        },
      ],
    })
    await setup(previewDraft)
    edit()
    await preview()
    expect(
      screen.getByText("Proposed value and source changes")
    ).toBeInTheDocument()
    expect(screen.getByText("Current")).toBeInTheDocument()
    expect(screen.getByText("Draft overlay")).toBeInTheDocument()
    expect(screen.getByText("base")).toBeInTheDocument()
    expect(screen.getByText("new-model")).toBeInTheDocument()
    fireEvent.change(screen.getByLabelText("Own skills"), {
      target: { value: "other\n second" },
    })
    expect(screen.queryByText("new-model")).not.toBeInTheDocument()
    await preview()
    expect(previewDraft.mock.lastCall?.[3]).toEqual({
      schema_version: 1,
      skills: { own: ["other", "second"] },
    })
  })

  it.each(["already_exists", "not_found", "not_implemented", "io_error"])(
    "handles %s without using another backend",
    async (code) => {
      const { previewDraft } = await setup(
        vi.fn().mockRejectedValue({ code, message: "private server error" })
      )
      edit()
      await preview()
      expect(screen.getByRole("alert")).not.toHaveTextContent(
        "private server error"
      )
      expect(screen.getByRole("alert")).toHaveTextContent(
        code === "not_implemented"
          ? "supports viewing only"
          : code === "io_error"
            ? "Try again"
            : "changed"
      )
      expect(previewDraft).toHaveBeenCalledTimes(1)
    }
  )

  it("rejects a result from another member or file version", async () => {
    const call = vi
      .fn()
      .mockResolvedValue({ ...unchanged, fileVersion: "b".repeat(64) })
    await setup(call)
    edit()
    await preview()
    expect(screen.getByRole("alert")).toHaveTextContent("changed")
    expect(
      screen.queryByText("No value or source changes")
    ).not.toBeInTheDocument()
  })

  it("does not render partial values or raw unknown diagnostic keys", async () => {
    await setup(
      vi.fn().mockResolvedValue({
        ...unchanged,
        valid: false,
        effective: null,
        diagnostics: [
          { code: "__proto__", source: "draft", field: "document" },
        ],
        changes: [
          { field: "model", before: "base", after: "should-not-display" },
        ],
      })
    )
    edit()
    await preview()
    expect(screen.getByRole("alert")).toHaveTextContent("unsupported")
    expect(screen.queryByText("should-not-display")).not.toBeInTheDocument()
  })

  it("keeps older read-only backends usable", async () => {
    const { previewDraft } = await setup(undefined, {
      ...baseline,
      fileVersion: undefined,
      overlay: undefined,
    })
    expect(screen.getByText(/supports viewing only/)).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Edit draft" })
    ).not.toBeInTheDocument()
    expect(previewDraft).not.toHaveBeenCalled()
  })

  it("blocks duplicate requests and ignores a response after discard and reopen", async () => {
    let resolve!: (value: HallMemberConfigDraftPreview) => void
    const call = vi.fn().mockReturnValue(
      new Promise<HallMemberConfigDraftPreview>((done) => {
        resolve = done
      })
    )
    await setup(call)
    edit()
    fireEvent.click(screen.getByRole("button", { name: "Preview changes" }))
    fireEvent.click(screen.getByRole("button", { name: "Previewing…" }))
    expect(call).toHaveBeenCalledTimes(1)
    expect(screen.getByLabelText("Own skills")).toBeDisabled()
    fireEvent.click(screen.getByRole("button", { name: "Discard draft" }))
    edit()
    await act(async () => resolve(unchanged))
    expect(
      screen.queryByText("No value or source changes")
    ).not.toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Preview changes" })
    ).toBeEnabled()
  })

  it("never picks an execution method for the operator", async () => {
    const { previewDraft } = await setup(vi.fn().mockResolvedValue(changed), {
      ...baseline,
      saveSupported: true,
    })
    edit()
    expect(screen.getByLabelText("Override Execution method")).not.toBeChecked()
    fireEvent.click(
      screen.getByRole("switch", { name: "Override Execution method" })
    )
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Choose an execution method, or turn the override off."
    )
    expect(
      screen.getByRole("button", { name: "Preview changes" })
    ).toBeDisabled()
    expect(screen.getByText(/earlier approvals invalid/)).toBeInTheDocument()
    expect(previewDraft).not.toHaveBeenCalled()
    fireEvent.click(
      screen.getByRole("switch", { name: "Override Execution method" })
    )
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })
})
