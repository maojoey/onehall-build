import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import type { HallMemberSnapshot, HallMembersClient } from "@/lib/hall/members"
import { MemberSnapshotPanel } from "./member-snapshot-panel"
import { MemberCheckPanel } from "./member-check-panel"

const memberId = "a81e0597-6d70-4b59-9b40-b8416b69b939"
const record: HallMemberSnapshot = {
  snapshotId: "eb602f73-2e7c-4f70-ad59-a782dd3f83fa",
  memberId,
  memberRevision: 4,
  checkVersion: "c".repeat(64),
  createdAt: "2026-10-24T00:00:00Z",
  launchAllowed: false,
  summary: {
    snapshotVersion: "d".repeat(64),
    engine: "codex",
    requestedMode: "read-only",
    modeSource: "scope",
    modelSource: "engine_default",
    permissionPolicy: "wait",
    optionCount: 0,
    templateRevision: null,
    instructionsIncluded: false,
    readRuleCount: 1,
    writeRuleCount: 0,
    shellRuleCount: 0,
    network: "none",
    runtimeVerified: false,
  },
}
function setup(
  overrides: Partial<HallMembersClient> = {},
  expected: string | null = "d".repeat(64)
) {
  const saveSnapshot = vi.fn().mockResolvedValue(record)
  const listSnapshots = vi.fn().mockResolvedValue([])
  const client = {
    saveSnapshot,
    listSnapshots,
    ...overrides,
  } as unknown as HallMembersClient
  const view = render(
    <NextIntlClientProvider locale="en" messages={en}>
      <MemberSnapshotPanel
        memberId={memberId}
        revision={4}
        checkVersion={"c".repeat(64)}
        expectedSnapshotVersion={expected}
        client={client}
      />
    </NextIntlClientProvider>
  )
  return { ...view, client, saveSnapshot, listSnapshots }
}
async function click(name = "Save configuration snapshot") {
  await act(async () => fireEvent.click(screen.getByRole("button", { name })))
}
const success = "Configuration snapshot saved. Tasks can now use it."

describe("configuration snapshots", () => {
  it("only saves explicitly and shows the matching redacted record", async () => {
    const { saveSnapshot, listSnapshots } = setup({
      saveSnapshot: vi.fn().mockResolvedValue({
        ...record,
        payload: "private fixture",
        instructions: "private instructions",
      }),
    })
    expect(saveSnapshot).not.toHaveBeenCalled()
    expect(listSnapshots).not.toHaveBeenCalled()
    await click()
    expect(screen.getByText(success)).toBeInTheDocument()
    expect(screen.getByText(record.snapshotId)).toBeInTheDocument()
    expect(screen.getByText(record.summary.snapshotVersion)).toBeInTheDocument()
    expect(
      screen.queryByText(/private fixture|private instructions/)
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /activate|launch/i })
    ).not.toBeInTheDocument()
    expect(listSnapshots).not.toHaveBeenCalled()
  })

  it("requires a candidate to save but allows reading older snapshots", async () => {
    const listSnapshots = vi
      .fn()
      .mockResolvedValue([
        { ...record, memberRevision: 1, checkVersion: "a".repeat(64) },
      ])
    const { saveSnapshot } = setup({ listSnapshots }, null)
    expect(
      screen.getByRole("button", { name: "Save configuration snapshot" })
    ).toBeDisabled()
    await click("Read saved snapshots")
    expect(listSnapshots).toHaveBeenCalledExactlyOnceWith(memberId)
    expect(
      screen.getByText("Member data revision 1 · Read only")
    ).toBeInTheDocument()
    expect(saveSnapshot).not.toHaveBeenCalled()
  })

  it("deduplicates rapid requests and ignores a reply after closure", async () => {
    let resolve!: (value: HallMemberSnapshot) => void
    const saveSnapshot = vi.fn().mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const view = setup({ saveSnapshot })
    const button = screen.getByRole("button", {
      name: "Save configuration snapshot",
    })
    await act(async () => {
      fireEvent.click(button)
      fireEvent.click(button)
    })
    expect(saveSnapshot).toHaveBeenCalledExactlyOnceWith(
      memberId,
      4,
      "c".repeat(64),
      "d".repeat(64)
    )
    expect(
      screen.getByRole("button", { name: "Read saved snapshots" })
    ).toBeDisabled()
    view.unmount()
    await act(async () => resolve(record))
    expect(screen.queryByText(success)).not.toBeInTheDocument()
  })

  it.each([
    { ...record, memberId: "other" },
    { ...record, memberRevision: 3 },
    { ...record, checkVersion: "b".repeat(64) },
    { ...record, launchAllowed: true },
    {
      ...record,
      summary: { ...record.summary, snapshotVersion: "b".repeat(64) },
    },
    { ...record, summary: { ...record.summary, runtimeVerified: true } },
    { ...record, summary: { ...record.summary, permissionPolicy: "allow" } },
  ])("rejects a mismatched or unsafe save reply %#", async (response) => {
    setup({ saveSnapshot: vi.fn().mockResolvedValue(response) })
    await click()
    expect(screen.getByRole("alert")).toHaveTextContent("changed")
    expect(screen.queryByText(success)).not.toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Save configuration snapshot" })
    ).toBeDisabled()
    expect(screen.queryByText(record.snapshotId)).not.toBeInTheDocument()
  })

  it("leaves an unconfirmed save retryable with identical parameters", async () => {
    const saveSnapshot = vi
      .fn()
      .mockRejectedValueOnce(new Error("private fixture detail"))
      .mockResolvedValueOnce(record)
    setup({ saveSnapshot })
    await click()
    expect(screen.getByRole("alert")).toHaveTextContent("unconfirmed")
    expect(screen.getByRole("alert")).not.toHaveTextContent(
      "private fixture detail"
    )
    await click()
    expect(saveSnapshot.mock.calls[1]).toEqual(saveSnapshot.mock.calls[0])
    expect(screen.getByText(success)).toBeInTheDocument()
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it.each(["already_exists", "not_found", "not_implemented"])(
    "handles %s without falling back or exposing details",
    async (code) => {
      const saveSnapshot = vi
        .fn()
        .mockRejectedValue({ code, message: "private backend fixture" })
      setup({ saveSnapshot })
      await click()
      expect(screen.getByRole("alert")).toHaveTextContent(
        code === "not_implemented" ? "does not support" : "changed"
      )
      expect(screen.getByRole("alert")).not.toHaveTextContent(
        "private backend fixture"
      )
      expect(saveSnapshot).toHaveBeenCalledTimes(1)
    }
  )

  it.each([
    { rows: [{ ...record, memberId: "other" }] },
    { rows: [record, record] },
    { rows: Array(21).fill(record) },
    { rows: [{ ...record, summary: null }] },
  ])("rejects invalid history %#", async ({ rows }) => {
    setup({ listSnapshots: vi.fn().mockResolvedValue(rows) })
    await click("Read saved snapshots")
    expect(screen.getByRole("alert")).toHaveTextContent("Could not read")
    expect(screen.queryByText(record.snapshotId)).not.toBeInTheDocument()
  })

  it("reads an empty history only when requested", async () => {
    const { listSnapshots } = setup()
    expect(listSnapshots).not.toHaveBeenCalled()
    await click("Read saved snapshots")
    expect(
      screen.getByText("No configuration snapshots have been saved.")
    ).toBeInTheDocument()
  })

  it("withdraws save context and feedback when saved data is checked again", async () => {
    const report = {
      memberId,
      revision: 4,
      checkVersion: "c".repeat(64),
      configurationValid: true,
      activationAvailable: false,
      checks: [],
      diagnostics: [],
      templateRecordedRevision: null,
      templateCurrentRevision: null,
      executionPreviewSupported: true,
      snapshotStorageSupported: true,
    }
    const client = {
      checkMember: vi.fn().mockResolvedValue(report),
      previewExecution: vi.fn().mockResolvedValue({
        memberId,
        revision: 4,
        checkVersion: report.checkVersion,
        valid: true,
        launchAllowed: false,
        preflight: report,
        summary: record.summary,
        issues: [],
      }),
      saveSnapshot: vi.fn().mockResolvedValue(record),
      listSnapshots: vi.fn(),
    } as unknown as HallMembersClient
    render(
      <NextIntlClientProvider locale="en" messages={en}>
        <MemberCheckPanel
          memberId={memberId}
          revision={4}
          checkVersion={report.checkVersion}
          client={client}
        />
      </NextIntlClientProvider>
    )
    await click("Check saved data")
    expect(
      screen.getByRole("button", { name: "Save configuration snapshot" })
    ).toBeDisabled()
    await click("Generate execution candidate")
    await click()
    expect(screen.getByText(success)).toBeInTheDocument()
    await click("Check saved data")
    expect(screen.queryByText(success)).not.toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Save configuration snapshot" })
    ).toBeDisabled()
    expect(client.saveSnapshot).toHaveBeenCalledTimes(1)
  })
})
