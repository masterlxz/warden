//! Tauri commands backing the Workspace screen's "Hubs" (P102): this computer as a **client** of a hub, beside
//! being one (`server_cmds.rs`) and a node of one (`lend_cmds.rs`). Each saved hub opens in a window of its own
//! on the web interface the hub serves, so everything the hub has is there.
//!
//! The window of a hub gets no Tauri commands: `capabilities/default.json` is for the `main` window only, so a
//! page that comes from a hub (another machine, maybe on the internet) can't reach this app's `invoke`. It signs
//! in like any browser and keeps its identity in its own storage, one per hub, since the page's origin is the hub.
//! Nothing secret is kept here (see `warden_bootstrap::saved_hubs`).

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use warden_bootstrap::saved_hubs::{self, SavedHub};

/// A saved hub as the Workspace screen shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HubPayload {
    id: String,
    name: String,
    url: String,
    /// Whether its window is open right now.
    open: bool,
}

fn hubs_path() -> Result<PathBuf, String> {
    crate::config_paths::saved_hubs_file()
}

// The commands are generic over the runtime (the app runs on `Wry`) so a test can drive them through Tauri's mock one.
fn payload<R: tauri::Runtime>(app: &AppHandle<R>, hub: SavedHub) -> HubPayload {
    let open = app.get_webview_window(&hub.id).is_some();
    HubPayload { id: hub.id, name: hub.name, url: hub.url, open }
}

#[tauri::command]
pub fn list_hubs<R: tauri::Runtime>(app: AppHandle<R>) -> Result<Vec<HubPayload>, String> {
    let hubs = saved_hubs::list(&hubs_path()?).map_err(|e| format!("{e:#}"))?;
    Ok(hubs.into_iter().map(|h| payload(&app, h)).collect())
}

/// Adds a hub (`id` absent) or changes the name and address of one. A window already open on the old address is
/// closed, so the next "Open" goes to the new one.
#[tauri::command]
pub fn save_hub<R: tauri::Runtime>(app: AppHandle<R>, id: Option<String>, name: String, url: String) -> Result<HubPayload, String> {
    let hub = saved_hubs::save(&hubs_path()?, id.as_deref(), &name, &url).map_err(|e| format!("{e:#}"))?;
    if id.is_some() {
        close_window(&app, &hub.id);
    }
    Ok(payload(&app, hub))
}

/// The saved hub at `url`, or a new one named `name` — for "open this computer's own hub", which can be pressed any
/// number of times.
#[tauri::command]
pub fn ensure_hub<R: tauri::Runtime>(app: AppHandle<R>, name: String, url: String) -> Result<HubPayload, String> {
    let hub = saved_hubs::ensure(&hubs_path()?, &name, &url).map_err(|e| format!("{e:#}"))?;
    Ok(payload(&app, hub))
}

#[tauri::command]
pub fn remove_hub<R: tauri::Runtime>(app: AppHandle<R>, state: tauri::State<'_, crate::AppState>, id: String) -> Result<(), String> {
    saved_hubs::remove(&hubs_path()?, &id).map_err(|e| format!("{e:#}"))?;
    close_window(&app, &id);
    // P102: a hub taken off the list is not the one in use any more, and what this computer kept of it goes too.
    let session = {
        let mut remote = state.remote.lock().unwrap_or_else(|e| e.into_inner());
        if remote.as_ref().is_some_and(|s| s.hub_id == id) {
            remote.take()
        } else {
            None
        }
    };
    if let Some(session) = session {
        session.handle.stop();
    }
    if let Ok(config) = crate::config_paths::config_file() {
        let _ = warden_server::remote_client::RemoteIdentities::beside(&config).forget(&id);
    }
    Ok(())
}

fn close_window<R: tauri::Runtime>(app: &AppHandle<R>, id: &str) {
    if let Some(window) = app.get_webview_window(id) {
        let _ = window.close();
    }
}

/// Opens the hub's web interface in a window of its own, or brings the one that is open to the front. Async on
/// purpose: Tauri creates windows from the main thread, and a synchronous command that does it can deadlock.
#[tauri::command]
pub async fn open_hub_window<R: tauri::Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    let hub = saved_hubs::find(&hubs_path()?, &id).map_err(|e| format!("{e:#}"))?.ok_or_else(|| format!("there is no saved hub '{id}' (it may have been removed)"))?;
    if let Some(window) = app.get_webview_window(&hub.id) {
        let _ = window.unminimize();
        let _ = window.show();
        return window.set_focus().map_err(|e| format!("{e}"));
    }
    let url: tauri::Url = hub.url.parse().map_err(|e| format!("'{}' is not an address: {e}", hub.url))?;
    // The label is the hub's id (letters, digits and `-`), which is how the window is found again.
    WebviewWindowBuilder::new(&app, &hub.id, WebviewUrl::External(url))
        .title(format!("Warden — {}", hub.name))
        .inner_size(1100.0, 800.0)
        .build()
        .map(|_| ())
        .map_err(|e| format!("could not open the window: {e}"))
}
