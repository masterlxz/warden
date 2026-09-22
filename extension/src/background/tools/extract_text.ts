import type { ToolSpec } from "../../protocol/messages";
import { parseOptionalTabId, runInPage } from "./dom_executor";

export const extractTextSpec: ToolSpec = {
  name: "browser_extract_text",
  description:
    "Extracts text from a browser tab. With 'selector', returns that element's visible text. " +
    "Without it, returns the current text selection if there is one, otherwise the whole page's " +
    "visible text (truncated).",
  parameters: {
    type: "object",
    properties: {
      selector: { type: "string", description: "Optional CSS selector; omit to use the page's current selection or full text" },
      tabId: { type: "number", description: "Tab id from browser_list_tabs to act on — omit to use whichever tab is currently active/focused." },
    },
  },
};

const MAX_TEXT_CHARS = 4000;

/** Runs inside the page — must be fully self-contained, see `dom_executor.ts`. */
function extractTextInPage(selector: string | null, maxChars: number): string {
  if (selector) {
    const el = document.querySelector(selector);
    if (!el) {
      throw new Error(`no element matches selector '${selector}'`);
    }
    return ((el as HTMLElement).innerText || "").trim().slice(0, maxChars);
  }
  const selected = window.getSelection()?.toString().trim() ?? "";
  if (selected) return selected.slice(0, maxChars);
  return (document.body.innerText || "").trim().slice(0, maxChars);
}

export async function extractText(args: unknown): Promise<string> {
  const selector = (args as { selector?: unknown } | null)?.selector;
  if (selector !== undefined && typeof selector !== "string") {
    throw new Error("'selector' argument must be a string when provided");
  }
  const tabId = parseOptionalTabId(args);
  return runInPage(extractTextInPage, [selector ?? null, MAX_TEXT_CHARS], tabId);
}
