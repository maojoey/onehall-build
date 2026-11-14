import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import type { LocalInboxClient } from "@/lib/hall/branch-inbox"
vi.mock("next-intl", () => ({ useTranslations: () => (key: string) => key }))
import { LocalBranchInbox } from "./local-inbox"
it("explains that acceptance belongs to the branch's local desktop", () => {
  render(<LocalBranchInbox client={null} />)
  expect(screen.getByText("localOnly")).toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "accept" })
  ).not.toBeInTheDocument()
})
it("requires an enabled local member and explicit snapshot before binding a draft", async () => {
  const item = {
    issuerId: "issuer",
    dispatchId: "dispatch",
    title: "Sum",
    description: "Add",
    revision: 1,
    status: "awaiting_confirmation",
    memberRef: "worker",
    launchAllowed: false,
  }
  const c = {
    list: vi.fn().mockResolvedValue([item]),
    members: vi.fn().mockResolvedValue([
      { memberId: "member", name: "Windows", enabled: true },
      { memberId: "off", name: "Disabled", enabled: false },
    ]),
    snapshots: vi.fn().mockResolvedValue([
      {
        snapshotId: "snapshot",
        memberId: "member",
        projectId: null,
        createdAt: "now",
      },
    ]),
    accept: vi.fn().mockResolvedValue({ taskId: "task" }),
    reject: vi.fn(),
  }
  const accepted = vi.fn()
  render(
    <LocalBranchInbox
      client={c as unknown as LocalInboxClient}
      onAccepted={accepted}
    />
  )
  await screen.findByText("Sum")
  const button = screen.getByRole("button", { name: "accept" })
  expect(button).toBeDisabled()
  expect(
    screen.queryByRole("option", { name: "Disabled" })
  ).not.toBeInTheDocument()
  fireEvent.change(screen.getByLabelText("member"), {
    target: { value: "member" },
  })
  await waitFor(() => expect(c.snapshots).toHaveBeenCalledWith("member"))
  expect(button).toBeDisabled()
  fireEvent.change(screen.getByLabelText("snapshot"), {
    target: { value: "snapshot" },
  })
  fireEvent.click(button)
  await waitFor(() =>
    expect(c.accept).toHaveBeenCalledWith({
      issuerId: "issuer",
      dispatchId: "dispatch",
      expectedRevision: 1,
      memberId: "member",
      snapshotId: "snapshot",
      projectId: null,
    })
  )
  expect(await screen.findByText("acceptedHint")).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "openTasks" }))
  expect(accepted).toHaveBeenCalledOnce()
})

it("describes historical acceptance without claiming that the linked task has not started", async () => {
  const c = {
    list: vi.fn().mockResolvedValue([
      {
        issuerId: "issuer",
        dispatchId: "dispatch",
        title: "Historical task",
        description: "Already run elsewhere",
        revision: 1,
        status: "accepted",
        memberRef: "worker",
        launchAllowed: false,
        taskId: "existing-task",
      },
    ]),
    members: vi.fn().mockResolvedValue([]),
    snapshots: vi.fn(),
    accept: vi.fn(),
    reject: vi.fn(),
  }
  render(<LocalBranchInbox client={c as unknown as LocalInboxClient} />)
  await screen.findByText("Historical task")
  expect(screen.getByRole("status")).toHaveTextContent("linkedTaskHint")
  expect(screen.queryByText("acceptedHint")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "openTasks" })).toBeEnabled()
})
it("shows the real input names, sizes and output slots the request brings in", async () => {
  const item = {
    issuerId: "issuer",
    dispatchId: "dispatch",
    title: "Sum",
    description: "Add",
    revision: 1,
    status: "awaiting_confirmation",
    memberRef: "worker",
    launchAllowed: false,
    exchangeManifest: {
      schemaVersion: 1,
      manifestId: "33333333-3333-4333-8333-333333333333",
      inputs: [
        {
          objectId: "44444444-4444-4444-8444-444444444444",
          displayName: "numbers.json",
          size: 47,
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
      digest: "0".repeat(64),
    },
  }
  const c = {
    list: vi.fn().mockResolvedValue([
      item,
      {
        ...item,
        dispatchId: "v1",
        title: "Old",
        exchangeManifest: undefined,
      },
    ]),
    members: vi.fn().mockResolvedValue([]),
    snapshots: vi.fn(),
    accept: vi.fn(),
    reject: vi.fn(),
  }
  render(<LocalBranchInbox client={c as unknown as LocalInboxClient} />)
  expect(await screen.findByText("numbers.json")).toBeInTheDocument()
  expect(screen.getByText("size")).toBeInTheDocument()
  expect(screen.getByText("da36d1f53c22")).toBeInTheDocument()
  expect(screen.getByText("sum.txt")).toBeInTheDocument()
  // A v1 item has no manifest section at all.
  expect(screen.getAllByText("inputs")).toHaveLength(1)
})

it("does not accept a malformed attachment manifest even after explicit member selection", async () => {
  const client = {
    list: vi.fn().mockResolvedValue([
      {
        issuerId: "issuer",
        dispatchId: "dispatch",
        title: "Unreadable files",
        description: "Review before acceptance",
        revision: 1,
        status: "awaiting_confirmation",
        memberRef: "worker",
        launchAllowed: false,
        exchangeManifest: { schemaVersion: 1, inputs: [] },
      },
    ]),
    members: vi
      .fn()
      .mockResolvedValue([
        { memberId: "member", name: "Local", enabled: true },
      ]),
    snapshots: vi.fn().mockResolvedValue([
      {
        snapshotId: "snapshot",
        memberId: "member",
        projectId: null,
        createdAt: "now",
      },
    ]),
    accept: vi.fn(),
    reject: vi.fn().mockResolvedValue(undefined),
  }
  render(<LocalBranchInbox client={client as unknown as LocalInboxClient} />)
  await screen.findByText("unreadable")
  fireEvent.change(screen.getByLabelText("member"), {
    target: { value: "member" },
  })
  await screen.findByRole("option", { name: "now" })
  fireEvent.change(screen.getByLabelText("snapshot"), {
    target: { value: "snapshot" },
  })
  const accept = screen.getByRole("button", { name: "accept" })
  expect(accept).toBeDisabled()
  fireEvent.click(accept)
  expect(client.accept).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole("button", { name: "reject" }))
  await waitFor(() => expect(client.reject).toHaveBeenCalledTimes(1))
})

it("preselects the member the sender asked for and its newest snapshot, but waits for the click", async () => {
  const item = {
    issuerId: "issuer",
    dispatchId: "dispatch",
    title: "Sum",
    description: "Add",
    revision: 1,
    status: "awaiting_confirmation",
    memberRef: "m-reviewer",
    launchAllowed: false,
  }
  const c = {
    list: vi.fn().mockResolvedValue([item]),
    members: vi.fn().mockResolvedValue([
      { memberId: "m-worker", name: "Worker", enabled: true },
      { memberId: "m-reviewer", name: "Reviewer", enabled: true },
    ]),
    snapshots: vi.fn().mockResolvedValue([
      {
        snapshotId: "old",
        memberId: "m-reviewer",
        projectId: 3,
        createdAt: "2026-11-01T00:00:00Z",
      },
      {
        snapshotId: "new",
        memberId: "m-reviewer",
        projectId: 3,
        createdAt: "2026-11-12T00:00:00Z",
      },
    ]),
    accept: vi.fn().mockResolvedValue({ taskId: "task" }),
    reject: vi.fn(),
  }
  render(<LocalBranchInbox client={c as unknown as LocalInboxClient} />)
  await screen.findByText("requested")
  await waitFor(() => expect(c.snapshots).toHaveBeenCalledWith("m-reviewer"))
  await waitFor(() =>
    expect((screen.getByLabelText("snapshot") as HTMLSelectElement).value).toBe(
      "new"
    )
  )
  expect((screen.getByLabelText("member") as HTMLSelectElement).value).toBe(
    "m-reviewer"
  )
  expect(c.accept).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole("button", { name: "accept" }))
  await waitFor(() =>
    expect(c.accept).toHaveBeenCalledWith(
      expect.objectContaining({ memberId: "m-reviewer", snapshotId: "new" })
    )
  )
})

it("leaves the choice open when the sender did not name a local member", async () => {
  const c = {
    list: vi.fn().mockResolvedValue([
      {
        issuerId: "issuer",
        dispatchId: "dispatch",
        title: "Sum",
        description: "Add",
        revision: 1,
        status: "awaiting_confirmation",
        memberRef: "worker",
        launchAllowed: false,
      },
    ]),
    members: vi
      .fn()
      .mockResolvedValue([
        { memberId: "m-worker", name: "Worker", enabled: true },
      ]),
    snapshots: vi.fn().mockResolvedValue([]),
    accept: vi.fn(),
    reject: vi.fn(),
  }
  render(<LocalBranchInbox client={c as unknown as LocalInboxClient} />)
  await screen.findByText("Sum")
  expect(screen.queryByText("requested")).toBeNull()
  expect(c.snapshots).not.toHaveBeenCalled()
  expect((screen.getByLabelText("member") as HTMLSelectElement).value).toBe("")
})
