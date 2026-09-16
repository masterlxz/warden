/**
 * Shared plumbing for the DOM tools (Fase 8.3-8.6) — one place to resolve "the active tab" and
 * to run a function inside its page, so the 4 tools don't each duplicate error handling.
 *
 * Chrome constraint that shapes every tool built on `runInPage`: the function passed to
 * `chrome.scripting.executeScript` is serialized and run in an isolated world in the page — it
 * can NOT close over anything from this module (no imports, no outer variables). Data only
 * crosses the boundary via `args` (in) and the function's `return` value (out), both of which
 * must be JSON-serializable. Every tool file defines its injected function as a fully
 * self-contained closure for this reason, even at the cost of a little duplication between them.
 */

async function getActiveTabId(): Promise<number> {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab?.id) {
    throw new Error("no active tab found");
  }
  return tab.id;
}

/**
 * Runs `func(...args)` inside the active tab's page and returns its result. Translates the two
 * permission failures `activeTab`-only scope can hit into one actionable message — see
 * `manifest.config.ts` for why `host_permissions` isn't used instead.
 */
export async function runInPage<A extends unknown[], R>(func: (...args: A) => R, args: A): Promise<R> {
  const tabId = await getActiveTabId();
  try {
    const [injection] = await chrome.scripting.executeScript({ target: { tabId }, func, args });
    return injection.result as R;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    throw new Error(`sem acesso à aba ativa (${message}) — abra o popup da extensão nessa aba e tente de novo`);
  }
}

export async function navigateActiveTab(url: string, timeoutMs = 10_000): Promise<void> {
  const tabId = await getActiveTabId();
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
