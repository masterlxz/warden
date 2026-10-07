import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { open } from "@tauri-apps/plugin-dialog";
import { nextCodeMode } from "../types";
import type { AgentEntry, Attachment, CodeMode, Combo, Conversation, ProjectEntry, ProviderEntry } from "../types";
import type { LiveTurn } from "../lib/liveTurn";
import { threadAnchor, type ThreadInfo } from "../lib/threads";
import { folderLabel, folderPlace, nodesWithFolders, type DirListing, type NodeInfo } from "../lib/workdir";
import FolderPicker from "./FolderPicker";
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
  /** The conversation runs on a hub (P102), not on this computer: the hub picks the model from the agent (a turn can't
   * name one), and its folders are browsed on the hub instead of with this computer's dialog. */
  remote?: boolean;
  /** What browsing a hub's folders needs (only with `remote`): the listing, the nodes known (to name them), and a call
   * made before the browser opens that reads the nodes (the owner's; a member has none to read). */
  hubFolders?: { listDirs: (path?: string) => Promise<DirListing>; nodes: NodeInfo[]; prepare: () => Promise<void> };
  /** Opens the thread of a message of this conversation (P125): on a hub the id is the one the hub gave the message, on this computer the
   * message's own. Absent where there are no threads (a code project). */
  onOpenThread?: (messageId: string) => void;
  /** The threads of this conversation by the id of the message they came from: the chip with the replies in place of the button. */
  threads?: Record<string, ThreadInfo>;
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
  remote = false,
  hubFolders,
  onOpenThread,
  threads,
}: ChatAreaProps) {
  // The hub's folder browser (P102), open while the person chooses.
  const [browsingHub, setBrowsingHub] = useState(false);
  const bottomRef = useRef<HTMLDivElement>(null);
  // A project picked for a conversation that has started waits for a yes: moving it changes what the AI can reach.
  const [pendingMove, setPendingMove] = useState<string | null>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [activeConversation?.messages.length, isSending, live]);

  // Another conversation, another question.
  useEffect(() => setPendingMove(null), [activeConversation?.id]);

  // The person chose to talk with no agent (P124): the pick screen is skipped. Per conversation, like the pick itself.
  const [withoutAgent, setWithoutAgent] = useState(false);
  useEffect(() => setWithoutAgent(false), [activeConversation?.id]);

  const hasMessages = !!activeConversation && activeConversation.messages.length > 0;
  // A conversation starts with an agent chosen or with the choice of none (P124), and the agent can be changed later from the header
  // (P45 locked it for good). This is the "not decided yet" gate: no messages yet, no agent picked and no "without an agent" either
  // (picking one doesn't send a message by itself, see onSelectAgent below).
  const needsAgentPick = !hasMessages && !selectedAgentId && !withoutAgent;
  // Like the agent, a conversation's project is chosen before its first message and then fixed (P103). One whose
  // project was removed since has nothing to show.
  const knownProject = projects.some((p) => p.id === selectedProjectId);
  // A conversation that works in a folder can't be moved into a project (P102), so it has no project picker.
  const showProjectPicker = (projects.length > 0 || knownProject) && !(hasMessages && selectedWorkdir);
  // The folder (P102): picked before the first message — with the system's own dialog on this computer, in the hub's
  // folder browser on a hub — then only shown. Not inside a project, which has its own.
  const hubNodes = hubFolders?.nodes ?? [];
  const folderName = remote ? folderLabel(selectedWorkdir, hubNodes) : (selectedWorkdir.split("/").filter(Boolean).pop() ?? selectedWorkdir);
  const folderTitle = remote ? folderPlace(selectedWorkdir, hubNodes) : selectedWorkdir;
  const showFolder = !knownProject && (hasMessages ? selectedWorkdir !== "" : true) && (!remote || hubFolders !== undefined);

  async function chooseFolder() {
    if (remote && hubFolders) {
      // The nodes are read first so the browser can offer them (the owner's; a member's call is a no-op).
      await hubFolders.prepare();
      setBrowsingHub(true);
      return;
    }
    try {
      const picked = await open({ directory: true, multiple: false, defaultPath: selectedWorkdir || undefined });
      if (typeof picked === "string") onSelectWorkdir(picked);
    } catch (err) {
      console.error("failed to pick a folder:", err);
    }
  }

  return (
    <div className="chat-area">
      {browsingHub && hubFolders && (
        <FolderPicker
          listDirs={hubFolders.listDirs}
          nodes={nodesWithFolders(hubNodes)}
          initialPath={selectedWorkdir || undefined}
          onPick={(path) => {
            onSelectWorkdir(path);
            setBrowsingHub(false);
          }}
          onCancel={() => setBrowsingHub(false)}
        />
      )}
      <div className="chat-header">
        {needsAgentPick ? (
          <span className="chat-header-label chat-header-label--muted">Pick an agent to start</span>
        ) : agents.length > 0 || selectedAgentId ? (
          // The agent can be changed at any time (P124): the next messages speak as the new one. Locked while an answer is on the way.
          <select
            className="chat-header-select"
            aria-label="Agent"
            title="The agent driving this conversation — the next messages speak as the one picked"
            value={selectedAgentId}
            disabled={isSending}
            onChange={(e) => {
              const next = e.currentTarget.value;
              if (next === "") setWithoutAgent(true);
              onSelectAgent(next);
            }}
          >
            <option value="">No agent</option>
            {agents.map((a) => (
              <option key={a.id} value={a.id}>
                {a.id}
              </option>
            ))}
            {selectedAgentId && !agents.some((a) => a.id === selectedAgentId) && <option value={selectedAgentId}>{selectedAgentId} (removed)</option>}
          </select>
        ) : (
          <span className="chat-header-label">No agent</span>
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
            <span className="chat-header-label" title={folderTitle}>
              Folder: {folderName}
            </span>
          ) : (
            <span className="chat-header-folder">
              <button
                type="button"
                className="chat-header-select"
                title={
                  folderTitle ||
                  (remote
                    ? "Pick a folder of the hub, or of one of its nodes, for the AI to work in: it reads and writes there and its shell starts there (every command asks first)"
                    : "Pick a folder of this computer for the AI to work in: it reads and writes there and its shell starts there (every command asks first)")
                }
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
        {!remote && (
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
        )}
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
                <p>Create one in Settings, or talk with no agent.</p>
                <button type="button" className="agent-picker-settings-btn" onClick={onOpenSettings}>
                  Open Settings
                </button>
                <button type="button" className="agent-picker-skip" onClick={() => setWithoutAgent(true)}>
                  Chat without an agent
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
                <button type="button" className="agent-picker-skip" onClick={() => setWithoutAgent(true)}>
                  Chat without an agent
                </button>
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
            {activeConversation!.messages.map((message) => {
              const anchor = onOpenThread ? threadAnchor(message, !remote) : null;
              return (
                <MessageBubble
                  key={message.id}
                  message={message}
                  thread={anchor ? { replies: threads?.[anchor]?.replies ?? null, onOpen: () => onOpenThread!(anchor) } : undefined}
                />
              );
            })}
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
