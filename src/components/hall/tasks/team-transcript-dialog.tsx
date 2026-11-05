"use client"
import { useEffect } from "react"
import { useAcpActions } from "@/contexts/acp-connections-context"
import { LiveTranscriptView } from "@/components/message/live-transcript-view"
import {
  Drawer,
  DrawerContent,
  DrawerTitle,
  DrawerDescription,
  SIDE_PANEL_CONTENT_CLASS,
} from "@/components/ui/drawer"
import type { TeamSession } from "@/lib/hall/control"
export function TeamTranscriptDialog({
  session,
  title,
  close,
}: {
  session: TeamSession
  title: string
  close: () => void
}) {
  const { attachDelegationChild, detachDelegationChild } = useAcpActions()
  useEffect(() => {
    const id = session.connectionId
    if (!id) return
    attachDelegationChild({
      connectionId: id,
      parentConnectionId: id,
      parentToolUseId: `hall-task-${session.conversationId}`,
      agentType: session.agentType,
      hydrate: true,
    })
    return () => detachDelegationChild(id)
  }, [session, attachDelegationChild, detachDelegationChild])
  return (
    <Drawer
      open
      onOpenChange={(open) => {
        if (!open) close()
      }}
      swipeDirection="right"
    >
      <DrawerContent className={SIDE_PANEL_CONTENT_CLASS}>
        <DrawerTitle className="border-b px-4 py-3 text-sm font-medium">
          {title}
        </DrawerTitle>
        <DrawerDescription className="sr-only">{title}</DrawerDescription>
        <LiveTranscriptView
          conversationId={session.conversationId}
          connectionId={session.connectionId}
          agentType={session.agentType}
        />
      </DrawerContent>
    </Drawer>
  )
}
