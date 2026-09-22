import type { ToolSpec } from "../../protocol/messages";
import { parseOptionalTabId, runInPage } from "./dom_executor";

export const clickElementSpec: ToolSpec = {
  name: "browser_click_element",
  description:
    "Clicks one element in a browser tab, identified by a CSS selector (as returned by " +
    "browser_read_page). If the selector matches more than one element, only the first is clicked " +
    "— make the selector specific enough to be unambiguous.",
  parameters: {
    type: "object",
    properties: {
      selector: { type: "string", description: "CSS selector of the element to click" },
      tabId: { type: "number", description: "Tab id from browser_list_tabs to act on — omit to use whichever tab is currently active/focused." },
    },
    required: ["selector"],
  },
};

interface ClickElementResult {
  clicked: true;
  tag: string;
  text: string;
}

/** Runs inside the page — must be fully self-contained, see `dom_executor.ts`. */
function clickElementInPage(selector: string): ClickElementResult {
  const el = document.querySelector(selector);
  if (!el) {
    throw new Error(`no element matches selector '${selector}'`);
  }
  el.scrollIntoView({ block: "center" });
  (el as HTMLElement).click();
  return { clicked: true, tag: el.tagName.toLowerCase(), text: (el as HTMLElement).innerText?.trim().slice(0, 120) ?? "" };
}

export async function clickElement(args: unknown): Promise<ClickElementResult> {
  const selector = (args as { selector?: unknown } | null)?.selector;
  if (typeof selector !== "string" || !selector) {
    throw new Error("missing required 'selector' argument");
  }
  const tabId = parseOptionalTabId(args);
  return runInPage(clickElementInPage, [selector], tabId);
}
