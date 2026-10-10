import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentEntry, AgentTask } from "../types";
import { activityLine, activityOf } from "../lib/agentTasks";
import { approvalCategoryLabel } from "../lib/approvalCategories";
import { hubAgentTasks, hubEditAgentOrg, hubEditAgentOrgAsMember, hubListAgentOrg } from "../lib/hub";
import { addReportEdit, buildOrg, moveEdit, positionEdit, superiorChoices, type OrgAccess, type OrgAgent, type OrgEdit, type OrgNode } from "../lib/org";
import { KeyCancelled, usePairingKey } from "./PairingKeyDialog";

const AUTONOMY: Record<number, string> = { 1: "answers only", 2: "suggests", 3: "asks first", 4: "acts alone", 5: "manages alone" };

/** The agent has the powers of the Settings screen (the owner gets them; a member gets only the id, the role and the superior). */
function hasPowers(agent: OrgAgent): agent is AgentEntry {
  return "autonomy" in agent;
}

/** The small facts shown next to an agent: what it is allowed to do, as the Settings screen sets it. */
function badges(agent: OrgAgent): string[] {
  const list: string[] = [];
  if (!hasPowers(agent)) return list;
  if (agent.canDelegateToAgents) list.push("delegates");
  if (agent.canManageAgents) list.push("manages agents");
  if (agent.canMessageAgents) list.push("leaves notes");
  if (agent.canManageTasks) list.push("schedules tasks");
  if (agent.autonomy !== 4) list.push(`autonomy ${agent.autonomy}: ${AUTONOMY[agent.autonomy] ?? ""}`.trim());
  if (agent.approvalRequired.length > 0) list.push(`asks before: ${agent.approvalRequired.map(approvalCategoryLabel).join(", ").toLowerCase()}`);
  return list;
}

/** What is open on the tree: the position of an agent, a new report under an agent (`null`: at the top), or the removal of one. */
type Panel = { kind: "edit"; id: string } | { kind: "add"; under: string | null } | { kind: "remove"; id: string };

interface Editing {
  /** Opens a new conversation with this agent, or the tasks it was given and delegated. */
  onOpenChat: (id: string) => void;
  onOpenTasks: (id: string) => void;
  agents: OrgAgent[];
  /** A member (P120): no chat or tasks per node (the owner's agents are not always theirs) and no powers to show. */
  member: boolean;
  /** Who looks at the tree may change it: the owner always, a member only with the `edit` access. */
  editable: boolean;
  panel: Panel | null;
  busy: boolean;
  open: (panel: Panel | null) => void;
  apply: (edit: OrgEdit) => Promise<void>;
  /** The agent whose card is being dragged (P120): another card, or the top, takes the drop when the change is valid. */
  dragging: string | null;
  drag: (id: string | null) => void;
  /** What the agent has been up to, on one line, taken from the hub's tasks; `null` with no task of its own. */
  activity: (id: string) => string | null;
}

/** Dropping the dragged card on `target` (`null`: the top): the change, if there is one. A card dropped under one of its own reports would close a circle. */
function onDropOn(editing: Editing, target: string | null): void {
  const edit = editing.dragging === null ? null : moveEdit(editing.agents, editing.dragging, target);
  editing.drag(null);
  if (edit) void editing.apply(edit);
}

function PositionForm({ agent, editing }: { agent: OrgAgent; editing: Editing }) {
  const [role, setRole] = useState(agent.role ?? "");
  const [superior, setSuperior] = useState(agent.reportsTo ?? "");
  const choices = superiorChoices(editing.agents, agent.id);
  return (
    <div className="org-form">
      <label className="settings-field">
        Role
        <input type="text" value={role} maxLength={80} placeholder="e.g. Backend lead" onChange={(e) => setRole(e.target.value)} />
      </label>
      <label className="settings-field">
        Reports to
        <select value={superior} onChange={(e) => setSuperior(e.target.value)}>
          <option value="">Nobody (top of the tree)</option>
          {choices.map((a) => (
            <option key={a.id} value={a.id}>
              {a.id}
            </option>
          ))}
        </select>
        <span className="field-hint">Whoever reports to {agent.id} moves with it.</span>
      </label>
      <div className="org-form-actions">
        <button type="button" className="settings-save-btn" disabled={editing.busy} onClick={() => void editing.apply(positionEdit(agent.id, role, superior))}>
          Save
        </button>
        <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open(null)}>
          Cancel
        </button>
      </div>
    </div>
  );
}

function AddForm({ under, editing }: { under: string | null; editing: Editing }) {
  const [id, setId] = useState("");
  const [role, setRole] = useState("");
  const [persona, setPersona] = useState("");
  const ready = id.trim() !== "" && persona.trim() !== "";
  return (
    <div className="org-form">
      <label className="settings-field">
        Name
        <input type="text" value={id} maxLength={64} placeholder="e.g. reviewer" onChange={(e) => setId(e.target.value)} />
      </label>
      <label className="settings-field">
        Role
        <input type="text" value={role} maxLength={80} placeholder="optional" onChange={(e) => setRole(e.target.value)} />
      </label>
      <label className="settings-field">
        What it does
        <textarea value={persona} rows={3} placeholder="Its instructions, in a few lines." onChange={(e) => setPersona(e.target.value)} />
        <span className="field-hint">
          {under ? `It reports to ${under}. ` : "It starts at the top of the tree. "}
          {editing.member
            ? "It begins careful: read-only tools, asks before every change, and can't delegate or manage agents until the owner turns that on."
            : "It begins careful: read-only tools, asks before every change, and can't delegate or manage agents until you turn that on in Settings."}
        </span>
      </label>
      <div className="org-form-actions">
        <button type="button" className="settings-save-btn" disabled={editing.busy || !ready} onClick={() => void editing.apply(addReportEdit(id, persona, role, under))}>
          Add agent
        </button>
        <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open(null)}>
          Cancel
        </button>
      </div>
    </div>
  );
}

function RemoveConfirm({ node, editing }: { node: OrgNode<OrgAgent>; editing: Editing }) {
  const { agent, children } = node;
  const superior = agent.reportsTo;
  return (
    <div className="org-form">
      <p className="settings-hint">
        Remove <strong>{agent.id}</strong>?
        {children.length > 0 && ` ${children.length === 1 ? "Its report" : `Its ${children.length} reports`} will report to ${superior ?? "nobody (the top of the tree)"}.`}
        {" "}Its conversations stay; it can't be undone.
      </p>
      <div className="org-form-actions">
        <button type="button" className="provider-delete-btn" disabled={editing.busy} onClick={() => void editing.apply({ kind: "remove", id: agent.id })}>
          Remove
        </button>
        <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open(null)}>
          Keep it
        </button>
      </div>
    </div>
  );
}

function Node({ node, editing }: { node: OrgNode<OrgAgent>; editing: Editing }) {
  const { agent, children } = node;
  const { panel } = editing;
  const validTarget = editing.dragging !== null && moveEdit(editing.agents, editing.dragging, agent.id) !== null;
  return (
    <li className="org-node">
      {/* The drag sits on the card, not on the <li>: the `dragover` of the reports must not bubble up to their superior's card. */}
      <div
        className={`org-card${editing.dragging === agent.id ? " org-card--dragging" : ""}${validTarget ? " org-card--target" : ""}`}
        draggable={editing.editable && !editing.busy}
        onDragStart={(e) => {
          e.dataTransfer.setData("text/plain", agent.id);
          e.dataTransfer.effectAllowed = "move";
          editing.drag(agent.id);
        }}
        onDragEnd={() => editing.drag(null)}
        onDragOver={(e) => {
          if (validTarget) e.preventDefault();
        }}
        onDrop={(e) => {
          e.preventDefault();
          onDropOn(editing, agent.id);
        }}
      >
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
        {editing.activity(agent.id) && <span className="org-activity">{editing.activity(agent.id)}</span>}
        <span className="org-actions">
          {!editing.member && (
            <>
              <button type="button" className="settings-browse-btn" onClick={() => editing.onOpenChat(agent.id)} title={`Start a conversation with ${agent.id}`}>
                Chat
              </button>
              <button type="button" className="settings-browse-btn" onClick={() => editing.onOpenTasks(agent.id)} title={`The tasks ${agent.id} was given and the ones it delegated`}>
                Tasks
              </button>
            </>
          )}
          {editing.editable && (
            <>
              <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open({ kind: "edit", id: agent.id })}>
                Edit
              </button>
              <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open({ kind: "add", under: agent.id })}>
                Add report
              </button>
              <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open({ kind: "remove", id: agent.id })}>
                Remove
              </button>
            </>
          )}
        </span>
      </div>
      {panel?.kind === "edit" && panel.id === agent.id && <PositionForm key={`edit-${agent.id}`} agent={agent} editing={editing} />}
      {panel?.kind === "remove" && panel.id === agent.id && <RemoveConfirm node={node} editing={editing} />}
      {panel?.kind === "add" && panel.under === agent.id && <AddForm key={`add-${agent.id}`} under={agent.id} editing={editing} />}
      {children.length > 0 && (
        <ul className="org-children">
          {children.map((child) => (
            <Node key={child.agent.id} node={child} editing={editing} />
          ))}
        </ul>
      )}
    </li>
  );
}

/** P120 — who reports to whom, and the place to change it: give an agent a role, move it under another, add a report, remove one.
 * Each change is written on its own (nothing else of the settings is touched) and an agent that manages or delegates reaches only the
 * ones below it, so the tree is what sets its reach. On a hub it asks for the pairing key, like any change to it. */
function OrganizationView({
  agents: ownerAgents,
  remote = false,
  memberAccess,
  onChanged,
  onEdit,
  onOpenChat,
  onOpenTasks,
}: {
  agents: AgentEntry[];
  remote?: boolean;
  /** A member of the hub (P120): the access the owner gave, `view` or `edit`. Absent for the owner and on this computer. The member reads
   * and edits by their session, with no pairing key, and gets only the id, the role and the superior of each agent. */
  memberAccess?: OrgAccess;
  onChanged: () => void;
  onEdit: () => void;
  onOpenChat: (id: string) => void;
  onOpenTasks: (id: string) => void;
}) {
  const member = memberAccess !== undefined;
  const editable = memberAccess === undefined || memberAccess === "edit";
  const [memberAgents, setMemberAgents] = useState<OrgAgent[]>([]);
  const agents: OrgAgent[] = member ? memberAgents : ownerAgents;
  const tree = buildOrg(agents);
  const nobodyReports = tree.every((node) => node.children.length === 0);
  const [panel, setPanel] = useState<Panel | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const [tasks, setTasks] = useState<AgentTask[]>([]);
  const { askKey, dialog } = usePairingKey();

  // A member's tree is the owner's, read from the hub as the access they were given allows.
  useEffect(() => {
    if (!member) return;
    let alive = true;
    hubListAgentOrg().then(
      (loaded) => alive && (setMemberAgents(loaded.agents), setError(null)),
      (err) => alive && setError(String(err instanceof Error ? err.message : err)),
    );
    return () => {
      alive = false;
    };
  }, [member]);

  // The activity of each node is an extra: without the tasks, the card just shows no line.
  useEffect(() => {
    if (member) return;
    let alive = true;
    (remote ? hubAgentTasks() : invoke<AgentTask[]>("list_agent_tasks")).then(
      (loaded) => alive && setTasks(loaded),
      () => {},
    );
    return () => {
      alive = false;
    };
  }, [remote, member, ownerAgents]);

  const editing: Editing = {
    dragging,
    drag: setDragging,
    activity: (id) => {
      const activity = activityOf(tasks, id);
      return activity ? activityLine(activity, Date.now()) : null;
    },
    onOpenChat,
    onOpenTasks,
    agents,
    member,
    editable,
    panel,
    busy,
    open: (next) => {
      setError(null);
      setPanel(next);
    },
    apply: async (edit) => {
      setBusy(true);
      setError(null);
      try {
        if (member) setMemberAgents((await hubEditAgentOrgAsMember(edit)).agents);
        else if (remote) await hubEditAgentOrg(askKey, edit);
        else await invoke("edit_agent_org", { edit });
        setPanel(null);
        if (!member) onChanged();
      } catch (err) {
        if (!(err instanceof KeyCancelled)) setError(String(err));
      } finally {
        setBusy(false);
      }
    },
  };

  return (
    <div className="settings-view">
      <h2 className="settings-title">Organization</h2>
      <p className="settings-hint">
        {member
          ? editable
            ? "Who reports to whom among the workspace's agents. The owner let you change the hierarchy: the role and the superior of an agent, a new report, a removal. What the agents may do is the owner's."
            : "Who reports to whom among the workspace's agents. You can look at the tree; changing it is the owner's."
          : "Who reports to whom among your agents. An agent that manages or delegates to other agents reaches only the ones below it; an agent outside the hierarchy delegates as before. Every change an agent makes still waits for your yes."}
      </p>
      {error && <p className="usage-error">{error}</p>}

      {tree.length === 0 ? (
        <p className="settings-hint">No agents yet.</p>
      ) : (
        <ul className="org-tree">
          {tree.map((node) => (
            <Node key={node.agent.id} node={node} editing={editing} />
          ))}
        </ul>
      )}
      {editable && dragging !== null && moveEdit(agents, dragging, null) !== null && (
        <div
          className="org-top"
          onDragOver={(e) => e.preventDefault()}
          onDrop={(e) => {
            e.preventDefault();
            onDropOn(editing, null);
          }}
        >
          Drop here to take it out from under its superior (top of the tree)
        </div>
      )}
      {editable && tree.length > 0 && nobodyReports && <p className="settings-hint">Nobody reports to anybody yet: drag a card onto another, or use "Edit" on an agent to pick its superior.</p>}
      {panel?.kind === "add" && panel.under === null && <AddForm key="add-top" under={null} editing={editing} />}
      {editable && (
        <div className="org-form-actions">
          <button type="button" className="settings-browse-btn" disabled={busy} onClick={() => editing.open({ kind: "add", under: null })}>
            Add an agent at the top
          </button>
          {!member && (
            <button type="button" className="settings-browse-btn" onClick={onEdit}>
              More settings per agent
            </button>
          )}
        </div>
      )}
      {dialog}
    </div>
  );
}

export default OrganizationView;
