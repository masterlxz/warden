//! Settings saves (P78): the checks every save runs, shared by the desktop's Settings screen and
//! the hub's web settings, and the slice of `config.toml` the web screen edits.
//!
//! The web slice is providers, agents, the Tavily/Whisper keys, spending limits and prices. Secrets
//! never leave the hub: the screen gets a `SecretStatusDto` and sends back a `SecretEdit`.
//! Everything the screen doesn't show (shell, MCP servers, SSH hosts, storage, paths, the embedded
//! hub) is carried over from the file untouched.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Context;
use warden_core::memory::content_version;
use warden_core::spend::Price;
use warden_server_protocol::protocol::{
    AgentSettingsDto, HubSettingsDto, HubSettingsUpdate, LimitSettingsDto, PriceSettingsDto, ProviderSettingsDto, SecretEdit, SecretStatusDto,
};

use crate::{
    default_limit_configs, default_model_for, env_switches_limits_off, remove_agent_from, AgentConfig, FileConfig, LimitConfig, LimitScope,
    Provider, ProviderConfig,
};

/// Shortest secret whose last four characters are shown as a hint. Below this, four characters
/// would be a real share of it.
const HINT_MIN_LEN: usize = 16;

/// The version of the config file at `path`: SHA-256 of its bytes, the same scheme the vault
/// editor uses. A missing file has the version of an empty one.
pub fn config_version(path: &Path) -> anyhow::Result<String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(content_version(&bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(content_version(b"")),
        Err(err) => Err(err).with_context(|| format!("failed to read config file at {}", path.display())),
    }
}

pub fn provider_kind_str(kind: Provider) -> &'static str {
    match kind {
        Provider::Gemini => "gemini",
        Provider::Openai => "openai",
        Provider::Anthropic => "anthropic",
        Provider::OpenaiCompatible => "openai_compatible",
    }
}

pub fn provider_kind_from_str(kind: &str) -> Result<Provider, String> {
    match kind {
        "gemini" => Ok(Provider::Gemini),
        "openai" => Ok(Provider::Openai),
        "anthropic" => Ok(Provider::Anthropic),
        "openai_compatible" => Ok(Provider::OpenaiCompatible),
        other => Err(format!("unknown provider kind '{other}'")),
    }
}

/// Provider kind → the model a provider with no `model` of its own uses.
pub fn default_models_by_kind() -> BTreeMap<String, String> {
    [Provider::Gemini, Provider::Openai, Provider::Anthropic, Provider::OpenaiCompatible]
        .into_iter()
        .filter_map(|kind| default_model_for(kind).map(|model| (provider_kind_str(kind).to_string(), model.to_string())))
        .collect()
}

pub fn secret_status(secret: Option<&str>) -> SecretStatusDto {
    match secret {
        None => SecretStatusDto { set: false, hint: None },
        Some(secret) => {
            let chars: Vec<char> = secret.chars().collect();
            let hint = (chars.len() >= HINT_MIN_LEN).then(|| chars[chars.len() - 4..].iter().collect());
            SecretStatusDto { set: true, hint }
        }
    }
}

fn apply_secret(edit: SecretEdit, current: Option<String>) -> Option<String> {
    match edit {
        SecretEdit::Keep => current,
        SecretEdit::Set(value) => non_empty(&value),
        SecretEdit::Clear => None,
    }
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn limit_scope_str(scope: LimitScope) -> &'static str {
    match scope {
        LimitScope::Global => "global",
        LimitScope::Agent => "agent",
        LimitScope::Channel => "channel",
        LimitScope::User => "user",
    }
}

fn limit_scope_from_str(scope: &str) -> Result<LimitScope, String> {
    match scope {
        "global" => Ok(LimitScope::Global),
        "agent" => Ok(LimitScope::Agent),
        "channel" => Ok(LimitScope::Channel),
        "user" => Ok(LimitScope::User),
        other => Err(format!("unknown limit scope '{other}'")),
    }
}

impl From<LimitConfig> for LimitSettingsDto {
    fn from(c: LimitConfig) -> Self {
        Self {
            id: c.id,
            scope: limit_scope_str(c.scope).to_string(),
            target: c.target.unwrap_or_default(),
            window_hours: c.window_hours,
            max_tokens: c.max_tokens,
            max_cost_usd: c.max_cost_usd,
            warn_at: c.warn_at,
            extend_step: c.extend_step,
        }
    }
}

/// Trims, then runs the very validation the guard applies at startup (`LimitConfig::to_limit`), so
/// what is saved is what will be enforced rather than skipped with a note at the next launch.
pub fn limit_into_config(dto: LimitSettingsDto) -> Result<LimitConfig, String> {
    let config = LimitConfig {
        id: dto.id.trim().to_string(),
        scope: limit_scope_from_str(dto.scope.trim())?,
        target: non_empty(&dto.target),
        window_hours: dto.window_hours,
        max_tokens: dto.max_tokens,
        max_cost_usd: dto.max_cost_usd,
        warn_at: dto.warn_at,
        extend_step: dto.extend_step,
    };
    config.to_limit().map_err(|e| format!("{e:#}"))?;
    Ok(config)
}

/// The whole list, with the cross-entry check: ids unique. It deliberately doesn't check that an
/// agent a limit names still exists: a limit left behind by a deleted agent is harmless, and
/// refusing every later save over it would be the dangling-reference trap the SSH hosts had.
pub fn limits_into_config(dtos: Vec<LimitSettingsDto>) -> Result<Vec<LimitConfig>, String> {
    let mut seen = HashSet::new();
    let mut limits = Vec::with_capacity(dtos.len());
    for dto in dtos {
        let limit = limit_into_config(dto)?;
        if !seen.insert(limit.id.clone()) {
            return Err(format!("duplicate limit name: {}", limit.id));
        }
        limits.push(limit);
    }
    Ok(limits)
}

/// Trims each model id and refuses an empty or repeated one and any price that is not a number of
/// zero or more (a free model is a fine price; a negative one is a typo).
pub fn prices_into_config(dtos: Vec<PriceSettingsDto>) -> Result<Vec<Price>, String> {
    let mut seen = HashSet::new();
    let mut prices = Vec::with_capacity(dtos.len());
    for dto in dtos {
        let model = dto.model.trim().to_string();
        if model.is_empty() {
            return Err("every price needs a model id".to_string());
        }
        let ok = |n: f64| n.is_finite() && n >= 0.0;
        if !ok(dto.input_per_mtok) || !ok(dto.output_per_mtok) {
            return Err(format!("price for '{model}': prices must be numbers of 0 or more"));
        }
        if !seen.insert(model.clone()) {
            return Err(format!("duplicate price for model: {model}"));
        }
        prices.push(Price { model, input_per_mtok: dto.input_per_mtok, output_per_mtok: dto.output_per_mtok });
    }
    Ok(prices)
}

/// Trims each provider and refuses a nameless or repeated one.
pub fn check_providers(providers: Vec<ProviderConfig>) -> Result<Vec<ProviderConfig>, String> {
    let mut seen = HashSet::new();
    let mut checked = Vec::with_capacity(providers.len());
    for p in providers {
        let id = p.id.trim().to_string();
        if id.is_empty() {
            return Err("every provider needs a name".to_string());
        }
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate provider name: {id}"));
        }
        let trim = |s: Option<String>| s.as_deref().and_then(non_empty);
        checked.push(ProviderConfig { id, kind: p.kind, api_key: trim(p.api_key), base_url: trim(p.base_url), model: trim(p.model) });
    }
    Ok(checked)
}

/// Trims each agent and refuses a nameless or repeated one, or one whose default model names a
/// provider that isn't in `providers`.
pub fn check_agents(agents: Vec<AgentConfig>, providers: &[ProviderConfig]) -> Result<Vec<AgentConfig>, String> {
    let mut seen = HashSet::new();
    let mut checked = Vec::with_capacity(agents.len());
    for a in agents {
        let id = a.id.trim().to_string();
        if id.is_empty() {
            return Err("every agent needs a name".to_string());
        }
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate agent name: {id}"));
        }
        let provider_id = a.provider_id.as_deref().and_then(non_empty);
        if let Some(pid) = &provider_id {
            if !providers.iter().any(|p| &p.id == pid) {
                return Err(format!("agent '{id}' has an unknown default provider '{pid}'"));
            }
        }
        checked.push(AgentConfig { id, provider_id, ..a });
    }
    Ok(checked)
}

/// An empty `active` means none; anything else must be one of `providers`.
pub fn check_active_provider(active: &str, providers: &[ProviderConfig]) -> Result<Option<String>, String> {
    let active = non_empty(active);
    if let Some(id) = &active {
        if !providers.iter().any(|p| &p.id == id) {
            return Err(format!("active provider '{id}' is not one of the configured providers"));
        }
    }
    Ok(active)
}

/// What the web settings screen shows of `config`. `tool_names` come from the running orchestrator;
/// `host_notes` are the host's own caveats (command-line flags that win over the file), shown
/// ahead of the ones read from the environment here.
pub fn hub_settings(config: &FileConfig, tool_names: Vec<String>, host_notes: Vec<String>) -> HubSettingsDto {
    let mut notes = host_notes;
    if config.providers.is_empty() {
        notes.push(
            "No providers are saved yet, so this hub runs on the older single-provider setup (GEMINI_API_KEY or OPENAI_API_KEY). \
             Saving a provider here replaces it."
                .to_string(),
        );
    }
    if std::env::var("TAVILY_API_KEY").is_ok_and(|v| !v.is_empty()) {
        notes.push("TAVILY_API_KEY is set in the hub's environment and wins over the Tavily key saved here.".to_string());
    }

    HubSettingsDto {
        providers: config
            .providers
            .iter()
            .map(|p| ProviderSettingsDto {
                id: p.id.clone(),
                kind: provider_kind_str(p.kind).to_string(),
                base_url: p.base_url.clone().unwrap_or_default(),
                model: p.model.clone().unwrap_or_default(),
                api_key: secret_status(p.api_key.as_deref()),
            })
            .collect(),
        active_provider: config.active_provider.clone().unwrap_or_default(),
        agents: config
            .agents
            .iter()
            .map(|a| AgentSettingsDto {
                original_id: None,
                id: a.id.clone(),
                persona: a.persona.clone(),
                provider_id: a.provider_id.clone().unwrap_or_default(),
                can_delegate_to_agents: a.can_delegate_to_agents,
                can_manage_agents: a.can_manage_agents,
                allowed_tools: a.allowed_tools.clone(),
            })
            .collect(),
        tavily_key: secret_status(config.api_keys.tavily.as_deref()),
        whisper_key: secret_status(config.api_keys.whisper.as_deref()),
        limits: config.limits.clone().map(|limits| limits.into_iter().map(Into::into).collect()),
        default_limits: default_limit_configs().into_iter().map(Into::into).collect(),
        limits_disabled_by_env: env_switches_limits_off(std::env::var("WARDEN_SPEND_LIMITS").ok().as_deref()),
        prices: config.prices.iter().cloned().map(Into::into).collect(),
        default_models: default_models_by_kind(),
        tool_names,
        notes,
    }
}

/// `existing` with the web screen's slice replaced by `update`, checked the same way the desktop's
/// Settings screen checks it. Everything else in the file is carried over.
///
/// Agents are matched to what the file had by `original_id`: a renamed agent is renamed in the SSH
/// hosts that name it, and a removed one is taken out of them the way `remove_agent_from` does (a
/// host left with no agent is switched off, never widened to every agent).
pub fn apply_hub_settings(existing: FileConfig, update: HubSettingsUpdate) -> Result<FileConfig, String> {
    let mut config = existing;

    let mut providers = Vec::with_capacity(update.providers.len());
    for edit in update.providers {
        let kind = provider_kind_from_str(edit.kind.trim())?;
        let saved_key = edit.original_id.as_deref().and_then(|original| config.providers.iter().find(|p| p.id == original)).and_then(|p| p.api_key.clone());
        providers.push(ProviderConfig {
            id: edit.id,
            kind,
            api_key: apply_secret(edit.api_key, saved_key),
            base_url: Some(edit.base_url),
            model: Some(edit.model),
        });
    }
    let providers = check_providers(providers)?;
    let active_provider = check_active_provider(&update.active_provider, &providers)?;

    let mut renames = Vec::new();
    let mut agents = Vec::with_capacity(update.agents.len());
    for dto in update.agents {
        if let Some(original) = dto.original_id.as_deref() {
            if original != dto.id.trim() {
                renames.push((original.to_string(), dto.id.trim().to_string()));
            }
        }
        agents.push(AgentConfig {
            id: dto.id,
            persona: dto.persona,
            provider_id: Some(dto.provider_id),
            can_delegate_to_agents: dto.can_delegate_to_agents,
            can_manage_agents: dto.can_manage_agents,
            allowed_tools: dto.allowed_tools,
        });
    }
    let kept: HashSet<String> = renames.iter().map(|(original, _)| original.clone()).chain(agents.iter().map(|a| a.id.trim().to_string())).collect();
    let agents = check_agents(agents, &providers)?;

    let removed: Vec<String> = config.agents.iter().map(|a| a.id.clone()).filter(|id| !kept.contains(id)).collect();
    let mut scratch = config.agents.clone();
    for id in &removed {
        remove_agent_from(&mut scratch, &mut config.ssh_hosts, id);
    }
    for host in &mut config.ssh_hosts {
        for name in &mut host.agents {
            if let Some((_, new)) = renames.iter().find(|(original, _)| original == name) {
                *name = new.clone();
            }
        }
    }

    config.limits = update.limits.map(limits_into_config).transpose()?;
    config.prices = prices_into_config(update.prices)?;
    config.api_keys.tavily = apply_secret(update.tavily_key, config.api_keys.tavily.take());
    config.api_keys.whisper = apply_secret(update.whisper_key, config.api_keys.whisper.take());

    // Same as the desktop's save: once the registry holds a provider, the legacy single-provider
    // fields are only stale duplicate secrets.
    if !providers.is_empty() {
        config.provider = None;
        config.model = None;
        config.api_keys.gemini = None;
        config.api_keys.openai = None;
    }
    config.providers = providers;
    config.active_provider = active_provider;
    config.agents = agents;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SshHostConfig;
    use warden_server_protocol::protocol::ProviderEditDto;

    fn provider(id: &str, key: Option<&str>) -> ProviderConfig {
        ProviderConfig { id: id.into(), kind: Provider::Gemini, api_key: key.map(Into::into), base_url: None, model: None }
    }

    fn agent(id: &str) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            persona: "p".into(),
            provider_id: None,
            can_delegate_to_agents: false,
            can_manage_agents: false,
            allowed_tools: None,
        }
    }

    fn host(id: &str, agents: &[&str]) -> SshHostConfig {
        SshHostConfig {
            id: id.into(),
            host: "h".into(),
            user: "u".into(),
            port: 22,
            identity_file: None,
            enabled: true,
            agents: agents.iter().map(|a| a.to_string()).collect(),
            require_approval: false,
        }
    }

    fn limit(id: &str) -> LimitSettingsDto {
        LimitSettingsDto {
            id: id.into(),
            scope: "global".into(),
            target: String::new(),
            window_hours: 24,
            max_tokens: Some(1_000),
            max_cost_usd: None,
            warn_at: None,
            extend_step: None,
        }
    }

    fn price(model: &str) -> PriceSettingsDto {
        PriceSettingsDto { model: model.into(), input_per_mtok: 3.0, output_per_mtok: 15.0 }
    }

    /// The update the web screen would send for `config` if nothing was touched.
    fn untouched(config: &FileConfig) -> HubSettingsUpdate {
        let view = hub_settings(config, Vec::new(), Vec::new());
        HubSettingsUpdate {
            providers: view
                .providers
                .into_iter()
                .map(|p| ProviderEditDto { original_id: Some(p.id.clone()), id: p.id, kind: p.kind, base_url: p.base_url, model: p.model, api_key: SecretEdit::Keep })
                .collect(),
            active_provider: view.active_provider,
            agents: view.agents.into_iter().map(|a| AgentSettingsDto { original_id: Some(a.id.clone()), ..a }).collect(),
            tavily_key: SecretEdit::Keep,
            whisper_key: SecretEdit::Keep,
            limits: view.limits,
            prices: view.prices,
        }
    }

    fn sample() -> FileConfig {
        FileConfig {
            providers: vec![provider("main", Some("AIzaSyD-very-long-secret-1234")), provider("spare", None)],
            active_provider: Some("main".into()),
            agents: vec![agent("pirate"), agent("chef")],
            ssh_hosts: vec![host("box", &["pirate"]), host("nas", &["chef", "pirate"])],
            enable_shell: Some(true),
            vault_path: Some("/somewhere".into()),
            api_keys: crate::ApiKeys { tavily: Some("tvly-0123456789abcdef".into()), telegram_bot_token: Some("tg".into()), ..Default::default() },
            limits: Some(vec![limit_into_config(limit("day")).unwrap()]),
            prices: vec![Price { model: "m".into(), input_per_mtok: 1.0, output_per_mtok: 2.0 }],
            ..Default::default()
        }
    }

    #[test]
    fn the_view_never_carries_a_secret_only_whether_one_is_set() {
        let view = hub_settings(&sample(), vec!["read_file".into()], vec!["flag".into()]);
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("very-long-secret"), "{json}");
        assert!(!json.contains("tvly-0123"), "{json}");
        assert_eq!(view.providers[0].api_key, SecretStatusDto { set: true, hint: Some("1234".into()) });
        assert_eq!(view.providers[1].api_key, SecretStatusDto { set: false, hint: None });
        assert_eq!(view.tavily_key.hint.as_deref(), Some("cdef"));
        assert_eq!(view.notes[0], "flag");
        assert_eq!(view.default_models.get("gemini").map(String::as_str), default_model_for(Provider::Gemini));
    }

    #[test]
    fn a_short_secret_gets_no_hint() {
        assert_eq!(secret_status(Some("abc123")), SecretStatusDto { set: true, hint: None });
    }

    #[test]
    fn saving_what_was_loaded_changes_nothing() {
        let config = sample();
        let saved = apply_hub_settings(sample(), untouched(&config)).unwrap();
        assert_eq!(saved, config);
    }

    #[test]
    fn secrets_are_kept_set_or_cleared_and_a_renamed_provider_keeps_its_key() {
        let mut update = untouched(&sample());
        update.providers[0].id = "primary".into();
        update.active_provider = "primary".into();
        update.providers[1].api_key = SecretEdit::Set("  sk-new  ".into());
        update.tavily_key = SecretEdit::Clear;
        update.whisper_key = SecretEdit::Set("whisper".into());

        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.providers[0].id, "primary");
        assert_eq!(saved.providers[0].api_key.as_deref(), Some("AIzaSyD-very-long-secret-1234"));
        assert_eq!(saved.providers[1].api_key.as_deref(), Some("sk-new"));
        assert_eq!(saved.active_provider.as_deref(), Some("primary"));
        assert_eq!(saved.api_keys.tavily, None);
        assert_eq!(saved.api_keys.whisper.as_deref(), Some("whisper"));
    }

    #[test]
    fn a_new_provider_has_no_key_to_keep() {
        let mut update = untouched(&sample());
        update.providers.push(ProviderEditDto {
            original_id: None,
            id: "main-copy".into(),
            kind: "anthropic".into(),
            base_url: String::new(),
            model: String::new(),
            api_key: SecretEdit::Keep,
        });
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.providers[2].api_key, None);
        assert_eq!(saved.providers[2].kind, Provider::Anthropic);
    }

    #[test]
    fn what_the_screen_does_not_show_is_carried_over() {
        let mut update = untouched(&sample());
        update.prices.clear();
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.enable_shell, Some(true));
        assert_eq!(saved.vault_path.as_deref(), Some("/somewhere"));
        assert_eq!(saved.api_keys.telegram_bot_token.as_deref(), Some("tg"));
        assert!(saved.prices.is_empty());
    }

    #[test]
    fn a_renamed_agent_is_renamed_in_the_ssh_hosts_and_a_removed_one_is_taken_out() {
        let mut update = untouched(&sample());
        update.agents[0].id = "captain".into();
        update.agents.remove(1);
        let saved = apply_hub_settings(sample(), update).unwrap();

        assert_eq!(saved.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["captain"]);
        assert_eq!(saved.ssh_hosts[0].agents, ["captain"]);
        assert!(saved.ssh_hosts[0].enabled);
        assert_eq!(saved.ssh_hosts[1].agents, ["captain"], "chef is gone, pirate renamed");
    }

    #[test]
    fn a_host_left_with_no_agent_is_switched_off_not_opened_to_everyone() {
        let mut update = untouched(&sample());
        update.agents.retain(|a| a.id != "pirate");
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert!(saved.ssh_hosts[0].agents.is_empty());
        assert!(!saved.ssh_hosts[0].enabled);
    }

    #[test]
    fn a_new_agent_reusing_a_renamed_agents_old_name_does_not_lose_the_rename() {
        let mut update = untouched(&sample());
        update.agents[0].id = "captain".into();
        update.agents.push(AgentSettingsDto { original_id: None, id: "pirate".into(), ..update.agents[1].clone() });
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.ssh_hosts[0].agents, ["captain"]);
        assert!(saved.ssh_hosts[0].enabled);
    }

    #[test]
    fn saving_a_provider_clears_the_legacy_single_provider_fields() {
        let mut legacy = FileConfig { provider: Some(Provider::Openai), model: Some("gpt".into()), ..Default::default() };
        legacy.api_keys.openai = Some("sk-old".into());
        let mut update = untouched(&legacy);
        let kept = apply_hub_settings(FileConfig { provider: Some(Provider::Openai), ..Default::default() }, update.clone()).unwrap();
        assert_eq!(kept.provider, Some(Provider::Openai), "no providers saved: the legacy setup stays");

        update.providers.push(ProviderEditDto {
            original_id: None,
            id: "oa".into(),
            kind: "openai".into(),
            base_url: String::new(),
            model: String::new(),
            api_key: SecretEdit::Set("sk-new".into()),
        });
        update.active_provider = "oa".into();
        let saved = apply_hub_settings(legacy, update).unwrap();
        assert_eq!((saved.provider, saved.model, saved.api_keys.openai), (None, None, None));
    }

    #[test]
    fn broken_input_is_refused() {
        let refuse = |edit: fn(&mut HubSettingsUpdate)| {
            let mut update = untouched(&sample());
            edit(&mut update);
            apply_hub_settings(sample(), update).unwrap_err()
        };
        assert!(refuse(|u| u.providers[1].id = " main ".into()).contains("duplicate provider name: main"));
        assert!(refuse(|u| u.providers[0].id = "  ".into()).contains("needs a name"));
        assert!(refuse(|u| u.providers[0].kind = "llama".into()).contains("unknown provider kind"));
        assert!(refuse(|u| u.active_provider = "ghost".into()).contains("active provider 'ghost'"));
        assert!(refuse(|u| u.agents[0].provider_id = "ghost".into()).contains("unknown default provider"));
        assert!(refuse(|u| u.agents[1].id = "pirate".into()).contains("duplicate agent name"));
        assert!(refuse(|u| u.limits = Some(vec![limit("a"), limit(" a ")])).contains("duplicate limit name: a"));
        assert!(refuse(|u| u.prices = vec![price("m"), price("m")]).contains("duplicate price"));
    }

    #[test]
    fn a_limit_round_trips_through_the_form_shape_without_losing_a_field() {
        let config = LimitConfig {
            id: "ana".into(),
            scope: LimitScope::User,
            target: Some("telegram:42".into()),
            window_hours: 6,
            max_tokens: Some(50_000),
            max_cost_usd: Some(1.5),
            warn_at: Some(0.5),
            extend_step: Some(0.1),
        };
        assert_eq!(limit_into_config(config.clone().into()).unwrap(), config);

        let global = LimitConfig { scope: LimitScope::Global, target: None, ..config };
        assert_eq!(LimitSettingsDto::from(global.clone()).target, "", "no target reaches the form as an empty string");
        assert_eq!(limit_into_config(global.clone().into()).unwrap(), global);
    }

    #[test]
    fn a_limit_is_trimmed_and_checked_with_the_same_rules_as_startup() {
        let mut padded = limit("  day  ");
        padded.scope = "channel".into();
        padded.target = " telegram ".into();
        let config = limit_into_config(padded).unwrap();
        assert_eq!((config.id.as_str(), config.target.as_deref()), ("day", Some("telegram")));

        let mut no_ceiling = limit("x");
        no_ceiling.max_tokens = None;
        assert!(limit_into_config(no_ceiling).unwrap_err().contains("max_tokens and/or max_cost_usd"));

        let mut no_window = limit("x");
        no_window.window_hours = 0;
        assert!(limit_into_config(no_window).unwrap_err().contains("window_hours"));

        let mut global_with_target = limit("x");
        global_with_target.target = "cli".into();
        assert!(limit_into_config(global_with_target).unwrap_err().contains("takes no target"));

        let mut agent_without_target = limit("x");
        agent_without_target.scope = "agent".into();
        assert!(limit_into_config(agent_without_target).unwrap_err().contains("needs a target"));

        let mut warn_over_one = limit("x");
        warn_over_one.warn_at = Some(1.5);
        assert!(limit_into_config(warn_over_one).unwrap_err().contains("warn_at"));

        let mut bad_scope = limit("x");
        bad_scope.scope = "planet".into();
        assert!(limit_into_config(bad_scope).unwrap_err().contains("unknown limit scope"));

        assert!(limit_into_config(limit("   ")).unwrap_err().contains("needs an id"));
    }

    #[test]
    fn the_limit_list_stops_at_the_first_bad_entry_and_may_be_empty() {
        assert_eq!(limits_into_config(vec![limit("a"), limit("b")]).unwrap().len(), 2);
        let mut bad = limit("b");
        bad.max_tokens = None;
        assert!(limits_into_config(vec![limit("a"), bad]).is_err());
        assert!(limits_into_config(vec![]).unwrap().is_empty(), "an empty list is a valid way to say every limit is off");

        let mut orphan = limit("ghost-cap");
        orphan.scope = "agent".into();
        orphan.target = "deleted-agent".into();
        assert!(limits_into_config(vec![orphan]).is_ok(), "a limit on an agent that no longer exists is still saved");
    }

    #[test]
    fn prices_need_a_model_a_number_of_zero_or_more_and_no_repeats() {
        let prices = prices_into_config(vec![price(" gpt-4o-mini "), price("claude")]).unwrap();
        assert_eq!(prices[0].model, "gpt-4o-mini");

        assert!(prices_into_config(vec![price("  ")]).unwrap_err().contains("model id"));
        assert!(prices_into_config(vec![price("m"), price("m")]).unwrap_err().contains("duplicate price"));
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            let mut p = price("m");
            p.input_per_mtok = bad;
            assert!(prices_into_config(vec![p]).is_err(), "{bad}");
        }
        let free = PriceSettingsDto { model: "local".into(), input_per_mtok: 0.0, output_per_mtok: 0.0 };
        assert_eq!(prices_into_config(vec![free]).unwrap().len(), 1, "free is a price");
    }

    #[test]
    fn the_config_version_follows_the_bytes_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("warden-settings-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let _ = std::fs::remove_file(&path);
        let missing = config_version(&path).unwrap();
        assert_eq!(missing, content_version(b""));
        std::fs::write(&path, "a = 1").unwrap();
        let one = config_version(&path).unwrap();
        assert_ne!(one, missing);
        std::fs::write(&path, "a = 1").unwrap();
        assert_eq!(config_version(&path).unwrap(), one, "rewriting the same bytes keeps the version");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
