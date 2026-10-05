//! The SSH-host half of the Settings screen (P47): the IPC shape for `SshHostConfig`, the checks
//! `save_settings` runs on it, and the "Test connection" command. Split out of `lib.rs` the same way
//! `skills_cmds.rs` is — the hosts themselves are saved through `save_settings` like agents are.

use serde::{Deserialize, Serialize};
use warden_bootstrap::machine_settings::{ssh_host_from_dto, ssh_hosts_into_config};
use warden_bootstrap::{AgentConfig, SshHostConfig};
use warden_core::tool::ssh::test_connection;
use warden_server_protocol::protocol::SshHostDto;

/// `port` is a `u32` here (not `u16`) so an out-of-range value reaches `into_config` and gets a
/// readable error instead of an opaque serde failure. `identity_file` empty = "none", the same
/// "not set is an empty string" convention every other payload in `lib.rs` uses.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SshHostPayload {
    pub id: String,
    pub host: String,
    pub user: String,
    pub port: u32,
    pub identity_file: String,
    pub enabled: bool,
    #[serde(default)]
    pub agents: Vec<String>,
    /// Ask before every command or file transfer on this host (the approval modal, `approval.rs`).
    #[serde(default)]
    pub require_approval: bool,
}

impl From<SshHostConfig> for SshHostPayload {
    fn from(c: SshHostConfig) -> Self {
        Self {
            id: c.id,
            host: c.host,
            user: c.user,
            port: c.port.into(),
            identity_file: c.identity_file.unwrap_or_default(),
            enabled: c.enabled,
            agents: c.agents,
            require_approval: c.require_approval,
        }
    }
}

impl From<SshHostPayload> for SshHostDto {
    fn from(p: SshHostPayload) -> Self {
        Self { id: p.id, host: p.host, user: p.user, port: p.port, identity_file: p.identity_file, enabled: p.enabled, agents: p.agents, require_approval: p.require_approval }
    }
}

impl SshHostPayload {
    /// Trims, range-checks and runs the same validation `ssh_exec` runs on every call, so a bad
    /// host is refused when it's saved rather than silently skipped at the next startup. The checks
    /// are the web settings screen's too (`warden_bootstrap::machine_settings`), so both refuse alike.
    pub fn into_config(self) -> Result<SshHostConfig, String> {
        ssh_host_from_dto(self.into())
    }
}

/// The whole list, with the cross-entry checks: ids unique, and every agent a host names must
/// exist (the frontend prunes a deleted agent from the lists, this is the defensive backstop).
pub fn hosts_into_config(payloads: Vec<SshHostPayload>, agents: &[AgentConfig]) -> Result<Vec<SshHostConfig>, String> {
    ssh_hosts_into_config(payloads.into_iter().map(Into::into).collect(), agents)
}

#[derive(Serialize, Debug, PartialEq)]
pub struct SshTestResult {
    pub ok: bool,
    pub message: String,
}

/// Tries the host as `ssh_exec` would, using the values currently in the form — not the saved
/// ones — so it works before the first save, and even when the host is switched off: testing is
/// not "using".
#[tauri::command]
pub async fn test_ssh_host(host: SshHostPayload) -> Result<SshTestResult, String> {
    let config = host.into_config()?;
    let outcome = test_connection(&config.to_host()).await.map_err(|e| format!("{e:#}"))?;
    Ok(SshTestResult { ok: outcome.ok, message: outcome.message })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(id: &str) -> SshHostPayload {
        SshHostPayload {
            id: id.into(),
            host: "example.com".into(),
            user: "deploy".into(),
            port: 22,
            identity_file: String::new(),
            enabled: true,
            agents: Vec::new(),
            require_approval: false,
        }
    }

    fn agent(id: &str) -> AgentConfig {
        AgentConfig { id: id.into(), persona: String::new(), provider_id: None, can_delegate_to_agents: false, can_manage_agents: false, can_message_agents: false, can_manage_tasks: false, allowed_tools: None, autonomy: warden_bootstrap::default_autonomy(), owner: None, shared_with: Vec::new() }
    }

    #[test]
    fn empty_identity_file_becomes_none_and_fields_are_trimmed() {
        let mut p = payload(" web ");
        p.host = " example.com ".into();
        let config = p.into_config().unwrap();
        assert_eq!((config.id.as_str(), config.host.as_str()), ("web", "example.com"));
        assert_eq!(config.identity_file, None);

        let mut p = payload("web");
        p.identity_file = " /home/me/.ssh/id ".into();
        assert_eq!(p.into_config().unwrap().identity_file.as_deref(), Some("/home/me/.ssh/id"));
    }

    #[test]
    fn rejects_bad_ports_names_and_option_smuggling() {
        let mut p = payload("web");
        p.port = 0;
        assert!(p.into_config().unwrap_err().contains("port"));
        let mut p = payload("web");
        p.port = 70_000;
        assert!(p.into_config().unwrap_err().contains("port"));
        assert!(payload("  ").into_config().unwrap_err().contains("name"));
        let mut p = payload("web");
        p.host = "-oProxyCommand=evil".into();
        assert!(p.into_config().is_err());
    }

    #[test]
    fn list_checks_duplicates_and_unknown_agents() {
        let agents = [agent("ops")];
        assert_eq!(hosts_into_config(vec![payload("a"), payload("b")], &agents).unwrap().len(), 2);
        assert!(hosts_into_config(vec![payload("a"), payload("a")], &agents).unwrap_err().contains("duplicate"));

        let mut scoped = payload("a");
        scoped.agents = vec!["ops".into()];
        assert!(hosts_into_config(vec![scoped.clone()], &agents).is_ok());
        scoped.agents = vec!["ghost".into()];
        assert!(hosts_into_config(vec![scoped], &agents).unwrap_err().contains("unknown agent"));
    }
}
