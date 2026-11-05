"use client"
import { Star } from "lucide-react"
import { useTranslations } from "next-intl"
import { cn } from "@/lib/utils"

/** Marks a OneHall-only feature, unavailable on an upstream Codeg backend. */
export function HallMark({ className }: { className?: string }) {
  const t = useTranslations("Hall")
  return (
    <span
      role="img"
      aria-label={t("mark")}
      title={t("mark")}
      className={cn("inline-flex shrink-0 items-center", className)}
    >
      <Star className="size-3 fill-amber-500 text-amber-500" />
    </span>
  )
}
