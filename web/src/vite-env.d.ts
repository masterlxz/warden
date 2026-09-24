/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Dev only: the hub to talk to when the page isn't served by the hub itself. */
  readonly VITE_HUB_URL?: string;
}
