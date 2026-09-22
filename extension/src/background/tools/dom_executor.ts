/**
 * Shared plumbing for the DOM tools (Fase 8.3-8.6) — one place to resolve which tab to act on and
 * to run a function inside its page, so the tools don't each duplicate error handling.
 *
 * Chrome constraint that shapes every tool built on `runInPage`: the function passed to
 * `chrome.scripting.executeScript` is serialized and run in an isolated world in the page — it
 * can NOT close over anything from this module (no imports, no outer variables). Data only
 * crosses the boundary via `args` (in) and the function's `return` value (out), both of which
 * must be JSON-serializable. Every tool file defines its injected function as a fully
 * self-contained closure for this reason, even at the cost of a little duplication between them.
 */

import { isTabInGroup } from "../tab_group";

async function getActiveTabId(): Promise<number> {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab?.id) {
    throw new Error("no active tab found");
  }
  return tab.id;
}

/** With an explicit `tabId` (P69, the Warden tab group), it must be one the user already added —
 * checked here so a stale/wrong id fails with a clear message instead of whatever generic error
 * `chrome.scripting.executeScript` throws. Without one, falls back to the tab that's currently
 * active — the original, single-tab behavior, unchanged for anyone who never opens the group. */
async function resolveTabId(explicitTabId?: number): Promise<number> {
  if (explicitTabId !== undefined) {
    if (!isTabInGroup(explicitTabId)) {
      throw new Error(`tab ${explicitTabId} is not in the Warden group — add it from the side panel first, or omit tabId to use the active tab`);
    }
    return explicitTabId;
  }
  return getActiveTabId();
}

/** Parses the optional `tabId` argument every DOM tool accepts (P69) out of its raw `args`. */
export function parseOptionalTabId(args: unknown): number | undefined {
  const raw = (args as { tabId?: unknown } | null)?.tabId;
  if (raw === undefined) return undefined;
  if (typeof raw !== "number") {
    throw new Error("'tabId' argument must be a number when provided");
  }
  return raw;
}

/**
 * Runs `func(...args)` inside the target tab's page and returns its result. Translates the two
 * permission failures `activeTab`-only scope can hit into one actionable message — see
 * `manifest.config.ts` for why `host_permissions` isn't used instead.
 */
export async function runInPage<A extends unknown[], R>(func: (...args: A) => R, args: A, explicitTabId?: number): Promise<R> {
  const tabId = await resolveTabId(explicitTabId);
  try {
    const [injection] = await chrome.scripting.executeScript({ target: { tabId }, func, args });
    return injection.result as R;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const hint =
      explicitTabId !== undefined
        ? "a aba pode ter navegado desde que foi adicionada ao grupo — remova e adicione de novo no painel lateral"
        : "abra o painel lateral nessa aba e tente de novo";
    throw new Error(`sem acesso à aba (${message}) — ${hint}`);
  }
}

export async function navigateActiveTab(url: string, explicitTabId?: number, timeoutMs = 10_000): Promise<void> {
  const tabId = await resolveTabId(explicitTabId);
  await new Promise<void>((resolve, reject) => {
    const timeoutId = setTimeout(() => {
      chrome.tabs.onUpdated.removeListener(onUpdated);
      reject(new Error(`navigation to ${url} did not complete within ${timeoutMs / 1000}s`));
    }, timeoutMs);

    const onUpdated = (updatedTabId: number, changeInfo: chrome.tabs.OnUpdatedInfo) => {
      if (updatedTabId === tabId && changeInfo.status === "complete") {
        clearTimeout(timeoutId);
        chrome.tabs.onUpdated.removeListener(onUpdated);
        resolve();
      }
    };
    chrome.tabs.onUpdated.addListener(onUpdated);

    chrome.tabs.update(tabId, { url }).catch((err: unknown) => {
      clearTimeout(timeoutId);
      chrome.tabs.onUpdated.removeListener(onUpdated);
      reject(err instanceof Error ? err : new Error(String(err)));
    });
  });
}
