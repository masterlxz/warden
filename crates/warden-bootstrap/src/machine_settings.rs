//! P119 — what the web settings screen edits beyond providers and agents: the delegation ceilings and
//! TruthID (`advanced`), and everything that reaches the hub's own machine (`machine`): the shell tool,
//! the vault and generated-files folders, the MCP servers it starts, the SSH hosts it may reach and the
//! embedded hub.
//!
//! The first group is ordinary settings. The second would let whoever holds the pairing key run commands
//! and read files on the hub's machine, so the hub (`warden-server`'s `settings.rs`) only lets a save
//! carry it over an encrypted or local connection, and only when it was started with
//! `--allow-machine-settings`; the checks here are what such a save is still held to. Secret values (an
//! MCP server's env entries and headers) never come back to the screen: it gets their names, and sends a
//! `SecretEdit` per entry.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::ops::RangeInclusive;
use std::path::{Component, Path};

use warden_server_protocol::protocol::{
    AdvancedSettingsDto, EmbeddedServerDto, MachineEditDto, MachineSettingsDto, McpServerEditDto, McpServerSettingsDto, SecretEdit, SecretEntryEdit, SshHostDto,
};
use warden_truthid::identity::Network;

use crate::{AgentConfig, EmbeddedServerConfig, FileConfig, McpServerConfig, SshHostConfig};

/// How deep a sub-agent may delegate, at most, from the web. A higher value is the worst-case blow-up
/// `DEFAULT_DELEGATE_MAX_DEPTH` documents; whoever wants more edits the file.
pub const MAX_DELEGATE_DEPTH: u32 = 5;
/// Model calls the sub-agents may make in one turn, at most, from the web. `0` (no ceiling) is only set by hand.
pub const MAX_DELEGATED_CALLS: u32 = 300;
/// Background jobs one turn may run at the same time, at most, from the web.
pub const MAX_PARALLEL_JOBS: u32 = 10;

/// What the screen shows of the delegation ceilings and TruthID.
pub fn advanced_settings(config: &FileConfig) -> AdvancedSettingsDto {
    AdvancedSettingsDto {
        delegate_max_depth: config.delegate_max_depth,
        max_delegated_calls: config.max_delegated_calls,
        max_parallel_jobs: config.max_parallel_jobs,
        truthid_network: network_str(config.truthid_network),
        truthid_rpc_url: config.truthid_rpc_url.clone().unwrap_or_default(),
        truthid_public_url: config.truthid_public_url.clone().unwrap_or_default(),
    }
}

fn network_str(network: Network) -> String {
    serde_json::to_value(network).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn network_from_str(network: &str) -> Result<Network, String> {
    serde_json::from_value(serde_json::Value::String(network.to_string())).map_err(|_| format!("unknown TruthID network '{network}': use base-mainnet or base-sepolia"))
}

/// A ceiling that was typed in: the value it already had always passes (a file edited by hand above the
/// ceiling must not stop the rest of the screen from saving), and a changed one has to be in `allowed`.
fn ceiling(what: &str, new: Option<u32>, current: Option<u32>, allowed: RangeInclusive<u32>, hint: &str) -> Result<Option<u32>, String> {
    match new {
        Some(value) if new != current && !allowed.contains(&value) => Err(format!("{what} must be between {} and {} here ({hint})", allowed.start(), allowed.end())),
        other => Ok(other),
    }
}

/// An optional URL: blank is none, one that was already there passes, a changed one has to satisfy `ok`.
fn url_setting(what: &str, new: &str, current: Option<&String>, ok: impl Fn(&str) -> bool, expected: &str) -> Result<Option<String>, String> {
    let new = new.trim();
    if new.is_empty() {
        return Ok(None);
    }
    if current.map(String::as_str) != Some(new) && !ok(new) {
        return Err(format!("{what} must be {expected}"));
    }
    Ok(Some(new.to_string()))
}

/// Applies the delegation ceilings and TruthID to `config`.
pub fn apply_advanced(config: &mut FileConfig, dto: AdvancedSettingsDto) -> Result<(), String> {
    let depth = ceiling("The delegation depth", dto.delegate_max_depth, config.delegate_max_depth, 0..=MAX_DELEGATE_DEPTH, "edit config.toml for more")?;
    let calls = ceiling("The delegated-calls ceiling", dto.max_delegated_calls, config.max_delegated_calls, 1..=MAX_DELEGATED_CALLS, "0 only in config.toml")?;
    let jobs = ceiling("The parallel jobs", dto.max_parallel_jobs, config.max_parallel_jobs, 0..=MAX_PARALLEL_JOBS, "edit config.toml for more")?;
    let network = network_from_str(dto.truthid_network.trim())?;
    let rpc_url = url_setting("The TruthID RPC address", &dto.truthid_rpc_url, config.truthid_rpc_url.as_ref(), |u| u.starts_with("http://") || u.starts_with("https://"), "an http:// or https:// address")?;
    let public_url = url_setting("The hub's public TruthID address", &dto.truthid_public_url, config.truthid_public_url.as_ref(), |u| u.starts_with("https://"), "an https:// address")?;

    config.delegate_max_depth = depth;
    config.max_delegated_calls = calls;
    config.max_parallel_jobs = jobs;
    config.truthid_network = network;
    config.truthid_rpc_url = rpc_url;
    config.truthid_public_url = public_url;
    Ok(())
}

impl From<SshHostConfig> for SshHostDto {
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

/// One SSH host as it would be saved: trimmed, the port in range, and the same validation `ssh_exec` runs
/// on every call, so a bad host is refused when it's saved rather than skipped at the next start. Shared
/// by the desktop's Settings screen and the web's.
pub fn ssh_host_from_dto(dto: SshHostDto) -> Result<SshHostConfig, String> {
    let id = dto.id.trim().to_string();
    if id.is_empty() {
        return Err("every SSH host needs a name".to_string());
    }
    let port = u16::try_from(dto.port).ok().filter(|p| *p != 0).ok_or_else(|| format!("SSH host '{id}': port must be between 1 and 65535"))?;
    let identity_file = Some(dto.identity_file.trim().to_string()).filter(|p| !p.is_empty());
    let config = SshHostConfig {
        id,
        host: dto.host.trim().to_string(),
        user: dto.user.trim().to_string(),
        port,
        identity_file,
        enabled: dto.enabled,
        agents: dto.agents,
        require_approval: dto.require_approval,
    };
    config.to_host().validate().map_err(|e| format!("{e:#}"))?;
    Ok(config)
}

/// The whole list, with the cross-entry checks: ids unique, and every agent a host names must exist.
pub fn ssh_hosts_into_config(dtos: Vec<SshHostDto>, agents: &[AgentConfig]) -> Result<Vec<SshHostConfig>, String> {
    let mut seen = HashSet::new();
    let mut hosts = Vec::with_capacity(dtos.len());
    for dto in dtos {
        let host = ssh_host_from_dto(dto)?;
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

fn embedded_to_dto(c: &EmbeddedServerConfig) -> EmbeddedServerDto {
    EmbeddedServerDto {
        enabled: c.enabled,
        port: c.port,
        listen_host: c.listen_host.clone().unwrap_or_default(),
        server_name: c.server_name.clone().unwrap_or_default(),
        tailscale_cert: c.tailscale_cert,
        tls_cert: c.tls_cert.clone().unwrap_or_default(),
        tls_key: c.tls_key.clone().unwrap_or_default(),
        tls_host: c.tls_host.clone().unwrap_or_default(),
        web_ui: c.web_ui,
    }
}

fn sorted_keys(map: &HashMap<String, String>) -> Vec<String> {
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

/// What the screen shows of what reaches the machine. `writable` and `blocked_reason` are the hub's to
/// fill in (they depend on how it was started and on the connection), so they come back locked.
pub fn machine_settings(config: &FileConfig) -> MachineSettingsDto {
    MachineSettingsDto {
        writable: false,
        blocked_reason: String::new(),
        enable_shell: config.enable_shell.unwrap_or(false),
        vault_path: config.vault_path.clone().unwrap_or_default(),
        generated_path: config.generated_path.clone().unwrap_or_default(),
        mcp_servers: config
            .mcp_servers
            .iter()
            .map(|server| match server {
                McpServerConfig::Stdio { name, command, args, env } => McpServerSettingsDto {
                    name: name.clone(),
                    kind: "stdio".to_string(),
                    command: command.clone(),
                    args: args.clone(),
                    env_keys: sorted_keys(env),
                    ..McpServerSettingsDto::default()
                },
                McpServerConfig::Http { name, url, headers, oauth } => McpServerSettingsDto {
                    name: name.clone(),
                    kind: "http".to_string(),
                    url: url.clone(),
                    header_keys: sorted_keys(headers),
                    oauth: *oauth,
                    ..McpServerSettingsDto::default()
                },
            })
            .collect(),
        ssh_hosts: config.ssh_hosts.iter().cloned().map(Into::into).collect(),
        embedded_server: config.embedded_server.as_ref().map(embedded_to_dto),
    }
}

/// A folder setting: blank is the default, the one already saved passes, and a changed one has to be an
/// absolute path with no `..`, since a paired device must not point the vault (or where files are written)
/// at a place it picked by climbing out of one.
fn path_setting(what: &str, new: &str, current: Option<&String>) -> Result<Option<String>, String> {
    let new = new.trim();
    if new.is_empty() {
        return Ok(None);
    }
    if current.map(String::as_str) != Some(new) {
        let path = Path::new(new);
        if !path.is_absolute() {
            return Err(format!("{what} must be an absolute path (it is a folder on the hub's machine)"));
        }
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(format!("{what} can't contain '..'"));
        }
    }
    Ok(Some(new.to_string()))
}

/// The entries of one MCP server's env or headers being saved: `Keep` carries the saved value over,
/// `Set` replaces it, `Clear` drops the entry.
fn secret_entries(what: &str, entries: Vec<SecretEntryEdit>, saved: Option<&HashMap<String, String>>) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    for entry in entries {
        let key = entry.key.trim().to_string();
        if key.is_empty() {
            return Err(format!("{what} entries need a name"));
        }
        if out.contains_key(&key) {
            return Err(format!("{what} '{key}' is listed twice"));
        }
        match entry.value {
            SecretEdit::Keep => {
                let value = saved.and_then(|s| s.get(&key)).ok_or_else(|| format!("{what} '{key}' has no saved value to keep: give it one"))?;
                out.insert(key, value.clone());
            }
            SecretEdit::Set(value) => {
                if value.trim().is_empty() {
                    return Err(format!("give {what} '{key}' a value, or remove it"));
                }
                out.insert(key, value);
            }
            SecretEdit::Clear => {}
        }
    }
    Ok(out)
}

fn mcp_server_from_edit(edit: McpServerEditDto, saved: &[McpServerConfig]) -> Result<McpServerConfig, String> {
    let name = edit.name.trim().to_string();
    if name.is_empty() {
        return Err("every MCP server needs a name".to_string());
    }
    let before = edit.original_name.as_deref().and_then(|original| saved.iter().find(|s| s.name() == original));
    match edit.kind.as_str() {
        "stdio" => {
            let command = edit.command.trim().to_string();
            if command.is_empty() {
                return Err(format!("MCP server '{name}' needs a command to run"));
            }
            let saved_env = match before {
                Some(McpServerConfig::Stdio { env, .. }) => Some(env),
                _ => None,
            };
            let env = secret_entries(&format!("MCP server '{name}': environment entry"), edit.env, saved_env)?;
            let args = edit.args.into_iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
            Ok(McpServerConfig::Stdio { name, command, args, env })
        }
        "http" => {
            let url = edit.url.trim().to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!("MCP server '{name}' needs an http:// or https:// address"));
            }
            let (saved_headers, oauth) = match before {
                Some(McpServerConfig::Http { headers, oauth, .. }) => (Some(headers), *oauth),
                _ => (None, false),
            };
            let headers = secret_entries(&format!("MCP server '{name}': header"), edit.headers, saved_headers)?;
            if oauth && !headers.is_empty() {
                return Err(format!("MCP server '{name}' signs in with OAuth, so it takes no headers"));
            }
            Ok(McpServerConfig::Http { name, url, headers, oauth })
        }
        other => Err(format!("MCP server '{name}': unknown kind '{other}'")),
    }
}

/// Applies the machine slice to `config`. `agents` are the owner's agents as this same save leaves them,
/// which is what an SSH host may name.
pub fn apply_machine(config: &mut FileConfig, edit: MachineEditDto, agents: &[AgentConfig]) -> Result<(), String> {
    let vault_path = path_setting("The vault folder", &edit.vault_path, config.vault_path.as_ref())?;
    let generated_path = path_setting("The generated-files folder", &edit.generated_path, config.generated_path.as_ref())?;

    let mut mcp_servers = Vec::with_capacity(edit.mcp_servers.len());
    for server in edit.mcp_servers {
        mcp_servers.push(mcp_server_from_edit(server, &config.mcp_servers)?);
    }
    let mut names = HashSet::new();
    if let Some(repeated) = mcp_servers.iter().find(|s| !names.insert(s.name().to_string())) {
        return Err(format!("duplicate MCP server name: {}", repeated.name()));
    }
    let ssh_hosts = ssh_hosts_into_config(edit.ssh_hosts, agents)?;
    let embedded_server = match edit.embedded_server {
        Some(dto) => {
            let current = config.embedded_server.as_ref().ok_or_else(|| "this hub has no embedded hub set up: do that on the desktop".to_string())?;
            Some(embedded_from_dto(dto, current)?)
        }
        None => config.embedded_server.take(),
    };

    // A flag nobody changed stays out of the file instead of being written as an explicit `false`.
    if edit.enable_shell != config.enable_shell.unwrap_or(false) {
        config.enable_shell = Some(edit.enable_shell);
    }
    config.vault_path = vault_path;
    config.generated_path = generated_path;
    config.mcp_servers = mcp_servers;
    config.ssh_hosts = ssh_hosts;
    config.embedded_server = embedded_server;
    Ok(())
}

/// The embedded hub as the web edits it: everything but the key, which stays what it was.
fn embedded_from_dto(dto: EmbeddedServerDto, current: &EmbeddedServerConfig) -> Result<EmbeddedServerConfig, String> {
    let blank = |value: String| Some(value.trim().to_string()).filter(|v| !v.is_empty());
    let config = EmbeddedServerConfig {
        enabled: dto.enabled,
        port: dto.port,
        listen_host: blank(dto.listen_host),
        auth_key: current.auth_key.clone(),
        server_name: blank(dto.server_name),
        tailscale_cert: dto.tailscale_cert,
        tls_cert: blank(dto.tls_cert),
        tls_key: blank(dto.tls_key),
        tls_host: blank(dto.tls_host),
        web_ui: dto.web_ui,
    };
    if config.port == 0 {
        return Err("the embedded hub needs a port".to_string());
    }
    if let Some(host) = &config.listen_host {
        host.parse::<IpAddr>().map_err(|_| format!("'{host}' is not an IP address (for example 0.0.0.0, 127.0.0.1 or 192.168.1.10)"))?;
    }
    match (&config.tls_cert, &config.tls_key) {
        (Some(_), None) | (None, Some(_)) => return Err("give the certificate and its private key together".to_string()),
        (Some(_), Some(_)) if config.tailscale_cert => return Err("pick one: HTTPS through Tailscale or your own certificate".to_string()),
        (None, None) if config.tls_host.is_some() => return Err("the certificate's name only applies with your own certificate".to_string()),
        _ => {}
    }
    Ok(config)
}

/// One line saying what a machine save changes, for the hub's log: which parts, never their values.
pub fn machine_change_summary(before: &FileConfig, after: &FileConfig) -> String {
    let mut parts = Vec::new();
    if before.enable_shell.unwrap_or(false) != after.enable_shell.unwrap_or(false) {
        parts.push(format!("shell {}", if after.enable_shell.unwrap_or(false) { "on" } else { "off" }));
    }
    if before.vault_path != after.vault_path {
        parts.push("vault folder".to_string());
    }
    if before.generated_path != after.generated_path {
        parts.push("generated-files folder".to_string());
    }
    if before.mcp_servers != after.mcp_servers {
        parts.push(format!("MCP servers ({} now)", after.mcp_servers.len()));
    }
    if before.ssh_hosts != after.ssh_hosts {
        parts.push(format!("SSH hosts ({} now)", after.ssh_hosts.len()));
    }
    if before.embedded_server != after.embedded_server {
        parts.push("embedded hub".to_string());
    }
    if parts.is_empty() {
        "nothing changed".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str) -> AgentConfig {
        AgentConfig { id: id.into(), persona: String::new(), provider_id: None, can_delegate_to_agents: false, can_manage_agents: false, can_message_agents: false, can_manage_tasks: false, allowed_tools: None, autonomy: crate::default_autonomy(), approval_required: Vec::new(), role: None, reports_to: None, owner: None, shared_with: Vec::new(), delegation_models: Vec::new(), can_start_tasks: true, can_create_workers: true }
    }

    fn ssh(id: &str) -> SshHostDto {
        SshHostDto { id: id.into(), host: "example.com".into(), user: "deploy".into(), port: 22, ..SshHostDto::default() }
    }

    fn stdio(name: &str, env: &[(&str, &str)]) -> McpServerConfig {
        McpServerConfig::Stdio { name: name.into(), command: "npx".into(), args: vec!["-y".into(), "notes".into()], env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    fn http(name: &str, headers: &[(&str, &str)], oauth: bool) -> McpServerConfig {
        McpServerConfig::Http { name: name.into(), url: "https://mcp.example.com".into(), headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), oauth }
    }

    fn edit_of(config: &FileConfig) -> MachineEditDto {
        let view = machine_settings(config);
        MachineEditDto {
            enable_shell: view.enable_shell,
            vault_path: view.vault_path,
            generated_path: view.generated_path,
            mcp_servers: view
                .mcp_servers
                .into_iter()
                .map(|s| McpServerEditDto {
                    original_name: Some(s.name.clone()),
                    name: s.name,
                    kind: s.kind,
                    command: s.command,
                    args: s.args,
                    env: s.env_keys.into_iter().map(|key| SecretEntryEdit { key, value: SecretEdit::Keep }).collect(),
                    url: s.url,
                    headers: s.header_keys.into_iter().map(|key| SecretEntryEdit { key, value: SecretEdit::Keep }).collect(),
                })
                .collect(),
            ssh_hosts: view.ssh_hosts,
            embedded_server: view.embedded_server,
        }
    }

    fn entry(key: &str, value: SecretEdit) -> SecretEntryEdit {
        SecretEntryEdit { key: key.into(), value }
    }

    fn configured() -> FileConfig {
        FileConfig {
            enable_shell: Some(true),
            vault_path: Some("/srv/vault".into()),
            mcp_servers: vec![stdio("notes", &[("TOKEN", "s3cret")]), http("search", &[("Authorization", "Bearer abc")], false), http("drive", &[], true)],
            ssh_hosts: vec![crate::SshHostConfig { id: "web".into(), host: "example.com".into(), user: "deploy".into(), port: 2222, identity_file: Some("/home/me/.ssh/id".into()), enabled: true, agents: vec!["writer".into()], require_approval: true }],
            embedded_server: Some(EmbeddedServerConfig::new(7420, "a-very-long-pairing-key-0123456789")),
            ..FileConfig::default()
        }
    }

    #[test]
    fn saving_what_was_shown_changes_nothing() {
        let before = configured();
        let mut after = configured();
        apply_machine(&mut after, edit_of(&before), &[agent("writer")]).unwrap();
        assert_eq!(after, before, "the secret values, the OAuth choice and the key all came through");
    }

    #[test]
    fn the_view_names_the_secrets_but_never_carries_one() {
        let view = machine_settings(&configured());
        let text = serde_json::to_string(&view).unwrap();
        for secret in ["s3cret", "Bearer abc", "a-very-long-pairing-key"] {
            assert!(!text.contains(secret), "'{secret}' reached the screen");
        }
        assert_eq!(view.mcp_servers[0].env_keys, ["TOKEN"]);
        assert_eq!(view.mcp_servers[1].header_keys, ["Authorization"]);
        assert!(view.mcp_servers[2].oauth);
        assert!(!view.writable, "the hub decides whether it may be saved");
    }

    #[test]
    fn an_mcp_value_is_kept_replaced_or_dropped_and_a_rename_keeps_its_values() {
        let mut config = configured();
        let mut edit = edit_of(&config);
        edit.mcp_servers[0].name = "notes-2".into();
        edit.mcp_servers[0].env = vec![entry("TOKEN", SecretEdit::Keep), entry("NEW", SecretEdit::Set("v".into())), entry("GONE", SecretEdit::Clear)];
        apply_machine(&mut config, edit, &[agent("writer")]).unwrap();
        match &config.mcp_servers[0] {
            McpServerConfig::Stdio { name, env, .. } => {
                assert_eq!(name, "notes-2");
                assert_eq!(env.get("TOKEN").map(String::as_str), Some("s3cret"), "kept through the rename");
                assert_eq!(env.get("NEW").map(String::as_str), Some("v"));
                assert!(!env.contains_key("GONE"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn mcp_servers_are_checked() {
        let try_edit = |edit: fn(&mut MachineEditDto)| {
            let mut config = configured();
            let mut dto = edit_of(&config);
            edit(&mut dto);
            apply_machine(&mut config, dto, &[agent("writer")]).unwrap_err()
        };
        assert!(try_edit(|e| e.mcp_servers[0].name = " ".into()).contains("needs a name"));
        assert!(try_edit(|e| e.mcp_servers[0].command = String::new()).contains("needs a command"));
        assert!(try_edit(|e| e.mcp_servers[1].url = "ftp://x".into()).contains("http:// or https://"));
        assert!(try_edit(|e| e.mcp_servers[1].name = "notes".into()).contains("duplicate MCP server"));
        assert!(try_edit(|e| e.mcp_servers[0].kind = "carrier-pigeon".into()).contains("unknown kind"));
        assert!(try_edit(|e| e.mcp_servers[0].env = vec![entry("X", SecretEdit::Keep)]).contains("no saved value"), "nothing to keep for a new entry");
        assert!(try_edit(|e| e.mcp_servers[0].env = vec![entry("X", SecretEdit::Set("  ".into()))]).contains("a value"));
        assert!(try_edit(|e| e.mcp_servers[0].env = vec![entry("X", SecretEdit::Set("1".into())), entry("X", SecretEdit::Set("2".into()))]).contains("twice"));
        assert!(try_edit(|e| e.mcp_servers[2].headers = vec![entry("A", SecretEdit::Set("1".into()))]).contains("OAuth"), "a server that signs in with OAuth takes no headers");
    }

    #[test]
    fn a_changed_folder_must_be_absolute_and_stay_put_but_the_saved_one_always_passes() {
        let try_path = |vault: &str| {
            let mut config = configured();
            let mut dto = edit_of(&config);
            dto.vault_path = vault.into();
            apply_machine(&mut config, dto, &[agent("writer")]).map(|()| config.vault_path)
        };
        assert_eq!(try_path("/data/other").unwrap().as_deref(), Some("/data/other"));
        assert_eq!(try_path("  ").unwrap(), None, "blank is the default again");
        assert!(try_path("vault").unwrap_err().contains("absolute"));
        assert!(try_path("/srv/../etc").unwrap_err().contains(".."));
        let mut hand_edited = FileConfig { vault_path: Some("relative/vault".into()), ..FileConfig::default() };
        let dto = edit_of(&hand_edited);
        apply_machine(&mut hand_edited, dto, &[]).unwrap();
        assert_eq!(hand_edited.vault_path.as_deref(), Some("relative/vault"), "a path set by hand doesn't block the rest of the save");
    }

    #[test]
    fn ssh_hosts_are_checked_like_the_desktop_checks_them() {
        let mut bad_port = ssh("web");
        bad_port.port = 70000;
        assert!(ssh_hosts_into_config(vec![bad_port], &[]).unwrap_err().contains("port"));
        assert!(ssh_hosts_into_config(vec![ssh(" ")], &[]).unwrap_err().contains("needs a name"));
        assert!(ssh_hosts_into_config(vec![ssh("web"), ssh("web")], &[]).unwrap_err().contains("duplicate"));
        let mut option = ssh("web");
        option.host = "-oProxyCommand=evil".into();
        assert!(ssh_hosts_into_config(vec![option], &[]).is_err(), "an option can't be smuggled in as a host");
        let mut names_ghost = ssh("web");
        names_ghost.agents = vec!["ghost".into()];
        assert!(ssh_hosts_into_config(vec![names_ghost.clone()], &[agent("writer")]).unwrap_err().contains("unknown agent"));
        let mut padded = ssh(" web ");
        padded.identity_file = "  ".into();
        padded.agents = vec!["writer".into()];
        let hosts = ssh_hosts_into_config(vec![padded], &[agent("writer")]).unwrap();
        assert_eq!((hosts[0].id.as_str(), hosts[0].identity_file.clone()), ("web", None));
        assert!(!hosts[0].enabled, "a host nobody switched on stays off");
    }

    #[test]
    fn the_embedded_hub_keeps_its_key_and_is_never_created_from_the_web() {
        let mut config = configured();
        let key = config.embedded_server.as_ref().unwrap().auth_key.clone();
        let mut edit = edit_of(&config);
        let hub = edit.embedded_server.as_mut().unwrap();
        hub.port = 9000;
        hub.listen_host = "127.0.0.1".into();
        hub.web_ui = false;
        apply_machine(&mut config, edit, &[agent("writer")]).unwrap();
        let saved = config.embedded_server.as_ref().unwrap();
        assert_eq!((saved.port, saved.listen_host.as_deref(), saved.web_ui, saved.auth_key.as_str()), (9000, Some("127.0.0.1"), false, key.as_str()));

        let mut bare = FileConfig::default();
        let mut edit = edit_of(&configured());
        edit.mcp_servers.clear();
        edit.ssh_hosts.clear();
        assert!(apply_machine(&mut bare, edit, &[]).unwrap_err().contains("desktop"));
    }

    #[test]
    fn the_embedded_hub_is_checked() {
        let try_hub = |edit: fn(&mut EmbeddedServerDto)| {
            let mut config = configured();
            let mut dto = edit_of(&config);
            edit(dto.embedded_server.as_mut().unwrap());
            apply_machine(&mut config, dto, &[agent("writer")]).unwrap_err()
        };
        assert!(try_hub(|h| h.port = 0).contains("port"));
        assert!(try_hub(|h| h.listen_host = "localhost".into()).contains("IP address"));
        assert!(try_hub(|h| h.tls_cert = "/c.pem".into()).contains("together"));
        assert!(try_hub(|h| {
            h.tls_cert = "/c.pem".into();
            h.tls_key = "/k.pem".into();
            h.tailscale_cert = true;
        })
        .contains("pick one"));
        assert!(try_hub(|h| h.tls_host = "hub.example.com".into()).contains("own certificate"));
    }

    #[test]
    fn the_shell_flag_is_only_written_when_it_changes() {
        let mut config = FileConfig::default();
        let dto = edit_of(&config);
        apply_machine(&mut config, dto, &[]).unwrap();
        assert_eq!(config.enable_shell, None, "left alone, it stays out of the file");
        let mut dto = edit_of(&config);
        dto.enable_shell = true;
        apply_machine(&mut config, dto, &[]).unwrap();
        assert_eq!(config.enable_shell, Some(true));
    }

    #[test]
    fn delegation_ceilings_only_count_when_they_change_and_zero_calls_stays_in_the_file() {
        let try_advanced = |current: FileConfig, edit: fn(&mut AdvancedSettingsDto)| {
            let mut config = current;
            let mut dto = advanced_settings(&config);
            edit(&mut dto);
            apply_advanced(&mut config, dto).map(|()| config)
        };
        let ok = try_advanced(FileConfig::default(), |d| {
            d.delegate_max_depth = Some(MAX_DELEGATE_DEPTH);
            d.max_delegated_calls = Some(MAX_DELEGATED_CALLS);
            d.max_parallel_jobs = Some(0);
        })
        .unwrap();
        assert_eq!((ok.delegate_max_depth, ok.max_delegated_calls, ok.max_parallel_jobs), (Some(5), Some(300), Some(0)));
        assert!(try_advanced(FileConfig::default(), |d| d.delegate_max_depth = Some(6)).unwrap_err().contains("depth"));
        assert!(try_advanced(FileConfig::default(), |d| d.max_delegated_calls = Some(301)).unwrap_err().contains("ceiling"));
        assert!(try_advanced(FileConfig::default(), |d| d.max_delegated_calls = Some(0)).unwrap_err().contains("0 only in config.toml"));
        assert!(try_advanced(FileConfig::default(), |d| d.max_parallel_jobs = Some(11)).unwrap_err().contains("parallel"));
        assert_eq!(try_advanced(FileConfig::default(), |d| d.max_delegated_calls = None).unwrap().max_delegated_calls, None, "back to the built-in one");

        // Set by hand above the ceiling (or to 0): saving something else doesn't trip over it.
        let by_hand = FileConfig { delegate_max_depth: Some(9), max_delegated_calls: Some(0), ..FileConfig::default() };
        let kept = try_advanced(by_hand, |d| d.truthid_public_url = "https://hub.example.com".into()).unwrap();
        assert_eq!((kept.delegate_max_depth, kept.max_delegated_calls), (Some(9), Some(0)));
    }

    #[test]
    fn truthid_settings_are_checked_and_a_value_already_there_passes() {
        let try_truthid = |edit: fn(&mut AdvancedSettingsDto)| {
            let mut config = FileConfig::default();
            let mut dto = advanced_settings(&config);
            edit(&mut dto);
            apply_advanced(&mut config, dto).map(|()| config)
        };
        let config = try_truthid(|d| {
            d.truthid_network = "base-sepolia".into();
            d.truthid_rpc_url = " https://rpc.example.com ".into();
            d.truthid_public_url = "https://hub.example.com".into();
        })
        .unwrap();
        assert_eq!(config.truthid_network, Network::BaseSepolia);
        assert_eq!(config.truthid_rpc_url.as_deref(), Some("https://rpc.example.com"));
        assert_eq!(advanced_settings(&config).truthid_network, "base-sepolia");
        assert!(try_truthid(|d| d.truthid_network = "ethereum".into()).unwrap_err().contains("base-mainnet"));
        assert!(try_truthid(|d| d.truthid_public_url = "http://hub.example.com".into()).unwrap_err().contains("https://"));
        assert!(try_truthid(|d| d.truthid_rpc_url = "ws://rpc".into()).unwrap_err().contains("http"));
        assert_eq!(try_truthid(|d| d.truthid_public_url = String::new()).unwrap().truthid_public_url, None, "blank turns it off");
        let mut by_hand = FileConfig { truthid_public_url: Some("http://legacy.example.com".into()), ..FileConfig::default() };
        let dto = advanced_settings(&by_hand);
        apply_advanced(&mut by_hand, dto).unwrap();
        assert_eq!(by_hand.truthid_public_url.as_deref(), Some("http://legacy.example.com"), "unchanged passes");
    }

    #[test]
    fn the_log_line_names_what_changed_and_never_a_value() {
        let before = configured();
        let mut after = configured();
        after.enable_shell = Some(false);
        after.mcp_servers.push(stdio("extra", &[("TOKEN", "hunter2")]));
        let line = machine_change_summary(&before, &after);
        assert_eq!(line, "shell off, MCP servers (4 now)");
        assert!(!line.contains("hunter2"));
        assert_eq!(machine_change_summary(&before, &before), "nothing changed");
    }
}
