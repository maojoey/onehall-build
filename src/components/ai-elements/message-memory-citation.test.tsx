import { render, waitFor } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

vi.mock("@/components/ai-elements/link-safety", () => ({
  useStreamdownLinkSafety: () => ({ enabled: false }),
}))

import { MessageResponse } from "./message"

// Synthetic citation metadata: never copy private memory notes into fixtures.
const ENTRY = "MEMORY.md:10-12|note=[verification workflow]"
const ROLLOUT = "00000000-0000-4000-8000-000000000001"
const CITATION = `<oai-mem-citation>
<citation_entries>
${ENTRY}
</citation_entries>
<rollout_ids>
${ROLLOUT}
</rollout_ids>
</oai-mem-citation>`

describe("assistant memory citation metadata", () => {
  it("omits the internal footer while keeping the answer and its links", () => {
    const { container } = render(
      <MessageResponse>
        {`Checks complete. [Details](https://example.com/report).\n${CITATION}`}
      </MessageResponse>
    )
    expect(container.textContent).toContain("Checks complete.")
    expect(container.querySelector('[data-streamdown="link"]')).toHaveAttribute(
      "title",
      "https://example.com/report"
    )
    expect(container.textContent).not.toContain(ENTRY)
    expect(container.textContent).not.toContain(ROLLOUT)
  })

  it("does not leak citation contents before the streaming closer arrives", () => {
    const { container, rerender } = render(
      <MessageResponse mode="streaming" parseIncompleteMarkdown>
        {`Checks complete.\n<oai-mem-citation>\n<citation_entries>\n${ENTRY}`}
      </MessageResponse>
    )
    expect(container.textContent).toContain("Checks complete.")
    expect(container.textContent).not.toContain(ENTRY)

    rerender(
      <MessageResponse>{`Checks complete.\n${CITATION}\n\nNext steps.`}</MessageResponse>
    )
    expect(container.textContent).toContain("Next steps.")
    expect(container.textContent).not.toContain(ENTRY)
    expect(container.textContent).not.toContain(ROLLOUT)
  })

  it("keeps literal metadata in inline code and fenced examples", async () => {
    const { container } = render(
      <MessageResponse>
        {`Literal: \`<oai-mem-citation>${ENTRY}</oai-mem-citation>\`\n\n\`\`\`text\n${CITATION}\n\`\`\``}
      </MessageResponse>
    )
    await waitFor(() => expect(container.textContent).toContain(ROLLOUT))
    expect(container.querySelector("code")?.textContent).toContain(ENTRY)
    expect(container.textContent).toContain("<oai-mem-citation>")
  })

  it("keeps ordinary file references, notes and UUIDs outside the wrapper", () => {
    const text = `Investigate ${ENTRY} and session ${ROLLOUT}.`
    const { container } = render(<MessageResponse>{text}</MessageResponse>)
    expect(container.textContent).toBe(text)
  })

  it.each([false, true])(
    "omits a footer with blank lines (streaming=%s)",
    (streaming) => {
      const { container } = render(
        <MessageResponse
          mode={streaming ? "streaming" : "static"}
          parseIncompleteMarkdown={streaming}
        >
          {`Checks complete.\n\n${CITATION.split("\n").join("\n\n")}`}
        </MessageResponse>
      )
      expect(container.textContent).toContain("Checks complete.")
      expect(container.textContent).not.toContain(ENTRY)
      expect(container.textContent).not.toContain(ROLLOUT)
    }
  )
})
