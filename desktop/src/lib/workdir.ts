// A conversation's working folder on a hub (P102) is one string: a path of the hub's own machine ("/srv/work"), or a
// folder on a node, written `node:<device id>:<path>` with the path relative to the folder the node lends ("" is that
// folder itself). Mirrors `warden_bootstrap::node_folder` and the web's `hub/workdir.ts`. Pure: tests/workdir.test.mjs.

/** What the picker needs of a node (the hub's `NodeInfoDto`, only these fields). */
export interface NodeInfo {
  deviceId: string;
  name: string;
  online: boolean;
  /** Approved in the device list — needed before any agent can use it. */
  approved: boolean;
  enabled: boolean;
  offer?: { files: boolean };
}

/** The folders in a path of the hub's machine or of a node (`ListDirs`/`DirList`). */
export interface DirListing {
  /** Empty for a member's list of the folders the owner allowed. */
  path: string;
  /** Absent at the top of what the person may see. */
  parent?: string;
  dirs: { name: string; path: string }[];
}

const NODE_PREFIX = "node:";

/** `{ node, path }` of a folder on a node, or `null` for a folder of the hub's own machine. */
export function parseNodeFolder(workdir: string): { node: string; path: string } | null {
  if (!workdir.startsWith(NODE_PREFIX)) return null;
  const rest = workdir.slice(NODE_PREFIX.length);
  const colon = rest.indexOf(":");
  if (colon <= 0) return null;
  return { node: rest.slice(0, colon), path: rest.slice(colon + 1) };
}

export function nodeFolderRef(node: string, path: string): string {
  return `${NODE_PREFIX}${node}:${path}`;
}

const lastName = (path: string): string => path.split("/").filter(Boolean).pop() ?? path;

/** What the chip shows: the folder's name, and which machine when it isn't the hub. */
export function folderLabel(workdir: string, nodes: NodeInfo[] = []): string {
  const onNode = parseNodeFolder(workdir);
  if (!onNode) return lastName(workdir);
  const machine = nodes.find((n) => n.deviceId === onNode.node)?.name ?? onNode.node;
  return `${onNode.path ? lastName(onNode.path) : "shared folder"} · ${machine}`;
}

/** The full place, for a tooltip or the browser's header. */
export function folderPlace(workdir: string, nodes: NodeInfo[] = []): string {
  const onNode = parseNodeFolder(workdir);
  if (!onNode) return workdir;
  const machine = nodes.find((n) => n.deviceId === onNode.node)?.name ?? onNode.node;
  return `${machine}: /${onNode.path}`;
}

/** The nodes a folder can be picked on: online, approved, switched on, and sharing a folder. */
export function nodesWithFolders(nodes: NodeInfo[]): NodeInfo[] {
  return nodes.filter((n) => n.online && n.approved && n.enabled && n.offer?.files);
}
