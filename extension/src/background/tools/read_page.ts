import type { ToolSpec } from "../../protocol/messages";
import { parseOptionalTabId, runInPage } from "./dom_executor";

export const readPageSpec: ToolSpec = {
  name: "browser_read_page",
  description:
    "Reads a browser tab: page title, URL, visible text (truncated), and a list of visible " +
    "interactive elements (links, buttons, inputs, selects, textareas) each with a CSS selector. " +
    "Use the returned selectors with browser_click_element/browser_extract_text.",
  parameters: {
    type: "object",
    properties: {
      tabId: { type: "number", description: "Tab id from browser_list_tabs to read — omit to use whichever tab is currently active/focused." },
    },
  },
};

interface ReadPageElement {
  selector: string;
  tag: string;
  text: string;
}

interface ReadPageResult {
  title: string;
  url: string;
  text: string;
  elements: ReadPageElement[];
}

const MAX_TEXT_CHARS = 4000;
const MAX_ELEMENTS = 50;

/**
 * Runs inside the page (see `dom_executor.ts` doc comment) — must be fully self-contained, no
 * closures over this module's imports.
 */
function readPageInPage(maxTextChars: number, maxElements: number): ReadPageResult {
  function isVisible(el: Element): boolean {
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && (el as HTMLElement).offsetParent !== null;
  }

  function cssSelector(el: Element): string {
    if (el.id) return `#${CSS.escape(el.id)}`;
    const parts: string[] = [];
    let node: Element | null = el;
    while (node && node.nodeType === Node.ELEMENT_NODE && parts.length < 6) {
      if (node.id) {
        parts.unshift(`#${CSS.escape(node.id)}`);
        break;
      }
      const tag = node.tagName.toLowerCase();
      const parent: Element | null = node.parentElement;
      if (!parent) {
        parts.unshift(tag);
        break;
      }
      const siblings = Array.from(parent.children).filter((sib) => sib.tagName === node!.tagName);
      const index = siblings.indexOf(node) + 1;
      parts.unshift(siblings.length > 1 ? `${tag}:nth-of-type(${index})` : tag);
      node = parent;
    }
    return parts.join(" > ");
  }

  function elementText(el: Element): string {
    const text =
      (el as HTMLInputElement).value ||
      el.getAttribute("aria-label") ||
      el.getAttribute("placeholder") ||
      (el as HTMLElement).innerText ||
      "";
    return text.trim().slice(0, 120);
  }

  const selector = "a, button, input, select, textarea, [role='button'], [role='link']";
  const candidates = Array.from(document.querySelectorAll(selector)).filter(isVisible);

  const elements: ReadPageElement[] = candidates.slice(0, maxElements).map((el) => ({
    selector: cssSelector(el),
    tag: el.tagName.toLowerCase(),
    text: elementText(el),
  }));

  return {
    title: document.title,
    url: location.href,
    text: (document.body.innerText || "").trim().slice(0, maxTextChars),
    elements,
  };
}

export async function readPage(args: unknown): Promise<ReadPageResult> {
  const tabId = parseOptionalTabId(args);
  return runInPage(readPageInPage, [MAX_TEXT_CHARS, MAX_ELEMENTS], tabId);
}
