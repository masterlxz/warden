import type { ToolSpec } from "../../protocol/messages";
import { listGroupTabs, type GroupTab } from "../tab_group";

export const listTabsSpec: ToolSpec = {
  name: "browser_list_tabs",
  description:
    "Lists the browser tabs the user has added to the Warden tab group (P69). Pass one of the " +
    "returned tabId values to browser_read_page/browser_click_element/browser_navigate/" +
    "browser_extract_text to act on that specific tab instead of whichever tab is currently " +
    "active. Empty means the user hasn't added any tab yet — those 4 tools still work on whichever " +
    "tab is currently active/focused.",
  parameters: { type: "object", properties: {} },
};

export async function listTabs(): Promise<GroupTab[]> {
  return listGroupTabs();
}
