/**
 * P69 item 2 — the two places where Chrome and Firefox actually diverge at runtime. Everything
 * else (`tabs`/`tabGroups`/`scripting`/`storage`/`runtime`) is called through `chrome.*` on both:
 * Firefox exposes the same namespace, promise-based under MV3. Feature checks, not user-agent
 * sniffing — each one tests for the exact API it's about to call.
 */

/** Makes the toolbar icon open the chat panel. Chrome: the docked side panel (`side_panel` in the
 * manifest), wired once via `setPanelBehavior`. Firefox: no `sidePanel` API — the panel is a
 * `sidebar_action`, toggled from the icon's click (a user-input handler, which `sidebarAction`
 * requires). */
export function setUpPanelOpening(): void {
  if (chrome.sidePanel) {
    chrome.sidePanel
      .setPanelBehavior({ openPanelOnActionClick: true })
      .catch((error) => console.error("Falha ao configurar o painel lateral:", error));
    return;
  }
  const sidebarAction = chrome.sidebarAction;
  if (sidebarAction) {
    chrome.action.onClicked.addListener(() => {
      sidebarAction.toggle().catch((error) => console.error("Falha ao abrir a barra lateral:", error));
    });
  }
}

/** P121 — opens the chat panel from a click on a notification. Both browsers only open it from a user
 * gesture, and whether a notification's click counts as one is theirs to say: a refusal just leaves
 * the panel closed, already set on the channel for when the person opens it. Called with no `await`
 * before the open: a gesture does not outlive one. */
export function openPanel(): void {
  // Not counted as a gesture here: the icon still opens it.
  const refused = () => undefined;
  if (chrome.sidePanel) {
    chrome.sidePanel.open({ windowId: chrome.windows.WINDOW_ID_CURRENT }).catch(refused);
    return;
  }
  chrome.sidebarAction?.open().catch(refused);
}

/** LAN hub discovery (`discovery.ts`) needs the machine's own IPv4 addresses, which only Chrome's
 * `system.network` exposes to an extension — Firefox has no equivalent, so there the host is
 * typed by hand. Safe to call from both the background and the panel. */
export function supportsHubDiscovery(): boolean {
  return typeof chrome.system?.network?.getNetworkInterfaces === "function";
}
