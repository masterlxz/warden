import type { Conversation } from "../types";
import { ChartIcon, ChevronIcon, LogoMark, PlusIcon, SettingsIcon } from "./Icons";

interface SidebarProps {
  conversations: Conversation[];
  activeConversationId: string | null;
  onSelectConversation: (id: string) => void;
  onNewConversation: () => void;
  onOpenSettings: () => void;
  onOpenUsage: () => void;
  view: "chat" | "settings" | "usage";
  collapsed: boolean;
  onToggleCollapsed: () => void;
}

function Sidebar({
  conversations,
  activeConversationId,
  onSelectConversation,
  onNewConversation,
  onOpenSettings,
  onOpenUsage,
  view,
  collapsed,
  onToggleCollapsed,
}: SidebarProps) {
  return (
    <div className={`sidebar${collapsed ? " sidebar--collapsed" : ""}`}>
      <div className="sidebar-header">
        <div className="sidebar-topbar">
          <div className="sidebar-brand">
            <LogoMark size={24} />
            {!collapsed && <span className="sidebar-title">Warden</span>}
          </div>
          <button
            type="button"
            className="sidebar-collapse-btn"
            onClick={onToggleCollapsed}
            aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          >
            <ChevronIcon size={14} />
          </button>
        </div>
        <button type="button" className="new-conversation-btn" onClick={onNewConversation} title="New chat">
          <PlusIcon size={16} />
          {!collapsed && "New chat"}
        </button>
      </div>

      {!collapsed && (
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
      )}

      <div className="sidebar-footer">
        <button
          type="button"
          className={`sidebar-footer-btn${view === "usage" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenUsage}
          title="Usage"
        >
          <ChartIcon size={17} />
          {!collapsed && "Usage"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "settings" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenSettings}
          title="Settings"
        >
          <SettingsIcon size={17} />
          {!collapsed && "Settings"}
        </button>
      </div>
    </div>
  );
}

export default Sidebar;
