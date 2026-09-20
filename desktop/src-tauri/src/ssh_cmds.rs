//! The SSH-host half of the Settings screen (P47): the IPC shape for `SshHostConfig`, the checks
//! `save_settings` runs on it, and the "Test connection" command. Split out of `lib.rs` the same way
//! `skills_cmds.rs` is — the hosts themselves are saved through `save_settings` like agents are.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use warden_bootstrap::{AgentConfig, SshHostConfig};
use warden_core::tool::ssh::test_connection;

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
        }
    }
}

impl SshHostPayload {
    /// Trims, range-checks and runs the same validation `ssh_exec` runs on every call, so a bad
    /// host is refused when it's saved rather than silently skipped at the next startup.
    pub fn into_config(self) -> Result<SshHostConfig, String> {
        let id = self.id.trim().to_string();
        if id.is_empty() {
            return Err("every SSH host needs a name".to_string());
        }
        let port = u16::try_from(self.port).ok().filter(|p| *p != 0).ok_or_else(|| format!("SSH host '{id}': port must be between 1 and 65535"))?;
        let identity_file = Some(self.identity_file.trim().to_string()).filter(|p| !p.is_empty());
        let config = SshHostConfig {
            id,
            host: self.host.trim().to_string(),
            user: self.user.trim().to_string(),
            port,
            identity_file,
            enabled: self.enabled,
            agents: self.agents,
        };
        config.to_host().validate().map_err(|e| format!("{e:#}"))?;
        Ok(config)
    }
}

/// The whole list, with the cross-entry checks: ids unique, and every agent a host names must
/// exist (the frontend prunes a deleted agent from the lists, this is the defensive backstop).
pub fn hosts_into_config(payloads: Vec<SshHostPayload>, agents: &[AgentConfig]) -> Result<Vec<SshHostConfig>, String> {
    let mut seen = HashSet::new();
    let mut hosts = Vec::with_capacity(payloads.len());
    for payload in payloads {
        let host = payload.into_config()?;
        if !seen.insert(host.id.clone()) {
            return Err(format!("duplicate SSH host name: {}", host.id));
        }
        if let Some(unknown) = host.agents.iter().find(|a| !agents.iter().any(|known| &known.id == *a)) {
            return Err(format!("SSH host '{}' names an unknown agent '{unknown}'", host.id));
        }
        hosts.push(host);
    }
    Ok(hosts)
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
        }
    }

    fn agent(id: &str) -> AgentConfig {
        AgentConfig { id: id.into(), persona: String::new(), provider_id: None, can_delegate_to_agents: false }
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
