"use client"

import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"

import { toErrorMessage } from "@/lib/app-error"
import { readTargetInfo, type TargetInfo } from "@/lib/hall/target-info"

/** What is on the machine a task is about to be sent to. */
export function BranchTargetInfo({
  connectBranch,
}: {
  /** Resolves the connection id that reaches the branch. */
  connectBranch: () => Promise<number>
}) {
  const t = useTranslations("Hall.branches.target")
  // Built inline by the parent; a ref keeps the load from rerunning each render.
  const connect = useRef(connectBranch)
  useLayoutEffect(() => {
    connect.current = connectBranch
  })
  const [info, setInfo] = useState<TargetInfo | null>(null)
  const [failure, setFailure] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const next = await readTargetInfo(await connect.current())
        if (!cancelled) setInfo(next)
      } catch (error) {
        if (!cancelled) setFailure(toErrorMessage(error))
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  if (failure)
    return (
      <p role="alert" className="text-xs text-muted-foreground">
        {t("unavailable")}
      </p>
    )
  if (!info)
    return <p className="text-xs text-muted-foreground">{t("loading")}</p>
  const members = info.members.filter((m) => m.enabled)
  return (
    <div className="space-y-1 rounded-md border p-2 text-xs">
      <p className="font-medium">{t("title")}</p>
      {members.length === 0 ? (
        <p className="text-muted-foreground">{t("noMembers")}</p>
      ) : (
        <ul className="space-y-0.5">
          {members.map((m) => (
            <li key={`${m.name}:${m.engine}`}>
              <span className="font-medium">{m.name}</span>
              <span className="text-muted-foreground">
                {" "}
                · {m.engine}
                {m.role ? ` · ${m.role}` : ""}
              </span>
            </li>
          ))}
        </ul>
      )}
      <p className="text-muted-foreground">
        {info.agents
          .map((a) => `${a.agent} ${a.adapter ?? "—"} (CLI ${a.cli ?? "—"})`)
          .join(" · ")}
      </p>
      <p className="text-2xs text-muted-foreground">{t("note")}</p>
    </div>
  )
}
