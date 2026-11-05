"use client"

import { useTranslations } from "next-intl"
import type {
  HallMemberConfigField,
  HallMemberConfigPreview,
  HallMemberConfigValue,
} from "@/lib/hall/members"

export const labels = {
  model: "model",
  "scope.read": "read",
  "scope.write": "write",
  "scope.shell": "shell",
  "scope.network": "network",
  "skills.shared": "shared",
  "skills.own": "own",
  onedesk_key: "onedeskKey",
  execution_mode: "executionMode",
} as const
const diagnosticMessages = {
  directory_unavailable: "config.errors.directory_unavailable",
  unreadable: "config.errors.unreadable",
  not_regular: "config.errors.not_regular",
  too_large: "config.errors.too_large",
  invalid_yaml: "config.errors.invalid_yaml",
  invalid_value: "config.errors.invalid_value",
  unsupported_version: "config.errors.unsupported_version",
  unknown_field: "config.errors.unknown_field",
  identity_conflict: "config.errors.identity_conflict",
  multiple_manuals: "config.errors.multiple_manuals",
} as const

export function diagnosticMessage(code: string) {
  return Object.prototype.hasOwnProperty.call(diagnosticMessages, code)
    ? diagnosticMessages[code as keyof typeof diagnosticMessages]
    : diagnosticMessages.invalid_value
}

export function fieldValue(
  result: HallMemberConfigPreview,
  field: HallMemberConfigField
) {
  const value = result.effective
  if (!value) return null
  switch (field) {
    case "model":
      return value.model
    case "scope.read":
      return value.scope.read
    case "scope.write":
      return value.scope.write
    case "scope.shell":
      return value.scope.shell
    case "scope.network":
      return value.scope.network
    case "skills.shared":
      return value.skills.shared
    case "skills.own":
      return value.skills.own
    case "onedesk_key":
      return value.onedesk_key ?? "none"
    case "execution_mode":
      return value.execution_mode ?? null
  }
}

export function ConfigValue({ value }: { value: HallMemberConfigValue }) {
  const t = useTranslations("Hall.members.config")
  return (
    <div className="space-y-1 break-all font-mono text-xs text-muted-foreground">
      {value === null ? (
        <p>{t("unspecified")}</p>
      ) : Array.isArray(value) ? (
        value.length ? (
          value.map((entry, i) => (
            <p key={i} dir="auto">
              {entry}
            </p>
          ))
        ) : (
          <p>{t("empty")}</p>
        )
      ) : (
        <p dir="auto">{value}</p>
      )}
    </div>
  )
}
