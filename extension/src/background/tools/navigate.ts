import type { ToolSpec } from "../../protocol/messages";
import { navigateActiveTab } from "./dom_executor";

export const navigateSpec: ToolSpec = {
  name: "browser_navigate",
  description:
    "Navigates the active browser tab to a URL. Note: after this, the extension's activeTab " +
    "permission for that tab resets — a following browser_read_page/browser_click_element/" +
    "browser_extract_text call may fail until the user reopens the extension popup on the tab.",
  parameters: {
    type: "object",
    properties: { url: { type: "string", description: "URL to navigate the active tab to" } },
    required: ["url"],
  },
};

export async function navigate(args: unknown): Promise<{ navigated: true; url: string }> {
  const url = (args as { url?: unknown } | null)?.url;
  if (typeof url !== "string" || !url) {
    throw new Error("missing required 'url' argument");
  }
  await navigateActiveTab(url);
  return { navigated: true, url };
}
