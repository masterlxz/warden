/** DOM tools (Fase 8.3-8.6) — always advertised, no config prerequisite (unlike mobile's
 * folder-gated file tools). See `dom_executor.ts` for the shared `activeTab` access pattern. */

import type { ToolSpec } from "../../protocol/messages";
import type { ToolHandler } from "../connection";
import { readPageSpec, readPage } from "./read_page";
import { clickElementSpec, clickElement } from "./click_element";
import { navigateSpec, navigate } from "./navigate";
import { extractTextSpec, extractText } from "./extract_text";
import { listTabsSpec, listTabs } from "./list_tabs";

export const toolSpecs: ToolSpec[] = [readPageSpec, clickElementSpec, navigateSpec, extractTextSpec, listTabsSpec];

export const toolHandlers: Record<string, ToolHandler> = {
  [readPageSpec.name]: (args) => readPage(args),
  [clickElementSpec.name]: (args) => clickElement(args),
  [navigateSpec.name]: (args) => navigate(args),
  [extractTextSpec.name]: (args) => extractText(args),
  [listTabsSpec.name]: () => listTabs(),
};
