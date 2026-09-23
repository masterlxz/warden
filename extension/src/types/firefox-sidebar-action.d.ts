/**
 * Firefox's `sidebarAction` (P69 item 2) — not in `@types/chrome`, since Chrome has no such API.
 * Firefox exposes it under the `chrome` namespace too, so `platform.ts` reaches it the same way as
 * everything else. Declared optional: it's `undefined` on Chrome, and every caller must check.
 */
declare namespace chrome {
  // eslint-disable-next-line no-var
  var sidebarAction:
    | {
        toggle(): Promise<void>;
      }
    | undefined;
}
