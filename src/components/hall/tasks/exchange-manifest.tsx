"use client"
import { useTranslations } from "next-intl"
import { isExchangeManifest } from "@/lib/hall/exchange"

/**
 * Frozen input files and output slots, exactly as the host reports them.
 * Shown only when well formed and, if a bound digest is given, matching it;
 * otherwise the reviewer sees that the details are unavailable, never a guess.
 */
export function ExchangeManifestView({
  manifest,
  digest,
}: {
  manifest: unknown
  digest?: string
}) {
  const t = useTranslations("Hall.manifest")
  if (manifest === undefined)
    return (
      <p className="text-xs text-muted-foreground">{t("detailsMissing")}</p>
    )
  if (
    !isExchangeManifest(manifest) ||
    (digest !== undefined && manifest.digest !== digest)
  )
    return (
      <p role="alert" className="text-sm text-destructive">
        {t("unreadable")}
      </p>
    )
  return (
    <div className="space-y-2 text-sm" aria-label={t("title")}>
      <p className="font-medium">{t("inputs")}</p>
      {manifest.inputs.length === 0 ? (
        <p className="text-muted-foreground">{t("noInputs")}</p>
      ) : (
        <ul className="space-y-1">
          {manifest.inputs.map((input) => (
            <li
              key={input.objectId}
              className="flex flex-wrap items-center gap-2"
            >
              <span className="break-all">{input.displayName}</span>
              <span className="text-xs text-muted-foreground">
                {t("size", { bytes: input.size })}
              </span>
              <code
                dir="ltr"
                className="text-xs text-muted-foreground"
                title={input.sha256}
              >
                {input.sha256.slice(0, 12)}
              </code>
            </li>
          ))}
        </ul>
      )}
      <p className="font-medium">{t("outputs")}</p>
      {manifest.outputs.length === 0 ? (
        <p className="text-muted-foreground">{t("noOutputs")}</p>
      ) : (
        <ul className="list-disc space-y-1 ps-5">
          {manifest.outputs.map((slot) => (
            <li key={slot.slotId} className="break-all">
              {slot.displayName}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
