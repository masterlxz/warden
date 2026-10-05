/** P122 — the kinds of action an agent can be told to get approved even when it otherwise acts alone. The ids are
 * `warden_core::autonomy::Category`'s, as the config file, the hub and `approvalRequired` write them. */
export interface ApprovalCategory {
  id: string;
  label: string;
  hint: string;
}

export const APPROVAL_CATEGORIES: ApprovalCategory[] = [
  { id: "delete_data", label: "Delete data", hint: "Removing an agent or a scheduled task, or a tool that says it destroys." },
  { id: "spend_money", label: "Spend money", hint: "A paid service or a purchase. Only tools you map to it in config.toml." },
  { id: "critical_infra", label: "Change critical infrastructure", hint: "Shell commands, SSH and node commands, files copied onto a server." },
  { id: "external_message", label: "Send an external message", hint: "Clicking or navigating in your browser, or a tool that reaches the outside world." },
  { id: "publish_code", label: "Publish code", hint: "A push, a release or a deploy. Only tools you map to it in config.toml." },
  { id: "important_config", label: "Change important configuration", hint: "Creating or editing scheduled tasks." },
  { id: "elevated_agent", label: "Create or change an agent", hint: "Creating or editing agents through manage_agents." },
];

/** The label of a category id, or the id itself when a newer hub sends one this app doesn't know. */
export function approvalCategoryLabel(id: string): string {
  return APPROVAL_CATEGORIES.find((c) => c.id === id)?.label ?? id;
}
