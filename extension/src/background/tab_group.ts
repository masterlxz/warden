/**
 * The "Warden" tab group (P69) — lets the DOM tools act on more than the currently active tab,
 * modeled after Claude in Chrome's own tab group. State lives entirely in memory, same posture as
 * `history`/`connection` in `index.ts`: it dies with the service worker, no new failure mode to
 * guard against.
 *
 * Permission model, confirmed with the user before building this: the user adds tabs one at a
 * time from the side panel (`addActiveTabToGroup`) — that click *is* the qualifying gesture that
 * grants `activeTab` for that specific tab, same permission the single-tab flow already relied on,
 * just accumulated per tab instead of re-earned on every call. Nothing here requests the broader
 * `tabs` permission or any `host_permissions` — the extension never reads a tab it wasn't
 * explicitly handed.
 */

const GROUP_TITLE = "Warden";

const grantedTabs = new Set<number>();
let groupId: number | null = null;
let onGroupChanged: (() => void) | null = null;

export interface GroupTab {
  tabId: number;
  title: string;
  url: string;
  active: boolean;
}

export function setGroupChangeListener(callback: () => void): void {
  onGroupChanged = callback;
}

function notifyChanged(): void {
  onGroupChanged?.();
}

/** Adds the currently active tab to the group — the click that triggers this IS the gesture that
 * grants `activeTab` for it. Creates the group on the first call, reuses it after. No-op if the
 * tab is already in the group. */
export async function addActiveTabToGroup(): Promise<GroupTab> {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab?.id) {
    throw new Error("no active tab found");
  }
  const tabId = tab.id;

  if (!grantedTabs.has(tabId)) {
    const newGroupId = groupId !== null ? await chrome.tabs.group({ tabIds: [tabId], groupId }) : await chrome.tabs.group({ tabIds: [tabId] });
    if (groupId === null) {
      groupId = newGroupId;
      await chrome.tabGroups.update(groupId, { title: GROUP_TITLE, color: "blue" });
    }
    grantedTabs.add(tabId);
    notifyChanged();
  }

  return { tabId, title: tab.title ?? "", url: tab.url ?? "", active: true };
}

/** Removes a tab from the group (ungroups it visually too). Tolerates the tab already being
 * closed — same idempotent-on-already-gone posture as `Vault::delete`'s Rust-side counterpart. */
export async function removeTabFromGroup(tabId: number): Promise<void> {
  const had = grantedTabs.delete(tabId);
  try {
    await chrome.tabs.ungroup(tabId);
  } catch {
    // Tab already closed or already out of the group — nothing left to undo.
  }
  if (had) notifyChanged();
}

/** Whether `tabId` was explicitly added by the user — `dom_executor.ts` checks this before trying
 * to act on an explicit `tabId`, so a stale/wrong id fails with a clear message instead of
 * whatever generic error `chrome.scripting.executeScript` would throw. */
export function isTabInGroup(tabId: number): boolean {
  return grantedTabs.has(tabId);
}

/** Current group membership, pruning any tabId that no longer exists. Reads `title`/`url` without
 * needing the `tabs` permission — `activeTab`, once granted to a specific tab via
 * `addActiveTabToGroup`'s gesture, already unlocks reading those fields for that tab. */
export async function listGroupTabs(): Promise<GroupTab[]> {
  const [activeTab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const result: GroupTab[] = [];
  let pruned = false;

  for (const tabId of grantedTabs) {
    try {
      const tab = await chrome.tabs.get(tabId);
      result.push({ tabId, title: tab.title ?? "", url: tab.url ?? "", active: tabId === activeTab?.id });
    } catch {
      grantedTabs.delete(tabId);
      pruned = true;
    }
  }
  if (pruned) notifyChanged();
  return result;
}

// Proactive pruning when a grouped tab closes, so the panel's list stays accurate without waiting
// for the next `listGroupTabs` poll.
chrome.tabs.onRemoved.addListener((tabId) => {
  if (grantedTabs.delete(tabId)) notifyChanged();
});
