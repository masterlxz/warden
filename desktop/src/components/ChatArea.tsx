import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { open } from "@tauri-apps/plugin-dialog";
import { nextCodeMode } from "../types";
import type { AgentEntry, Attachment, CodeMode, Combo, Conversation, ProjectEntry, ProviderEntry } from "../types";
import type { LiveTurn } from "../lib/liveTurn";
import { LogoMark } from "./Icons";
import MessageBubble, { MarkdownLink } from "./MessageBubble";
import MessageInput from "./MessageInput";

interface ChatAreaProps {
  activeConversation: Conversation | undefined;
  onSendMessage: (content: string, attachments: Attachment[]) => void;
  isSending: boolean;
  /** What the code engine is doing in the turn being sent (P103 b); none until it says something. */
  live?: LiveTurn;
  /** Stops that task. */
  onCancel: () => void;
  /** How much a code conversation asks before the engine acts (P103 b); the picker shows only in a code project. */
  codeMode: CodeMode;
  onCodeMode: (mode: CodeMode) => void;
  sendError: string | null;
  agents: AgentEntry[];
  providers: ProviderEntry[];
  /** Combos (P90) are picked here like providers. */
  combos: Combo[];
  /** Projects (P103) a new conversation can start in. */
  projects: ProjectEntry[];
  /** The conversation's project, or the one a new conversation will start in; "" = none. */
  selectedProjectId: string;
  /** Chooses where a conversation that hasn't started will begin. */
  onSelectProject: (projectId: string) => void;
  /** Moves a conversation that has started into a project ("" = out of any). */
  onMoveProject: (projectId: string) => void;
  /** The folder of this computer the conversation works in (P102), or the one a new conversation will start in; "" = none. */
  selectedWorkdir: string;
  /** Chooses the folder of a conversation that hasn't started ("" = none). After its first message it is fixed. */
  onSelectWorkdir: (folder: string) => void;
  selectedAgentId: string;
  selectedProviderId: string;
  onSelectAgent: (agentId: string) => void;
  onSelectProvider: (providerId: string) => void;
  onOpenSettings: () => void;
}

function personaPreview(persona: string): string {
  const collapsed = persona.trim().replace(/\s+/g, " ");
  return collapsed.length > 80 ? `${collapsed.slice(0, 80)}…` : collapsed;
}

function ThinkingIndicator() {
  return (
    <div className="message-row message-row--assistant">
      <div className="message-avatar">
        <LogoMark size={18} />
      </div>
      <div className="thinking-indicator" aria-label="Warden is thinking">
        <span />
        <span />
        <span />
      </div>
    </div>
  );
}

const TOOL_MARK = { running: "…", completed: "✓", failed: "✗" } as const;

/** The task a code engine is running: its words and its tools in the order they happened, and the way to stop it. */
function LiveBubble({ live, onCancel }: { live: LiveTurn; onCancel: () => void }) {
  return (
    <div className="message-row message-row--assistant">
      <div className="message-avatar">
        <LogoMark size={18} />
      </div>
      <div className="message-assistant-body bubble--live" aria-label="In progress" aria-live="polite">
        {live.items.map((item, i) =>
          item.kind === "text" ? (
            <div key={i} className="message-bubble-content">
              <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
                {item.text}
              </ReactMarkdown>
            </div>
          ) : (
            <p key={i} className={`live-tool live-tool--${item.status}`}>
              <span className="live-tool-mark" aria-hidden="true">
                {TOOL_MARK[item.status]}
              </span>
              <span className="live-tool-name">{item.tool}</span>
              <span className="live-tool-title">{item.title}</span>
            </p>
          ),
        )}
        {live.notice && <p className="live-notice">{live.notice}</p>}
        <button type="button" className="live-stop" onClick={onCancel}>
          Stop
        </button>
      </div>
    </div>
  );
}

function ChatArea({
  activeConversation,
  onSendMessage,
  isSending,
  live,
  onCancel,
  codeMode,
  onCodeMode,
  sendError,
  agents,
  providers,
  combos,
  projects,
  selectedProjectId,
  onSelectProject,
  onMoveProject,
  selectedWorkdir,
  onSelectWorkdir,
  selectedAgentId,
  selectedProviderId,
  onSelectAgent,
  onSelectProvider,
  onOpenSettings,
}: ChatAreaProps) {
  const bottomRef = useRef<HTMLDivElement>(null);
  // A project picked for a conversation that has started waits for a yes: moving it changes what the AI can reach.
  const [pendingMove, setPendingMove] = useState<string | null>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [activeConversation?.messages.length, isSending, live]);

  // Another conversation, another question.
  useEffect(() => setPendingMove(null), [activeConversation?.id]);

  const hasMessages = !!activeConversation && activeConversation.messages.length > 0;
  // A conversation's agent is chosen once, before its first message, then locked for good (P45)
  // — this is the "not chosen yet" gate: no messages persisted yet, and no agent picked yet
  // either (picking one doesn't send a message by itself, see onSelectAgent below).
  const needsAgentPick = !hasMessages && !selectedAgentId;
  // Like the agent, a conversation's project is chosen before its first message and then fixed (P103). One whose
  // project was removed since has nothing to show.
  const knownProject = projects.some((p) => p.id === selectedProjectId);
  // A conversation that works in a folder can't be moved into a project (P102), so it has no project picker.
  const showProjectPicker = (projects.length > 0 || knownProject) && !(hasMessages && selectedWorkdir);
  // The folder (P102): picked before the first message with the system's own dialog (this is the person's computer),
  // then only shown. Not inside a project, which has its own.
  const folderName = selectedWorkdir.split("/").filter(Boolean).pop() ?? selectedWorkdir;
  const showFolder = !knownProject && (hasMessages ? selectedWorkdir !== "" : true);

  async function chooseFolder() {
    try {
      const picked = await open({ directory: true, multiple: false, defaultPath: selectedWorkdir || undefined });
      if (typeof picked === "string") onSelectWorkdir(picked);
    } catch (err) {
      console.error("failed to pick a folder:", err);
    }
  }

  return (
    <div className="chat-area">
      <div className="chat-header">
        {needsAgentPick ? (
          <span className="chat-header-label chat-header-label--muted">Pick an agent to start</span>
        ) : (
          <span className="chat-header-label" title="The agent driving this conversation — locked once chosen">
            {selectedAgentId || "No agent"}
          </span>
        )}
        {showProjectPicker && (
          <select
            className="chat-header-select"
            aria-label="Project"
            title="The AI works only on this project's files, with its instructions. Picking another one on a conversation that has started moves it."
            value={knownProject ? selectedProjectId : ""}
            disabled={isSending}
            onChange={(e) => {
              const next = e.currentTarget.value;
              if (!hasMessages) onSelectProject(next);
              else if (next !== (knownProject ? selectedProjectId : "")) setPendingMove(next);
            }}
          >
            <option value="">No project</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        )}
        {showFolder &&
          (hasMessages ? (
            <span className="chat-header-label" title={selectedWorkdir}>
              Folder: {folderName}
            </span>
          ) : (
            <span className="chat-header-folder">
              <button
                type="button"
                className="chat-header-select"
                title={selectedWorkdir || "Pick a folder of this computer for the AI to work in: it reads and writes there and its shell starts there (every command asks first)"}
                disabled={isSending}
                onClick={() => void chooseFolder()}
              >
                {selectedWorkdir ? `Folder: ${folderName}` : "No folder"}
              </button>
              {selectedWorkdir && (
                <button type="button" className="chat-header-select" aria-label="Clear folder" onClick={() => onSelectWorkdir("")}>
                  ×
                </button>
              )}
            </span>
          ))}
        {projects.some((p) => p.id === selectedProjectId && p.code) && (
          // Changeable at any moment, a task that is running included (P103 b).
          <select
            className={`chat-header-select${codeMode === "acceptAll" ? " chat-header-select--warn" : ""}`}
            aria-label="Mode"
            title="How much the AI asks before acting. Takes effect at once, even in the middle of a task; reopening the conversation goes back to Manual."
            value={codeMode}
            onChange={(e) => onCodeMode(e.currentTarget.value as CodeMode)}
          >
            <option value="manual">Manual (asks everything)</option>
            <option value="acceptEdits">Accept edits</option>
            <option value="acceptAll">Accept all (asks nothing)</option>
            <option value="plan">Plan (changes nothing)</option>
          </select>
        )}
        <select
          className="chat-header-select"
          aria-label="Model"
          value={selectedProviderId}
          onChange={(e) => onSelectProvider(e.currentTarget.value)}
        >
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.id}
            </option>
          ))}
          {combos.map((c) => (
            <option key={c.id} value={c.id} title={c.providers.join(" → ")}>
              {c.id} (combo)
            </option>
          ))}
        </select>
      </div>
      {pendingMove !== null && (
        <div className="chat-move-banner" role="alert">
          <span>
            {pendingMove === ""
              ? "Take this conversation out of its project? What was said stays in it and becomes part of the context outside the project; the next messages see your whole vault again."
              : `Move this conversation to "${projects.find((p) => p.id === pendingMove)?.name ?? pendingMove}"? The next messages only work with that project's files and instructions; what was said stays in the conversation.`}
          </span>
          <button
            type="button"
            className="settings-save-btn"
            onClick={() => {
              onMoveProject(pendingMove);
              setPendingMove(null);
            }}
          >
            Move
          </button>
          <button type="button" className="settings-browse-btn" onClick={() => setPendingMove(null)}>
            Cancel
          </button>
        </div>
      )}
      <div className="chat-messages" role="log" aria-live="polite" aria-label="Conversation messages">
        {needsAgentPick ? (
          <div className="agent-picker">
            {agents.length === 0 ? (
              <>
                <LogoMark size={40} />
                <h1>No agents yet</h1>
                <p>Create one in Settings to start a conversation.</p>
                <button type="button" className="agent-picker-settings-btn" onClick={onOpenSettings}>
                  Open Settings
                </button>
              </>
            ) : (
              <>
                <h1>Which agent should drive this conversation?</h1>
                <div className="agent-picker-list">
                  {agents.map((a) => (
                    <button
                      key={a.id}
                      type="button"
                      className="agent-picker-card"
                      onClick={() => onSelectAgent(a.id)}
                    >
                      <span className="agent-picker-card-name">{a.id}</span>
                      {a.persona && <span className="agent-picker-card-persona">{personaPreview(a.persona)}</span>}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        ) : !hasMessages ? (
          <div className="chat-empty-state">
            <LogoMark size={40} />
            <h1>How can I help you today?</h1>
          </div>
        ) : (
          <div className="chat-messages-column">
            {activeConversation!.messages.map((message) => (
              <MessageBubble key={message.id} message={message} />
            ))}
            {isSending && live && <LiveBubble live={live} onCancel={onCancel} />}
            {isSending && !live && <ThinkingIndicator />}
            <div ref={bottomRef} />
          </div>
        )}
      </div>
      {sendError && (
        <div className="chat-error-banner" role="alert">
          {sendError}
        </div>
      )}
      <MessageInput
        onSend={onSendMessage}
        focusKey={activeConversation?.id ?? null}
        disabled={isSending || needsAgentPick}
        onCycleMode={projects.some((p) => p.id === selectedProjectId && p.code) ? () => onCodeMode(nextCodeMode(codeMode)) : undefined}
      />
    </div>
  );
}

export default ChatArea;
