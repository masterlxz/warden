import type { ToolSpec } from "../../protocol/messages";
import { runInPage } from "./dom_executor";

export const extractTextSpec: ToolSpec = {
  name: "browser_extract_text",
  description:
    "Extracts text from the active browser tab. With 'selector', returns that element's visible " +
    "text. Without it, returns the current text selection if there is one, otherwise the whole " +
    "page's visible text (truncated).",
  parameters: {
    type: "object",
    properties: {
      selector: { type: "string", description: "Optional CSS selector; omit to use the page's current selection or full text" },
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
  return runInPage(extractTextInPage, [selector ?? null, MAX_TEXT_CHARS]);
}
