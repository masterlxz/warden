/** Only the roles ever shown to the human user; warden-core's Role also has
 * System and Tool, but those are internal orchestration detail and never
 * render in the chat UI. */
export type ChatRole = "user" | "assistant";

export interface Usage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  createdAt: number;
  usage?: Usage;
}

export interface Conversation {
  id: string;
  title: string;
  messages: ChatMessage[];
  createdAt: number;
  updatedAt: number;
}

/** Mirrors `warden_bootstrap::Provider`. `openaiCompatible` covers any other server that speaks
 * the OpenAI chat-completions wire format — Ollama (local, no real key needed), OpenRouter,
 * Groq, DeepSeek, etc. — via a configurable `baseUrl` instead of one dedicated kind per company. */
export type ProviderKind = "gemini" | "openai" | "anthropic" | "openai_compatible";

/** One entry of the provider registry (Sessão 35) — the user can add/edit/delete any number of
 * these in Settings, each independently selectable as the active one. Empty string means
 * "not set" for every field, including `apiKey` (shown in the UI masked, with a reveal toggle,
 * and directly editable — this app's own explicit choice, same as before the registry existed). */
export interface ProviderEntry {
  /** User-chosen, unique among the list — how `activeProvider` refers to one. */
  id: string;
  kind: ProviderKind;
  apiKey: string;
  /** Only meaningful (and required) for `kind === "openai_compatible"`. */
  baseUrl: string;
  /** Falls back to `defaultModels[kind]` when empty — no such default for `openai_compatible`. */
  model: string;
}

/** One external MCP server (Phase 5.2/P25) to connect to on startup — mirrors
 * `warden_bootstrap::McpServerConfig` field for field (no camelCase remapping needed, every
 * field is already a single word). It's an untagged union on the Rust side, discriminated purely
 * by which fields are present (`command` vs `url`) — mirrored here the same way rather than with
 * a synthetic `transport` tag, since that's the actual wire shape. */
export interface McpServerStdio {
  /** Only used for display/error messages — not sent to the server. */
  name: string;
  /** Same shape any MCP client config uses (e.g. Claude Desktop's `mcpServers`). */
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface McpServerHttp {
  /** Only used for display/error messages — not sent to the server. */
  name: string;
  url: string;
  /** Sent on every request — typically just `{ Authorization: "Bearer <token>" }` for a server
   * that authenticates that way (there's no OAuth flow here, just a static header). */
  headers: Record<string, string>;
}

export type McpServer = McpServerStdio | McpServerHttp;

export function isMcpServerHttp(server: McpServer): server is McpServerHttp {
  return "url" in server;
}

/** What `get_settings` returns, and also what the settings form holds — the shapes are
 * identical so the fetched snapshot can be used directly as initial form state. */
export interface Settings {
  providers: ProviderEntry[];
  /** `id` of the `providers` entry currently in use — empty string means none selected. */
  activeProvider: string;
  vaultPath: string;
  tavilyKey: string;
  /** Opt-in gate for the `shell` tool — off by default, since it lets the model run arbitrary
   * commands on this machine with no sandboxing. */
  enableShell: boolean;
  /** Default model per provider kind (`"gemini"`/`"openai"`/`"anthropic"`), shown as the Model
   * field's placeholder — no entry for `openaiCompatible`. */
  defaultModels: Record<string, string>;
  mcpServers: McpServer[];
}
