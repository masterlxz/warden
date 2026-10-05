import type { Conversation, ProjectEntry } from "../types";
import { ChartIcon, ChevronIcon, ClockIcon, DevicesIcon, LogoMark, OrgIcon, PlusIcon, ProjectsIcon, SettingsIcon, SkillsIcon, SyncIcon, VaultIcon, WebhookIcon } from "./Icons";

interface SidebarProps {
  conversations: Conversation[];
  /** The projects that exist (P103): the list groups the conversations that were started in one. */
  projects: ProjectEntry[];
  activeConversationId: string | null;
  onSelectConversation: (id: string) => void;
  onNewConversation: () => void;
  onOpenSettings: () => void;
  onOpenUsage: () => void;
  onOpenSync: () => void;
  onOpenVault: () => void;
  onOpenSkills: () => void;
  onOpenProjects: () => void;
  onOpenTasks: () => void;
  onOpenWebhooks: () => void;
  onOpenWorkspace: () => void;
  onOpenOrganization: () => void;
  onOpenAgentWork: () => void;
  view: "chat" | "settings" | "usage" | "sync" | "vault" | "skills" | "projects" | "tasks" | "webhooks" | "workspace" | "organization" | "agentWork";
  collapsed: boolean;
  onToggleCollapsed: () => void;
  /** Which machine the screens use (P102): this computer or a hub. Shown under the brand, not in the collapsed bar. */
  machine?: React.ReactNode;
}

function Sidebar({
  conversations,
  projects,
  activeConversationId,
  onSelectConversation,
  onNewConversation,
  onOpenSettings,
  onOpenUsage,
  onOpenSync,
  onOpenVault,
  onOpenSkills,
  onOpenProjects,
  onOpenTasks,
  onOpenWebhooks,
  onOpenWorkspace,
  onOpenOrganization,
  onOpenAgentWork,
  view,
  collapsed,
  onToggleCollapsed,
  machine,
}: SidebarProps) {
  // A conversation whose project no longer exists (removed since) is listed with the others, as an ordinary one.
  const projectIds = new Set(projects.map((p) => p.id));
  const loose = conversations.filter((c) => !c.projectId || !projectIds.has(c.projectId));
  const groups = projects
    .map((project) => ({ project, items: conversations.filter((c) => c.projectId === project.id) }))
    .filter((group) => group.items.length > 0);

  const renderItem = (conversation: Conversation) => (
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
  );

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
        {!collapsed && machine}
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
            <>
              {groups.map(({ project, items }) => (
                <div className="conversation-group" key={project.id}>
                  <div className="conversation-group-title" title={project.description || project.name}>
                    <ProjectsIcon size={13} />
                    {project.name}
                  </div>
                  <ul className="conversation-list">{items.map(renderItem)}</ul>
                </div>
              ))}
              {loose.length > 0 && <ul className="conversation-list">{loose.map(renderItem)}</ul>}
            </>
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
          className={`sidebar-footer-btn${view === "sync" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenSync}
          title="Sync"
        >
          <SyncIcon size={17} />
          {!collapsed && "Sync"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "vault" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenVault}
          title="Vault"
        >
          <VaultIcon size={17} />
          {!collapsed && "Vault"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "skills" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenSkills}
          title="Skills"
        >
          <SkillsIcon size={17} />
          {!collapsed && "Skills"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "projects" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenProjects}
          title="Projects"
        >
          <ProjectsIcon size={17} />
          {!collapsed && "Projects"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "organization" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenOrganization}
          title="Organization"
        >
          <OrgIcon size={17} />
          {!collapsed && "Organization"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "agentWork" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenAgentWork}
          title="Agent work"
        >
          <ChartIcon size={17} />
          {!collapsed && "Agent work"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "tasks" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenTasks}
          title="Tasks"
        >
          <ClockIcon size={17} />
          {!collapsed && "Tasks"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "webhooks" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenWebhooks}
          title="Webhooks"
        >
          <WebhookIcon size={17} />
          {!collapsed && "Webhooks"}
        </button>
        <button
          type="button"
          className={`sidebar-footer-btn${view === "workspace" ? " sidebar-footer-btn--active" : ""}`}
          onClick={onOpenWorkspace}
          title="Workspace"
        >
          <DevicesIcon size={17} />
          {!collapsed && "Workspace"}
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
