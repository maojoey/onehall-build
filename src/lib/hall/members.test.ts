import { beforeEach, describe, expect, it, vi } from "vitest"
import type { AssistantInfo } from "@/lib/types"
import type { Transport } from "@/lib/transport"

const mocks = vi.hoisted(() => ({ transport: null as unknown }))
vi.mock("@/lib/transport", () => ({
  getTransport: () => mocks.transport,
}))

import { createHallMembersClient, isMembersUnavailable } from "./members"

it("prepares execution configuration on the observed backend only", async () => {
  const transport = {
    call: vi.fn().mockResolvedValue({ launchAllowed: false }),
  }
  await createHallMembersClient(transport as never).previewExecution(
    "member-id",
    3,
    "c".repeat(64)
  )
  expect(transport.call).toHaveBeenCalledExactlyOnceWith(
    "hall_preview_member_execution",
    {
      memberId: "member-id",
      expectedRevision: 3,
      expectedCheckVersion: "c".repeat(64),
    }
  )
})

it("checks the persisted member on the pinned backend and observation version", async () => {
  const transport = {
    call: vi.fn().mockResolvedValue({ activationAvailable: false }),
  }
  await createHallMembersClient(transport as never).checkMember(
    "member-id",
    3,
    "c".repeat(64)
  )
  expect(transport.call).toHaveBeenCalledExactlyOnceWith("hall_check_member", {
    memberId: "member-id",
    expectedRevision: 3,
    expectedCheckVersion: "c".repeat(64),
  })
})

it("saves only the previewed member overlay on its pinned backend", async () => {
  const transport = { call: vi.fn().mockResolvedValue({ valid: true }) }
  const client = createHallMembersClient(transport as never)
  await client.saveConfig("member-id", 3, "a".repeat(64), {
    schema_version: 1,
    model: "",
  })
  expect(transport.call).toHaveBeenCalledWith("hall_save_member_config", {
    memberId: "member-id",
    expectedRevision: 3,
    expectedFileVersion: "a".repeat(64),
    draft: { schema_version: 1, model: "" },
  })
})

it("previews a sparse draft through the pinned transport", async () => {
  const transport = { call: vi.fn().mockResolvedValue({ valid: true }) }
  const client = createHallMembersClient(transport as never)
  await client.previewDraft("member-id", 3, "a".repeat(64), {
    schema_version: 1,
    scope: { write: [] },
  })
  expect(transport.call).toHaveBeenCalledWith(
    "hall_preview_member_config_draft",
    {
      memberId: "member-id",
      expectedRevision: 3,
      expectedFileVersion: "a".repeat(64),
      draft: { schema_version: 1, scope: { write: [] } },
    }
  )
})

beforeEach(() => {
  mocks.transport = { call: vi.fn().mockResolvedValue([]) }
})

describe("member registry transport", () => {
  it("pins snapshot save and history to the captured backend and reviewed digest", async () => {
    const call = vi.fn().mockResolvedValue([])
    const client = createHallMembersClient({ call } as unknown as Transport)
    await client.saveSnapshot("member-a", 4, "c".repeat(64), "d".repeat(64))
    await client.listSnapshots("member-a")
    expect(call.mock.calls).toEqual([
      [
        "hall_save_member_snapshot",
        {
          memberId: "member-a",
          expectedRevision: 4,
          expectedCheckVersion: "c".repeat(64),
          expectedSnapshotVersion: "d".repeat(64),
        },
      ],
      ["hall_list_member_snapshots", { memberId: "member-a" }],
    ])
  })

  it("keeps reads, templates and writes on the backend that owns the page", async () => {
    const first = mocks.transport as Transport
    const client = createHallMembersClient()
    const second = { call: vi.fn().mockResolvedValue([]) }
    mocks.transport = second
    await client.list()
    await client.templates()
    await client.projects()
    await client.previewConfig("member-id", 3)
    const draft = {
      name: "Reviewer",
      role: "Review",
      cwd: "/member",
      assistantUid: null,
      projectId: 17,
    }
    await client.register(draft)
    await client.update("member-id", 3, {
      name: "Renamed",
      role: "Review",
      assistantUid: null,
      projectId: null,
    })
    expect(first.call).toHaveBeenCalledWith("hall_list_members", {})
    expect(first.call).toHaveBeenCalledWith("assistant_list", {})
    expect(first.call).toHaveBeenCalledWith("hall_list_member_projects", {})
    expect(first.call).toHaveBeenCalledWith("hall_preview_member_config", {
      memberId: "member-id",
      expectedRevision: 3,
    })
    expect(first.call).toHaveBeenCalledWith("hall_register_member", { draft })
    expect(first.call).toHaveBeenCalledWith("hall_update_member", {
      memberId: "member-id",
      expectedRevision: 3,
      draft: {
        name: "Renamed",
        role: "Review",
        assistantUid: null,
        projectId: null,
      },
    })
    expect(second.call).not.toHaveBeenCalled()
    await createHallMembersClient().list()
    expect(second.call).toHaveBeenCalledWith("hall_list_members", {})
  })

  it("offers only live enabled Codex and Claude Code templates without modifying the source", async () => {
    const rows = [
      { uid: "good", agent_type: "codex", enabled: true, deleted_at: null },
      { uid: "off", agent_type: "codex", enabled: false, deleted_at: null },
      {
        uid: "deleted",
        agent_type: "codex",
        enabled: true,
        deleted_at: "today",
      },
      {
        uid: "claude",
        agent_type: "claude_code",
        enabled: true,
        deleted_at: null,
      },
      { uid: "other", agent_type: "gemini", enabled: true, deleted_at: null },
    ] as AssistantInfo[]
    const call = vi.fn().mockResolvedValue(rows)
    mocks.transport = { call }
    expect(await createHallMembersClient().templates()).toEqual([
      rows[0],
      rows[3],
    ])
    expect(rows).toHaveLength(5)
    expect(call).toHaveBeenCalledTimes(1)
  })

  it("recognizes an unsupported member API without treating network failures as compatibility", () => {
    expect(
      isMembersUnavailable({ code: "not_implemented", message: "Unavailable" })
    ).toBe(true)
    expect(isMembersUnavailable("Command hall_list_members not found")).toBe(
      true
    )
    expect(
      isMembersUnavailable("Command hall_list_member_projects not found")
    ).toBe(true)
    expect(isMembersUnavailable(new Error("network disconnected"))).toBe(false)
    expect(
      isMembersUnavailable({ code: "permission_denied", message: "Denied" })
    ).toBe(false)
  })
})
