import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import type { HallMember } from "@/lib/hall/members"
import enMessages from "@/i18n/messages/en.json"

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  templates: vi.fn(),
  projects: vi.fn(),
  previewConfig: vi.fn(),
  register: vi.fn(),
  update: vi.fn(),
  setEnabled: vi.fn(),
  backendId: 1,
}))
vi.mock("@/lib/hall/members", async (original) => ({
  ...(await original<typeof import("@/lib/hall/members")>()),
  createHallMembersClient: () => ({
    list: mocks.list,
    templates: mocks.templates,
    projects: mocks.projects,
    previewConfig: mocks.previewConfig,
    register: mocks.register,
    update: mocks.update,
    setEnabled: mocks.setEnabled,
  }),
}))
vi.mock("@/contexts/remote-connection-context", () => ({
  useRemoteConnection: () => ({
    connection: { id: mocks.backendId, name: "Hall server" },
  }),
}))
vi.mock("@/components/layout/backend-badge", () => ({
  BackendBadge: () => <span>Hall server</span>,
  useBackendLabel: () => "Hall server",
}))
vi.mock("@/components/shared/directory-browser-dialog", () => ({
  DirectoryBrowserDialog: ({
    open,
    onSelect,
    onOpenChange,
  }: {
    open: boolean
    onSelect: (path: string) => void
    onOpenChange: (open: boolean) => void
  }) =>
    open ? (
      <button
        onClick={() => {
          onSelect("/server/member")
          onOpenChange(false)
        }}
      >
        Pick server directory
      </button>
    ) : null,
}))

import { MembersSettings } from "./members-settings"

function member(name = "Reviewer", revision = 1): HallMember {
  return {
    memberId: "member-a",
    name,
    role: "Review code",
    cwd: "/server/member",
    projectId: null,
    engine: "codex",
    assistantUid: null,
    assistantRevision: null,
    enabled: false,
    revision,
    createdAt: "2026-10-20T00:00:00Z",
    updatedAt: "2026-10-20T00:00:00Z",
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function page() {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <MembersSettings />
    </NextIntlClientProvider>
  )
}

async function renderPage() {
  let view!: ReturnType<typeof render>
  await act(async () => {
    view = render(page())
  })
  return view
}

async function openRegistration() {
  fireEvent.click(screen.getByRole("button", { name: "Register member" }))
  const dialog = screen.getByRole("dialog")
  fireEvent.change(within(dialog).getByLabelText("Name"), {
    target: { value: "New member" },
  })
  fireEvent.change(within(dialog).getByLabelText("Role"), {
    target: { value: "Review code" },
  })
  fireEvent.click(within(dialog).getByRole("button", { name: "Browse" }))
  fireEvent.click(screen.getByRole("button", { name: "Pick server directory" }))
  return dialog
}

beforeEach(() => {
  vi.resetAllMocks()
  mocks.backendId = 1
  mocks.list.mockResolvedValue([])
  mocks.templates.mockResolvedValue([])
  mocks.projects.mockResolvedValue([])
  mocks.register.mockResolvedValue(member("New member"))
  mocks.update.mockResolvedValue(member("Renamed", 2))
})

describe("member management", () => {
  it("discards a pending file read when a refreshed registry revision replaces the preview", async () => {
    const refresh = deferred<HallMember[]>()
    const pending = deferred<unknown>()
    mocks.list
      .mockResolvedValueOnce([member()])
      .mockReturnValueOnce(refresh.promise)
    mocks.previewConfig.mockReturnValue(pending.promise)
    await renderPage()
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Refresh members" }))
    })
    fireEvent.click(screen.getByRole("button", { name: "Preview Reviewer" }))
    const dialog = screen.getByRole("dialog")
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Read configuration" })
    )
    expect(mocks.previewConfig).toHaveBeenCalledWith("member-a", 1)
    await act(async () => {
      refresh.resolve([member("Reviewer", 2)])
    })
    expect(
      within(dialog).getByRole("button", { name: "Read configuration" })
    ).toBeEnabled()
    await act(async () => {
      pending.resolve({
        memberId: "member-a",
        revision: 1,
        valid: true,
        files: [],
        diagnostics: [],
        fields: [{ field: "model", source: "defaults", overridden: false }],
        effective: { model: "old-read-value", scope: {}, skills: {} },
      })
    })
    expect(screen.queryByText("old-read-value")).not.toBeInTheDocument()
  })
  it("registers an existing directory explicitly, once even under duplicate submits", async () => {
    const pending = deferred<HallMember>()
    mocks.register.mockReturnValue(pending.promise)
    await renderPage()
    expect(screen.getByText("No members registered yet.")).toBeInTheDocument()
    const dialog = await openRegistration()
    const submit = within(dialog).getByRole("button", {
      name: "Register member",
    })
    const form = submit.closest("form")!
    fireEvent.submit(form)
    fireEvent.submit(form)
    expect(mocks.register).toHaveBeenCalledTimes(1)
    expect(mocks.register).toHaveBeenCalledWith({
      name: "New member",
      role: "Review code",
      cwd: "/server/member",
      assistantUid: null,
      projectId: null,
      engine: "codex",
    })
    expect(submit).toBeDisabled()
    await act(async () => {
      pending.resolve(member("New member"))
    })
    expect(screen.getByText("New member")).toBeInTheDocument()
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(screen.getByText("Dispatch off")).toBeInTheDocument()
  })

  it("flips the member dispatch switch without touching the revision", async () => {
    mocks.list.mockResolvedValue([member()])
    mocks.setEnabled.mockResolvedValue({ ...member(), enabled: true })
    await renderPage()
    const toggle = screen.getByRole("switch", {
      name: "Allow dispatch to Reviewer",
    })
    expect(toggle).toHaveAttribute("aria-checked", "false")
    await act(async () => {
      fireEvent.click(toggle)
    })
    expect(mocks.setEnabled).toHaveBeenCalledWith("member-a", true)
    expect(mocks.update).not.toHaveBeenCalled()
    expect(toggle).toHaveAttribute("aria-checked", "true")
    expect(screen.getByText("Dispatch on")).toBeInTheDocument()
  })

  it("edits metadata with the saved revision and keeps the directory read-only", async () => {
    mocks.list.mockResolvedValue([member()])
    await renderPage()
    fireEvent.click(screen.getByRole("button", { name: "Edit Reviewer" }))
    const dialog = screen.getByRole("dialog")
    expect(within(dialog).getByLabelText("Member directory")).toHaveAttribute(
      "readonly"
    )
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Renamed" },
    })
    await act(async () => {
      fireEvent.click(
        within(dialog).getByRole("button", { name: "Save changes" })
      )
    })
    expect(mocks.update).toHaveBeenCalledWith("member-a", 1, {
      name: "Renamed",
      role: "Review code",
      assistantUid: null,
      projectId: null,
      engine: "codex",
    })
    expect(screen.getByText("Renamed")).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Activate" })
    ).not.toBeInTheDocument()
  })

  it("keeps unsaved edits on conflict and does not overwrite the list", async () => {
    mocks.list.mockResolvedValue([member()])
    mocks.update.mockRejectedValue({
      code: "already_exists",
      message: "Revision conflict",
    })
    await renderPage()
    fireEvent.click(screen.getByRole("button", { name: "Edit Reviewer" }))
    const dialog = screen.getByRole("dialog")
    const name = within(dialog).getByLabelText("Name")
    fireEvent.change(name, { target: { value: "My unsaved name" } })
    await act(async () => {
      fireEvent.click(
        within(dialog).getByRole("button", { name: "Save changes" })
      )
    })
    expect(name).toHaveValue("My unsaved name")
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "changed elsewhere"
    )
    expect(screen.getByText("Reviewer")).toBeInTheDocument()
  })

  it("explains an old backend instead of showing an empty registry or falling back", async () => {
    mocks.list.mockRejectedValue({
      code: "not_implemented",
      message: "Not available",
    })
    await renderPage()
    expect(screen.getByRole("alert")).toHaveTextContent(
      "does not support member management"
    )
    expect(
      screen.queryByText("No members registered yet.")
    ).not.toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Register member" })
    ).toBeDisabled()
    expect(mocks.register).not.toHaveBeenCalled()
  })

  it("keeps the registry readable when templates fail and retries templates separately", async () => {
    mocks.list.mockResolvedValue([member()])
    mocks.templates.mockRejectedValueOnce(new Error("network"))
    await renderPage()
    expect(screen.getByText("Reviewer")).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Register member" })
    ).toBeDisabled()
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Retry templates" }))
    })
    expect(mocks.list).toHaveBeenCalledTimes(1)
    expect(mocks.templates).toHaveBeenCalledTimes(2)
    expect(
      screen.getByRole("button", { name: "Register member" })
    ).toBeEnabled()
  })

  it("does not let a slow refresh erase a newly saved member", async () => {
    const stale = deferred<HallMember[]>()
    mocks.list.mockResolvedValueOnce([]).mockReturnValueOnce(stale.promise)
    await renderPage()
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Refresh members" }))
    })
    const dialog = await openRegistration()
    await act(async () => {
      fireEvent.click(
        within(dialog).getByRole("button", { name: "Register member" })
      )
    })
    expect(screen.getByText("New member")).toBeInTheDocument()
    await act(async () => {
      stale.resolve([])
    })
    expect(screen.getByText("New member")).toBeInTheDocument()
  })

  it("discards a late save from the backend whose page was replaced", async () => {
    const pending = deferred<HallMember>()
    mocks.register.mockReturnValue(pending.promise)
    const view = await renderPage()
    const dialog = await openRegistration()
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Register member" })
    )
    mocks.backendId = 2
    await act(async () => {
      view.rerender(page())
    })
    await act(async () => {
      pending.resolve(member("Old backend member"))
    })
    expect(screen.queryByText("Old backend member")).not.toBeInTheDocument()
    expect(screen.getByText("No members registered yet.")).toBeInTheDocument()
  })

  it("keeps a newer refreshed revision when an older save reply arrives late", async () => {
    const pending = deferred<HallMember>()
    const refresh = deferred<HallMember[]>()
    mocks.list
      .mockResolvedValueOnce([member()])
      .mockReturnValueOnce(refresh.promise)
    mocks.update.mockReturnValue(pending.promise)
    await renderPage()
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Refresh members" }))
    })
    fireEvent.click(screen.getByRole("button", { name: "Edit Reviewer" }))
    const dialog = screen.getByRole("dialog")
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "My save" },
    })
    fireEvent.submit(
      within(dialog)
        .getByRole("button", { name: "Save changes" })
        .closest("form")!
    )
    await act(async () => {
      refresh.resolve([member("Newer server name", 3)])
    })
    await act(async () => {
      pending.resolve(member("My save", 2))
    })
    expect(screen.getByText("Newer server name")).toBeInTheDocument()
    expect(screen.queryByText("My save")).not.toBeInTheDocument()
    expect(mocks.templates).toHaveBeenCalledTimes(2)
  })

  it("requires an explicit replacement for an unavailable saved template", async () => {
    mocks.list.mockResolvedValue([
      { ...member(), assistantUid: "deleted-template", assistantRevision: 3 },
    ])
    await renderPage()
    fireEvent.click(screen.getByRole("button", { name: "Edit Reviewer" }))
    const dialog = screen.getByRole("dialog")
    const save = within(dialog).getByRole("button", { name: "Save changes" })
    expect(save).toBeDisabled()
    expect(mocks.update).not.toHaveBeenCalled()
    const template = within(dialog).getByRole("combobox", {
      name: "Assistant template",
    })
    expect(template).toHaveTextContent("Template unavailable")
    fireEvent.keyDown(template, { key: "ArrowDown" })
    fireEvent.click(await screen.findByRole("option", { name: "No template" }))
    expect(save).toBeEnabled()
    await act(async () => {
      fireEvent.click(save)
    })
    expect(mocks.update).toHaveBeenCalledWith("member-a", 1, {
      name: "Reviewer",
      role: "Review code",
      assistantUid: null,
      projectId: null,
      engine: "codex",
    })
  })

  it("does not turn a failed registry load into an empty list and can retry", async () => {
    mocks.list
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce([])
    await renderPage()
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Could not load members"
    )
    expect(
      screen.queryByText("No members registered yet.")
    ).not.toBeInTheDocument()
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Refresh members" }))
    })
    expect(screen.getByText("No members registered yet.")).toBeInTheDocument()
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it("binds a project from the current backend without replacing the member directory", async () => {
    mocks.list.mockResolvedValue([member()])
    mocks.projects.mockResolvedValue([
      { id: 17, name: "Project A", alias: null, path: "/server/project-a" },
    ])
    await renderPage()
    fireEvent.click(screen.getByRole("button", { name: "Edit Reviewer" }))
    const dialog = screen.getByRole("dialog")
    fireEvent.click(within(dialog).getByRole("button", { name: "No project" }))
    fireEvent.click(await screen.findByRole("option", { name: /Project A/ }))
    expect(within(dialog).getByLabelText("Member directory")).toHaveValue(
      "/server/member"
    )
    await act(async () => {
      fireEvent.click(
        within(dialog).getByRole("button", { name: "Save changes" })
      )
    })
    expect(mocks.update).toHaveBeenCalledWith("member-a", 1, {
      name: "Reviewer",
      role: "Review code",
      assistantUid: null,
      projectId: 17,
      engine: "codex",
    })
  })

  it("keeps an unavailable project visible and requires explicit removal or replacement", async () => {
    mocks.list.mockResolvedValue([{ ...member(), projectId: 88 }])
    await renderPage()
    fireEvent.click(screen.getByRole("button", { name: "Edit Reviewer" }))
    const dialog = screen.getByRole("dialog")
    const save = within(dialog).getByRole("button", { name: "Save changes" })
    expect(save).toBeDisabled()
    expect(within(dialog).getByText(/Project unavailable/)).toBeInTheDocument()
    fireEvent.click(within(dialog).getByRole("button", { name: "#88" }))
    fireEvent.click(await screen.findByRole("option", { name: "No project" }))
    expect(save).toBeEnabled()
    await act(async () => {
      fireEvent.click(save)
    })
    expect(mocks.update).toHaveBeenCalledWith("member-a", 1, {
      name: "Reviewer",
      role: "Review code",
      assistantUid: null,
      projectId: null,
      engine: "codex",
    })
  })

  it("previews saved configuration and observed template versions without saving or launching", async () => {
    mocks.list.mockResolvedValue([
      {
        ...member(),
        projectId: 17,
        assistantUid: "preset-a",
        assistantRevision: 7,
      },
    ])
    mocks.projects.mockResolvedValue([
      { id: 17, name: "Project A", alias: "Alias", path: "/server/project-a" },
    ])
    mocks.templates.mockResolvedValue([
      { uid: "preset-a", name: "Review preset", revision: 8 },
    ])
    await renderPage()
    fireEvent.click(screen.getByRole("button", { name: "Preview Reviewer" }))
    const dialog = screen.getByRole("dialog")
    expect(within(dialog).getByText("/server/member")).toBeInTheDocument()
    expect(within(dialog).getByText("/server/project-a")).toBeInTheDocument()
    expect(within(dialog).getByText("member-a")).toBeInTheDocument()
    expect(
      within(dialog).getByText("Recorded: 7 · Current: 8")
    ).toBeInTheDocument()
    expect(
      within(dialog).getByText(/Execution access is not connected yet/)
    ).toBeInTheDocument()
    expect(within(dialog).queryByRole("textbox")).not.toBeInTheDocument()
    expect(
      within(dialog).queryByRole("button", { name: "Save changes" })
    ).not.toBeInTheDocument()
    expect(mocks.register).not.toHaveBeenCalled()
    expect(mocks.update).not.toHaveBeenCalled()
  })

  it("retries projects independently and does not mislabel an unresolved saved binding as unbound", async () => {
    mocks.list.mockResolvedValue([{ ...member(), projectId: 17 }])
    mocks.projects
      .mockRejectedValueOnce(new Error("network"))
      .mockResolvedValueOnce([
        { id: 17, name: "Project A", alias: null, path: "/server/project-a" },
      ])
    await renderPage()
    expect(screen.getByText("Reviewer")).toBeInTheDocument()
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Could not load projects"
    )
    expect(screen.getByRole("button", { name: "Edit Reviewer" })).toBeDisabled()
    fireEvent.click(screen.getByRole("button", { name: "Preview Reviewer" }))
    const dialog = screen.getByRole("dialog")
    expect(
      within(dialog).getByText(/Project unavailable.*#17/)
    ).toBeInTheDocument()
    expect(within(dialog).queryByText("No project")).not.toBeInTheDocument()
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }))
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Retry projects" }))
    })
    expect(mocks.list).toHaveBeenCalledTimes(1)
    expect(mocks.templates).toHaveBeenCalledTimes(1)
    expect(mocks.projects).toHaveBeenCalledTimes(2)
    expect(screen.getByRole("button", { name: "Edit Reviewer" })).toBeEnabled()
  })
})
