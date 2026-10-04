//! P102: which folders of the hub's machine a person may pick as a conversation's working folder, and the list of them
//! a client browses to pick one. Two pure functions over a list of allowed roots, so the rule is one thing in one place
//! and testable without a socket: `None` roots is the owner (every folder is theirs, and browsing starts at their home);
//! `Some(roots)` is a member, who only sees and uses what is inside the folders the owner named for them.
//!
//! The client's list is a convenience, not the boundary: a `Chat` names its folder as a plain path, so `check_workdir`
//! runs again on it (and on a conversation's saved folder, every turn, so a root the owner took away stops working).
//! Both resolve symlinks before comparing, so a link inside an allowed folder can't lead out of it, and the listing
//! never shows files, hidden folders or links.

use std::path::{Path, PathBuf};

use warden_bootstrap::users::NodeFolder;
use warden_server_protocol::protocol::DirEntryDto;
use warden_server_protocol::ServerMessage;

use crate::node_tools::NodeToolFactory;

/// The most folders one listing returns: a folder with more is cut, not paged (this is a picker, not a file manager).
const MAX_DIRS: usize = 500;

/// What a `ListDirs` answers with (`ServerMessage::DirList` without the request id).
#[derive(Debug, PartialEq)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub dirs: Vec<DirEntryDto>,
}

/// The allowed roots that exist, resolved.
fn resolved(roots: &[String]) -> Vec<PathBuf> {
    roots.iter().filter_map(|root| Path::new(root).canonicalize().ok()).filter(|root| root.is_dir()).collect()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The folder `path` names, resolved, or why not: it must be absolute, free of `..` and really a folder.
fn existing_dir(path: &str) -> Result<PathBuf, String> {
    warden_core::project::validate_workdir(path).map_err(|e| format!("{e:#}"))?;
    let resolved = Path::new(path).canonicalize().map_err(|_| format!("'{path}' is not a folder on this machine"))?;
    if !resolved.is_dir() {
        return Err(format!("'{path}' is not a folder"));
    }
    Ok(resolved)
}

fn inside_any(roots: &[PathBuf], path: &Path) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

/// Whether the person may work in `folder`: the owner in any folder that exists, a member only inside their roots.
pub fn check_workdir(roots: Option<&[String]>, folder: &str) -> Result<(), String> {
    let path = existing_dir(folder)?;
    match roots {
        Some(roots) if !inside_any(&resolved(roots), &path) => Err(format!("'{folder}' is not a folder you can work in")),
        _ => Ok(()),
    }
}

/// The folders inside `path` (the person's starting place when there is none), for them to pick from.
pub fn list_dirs(roots: Option<&[String]>, path: Option<&str>) -> Result<Listing, String> {
    let path = path.filter(|p| !p.is_empty());
    let allowed = roots.map(resolved);
    let Some(path) = path else {
        return Ok(match &allowed {
            // A member starts at the list of their folders, which is not itself a folder.
            Some(allowed) => Listing {
                path: String::new(),
                parent: None,
                dirs: allowed.iter().map(|root| DirEntryDto { name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| text(root)), path: text(root) }).collect(),
            },
            None => {
                let home = dirs::home_dir().and_then(|h| h.canonicalize().ok()).filter(|h| h.is_dir()).unwrap_or_else(|| PathBuf::from("/"));
                return read(&home, None);
            }
        });
    };
    let dir = existing_dir(path)?;
    let parent = match &allowed {
        Some(allowed) => {
            if !inside_any(allowed, &dir) {
                return Err(format!("'{path}' is not a folder you can work in"));
            }
            // At one of their folders the way up is back to the list of them.
            if allowed.contains(&dir) {
                Some(String::new())
            } else {
                dir.parent().map(text)
            }
        }
        None => dir.parent().map(text),
    };
    read(&dir, parent)
}

/// Whether the person may work in `path` on node `node` (P102 fatia 2): the owner anywhere the node lends, a member only
/// inside the folders of that node the owner named for them. `path` is the one the node *resolved* (`NodeDirs::path`),
/// so a link can't lead a member out of what was named.
pub fn check_node_workdir(entries: Option<&[NodeFolder]>, node: &str, path: &str) -> Result<(), String> {
    match entries {
        Some(entries) if !entries.iter().any(|e| e.covers(node, path)) => Err("that is not a folder on a node you can work in".to_string()),
        _ => Ok(()),
    }
}

/// What a `ListDirs` of a node folder answers with, from what the node resolved and listed: every path becomes a node
/// reference (`node:<id>:<path>`). A member only sees what the owner named for them, and from one of those folders the
/// way up is the list of them (`parent` empty), as for the folders of the hub's own machine.
pub fn node_listing(entries: Option<&[NodeFolder]>, node: &str, path: &str, dirs: Vec<(String, String)>) -> Result<Listing, String> {
    check_node_workdir(entries, node, path)?;
    let at_a_root = entries.is_some_and(|entries| entries.iter().any(|e| e.node == node && Path::new(&e.path) == Path::new(path)));
    let parent = if at_a_root {
        Some(String::new())
    } else if path.is_empty() {
        None
    } else {
        Some(warden_bootstrap::node_folder_ref(node, &Path::new(path).parent().map(text).unwrap_or_default()))
    };
    let dirs = dirs.into_iter().map(|(name, path)| DirEntryDto { name, path: warden_bootstrap::node_folder_ref(node, &path) }).collect();
    Ok(Listing { path: warden_bootstrap::node_folder_ref(node, path), parent, dirs })
}

/// The folders on nodes a member starts from, next to their folders of the hub's machine. `name_of` gives a node's name
/// when it is connected.
pub fn member_node_start(entries: &[NodeFolder], name_of: &dyn Fn(&str) -> Option<String>) -> Vec<DirEntryDto> {
    entries
        .iter()
        .map(|e| {
            let place = Path::new(&e.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "shared folder".to_string());
            DirEntryDto { name: format!("{} · {place}", name_of(&e.node).unwrap_or_else(|| e.node.clone())), path: warden_bootstrap::node_folder_ref(&e.node, &e.path) }
        })
        .collect()
}

/// Answers a `ListDirs` (P102): the folders of the hub's machine, or — when `path` is a node reference — of a node, asked
/// of the node itself. `roots` and `node_roots` are `None` for the owner and what the owner named for a member. A member's
/// start (no path) is their folders of both kinds.
pub async fn answer_list_dirs(request_id: u64, path: Option<&str>, roots: Option<&[String]>, node_roots: Option<&[NodeFolder]>, nodes: Option<&NodeToolFactory>) -> ServerMessage {
    let listing = match path.and_then(warden_bootstrap::node_folder) {
        Some((node, on_node)) => match nodes {
            None => Err("this hub has no nodes".to_string()),
            Some(nodes) => match nodes.list_dirs(node, on_node).await {
                // What the node resolved is what is judged, and what the person gets.
                Ok(listed) => node_listing(node_roots, node, &listed.path, listed.dirs),
                Err(err) => Err(format!("{err:#}")),
            },
        },
        None => list_dirs(roots, path).map(|mut listing| {
            if let (None | Some(""), Some(entries)) = (path, node_roots) {
                listing.dirs.extend(member_node_start(entries, &|id| nodes.and_then(|n| n.node_name(id))));
            }
            listing
        }),
    };
    match listing {
        Ok(listing) => ServerMessage::DirList { request_id, path: listing.path, parent: listing.parent, dirs: listing.dirs },
        Err(message) => ServerMessage::DirError { request_id, message },
    }
}

/// Whether a conversation may run in the folder `path` of node `node` (P102 fatia 2), asked of the node: it has to be a
/// folder there, inside what it lends, and — for a member — inside what the owner named. Run on every turn.
pub async fn check_node_folder(nodes: Option<&NodeToolFactory>, entries: Option<&[NodeFolder]>, node: &str, path: &str) -> Result<(), String> {
    warden_bootstrap::check_node_path(path)?;
    let nodes = nodes.ok_or_else(|| "this hub has no nodes".to_string())?;
    let listed = nodes.list_dirs(node, path).await.map_err(|e| format!("{e:#}"))?;
    check_node_workdir(entries, node, &listed.path)
}

fn read(dir: &Path, parent: Option<String>) -> Result<Listing, String> {
    let mut dirs: Vec<DirEntryDto> = std::fs::read_dir(dir)
        .map_err(|e| format!("can't read '{}': {e}", dir.display()))?
        .filter_map(Result::ok)
        // `file_type` doesn't follow a link, so a symlink to a folder is not a folder here.
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .map(|name| DirEntryDto { path: text(&dir.join(&name)), name })
        .collect();
    dirs.sort_by_key(|d| d.name.to_lowercase());
    dirs.truncate(MAX_DIRS);
    Ok(Listing { path: text(dir), parent, dirs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("warden-dirs-{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        for dir in ["allowed/Beta", "allowed/alpha/deep", "allowed/.hidden", "other/secret"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("allowed/file.txt"), "x").unwrap();
        root.canonicalize().unwrap()
    }

    fn names(listing: &Listing) -> Vec<&str> {
        listing.dirs.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn a_listing_has_folders_only_sorted_without_files_or_hidden_ones_and_points_up() {
        let root = tree();
        let listing = list_dirs(None, Some(root.join("allowed").to_str().unwrap())).unwrap();
        assert_eq!(names(&listing), ["alpha", "Beta"], "no file.txt, no .hidden, sorted ignoring case");
        assert_eq!(listing.path, text(&root.join("allowed")));
        assert_eq!(listing.parent.as_deref(), Some(text(&root).as_str()));
        assert_eq!(listing.dirs[0].path, text(&root.join("allowed/alpha")), "each entry carries its whole path");
    }

    #[test]
    fn the_owner_starts_at_home_and_can_open_any_folder() {
        let start = list_dirs(None, None).unwrap();
        assert!(!start.path.is_empty());
        let root = tree();
        assert!(list_dirs(None, Some(root.join("other").to_str().unwrap())).is_ok());
        assert_eq!(list_dirs(None, Some("")).unwrap().path, start.path, "an empty path is no path");
        assert!(check_workdir(None, root.join("other").to_str().unwrap()).is_ok());
    }

    #[test]
    fn bad_paths_are_refused() {
        let root = tree();
        let file = root.join("allowed/file.txt");
        for path in ["relative/dir", "/tmp/../etc", "/no/such/folder/for/warden", file.to_str().unwrap()] {
            assert!(list_dirs(None, Some(path)).is_err(), "{path}");
            assert!(check_workdir(None, path).is_err(), "{path}");
        }
    }

    #[test]
    fn a_member_sees_and_uses_only_their_folders() {
        let root = tree();
        let roots = vec![text(&root.join("allowed")), "/no/such/root".to_string()];
        let roots = Some(roots.as_slice());

        let start = list_dirs(roots, None).unwrap();
        assert_eq!((start.path.as_str(), start.parent.as_deref()), ("", None));
        assert_eq!(names(&start), ["allowed"], "their folders, and not the one that isn't there");

        // Inside a root: its folders, and the way up from the root itself is the list of roots.
        let at_root = list_dirs(roots, Some(&text(&root.join("allowed")))).unwrap();
        assert_eq!((names(&at_root), at_root.parent.as_deref()), (vec!["alpha", "Beta"], Some("")));
        let deeper = list_dirs(roots, Some(&text(&root.join("allowed/alpha")))).unwrap();
        assert_eq!(deeper.parent.as_deref(), Some(text(&root.join("allowed")).as_str()));

        // Outside: refused to list and to work in, and `..` and a prefix lookalike get nowhere.
        let outside = text(&root.join("other"));
        assert!(list_dirs(roots, Some(&outside)).is_err());
        assert!(check_workdir(roots, &outside).is_err());
        assert!(check_workdir(roots, &text(&root.join("allowed/alpha"))).is_ok());
        assert!(check_workdir(roots, &format!("{}/../other", root.join("allowed").display())).is_err());
        std::fs::create_dir_all(root.join("allowed-not")).unwrap();
        assert!(check_workdir(roots, &text(&root.join("allowed-not"))).is_err(), "a folder whose name merely starts like a root's");

        // A member the owner gave no folder has none to pick.
        assert!(names(&list_dirs(Some(&[]), None).unwrap()).is_empty());
        assert!(check_workdir(Some(&[]), &text(&root.join("allowed"))).is_err());
    }

    fn entry(node: &str, path: &str) -> NodeFolder {
        NodeFolder { node: node.into(), path: path.into() }
    }

    fn pairs(names: &[&str], under: &str) -> Vec<(String, String)> {
        names.iter().map(|n| (n.to_string(), if under.is_empty() { n.to_string() } else { format!("{under}/{n}") })).collect()
    }

    #[test]
    fn the_owner_browses_a_nodes_folder_with_node_references_and_a_way_up() {
        let top = node_listing(None, "node-a-1", "", pairs(&["docs", "src"], "")).unwrap();
        assert_eq!((top.path.as_str(), top.parent.clone()), ("node:node-a-1:", None), "the top of what the node lends has no way up");
        assert_eq!(top.dirs[0], DirEntryDto { name: "docs".into(), path: "node:node-a-1:docs".into() });

        let deeper = node_listing(None, "node-a-1", "docs/old", pairs(&["x"], "docs/old")).unwrap();
        assert_eq!(deeper.path, "node:node-a-1:docs/old");
        assert_eq!(deeper.parent.as_deref(), Some("node:node-a-1:docs"));
        let one_down = node_listing(None, "node-a-1", "docs", Vec::new()).unwrap();
        assert_eq!(one_down.parent.as_deref(), Some("node:node-a-1:"), "from a folder at the top the way up is the top");
    }

    #[test]
    fn a_member_sees_and_uses_only_the_node_folders_named_for_them() {
        let named = [entry("node-a-1", "projects"), entry("node-b-2", "")];
        let named = Some(&named[..]);

        assert!(check_node_workdir(named, "node-a-1", "projects").is_ok());
        assert!(check_node_workdir(named, "node-a-1", "projects/web/src").is_ok(), "inside a named folder");
        assert!(check_node_workdir(named, "node-b-2", "anything/at/all").is_ok(), "an empty path names all the node lends");
        for (node, path) in [("node-a-1", ""), ("node-a-1", "other"), ("node-a-1", "projects-not"), ("node-c-3", "projects")] {
            assert!(check_node_workdir(named, node, path).is_err(), "{node}:{path}");
        }
        assert!(check_node_workdir(Some(&[]), "node-a-1", "projects").is_err(), "none named, none allowed");

        // At a named folder the way up is the list of them; below it, the folder above.
        let at_root = node_listing(named, "node-a-1", "projects", pairs(&["web"], "projects")).unwrap();
        assert_eq!(at_root.parent.as_deref(), Some(""));
        let below = node_listing(named, "node-a-1", "projects/web", Vec::new()).unwrap();
        assert_eq!(below.parent.as_deref(), Some("node:node-a-1:projects"));
        let all_of_b = node_listing(named, "node-b-2", "", Vec::new()).unwrap();
        assert_eq!(all_of_b.parent.as_deref(), Some(""), "a member whose folder is the whole node can't go above it either");
        assert!(node_listing(named, "node-a-1", "other", Vec::new()).is_err());
    }

    #[test]
    fn a_member_starts_from_their_folders_on_nodes_by_the_nodes_name_when_it_is_connected() {
        let named = [entry("node-a-1", "projects/web"), entry("node-b-2", "")];
        let start = member_node_start(&named, &|id| (id == "node-a-1").then(|| "Home PC".to_string()));
        assert_eq!(
            start,
            vec![
                DirEntryDto { name: "Home PC · web".into(), path: "node:node-a-1:projects/web".into() },
                DirEntryDto { name: "node-b-2 · shared folder".into(), path: "node:node-b-2:".into() },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_cannot_lead_a_member_out_and_is_not_listed() {
        let root = tree();
        std::os::unix::fs::symlink(root.join("other"), root.join("allowed/escape")).unwrap();
        let roots = vec![text(&root.join("allowed"))];
        let roots = Some(roots.as_slice());
        assert!(!names(&list_dirs(roots, Some(&text(&root.join("allowed")))).unwrap()).contains(&"escape"), "a link is not shown as a folder");
        let through = text(&root.join("allowed/escape"));
        assert!(list_dirs(roots, Some(&through)).is_err());
        assert!(check_workdir(roots, &through).is_err());
        assert!(check_workdir(roots, &text(&root.join("allowed/escape/secret"))).is_err());
    }
}
