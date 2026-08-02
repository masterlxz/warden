import type { Conversation } from "../types";

interface SidebarProps {
  conversations: Conversation[];
  activeConversationId: string | null;
  onSelectConversation: (id: string) => void;
  onNewConversation: () => void;
}

function Sidebar({ conversations, activeConversationId, onSelectConversation, onNewConversation }: SidebarProps) {
  return (
    <div className="sidebar">
      <div className="sidebar-header">
        <span className="sidebar-title">Warden</span>
        <button type="button" className="new-conversation-btn" onClick={onNewConversation}>
          + New conversation
        </button>
      </div>

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
                  (conversation.id === activeConversationId ? " conversation-list-item--active" : "")
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
  );
}

export default Sidebar;
