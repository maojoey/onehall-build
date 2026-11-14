"use client"

import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"

import { toErrorMessage } from "@/lib/app-error"
import { readTargetInfo, type TargetInfo } from "@/lib/hall/target-info"

/** What is on the machine a task is about to be sent to. */
export function BranchTargetInfo({
  connectBranch,
  selected = null,
  onSelect,
}: {
  /** Resolves the connection id that reaches the branch. */
  connectBranch: () => Promise<number>
  /** The member the sender asks for; null leaves the choice to the branch. */
  selected?: string | null
  onSelect?: (memberId: string | null) => void
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
          {onSelect && (
            <li>
              <label className="flex items-center gap-1.5">
                <input
                  type="radio"
                  name="branch-target-member"
                  checked={selected === null}
                  onChange={() => onSelect(null)}
                />
                <span>{t("anyMember")}</span>
              </label>
            </li>
          )}
          {members.map((m) => {
            const text = (
              <>
                <span className="font-medium">{m.name}</span>
                <span className="text-muted-foreground">
                  {" "}
                  · {m.engine}
                  {m.role ? ` · ${m.role}` : ""}
                </span>
              </>
            )
            return (
              <li key={m.memberId}>
                {onSelect ? (
                  <label className="flex items-center gap-1.5">
                    <input
                      type="radio"
                      name="branch-target-member"
                      checked={selected === m.memberId}
                      onChange={() => onSelect(m.memberId)}
                    />
                    <span>{text}</span>
                  </label>
                ) : (
                  text
                )}
              </li>
            )
          })}
        </ul>
      )}
      <p className="text-muted-foreground">
        {info.agents
          .map((a) => `${a.agent} ${a.adapter ?? "—"} (CLI ${a.cli ?? "—"})`)
          .join(" · ")}
      </p>
      <p className="text-2xs text-muted-foreground">
        {t(onSelect ? "pickNote" : "note")}
      </p>
    </div>
  )
}
