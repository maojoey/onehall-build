"use client"
import { useTranslations } from "next-intl"
import {
  isHallExecutionMode,
  readCapabilities,
  type HallExecutionMode,
} from "@/lib/hall/members"

const exitKeys = [
  "linux_cgroup_v2",
  "windows_job_object",
  "macos_process_group_best_effort",
  "unknown",
] as const

/**
 * Shows only what the host reported. A missing or malformed field is shown as
 * unknown; nothing here is inferred, and nothing is ever labelled "isolated".
 */
export function ExecutionCapabilities({
  mode,
  capabilities,
}: {
  mode?: HallExecutionMode | null
  capabilities?: unknown
}) {
  const t = useTranslations("Hall.members.executionMode")
  const known = isHallExecutionMode(mode) ? mode : null
  const caps = readCapabilities(capabilities)
  const exit = caps
    ? (exitKeys as readonly string[]).includes(caps.exitCoverage)
      ? caps.exitCoverage
      : "unknown"
    : "unknown"
  return (
    <div className="space-y-2 text-xs" data-mode={known ?? "unknown"}>
      <p className="text-sm font-medium">
        {t(known ? `label.${known}` : "label.unknown")}
      </p>
      <p className="leading-5 text-muted-foreground">
        {t(known ? `about.${known}` : "about.unknown")}
      </p>
      {known === "local_assistant" && (
        <p className="leading-5 text-muted-foreground">
          {t("noSystemIsolation")}
        </p>
      )}
      <ul aria-label={t("capabilities")} className="space-y-1">
        {(
          [
            ["read", caps?.readBoundary],
            ["write", caps?.writeBoundary],
            ["network", caps?.networkBoundary],
          ] as const
        ).map(([name, value]) => (
          <li key={name}>
            {t(`boundary.${name}`)}: {t(`enforcement.${value ?? "unknown"}`)}
          </li>
        ))}
        <li>
          {t("permissions")}:{" "}
          {t(`permissionRequests.${caps?.permissionRequests ?? "unknown"}`)}
        </li>
        <li>
          {t("hallIdentity")}:{" "}
          {t(caps ? (caps.hallIdentity ? "yes" : "no") : "enforcement.unknown")}
        </li>
        <li>
          {t("hallReport")}:{" "}
          {t(caps ? (caps.hallReport ? "yes" : "no") : "enforcement.unknown")}
        </li>
        <li>
          {t("exitCoverage")}: {t(`exit.${exit}`)}
        </li>
        {caps?.engineVersion && (
          <li>
            {t("engineVersion")}: <span dir="ltr">{caps.engineVersion}</span>
          </li>
        )}
        {caps?.adapterVersion && (
          <li>
            {t("adapterVersion")}: <span dir="ltr">{caps.adapterVersion}</span>
          </li>
        )}
      </ul>
    </div>
  )
}
