import type { AgentEntry } from "../types";
import { approvalCategoryLabel } from "../lib/approvalCategories";
import { buildOrg, type OrgNode } from "../lib/org";

const AUTONOMY: Record<number, string> = { 1: "answers only", 2: "suggests", 3: "asks first", 4: "acts alone" };

/** The small facts shown next to an agent: what it is allowed to do, as the Settings screen sets it. */
function badges(agent: AgentEntry): string[] {
  const list: string[] = [];
  if (agent.canDelegateToAgents) list.push("delegates");
  if (agent.canManageAgents) list.push("manages agents");
  if (agent.canMessageAgents) list.push("leaves notes");
  if (agent.canManageTasks) list.push("schedules tasks");
  if (agent.autonomy !== 4) list.push(`autonomy ${agent.autonomy}: ${AUTONOMY[agent.autonomy] ?? ""}`.trim());
  if (agent.approvalRequired.length > 0) list.push(`asks before: ${agent.approvalRequired.map(approvalCategoryLabel).join(", ").toLowerCase()}`);
  return list;
}

function Node({ node }: { node: OrgNode<AgentEntry> }) {
  const { agent, children } = node;
  return (
    <li className="org-node">
      <div className="org-card">
        <span className="org-name">{agent.id}</span>
        {agent.role && <span className="org-role">{agent.role}</span>}
        <span className="org-badges">
          {badges(agent).map((badge) => (
            <span key={badge} className="org-badge">
              {badge}
            </span>
          ))}
        </span>
        {children.length > 0 && <span className="org-count">{children.length === 1 ? "1 report" : `${children.length} reports`}</span>}
      </div>
      {children.length > 0 && (
        <ul className="org-children">
          {children.map((child) => (
            <Node key={child.agent.id} node={child} />
          ))}
        </ul>
      )}
    </li>
  );
}

/** P120 — who reports to whom, read only. The role and the superior are set on each agent in Settings; the tree sets
 * the reach of an agent that manages (`manage_agents`) or delegates (`delegate_to_agent`). */
function OrganizationView({ agents, onEdit }: { agents: AgentEntry[]; onEdit: () => void }) {
  const tree = buildOrg(agents);
  const nobodyReports = tree.every((node) => node.children.length === 0);

  return (
    <div className="settings-view">
      <h2 className="settings-title">Organization</h2>
      <p className="settings-hint">
        Who reports to whom among your agents. An agent that manages or delegates to other agents reaches only the ones below it; an agent
        outside the hierarchy delegates as before. Every change an agent makes still waits for your yes.
      </p>

      {tree.length === 0 ? (
        <p className="settings-hint">No agents yet. Add one in Settings.</p>
      ) : (
        <ul className="org-tree">
          {tree.map((node) => (
            <Node key={node.agent.id} node={node} />
          ))}
        </ul>
      )}
      {tree.length > 0 && nobodyReports && <p className="settings-hint">Nobody reports to anybody yet: pick a superior ("Reports to") on an agent in Settings.</p>}
      <button type="button" className="settings-browse-btn" onClick={onEdit}>
        Edit agents in Settings
      </button>
    </div>
  );
}

export default OrganizationView;
