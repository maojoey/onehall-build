"use client"
import { useState } from "react"
import { useTranslations } from "next-intl"
import { HallMark } from "@/components/hall/hall-mark"
import { useRemoteConnection } from "@/contexts/remote-connection-context"
import { Button } from "@/components/ui/button"
import { LocalBranchInbox } from "../branches/local-inbox"
import { TeamTasks } from "./team-tasks"
import { TeamMailbox } from "../mailbox/team-mailbox"
export function TeamWorkbenchTitle() {
  const t = useTranslations("Hall.workflow")
  return (
    <span className="inline-flex items-center gap-1 text-sm font-medium">
      {t("nav")}
      <HallMark />
    </span>
  )
}
export function TeamWorkbench() {
  const remote = useRemoteConnection()?.connection
  return <TeamWorkbenchBody key={remote?.id ?? "local"} />
}
function TeamWorkbenchBody() {
  const t = useTranslations("Hall.workflow")
  const inboxT = useTranslations("Hall.branchInbox")
  const [mailboxOpened, setMailboxOpened] = useState(false)
  const [view, setView] = useState<"tasks" | "mailbox" | "branchInbox">("tasks")
  const [refreshTasksHint, setRefreshTasksHint] = useState(false)
  return (
    <div className="flex h-full min-h-0 flex-col">
      <nav aria-label={t("nav")} className="flex gap-2 border-b px-4 py-2">
        <Button
          size="sm"
          variant={view === "tasks" ? "secondary" : "ghost"}
          aria-pressed={view === "tasks"}
          onClick={() => setView("tasks")}
        >
          {t("tasks")}
        </Button>
        <Button
          size="sm"
          variant={view === "mailbox" ? "secondary" : "ghost"}
          aria-pressed={view === "mailbox"}
          onClick={() => {
            setMailboxOpened(true)
            setView("mailbox")
          }}
        >
          {t("mailbox")}
        </Button>
        <Button
          size="sm"
          variant={view === "branchInbox" ? "secondary" : "ghost"}
          aria-pressed={view === "branchInbox"}
          onClick={() => setView("branchInbox")}
        >
          {t("branchInbox")}
        </Button>
      </nav>
      <div className="min-h-0 flex-1">
        <div hidden={view !== "tasks"} className="h-full">
          {refreshTasksHint && (
            <p
              role="status"
              className="px-4 pt-3 text-sm text-muted-foreground"
            >
              {inboxT("refreshTasksHint")}
            </p>
          )}
          <TeamTasks
            onOpenMailbox={() => {
              setMailboxOpened(true)
              setView("mailbox")
            }}
          />
        </div>
        {view === "branchInbox" && (
          <LocalBranchInbox
            onAccepted={() => {
              setRefreshTasksHint(true)
              setView("tasks")
            }}
          />
        )}
        {mailboxOpened && (
          <div hidden={view !== "mailbox"} className="h-full">
            <TeamMailbox />
          </div>
        )}
      </div>
    </div>
  )
}
