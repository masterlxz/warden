// A conversation's working folder (P102) is one string: a path of the hub's own machine ("/srv/work"), or a folder on a node,
// written `node:<device id>:<path>` with the path relative to the folder the node lends ("" is that folder itself).
// Mirrors `warden_bootstrap::node_folder`; this panel can't list nodes, so a node is named by its id.

const NODE_PREFIX = "node:";

const lastName = (path: string): string => path.split("/").filter(Boolean).pop() ?? path;

/** What the panel shows for a folder: its name, and which node when it isn't the hub's own machine. */
export function folderLabel(workdir: string): string {
  if (!workdir.startsWith(NODE_PREFIX)) return lastName(workdir);
  const rest = workdir.slice(NODE_PREFIX.length);
  const colon = rest.indexOf(":");
  if (colon <= 0) return workdir;
  const path = rest.slice(colon + 1);
  return `${path ? lastName(path) : "pasta compartilhada"} · ${rest.slice(0, colon)}`;
}
