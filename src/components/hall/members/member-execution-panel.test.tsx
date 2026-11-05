import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import type {
  HallMemberCheckReport,
  HallMemberExecutionPreview,
  HallMembersClient,
} from "@/lib/hall/members"
import { MemberExecutionPanel } from "./member-execution-panel"
import { MemberCheckPanel } from "./member-check-panel"

const report: HallMemberCheckReport = {
  memberId: "member-a",
  revision: 4,
  checkVersion: "c".repeat(64),
  configurationValid: true,
  activationAvailable: false,
  checks: [],
  diagnostics: [],
  templateRecordedRevision: 7,
  templateCurrentRevision: 7,
  executionPreviewSupported: true,
}
const candidate: HallMemberExecutionPreview = {
  memberId: "member-a",
  revision: 4,
  checkVersion: "c".repeat(64),
  valid: true,
  launchAllowed: false,
  preflight: report,
  issues: [],
  summary: {
    snapshotVersion: "d".repeat(64),
    engine: "codex",
    requestedMode: "read-only",
    modeSource: "scope",
    modelSource: "template",
    permissionPolicy: "wait",
    optionCount: 2,
    templateRevision: 7,
    instructionsIncluded: true,
    readRuleCount: 1,
    writeRuleCount: 0,
    shellRuleCount: 0,
    network: "none",
    runtimeVerified: false,
  },
}
const generated = "Candidate generated. Execution validation is still pending."
function setup(
  previewExecution = vi.fn().mockResolvedValue(candidate),
  dataValid = true
) {
  const onChecked = vi.fn()
  const view = render(
    <NextIntlClientProvider locale="en" messages={en}>
      <MemberExecutionPanel
        memberId="member-a"
        revision={4}
        checkVersion={"c".repeat(64)}
        client={{ previewExecution } as unknown as HallMembersClient}
        dataValid={dataValid}
        onChecked={onChecked}
      />
    </NextIntlClientProvider>
  )
  return { ...view, previewExecution, onChecked }
}
async function click(name = "Generate execution candidate") {
  await act(async () => fireEvent.click(screen.getByRole("button", { name })))
}

describe("execution candidate review", () => {
  it("only prepares explicitly and displays a redacted summary without granting launch access", async () => {
    const { previewExecution, onChecked } = setup(
      vi.fn().mockResolvedValue({
        ...candidate,
        instructions: "private fixture",
        agentOptions: { model: "private value" },
      })
    )
    expect(previewExecution).not.toHaveBeenCalled()
    await click()
    expect(previewExecution).toHaveBeenCalledExactlyOnceWith(
      "member-a",
      4,
      "c".repeat(64)
    )
    expect(onChecked).toHaveBeenCalledWith(report)
    expect(screen.getByText(generated)).toBeInTheDocument()
    expect(screen.getByText("d".repeat(64))).toBeInTheDocument()
    expect(
      screen.getByText("Permission requests wait for a user")
    ).toBeInTheDocument()
    expect(
      screen.getByText("Template instructions included; text is hidden.")
    ).toBeInTheDocument()
    expect(
      screen.queryByText(/private fixture|private value/)
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /launch|activate|save/i })
    ).not.toBeInTheDocument()
  })

  it("cannot prepare before data checks pass", async () => {
    const { previewExecution } = setup(undefined, false)
    expect(screen.getByRole("button")).toBeDisabled()
    await click()
    expect(previewExecution).not.toHaveBeenCalled()
  })

  it("shows fixed issue fields and one-based rule positions without a partial summary", async () => {
    setup(
      vi.fn().mockResolvedValue({
        ...candidate,
        valid: false,
        issues: [{ code: "invalid_path_rule", field: "scope.read", index: 0 }],
      })
    )
    await click()
    expect(screen.getByRole("alert")).toHaveTextContent("Read rules · 1")
    expect(screen.getByRole("alert")).toHaveTextContent("cannot be compiled")
    expect(screen.queryByText("d".repeat(64))).not.toBeInTheDocument()
  })

  it("maps unknown issue codes and fields to fixed fallbacks", async () => {
    setup(
      vi.fn().mockResolvedValue({
        ...candidate,
        valid: false,
        summary: null,
        issues: [{ code: "__proto__", field: "constructor", index: null }],
      })
    )
    await click()
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Unrecognized candidate issue"
    )
    expect(screen.getByRole("alert")).not.toHaveTextContent("__proto__")
  })

  it.each(["already_exists", "not_found", "not_implemented", "io_error"])(
    "handles %s on the same backend without echoing its error",
    async (code) => {
      const { onChecked } = setup(
        vi.fn().mockRejectedValue({ code, message: "private backend content" })
      )
      await click()
      expect(screen.getByRole("alert")).not.toHaveTextContent(
        "private backend content"
      )
      expect(screen.getByRole("alert")).toHaveTextContent(
        code === "not_implemented"
          ? "does not support"
          : code === "io_error"
            ? "Try again"
            : "changed"
      )
      expect(onChecked).not.toHaveBeenCalled()
    }
  )

  it.each([
    { memberId: "other" },
    { revision: 2 },
    { checkVersion: "x".repeat(64) },
    { launchAllowed: true },
  ])("rejects a wrong or launch-enabled response %j", async (patch) => {
    const { onChecked } = setup(
      vi.fn().mockResolvedValue({ ...candidate, ...patch })
    )
    await click()
    expect(screen.getByRole("alert")).toHaveTextContent("changed")
    expect(screen.queryByText(generated)).not.toBeInTheDocument()
    expect(onChecked).not.toHaveBeenCalled()
  })

  it.each([
    { runtimeVerified: true },
    { permissionPolicy: "allow" },
    { requestedMode: "agent-full-access" },
    { snapshotVersion: "bad" },
  ])("rejects unsafe summary metadata %j", async (patch) => {
    setup(
      vi.fn().mockResolvedValue({
        ...candidate,
        summary: { ...candidate.summary, ...patch },
      })
    )
    await click()
    expect(screen.getByRole("alert")).toHaveTextContent("Could not generate")
    expect(screen.queryByText(generated)).not.toBeInTheDocument()
  })

  it("deduplicates preparation and ignores a late response after closing", async () => {
    let resolve!: (value: HallMemberExecutionPreview) => void
    const call = vi.fn().mockReturnValue(
      new Promise<HallMemberExecutionPreview>((done) => {
        resolve = done
      })
    )
    const view = setup(call)
    fireEvent.click(
      screen.getByRole("button", { name: "Generate execution candidate" })
    )
    fireEvent.click(screen.getByRole("button", { name: "Generating…" }))
    expect(call).toHaveBeenCalledTimes(1)
    view.unmount()
    await act(async () => resolve(candidate))
    expect(view.onChecked).not.toHaveBeenCalled()
  })

  it("a new saved-data check withdraws the prior execution candidate", async () => {
    const client = {
      checkMember: vi.fn().mockResolvedValue(report),
      previewExecution: vi.fn().mockResolvedValue(candidate),
    }
    render(
      <NextIntlClientProvider locale="en" messages={en}>
        <MemberCheckPanel
          memberId="member-a"
          revision={4}
          checkVersion={"c".repeat(64)}
          client={client as unknown as HallMembersClient}
        />
      </NextIntlClientProvider>
    )
    expect(client.previewExecution).not.toHaveBeenCalled()
    await click("Check saved data")
    await click()
    expect(screen.getByText(generated)).toBeInTheDocument()
    await click("Check saved data")
    expect(screen.queryByText(generated)).not.toBeInTheDocument()
    expect(client.previewExecution).toHaveBeenCalledTimes(1)
  })

  it("an older backend can still check saved data without a preparation request", async () => {
    const client = {
      checkMember: vi
        .fn()
        .mockResolvedValue({ ...report, executionPreviewSupported: undefined }),
      previewExecution: vi.fn(),
    }
    render(
      <NextIntlClientProvider locale="en" messages={en}>
        <MemberCheckPanel
          memberId="member-a"
          revision={4}
          checkVersion={"c".repeat(64)}
          client={client as unknown as HallMembersClient}
        />
      </NextIntlClientProvider>
    )
    await click("Check saved data")
    expect(
      screen.getByText(
        "This backend does not support execution candidates yet."
      )
    ).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Generate execution candidate" })
    ).not.toBeInTheDocument()
    expect(client.previewExecution).not.toHaveBeenCalled()
  })

  it("shows an older summary as an unchosen method with unknown limits", async () => {
    setup()
    await click()
    expect(screen.getByText("Not chosen (older record)")).toBeInTheDocument()
    expect(screen.queryByText("Verified")).not.toBeInTheDocument()
    expect(screen.queryByText(/isolated/i)).not.toBeInTheDocument()
  })

  it("explains the chosen method and host limits without claiming isolation", async () => {
    const claude = {
      ...candidate,
      summary: {
        ...candidate.summary!,
        engine: "claude_code",
        requestedMode: "default",
        executionMode: "local_assistant",
        capabilities: {
          readBoundary: "not_enforced",
          writeBoundary: "not_enforced",
          networkBoundary: "not_enforced",
          permissionRequests: "supported",
          hallIdentity: true,
          hallReport: true,
          exitCoverage: "windows_job_object",
          engineVersion: null,
          adapterVersion: null,
          evidenceId: null,
        },
      },
    }
    setup(vi.fn().mockResolvedValue(claude))
    await click()
    expect(screen.getByText("Local assistant execution")).toBeInTheDocument()
    expect(screen.getByText(/Windows job object/)).toBeInTheDocument()
    expect(screen.getAllByText(/Not enforced/)).toHaveLength(3)
  })
})
