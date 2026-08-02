/** Only the roles ever shown to the human user; warden-core's Role also has
 * System and Tool, but those are internal orchestration detail and never
 * render in the chat UI. */
export type ChatRole = "user" | "assistant";

export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  createdAt: number;
}

export interface Conversation {
  id: string;
  title: string;
  messages: ChatMessage[];
  createdAt: number;
  updatedAt: number;
}

export type ModelProvider = "gemini" | "openai";

/** What `get_settings` returns, and also what the settings form holds — the shapes are
 * identical so the fetched snapshot can be used directly as initial form state. Empty string
 * means "not set" for every field, including the API keys (shown in the UI, masked with a
 * reveal toggle, and directly editable). */
export interface Settings {
  provider: ModelProvider;
  model: string;
  vaultPath: string;
  defaultModelGemini: string;
  defaultModelOpenai: string;
  geminiKey: string;
  openaiKey: string;
  tavilyKey: string;
}
