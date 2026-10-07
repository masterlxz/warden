import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentEntry } from "../types";
import { approvalCategoryLabel } from "../lib/approvalCategories";
import { hubEditAgentOrg } from "../lib/hub";
import { addReportEdit, buildOrg, positionEdit, superiorChoices, type OrgEdit, type OrgNode } from "../lib/org";
import { KeyCancelled, usePairingKey } from "./PairingKeyDialog";

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

/** What is open on the tree: the position of an agent, a new report under an agent (`null`: at the top), or the removal of one. */
type Panel = { kind: "edit"; id: string } | { kind: "add"; under: string | null } | { kind: "remove"; id: string };

interface Editing {
  agents: AgentEntry[];
  panel: Panel | null;
  busy: boolean;
  open: (panel: Panel | null) => void;
  apply: (edit: OrgEdit) => Promise<void>;
}

function PositionForm({ agent, editing }: { agent: AgentEntry; editing: Editing }) {
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
          It begins careful: read-only tools, asks before every change, and can't delegate or manage agents until you turn that on in Settings.
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

function RemoveConfirm({ node, editing }: { node: OrgNode<AgentEntry>; editing: Editing }) {
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

function Node({ node, editing }: { node: OrgNode<AgentEntry>; editing: Editing }) {
  const { agent, children } = node;
  const { panel } = editing;
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
        <span className="org-actions">
          <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open({ kind: "edit", id: agent.id })}>
            Edit
          </button>
          <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open({ kind: "add", under: agent.id })}>
            Add report
          </button>
          <button type="button" className="settings-browse-btn" disabled={editing.busy} onClick={() => editing.open({ kind: "remove", id: agent.id })}>
            Remove
          </button>
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
function OrganizationView({ agents, remote = false, onChanged, onEdit }: { agents: AgentEntry[]; remote?: boolean; onChanged: () => void; onEdit: () => void }) {
  const tree = buildOrg(agents);
  const nobodyReports = tree.every((node) => node.children.length === 0);
  const [panel, setPanel] = useState<Panel | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { askKey, dialog } = usePairingKey();

  const editing: Editing = {
    agents,
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
        if (remote) await hubEditAgentOrg(askKey, edit);
        else await invoke("edit_agent_org", { edit });
        setPanel(null);
        onChanged();
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
        Who reports to whom among your agents. An agent that manages or delegates to other agents reaches only the ones below it; an agent
        outside the hierarchy delegates as before. Every change an agent makes still waits for your yes.
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
      {tree.length > 0 && nobodyReports && <p className="settings-hint">Nobody reports to anybody yet: use "Edit" on an agent to pick its superior.</p>}
      {panel?.kind === "add" && panel.under === null && <AddForm key="add-top" under={null} editing={editing} />}
      <div className="org-form-actions">
        <button type="button" className="settings-browse-btn" disabled={busy} onClick={() => editing.open({ kind: "add", under: null })}>
          Add an agent at the top
        </button>
        <button type="button" className="settings-browse-btn" onClick={onEdit}>
          More settings per agent
        </button>
      </div>
      {dialog}
    </div>
  );
}

export default OrganizationView;
