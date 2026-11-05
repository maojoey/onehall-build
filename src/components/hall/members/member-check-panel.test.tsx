import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import type {
  HallMemberCheckReport,
  HallMemberConfigPreview,
  HallMembersClient,
} from "@/lib/hall/members"
import { MemberConfigPanel } from "./member-config-panel"

const config: HallMemberConfigPreview = {
  memberId: "member-a",
  revision: 4,
  valid: true,
  files: [],
  fields: [],
  diagnostics: [],
  effective: {
    model: null,
    scope: { read: ["**"], write: [], shell: [], network: "none" },
    skills: { shared: [], own: [] },
  },
  fileVersion: "a".repeat(64),
  overlay: { schema_version: 1 },
  saveSupported: true,
  checkSupported: true,
  checkVersion: "c".repeat(64),
}
const report: HallMemberCheckReport = {
  memberId: "member-a",
  revision: 4,
  checkVersion: "c".repeat(64),
  configurationValid: true,
  activationAvailable: false,
  diagnostics: [],
  templateRecordedRevision: null,
  templateCurrentRevision: null,
  checks: [
    { id: "directory", status: "passed", code: "directory_ok" },
    { id: "declaration", status: "warning", code: "defaults_only" },
    { id: "engine", status: "passed", code: "engine_codex" },
    { id: "template", status: "warning", code: "template_not_bound" },
    { id: "project", status: "warning", code: "project_not_bound" },
    { id: "runtime", status: "pending", code: "runtime_unverified" },
    { id: "execution", status: "pending", code: "execution_pending" },
  ],
}
const passed = "Saved data checks passed. Execution checks are still pending."
function setup(
  checkMember = vi.fn().mockResolvedValue(report),
  value = config
) {
  const previewConfig = vi.fn().mockResolvedValue(value)
  const previewDraft = vi.fn().mockResolvedValue({
    memberId: value.memberId,
    revision: 4,
    fileVersion: value.fileVersion,
    valid: true,
    diagnostics: [],
    changes: [
      {
        field: "model",
        before: null,
        after: "new-model",
        beforeSource: "defaults",
        afterSource: ".onehall/agent.ui.yaml",
      },
    ],
  })
  const saveConfig = vi.fn().mockResolvedValue({
    ...value,
    checkVersion: "d".repeat(64),
    fileVersion: "b".repeat(64),
    overlay: { schema_version: 1, model: "new-model" },
  })
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
            checkMember,
          } as unknown as HallMembersClient
        }
      />
    </NextIntlClientProvider>
  )
  return { ...view, previewConfig, previewDraft, saveConfig, checkMember }
}
async function click(name: string) {
  await act(async () => fireEvent.click(screen.getByRole("button", { name })))
}

describe("member pre-activation saved-data checks", () => {
  it("checks explicitly on the observed backend and never offers activation", async () => {
    const { checkMember } = setup()
    expect(
      screen.queryByRole("button", { name: "Check saved data" })
    ).not.toBeInTheDocument()
    await click("Read configuration")
    expect(checkMember).not.toHaveBeenCalled()
    await click("Check saved data")
    expect(checkMember).toHaveBeenCalledExactlyOnceWith(
      "member-a",
      4,
      "c".repeat(64)
    )
    expect(screen.getByText(passed)).toBeInTheDocument()
    expect(screen.getAllByText("Unverified")).toHaveLength(2)
    expect(screen.getByText(/The engine was not started/)).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /activate|enable/i })
    ).not.toBeInTheDocument()
  })

  it("can check an invalid declaration observation and displays fixed diagnostics", async () => {
    const checkMember = vi.fn().mockResolvedValue({
      ...report,
      configurationValid: false,
      checks: [
        { id: "declaration", status: "blocked", code: "declaration_invalid" },
      ],
      diagnostics: [
        { code: "unknown_field", source: "agent.yaml", field: "document" },
      ],
    })
    setup(checkMember, {
      ...config,
      valid: false,
      effective: null,
      overlay: undefined,
      fileVersion: undefined,
    })
    await click("Read configuration")
    await click("Check saved data")
    expect(
      screen.getByText(
        "Saved data needs changes. The member cannot be activated yet."
      )
    ).toBeInTheDocument()
    expect(screen.getByText("Fix required")).toBeInTheDocument()
    expect(
      screen.getByText("Fix the declaration errors shown below.")
    ).toBeInTheDocument()
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Unsupported fields are present."
    )
  })

  it("shows current and recorded template revisions without exposing template content", async () => {
    setup(
      vi.fn().mockResolvedValue({
        ...report,
        configurationValid: false,
        templateRecordedRevision: 7,
        templateCurrentRevision: 8,
        checks: [
          { id: "template", status: "blocked", code: "template_changed" },
        ],
      })
    )
    await click("Read configuration")
    await click("Check saved data")
    expect(
      screen.getByText(/Recorded version 7; current version 8/)
    ).toBeInTheDocument()
  })

  it.each(["already_exists", "not_found", "not_implemented", "io_error"])(
    "handles %s without echoing backend errors",
    async (code) => {
      setup(vi.fn().mockRejectedValue({ code, message: "private server text" }))
      await click("Read configuration")
      await click("Check saved data")
      expect(screen.getByRole("alert")).not.toHaveTextContent(
        "private server text"
      )
      expect(screen.getByRole("alert")).toHaveTextContent(
        code === "not_implemented"
          ? "does not support"
          : code === "io_error"
            ? "Try again"
            : "changed"
      )
      expect(screen.queryByText(passed)).not.toBeInTheDocument()
    }
  )

  it.each([
    { checkVersion: "x".repeat(64) },
    { memberId: "other" },
    { revision: 3 },
    { activationAvailable: true },
  ])("rejects a mismatched report %j", async (patch) => {
    setup(vi.fn().mockResolvedValue({ ...report, ...patch }))
    await click("Read configuration")
    await click("Check saved data")
    expect(screen.getByRole("alert")).toHaveTextContent("changed")
    expect(screen.queryByText(passed)).not.toBeInTheDocument()
  })

  it("uses a fixed message for unknown server check codes", async () => {
    setup(
      vi.fn().mockResolvedValue({
        ...report,
        configurationValid: false,
        checks: [{ id: "template", status: "blocked", code: "__proto__" }],
      })
    )
    await click("Read configuration")
    await click("Check saved data")
    expect(
      screen.getByText(
        "This check result is not recognized. Read the configuration again."
      )
    ).toBeInTheDocument()
  })

  it("withdraws results on editing, discard and reread", async () => {
    const { previewConfig } = setup()
    await click("Read configuration")
    await click("Check saved data")
    await click("Edit draft")
    expect(screen.queryByText(passed)).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Check saved data" })
    ).not.toBeInTheDocument()
    await click("Discard draft")
    expect(screen.queryByText(passed)).not.toBeInTheDocument()
    await click("Check saved data")
    expect(screen.getByText(passed)).toBeInTheDocument()
    previewConfig.mockResolvedValue({ ...config, checkVersion: "d".repeat(64) })
    await click("Read configuration")
    expect(screen.queryByText(passed)).not.toBeInTheDocument()
  })

  it("requires a new check after saving and uses the saved observation version", async () => {
    const checkMember = vi
      .fn()
      .mockImplementation((memberId, revision, checkVersion) =>
        Promise.resolve({ ...report, memberId, revision, checkVersion })
      )
    setup(checkMember)
    await click("Read configuration")
    await click("Check saved data")
    await click("Edit draft")
    fireEvent.click(
      screen.getByRole("switch", { name: "Override Model declaration" })
    )
    fireEvent.change(screen.getByLabelText("Model declaration"), {
      target: { value: "new-model" },
    })
    await click("Preview changes")
    await click("Save overlay")
    expect(screen.queryByText(passed)).not.toBeInTheDocument()
    await click("Check saved data")
    expect(checkMember).toHaveBeenLastCalledWith("member-a", 4, "d".repeat(64))
  })

  it("deduplicates checks and ignores a reply after rereading", async () => {
    let resolve!: (value: HallMemberCheckReport) => void
    const call = vi.fn().mockReturnValue(
      new Promise<HallMemberCheckReport>((done) => {
        resolve = done
      })
    )
    setup(call)
    await click("Read configuration")
    fireEvent.click(screen.getByRole("button", { name: "Check saved data" }))
    fireEvent.click(screen.getByRole("button", { name: "Checking…" }))
    expect(call).toHaveBeenCalledTimes(1)
    await click("Read configuration")
    await act(async () => resolve(report))
    expect(screen.queryByText(passed)).not.toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Check saved data" })
    ).toBeEnabled()
  })

  it("keeps older backends usable without attempting a check", async () => {
    const { checkMember } = setup(undefined, {
      ...config,
      checkSupported: undefined,
      checkVersion: undefined,
    })
    await click("Read configuration")
    expect(
      screen.getByText(
        "This backend does not support pre-activation checks yet."
      )
    ).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Check saved data" })
    ).not.toBeInTheDocument()
    expect(checkMember).not.toHaveBeenCalled()
  })
})
