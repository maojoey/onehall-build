// onehall: hall settings live in the LOCAL desktop's database (they describe
// this shell), so they always go through the shell transport.
import { getShellTransport } from "@/lib/transport"
import type { HallSettings } from "./types"

export async function hallGetSettings(): Promise<HallSettings> {
  return getShellTransport().call("hall_get_settings")
}

export async function hallSetSettings(
  settings: HallSettings
): Promise<HallSettings> {
  return getShellTransport().call("hall_set_settings", { settings })
}
