import type { Conversation } from "../types";
import { LogoMark, PlusIcon, SettingsIcon } from "./Icons";

interface SidebarProps {
  conversations: Conversation[];
  activeConversationId: string | null;
  onSelectConversation: (id: string) => void;
  onNewConversation: () => void;
  onOpenSettings: () => void;
  view: "chat" | "settings";
}

function Sidebar({
  conversations,
  activeConversationId,
  onSelectConversation,
  onNewConversation,
  onOpenSettings,
  view,
}: SidebarProps) {
  return (
    <div className="sidebar">
      <div className="sidebar-header">
        <div className="sidebar-brand">
          <LogoMark size={24} />
          <span className="sidebar-title">Warden</span>
        </div>
        <button type="button" className="new-conversation-btn" onClick={onNewConversation}>
          <PlusIcon size={16} />
          New chat
        </button>
      </div>

      <div className="conversation-list-scroll">
        {conversations.length === 0 ? (
          <p className="conversation-list-empty">No conversations yet.</p>
        ) : (
          <ul className="conversation-list">
            {conversations.map((conversation) => (
              <li key={conversation.id}>
                <button
                  type="button"
                  className={
                    "conversation-list-item" +
                    (view === "chat" && conversation.id === activeConversationId ? " conversation-list-item--active" : "")
                  }
                  onClick={() => onSelectConversation(conversation.id)}
                >
                  {conversation.title}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="sidebar-footer">
        <button type="button" className={`sidebar-footer-btn${view === "settings" ? " sidebar-footer-btn--active" : ""}`} onClick={onOpenSettings}>
          <SettingsIcon size={17} />
          Settings
        </button>
      </div>
    </div>
  );
}

export default Sidebar;
