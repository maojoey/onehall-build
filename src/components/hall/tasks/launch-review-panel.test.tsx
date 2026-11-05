import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import messages from "@/i18n/messages/en.json"
import { LaunchReviewPanel } from "./launch-review-panel"
import type {
  HallTaskDetail,
  HallTasksClient,
  LaunchReview,
} from "@/lib/hall/tasks"
const task = {
  taskId: "task",
  memberId: "member",
  revision: 1,
  status: "draft",
  snapshot: { snapshotId: "snapshot" },
  attempts: [],
} as unknown as HallTaskDetail
const plan = {
  taskId: "task",
  memberId: "member",
  taskRevision: 1,
  memberRevision: 1,
  projectId: null,
  admission: "closed_pending_native_b10_and_human_scope",
  snapshotId: "snapshot",
  snapshotVersion: "fingerprint",
  description: "Only this task",
  title: "Review me",
  cwd: "/tmp/owned-review",
  engine: "codex",
  mode: "review",
  options: { approval: "wait" },
  brief: "Saved instructions",
  planHash: "hash",
  realExecutionAllowed: false,
} as LaunchReview
function setup(row = task, reviewPlan = plan) {
  const client = {
    review: vi.fn().mockResolvedValue(reviewPlan),
    approve: vi.fn().mockResolvedValue({}),
    cancelQueued: vi.fn().mockResolvedValue(undefined),
  }
  const changed = vi.fn().mockResolvedValue(undefined)
  const page = render(
    <NextIntlClientProvider locale="en" messages={messages}>
      <LaunchReviewPanel
        client={client as unknown as HallTasksClient}
        task={row}
        memberName="Member One"
        onChanged={changed}
      />
    </NextIntlClientProvider>
  )
  return { client, changed, ...page }
}
describe("reviewed queue proposals", () => {
  it("requires an explicit review and preserves one request after uncertain approval", async () => {
    const { client, changed } = setup()
    expect(client.review).not.toHaveBeenCalled()
    expect(
      screen.queryByRole("button", { name: "Approve queue proposal" })
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Review plan" }))
    await screen.findByText("/tmp/owned-review")
    expect(
      screen.getByText(/Approving queues this exact plan/)
    ).toBeInTheDocument()
    client.approve.mockRejectedValueOnce(new Error("network uncertain"))
    fireEvent.click(
      screen.getByRole("button", { name: "Approve queue proposal" })
    )
    await screen.findByRole("alert")
    expect(changed).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Retry same approval" }))
    await waitFor(() => expect(changed).toHaveBeenCalledTimes(1))
    expect(client.approve.mock.calls[1]).toEqual(client.approve.mock.calls[0])
  })
  it("rejects a plan from another backend or member", async () => {
    const { client } = setup(task, { ...plan, memberId: "other" })
    fireEvent.click(screen.getByRole("button", { name: "Review plan" }))
    await screen.findByRole("alert")
    expect(client.approve).not.toHaveBeenCalled()
    expect(
      screen.queryByRole("button", { name: "Approve queue proposal" })
    ).not.toBeInTheDocument()
  })
  it("does not apply a late response after the owning backend unmounts", async () => {
    let resolve!: (p: LaunchReview) => void
    const pending = new Promise<LaunchReview>((r) => {
      resolve = r
    })
    const { client, changed, unmount } = setup()
    client.review.mockReturnValueOnce(pending)
    fireEvent.click(screen.getByRole("button", { name: "Review plan" }))
    unmount()
    await act(async () => resolve(plan))
    expect(client.approve).not.toHaveBeenCalled()
    expect(changed).not.toHaveBeenCalled()
  })
  it("can cancel an approved queued proposal without claiming execution", async () => {
    const { client, changed } = setup({
      ...task,
      status: "queued",
      attempts: [{ attemptId: "attempt" }],
      launchReviews: [
        { attemptId: "attempt", planHash: "hash", approvedAt: "2026-10-25" },
      ],
    } as HallTaskDetail)
    expect(
      screen.queryByRole("button", { name: "Review plan" })
    ).not.toBeInTheDocument()
    expect(screen.getByText(/Ready to dispatch/)).toBeInTheDocument()
    fireEvent.click(
      screen.getByRole("button", { name: "Cancel queued proposal" })
    )
    await waitFor(() => expect(changed).toHaveBeenCalled())
    expect(client.cancelQueued).toHaveBeenCalledWith(
      expect.objectContaining({ status: "queued" })
    )
    expect(client.approve).not.toHaveBeenCalled()
  })
})

describe("execution method on the review", () => {
  async function open(reviewPlan: LaunchReview) {
    const { client } = setup(task, reviewPlan)
    fireEvent.click(screen.getByRole("button", { name: "Review plan" }))
    await screen.findByText("Review me")
    return client
  }
  it("shows unknown for a plan from an older server and claims no isolation", async () => {
    await open(plan)
    expect(screen.getByText("Not chosen (older record)")).toBeInTheDocument()
    expect(screen.queryByText("Verified")).not.toBeInTheDocument()
    expect(screen.queryByText(/isolated/i)).not.toBeInTheDocument()
    expect(
      screen.queryByText("Local assistant execution")
    ).not.toBeInTheDocument()
  })
  it("shows host capabilities as reported and the bound manifest", async () => {
    await open({
      ...plan,
      executionMode: "local_assistant",
      exchangeManifestDigest: "d".repeat(64),
      capabilities: {
        readBoundary: "not_enforced",
        writeBoundary: "not_enforced",
        networkBoundary: "not_enforced",
        permissionRequests: "supported",
        hallIdentity: true,
        hallReport: true,
        exitCoverage: "macos_process_group_best_effort",
        engineVersion: null,
        adapterVersion: null,
        evidenceId: null,
      },
    } as LaunchReview)
    expect(screen.getByText("Local assistant execution")).toBeInTheDocument()
    expect(screen.getAllByText(/Not enforced/)).toHaveLength(3)
    expect(screen.getByText(/not system-level isolation/)).toBeInTheDocument()
    expect(screen.getByText("d".repeat(64))).toBeInTheDocument()
  })
  it("does not trust malformed capabilities", async () => {
    await open({
      ...plan,
      executionMode: "restricted",
      capabilities: { readBoundary: "verified" },
    } as unknown as LaunchReview)
    expect(screen.getByText("Restricted execution")).toBeInTheDocument()
    expect(screen.queryByText("Verified")).not.toBeInTheDocument()
  })

  it("shows the bound input files, sizes and output slots, or says they are unavailable", async () => {
    const manifest = {
      schemaVersion: 1 as const,
      manifestId: "33333333-3333-4333-8333-333333333333",
      inputs: [
        {
          objectId: "44444444-4444-4444-8444-444444444444",
          displayName: "numbers.json",
          size: 1234,
          sha256:
            "da36d1f53c22016dc9eeb7414d1be41ddca4b70f4bf1abd0deca0eecfe9ac355",
        },
      ],
      outputs: [
        {
          slotId: "55555555-5555-4555-8555-555555555555",
          displayName: "sum.txt",
        },
      ],
      digest: "d".repeat(64),
    }
    const bound = setup(task, {
      ...plan,
      exchangeManifestDigest: manifest.digest,
      exchangeManifest: manifest,
    })
    fireEvent.click(screen.getByRole("button", { name: "Review plan" }))
    expect(await screen.findByText("numbers.json")).toBeInTheDocument()
    expect(screen.getByText("1,234 bytes")).toBeInTheDocument()
    expect(screen.getByText("sum.txt")).toBeInTheDocument()
    bound.unmount()

    // A manifest that does not match the bound digest is never shown as files.
    const forged = setup(task, {
      ...plan,
      exchangeManifestDigest: "e".repeat(64),
      exchangeManifest: manifest,
    })
    fireEvent.click(screen.getByRole("button", { name: "Review plan" }))
    expect(
      await screen.findByText(messages.Hall.manifest.unreadable)
    ).toBeInTheDocument()
    expect(screen.queryByText("numbers.json")).not.toBeInTheDocument()
    const blocked = screen.getByRole("button", {
      name: "Approve queue proposal",
    })
    expect(blocked).toBeDisabled()
    fireEvent.click(blocked)
    expect(forged.client.approve).not.toHaveBeenCalled()
    forged.unmount()

    // Older server: digest only.
    const older = setup(task, {
      ...plan,
      exchangeManifestDigest: manifest.digest,
    })
    fireEvent.click(screen.getByRole("button", { name: "Review plan" }))
    expect(
      await screen.findByText(messages.Hall.manifest.detailsMissing)
    ).toBeInTheDocument()
    const unavailable = screen.getByRole("button", {
      name: "Approve queue proposal",
    })
    expect(unavailable).toBeDisabled()
    fireEvent.click(unavailable)
    expect(older.client.approve).not.toHaveBeenCalled()
  })
})
