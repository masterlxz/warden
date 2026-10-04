//! P102 — the hubs this computer is a **client** of: a name and an address each, in `hubs.json` beside the
//! `config.toml`. The desktop opens one window per hub on the web interface the hub itself serves, so the
//! person gets everything the hub has (projects, folder picker, people, webhooks...) without the desktop
//! knowing the protocol.
//!
//! Nothing secret lives here: no pairing key, no password, no device token. The window's page signs in like
//! any browser, and keeps its own identity in its own storage (the page's origin is the hub, so each hub
//! has its own). That is why the file is only owner-readable out of habit, not because losing it would
//! hand anyone a way in.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

/// The longest name the list takes.
const MAX_NAME_CHARS: usize = 60;
/// More than this is a mistake, not a use.
const MAX_HUBS: usize = 50;

/// One hub: how it is shown, and the address of its web interface (always `http(s)://host[:port]/`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SavedHub {
    /// `hub-` and eight hex digits; stable across renames, and what the window of this hub is labelled with.
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Serialize, Deserialize, Default)]
struct HubsFile {
    #[serde(default)]
    hubs: Vec<SavedHub>,
}

/// `hubs.json` in the OS config dir, beside `config.toml`.
pub fn default_saved_hubs_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("hubs.json"))
}

/// The address of a hub's web interface, from whatever was typed: `http`/`https` stay, `ws`/`wss` (the address
/// the apps pair with) become the same host over `http`/`https`, and an address with no scheme is taken as
/// `http` (a hub on the network is usually typed as `192.168.1.5:7420`). Anything after the host and port is
/// dropped: the interface is at the root. Other schemes, an address with no host, and one with a user or
/// password in it are refused.
pub fn normalize_hub_url(input: &str) -> anyhow::Result<String> {
    let typed = input.trim();
    if typed.is_empty() {
        bail!("the address is empty");
    }
    let with_scheme = if typed.contains("://") { typed.to_string() } else { format!("http://{typed}") };
    let url = url::Url::parse(&with_scheme).with_context(|| format!("'{typed}' is not an address"))?;
    let scheme = match url.scheme() {
        "http" | "ws" => "http",
        "https" | "wss" => "https",
        other => bail!("'{other}://' is not an address a hub answers on: use http, https, ws or wss"),
    };
    if url.host_str().is_none_or(str::is_empty) {
        bail!("the address has no host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("leave the user and password out of the address: the hub asks for them on its own page");
    }
    let host = url.host_str().unwrap_or_default();
    Ok(match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}/"),
        None => format!("{scheme}://{host}/"),
    })
}

fn clean_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("give the hub a name");
    }
    if name.chars().count() > MAX_NAME_CHARS {
        bail!("the name is longer than {MAX_NAME_CHARS} characters");
    }
    Ok(name.to_string())
}

fn read(path: &Path) -> anyhow::Result<HubsFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HubsFile::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write(path: &Path, file: &HubsFile) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(file)?).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

fn new_id() -> String {
    format!("hub-{:08x}", rand::random::<u32>())
}

/// The saved hubs, in the order they were added. A missing file is an empty list.
pub fn list(path: &Path) -> anyhow::Result<Vec<SavedHub>> {
    Ok(read(path)?.hubs)
}

/// One hub by id.
pub fn find(path: &Path, id: &str) -> anyhow::Result<Option<SavedHub>> {
    Ok(read(path)?.hubs.into_iter().find(|h| h.id == id))
}

/// Adds a hub (`id` is `None`) or changes the name and address of one (`id` is its id). The address is
/// normalized (see [`normalize_hub_url`]) and has to be new: two entries for one hub would open two windows
/// on the same page. Returns the hub as saved.
pub fn save(path: &Path, id: Option<&str>, name: &str, url: &str) -> anyhow::Result<SavedHub> {
    let name = clean_name(name)?;
    let url = normalize_hub_url(url)?;
    let mut file = read(path)?;
    if let Some(other) = file.hubs.iter().find(|h| h.url == url && Some(h.id.as_str()) != id) {
        bail!("'{}' already has this address", other.name);
    }
    let saved = match id {
        Some(id) => {
            let Some(existing) = file.hubs.iter_mut().find(|h| h.id == id) else {
                bail!("there is no saved hub '{id}' (it may have been removed)");
            };
            existing.name = name;
            existing.url = url;
            existing.clone()
        }
        None => {
            if file.hubs.len() >= MAX_HUBS {
                bail!("there are already {MAX_HUBS} hubs saved: remove one first");
            }
            let mut fresh = new_id();
            while file.hubs.iter().any(|h| h.id == fresh) {
                fresh = new_id();
            }
            let hub = SavedHub { id: fresh, name, url };
            file.hubs.push(hub.clone());
            hub
        }
    };
    write(path, &file)?;
    Ok(saved)
}

/// The saved hub at this address, or a new one named `name` when there is none: what "open this computer's own hub"
/// needs, so pressing it twice doesn't pile up entries.
pub fn ensure(path: &Path, name: &str, url: &str) -> anyhow::Result<SavedHub> {
    let url = normalize_hub_url(url)?;
    if let Some(existing) = read(path)?.hubs.into_iter().find(|h| h.url == url) {
        return Ok(existing);
    }
    save(path, None, name, &url)
}

/// Whether a hub with this id was saved (and now isn't).
pub fn remove(path: &Path, id: &str) -> anyhow::Result<bool> {
    let mut file = read(path)?;
    let before = file.hubs.len();
    file.hubs.retain(|h| h.id != id);
    if file.hubs.len() == before {
        return Ok(false);
    }
    write(path, &file)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_file() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("warden-saved-hubs-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("hubs.json")
    }

    #[test]
    fn addresses_are_normalized_to_the_web_interface_root() {
        for (typed, wanted) in [
            ("http://192.168.1.5:7420", "http://192.168.1.5:7420/"),
            ("  192.168.1.5:7420  ", "http://192.168.1.5:7420/"),
            ("ws://192.168.1.5:7420", "http://192.168.1.5:7420/"),
            ("wss://hub.example.ts.net", "https://hub.example.ts.net/"),
            ("https://hub.example.ts.net:8443/some/page?x=1#top", "https://hub.example.ts.net:8443/"),
            ("https://hub.example.ts.net:443/", "https://hub.example.ts.net/"),
            ("HTTP://Hub.Example/", "http://hub.example/"),
            ("http://[::1]:7420", "http://[::1]:7420/"),
            ("localhost:7420", "http://localhost:7420/"),
        ] {
            assert_eq!(normalize_hub_url(typed).unwrap(), wanted, "'{typed}'");
        }
    }

    #[test]
    fn what_is_not_a_hub_address_is_refused() {
        for bad in [
            "",
            "   ",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://hub.example/",
            "data:text/html,<script>1</script>",
            "http://",
            "http://user@hub.example/",
            "http://user:secret@hub.example/",
            "https://:secret@hub.example/",
            "not an address at all",
        ] {
            assert!(normalize_hub_url(bad).is_err(), "'{bad}' should be refused");
        }
    }

    #[test]
    fn a_refused_address_says_why_without_echoing_the_password() {
        let message = format!("{:#}", normalize_hub_url("http://ana:hunter2@hub.example/").unwrap_err());
        assert!(message.contains("user and password"), "{message}");
        assert!(!message.contains("hunter2"), "the password isn't repeated back: {message}");
    }

    #[test]
    fn hubs_are_added_listed_renamed_and_removed() {
        let path = temp_file();
        assert!(list(&path).unwrap().is_empty(), "no file is an empty list");

        let home = save(&path, None, "  Home  ", "192.168.1.5:7420").unwrap();
        let vps = save(&path, None, "VPS", "wss://vps.example.ts.net").unwrap();
        assert!(home.id.starts_with("hub-") && home.id.len() == 12, "{}", home.id);
        assert_ne!(home.id, vps.id);
        assert_eq!((home.name.as_str(), home.url.as_str()), ("Home", "http://192.168.1.5:7420/"));
        assert_eq!(list(&path).unwrap(), vec![home.clone(), vps.clone()], "in the order added");
        assert_eq!(find(&path, &vps.id).unwrap(), Some(vps.clone()));
        assert_eq!(find(&path, "hub-nope").unwrap(), None);

        // Changing one keeps its id (the window of this hub is labelled with it).
        let renamed = save(&path, Some(&home.id), "Casa", "http://192.168.1.9:7420").unwrap();
        assert_eq!(renamed.id, home.id);
        assert_eq!((renamed.name.as_str(), renamed.url.as_str()), ("Casa", "http://192.168.1.9:7420/"));
        assert_eq!(list(&path).unwrap().len(), 2);
        // Saving it again with its own address is not "already saved".
        save(&path, Some(&home.id), "Casa 2", "http://192.168.1.9:7420").unwrap();

        assert!(remove(&path, &home.id).unwrap());
        assert!(!remove(&path, &home.id).unwrap(), "already gone");
        assert_eq!(list(&path).unwrap(), vec![vps]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_hub_cannot_be_saved_twice_and_a_bad_entry_changes_nothing() {
        let path = temp_file();
        let first = save(&path, None, "Home", "192.168.1.5:7420").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        // The same hub typed another way is the same hub.
        for dup in ["http://192.168.1.5:7420", "ws://192.168.1.5:7420/", "192.168.1.5:7420/page"] {
            let err = save(&path, None, "Again", dup).unwrap_err();
            assert!(format!("{err:#}").contains("'Home' already has this address"), "{dup}: {err:#}");
        }
        let second = save(&path, None, "Other", "192.168.1.6:7420").unwrap();
        assert!(save(&path, Some(&second.id), "Other", "192.168.1.5:7420").is_err(), "renaming into another hub's address");

        for (id, name, url) in [(None, "", "192.168.1.7:1"), (None, "x", ""), (None, "x", "file:///x"), (Some("hub-nope"), "x", "192.168.1.7:1"), (None, &"n".repeat(61)[..], "192.168.1.7:1")] {
            assert!(save(&path, id, name, url).is_err(), "{id:?} '{name}' '{url}'");
        }
        assert_eq!(list(&path).unwrap(), vec![first, second]);
        assert_ne!(std::fs::read_to_string(&path).unwrap(), before, "only the good second one was written");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn ensure_finds_the_hub_at_an_address_before_adding_another() {
        let path = temp_file();
        let first = ensure(&path, "This computer", "http://localhost:7420").unwrap();
        let again = ensure(&path, "Another name", "localhost:7420/anything").unwrap();
        assert_eq!(again, first, "the same address is the same hub, whatever it is called now");
        assert_eq!(list(&path).unwrap().len(), 1);
        let other = ensure(&path, "Other", "localhost:7421").unwrap();
        assert_ne!(other.id, first.id);
        assert!(ensure(&path, "x", "file:///x").is_err());
        assert_eq!(list(&path).unwrap().len(), 2);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_list_has_a_ceiling() {
        let path = temp_file();
        for n in 0..MAX_HUBS {
            save(&path, None, &format!("hub {n}"), &format!("10.0.0.{}:7420", n + 1)).unwrap();
        }
        let err = save(&path, None, "one too many", "10.0.1.1:7420").unwrap_err();
        assert!(format!("{err:#}").contains("remove one first"), "{err:#}");
        assert_eq!(list(&path).unwrap().len(), MAX_HUBS);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn only_a_name_and_an_address_are_written_and_the_file_is_the_owners() {
        let path = temp_file();
        save(&path, None, "Home", "192.168.1.5:7420").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let hub = &json["hubs"][0];
        let mut keys: Vec<&str> = hub.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["id", "name", "url"], "no key, password or token field exists: {text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(!path.with_extension("json.tmp").exists(), "no leftover temp file");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_broken_file_is_an_error_not_an_empty_list() {
        let path = temp_file();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(list(&path).is_err());
        assert!(save(&path, None, "Home", "192.168.1.5:7420").is_err(), "a save doesn't overwrite what it can't read");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
