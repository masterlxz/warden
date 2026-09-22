import type { ToolSpec } from "../../protocol/messages";
import { navigateActiveTab, parseOptionalTabId } from "./dom_executor";

export const navigateSpec: ToolSpec = {
  name: "browser_navigate",
  description:
    "Navigates a browser tab to a URL. Note: after this, the extension's activeTab permission " +
    "for that tab resets — a following browser_read_page/browser_click_element/browser_extract_text " +
    "call on it may fail until the user reopens the extension side panel (or re-adds the tab to the " +
    "Warden group) on that tab.",
  parameters: {
    type: "object",
    properties: {
      url: { type: "string", description: "URL to navigate the tab to" },
      tabId: { type: "number", description: "Tab id from browser_list_tabs to navigate — omit to use whichever tab is currently active/focused." },
    },
    required: ["url"],
  },
};

export async function navigate(args: unknown): Promise<{ navigated: true; url: string }> {
  const url = (args as { url?: unknown } | null)?.url;
  if (typeof url !== "string" || !url) {
    throw new Error("missing required 'url' argument");
  }
  const tabId = parseOptionalTabId(args);
  await navigateActiveTab(url, tabId);
  return { navigated: true, url };
}
