// All conversation operations use the pet's bound transport. Only shortcut
// settings and window management belong to the local desktop shell.
import { acpConnect, acpFindConnectionForConversation } from "@/lib/api"
import { getShellTransport } from "@/lib/transport"
import type {
  DbConversationSummary,
  FolderDetail,
  MessageTurn,
} from "@/lib/types"

export interface PetChatSettings {
  shortcut: string
  status: { active?: string; failed?: string; detail?: string }
}

export function getPetChatSettings(): Promise<PetChatSettings> {
  return getShellTransport().call("hall_get_pet_chat_settings")
}

export function setPetChatShortcut(shortcut: string): Promise<PetChatSettings> {
  return getShellTransport().call("hall_set_pet_chat_shortcut", { shortcut })
}

export function petChatStorageKey(
  remoteId: number | null,
  conversationId?: number
): string {
  const realm = remoteId === null ? "local" : `r${remoteId}`
  return `onehall:pet-chat:${realm}:${conversationId === undefined ? "target" : `draft:${conversationId}`}`
}

export function lastAssistantText(turns: MessageTurn[]): string {
  const turn = [...turns].reverse().find((turn) => turn.role === "assistant")
  return (
    turn?.blocks
      .flatMap((block) => (block.type === "text" ? [block.text] : []))
      .join("\n") ?? ""
  )
}

export async function connectPetChat(
  target: DbConversationSummary,
  folder: FolderDetail
): Promise<string> {
  const live = await acpFindConnectionForConversation(
    target.id,
    target.external_id ?? undefined,
    target.agent_type
  )
  if (live) return live.connection_id
  // No shell, new task or prompt on mere panel-open. This is called only on
  // explicit send. acp_connect resumes/deduplicates by agent/cwd/session.
  return acpConnect(
    target.agent_type,
    folder.path,
    target.external_id ?? undefined
  )
}
