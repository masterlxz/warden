//! Settings saves (P78): the checks every save runs, shared by the desktop's Settings screen and
//! the hub's web settings, and the slice of `config.toml` the web screen edits.
//!
//! The web slice is providers, agents, the Tavily/Whisper keys, spending limits, prices, the git
//! sync remote (P61), the bots, the Telegram token and the delegation/TruthID settings. Secrets
//! never leave the hub: the screen gets a `SecretStatusDto` and sends back a `SecretEdit`.
//! What reaches the hub's machine (shell, MCP servers, SSH hosts, folders, the embedded hub) is its
//! own slice (`machine_settings`), which the hub only accepts under its own conditions; a save without
//! it carries all of that over from the file untouched.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Context;
use warden_core::memory::content_version;
use warden_core::spend::Price;
use warden_server_protocol::protocol::{
    AgentSettingsDto, BotsSettingsDto, ComboDto, GitSyncEditDto, GitSyncSettingsDto, HubSettingsDto, HubSettingsUpdate, LimitSettingsDto, ModelPolicyDto, PriceSettingsDto,
    ProviderSettingsDto, SecretEdit, SecretStatusDto,
};

use crate::bot_access::{TelegramSettings, WhatsAppSettings};
use crate::learning::LearningSettings;
use crate::machine_settings::{advanced_settings, apply_advanced, apply_machine, machine_settings};
use crate::{
    default_limit_configs, default_model_for, env_switches_limits_off, forget_agent_in_nodes, remove_agent_from, AgentConfig, ComboConfig, FileConfig, GitSyncConfig, LimitConfig, LimitScope,
    ModelPolicyConfig, Provider, ProviderConfig,
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
        Provider::Node => "node",
    }
}

pub fn provider_kind_from_str(kind: &str) -> Result<Provider, String> {
    match kind {
        "gemini" => Ok(Provider::Gemini),
        "openai" => Ok(Provider::Openai),
        "anthropic" => Ok(Provider::Anthropic),
        "openai_compatible" => Ok(Provider::OpenaiCompatible),
        "node" => Ok(Provider::Node),
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
        LimitScope::Person => "person",
    }
}

fn limit_scope_from_str(scope: &str) -> Result<LimitScope, String> {
    match scope {
        "global" => Ok(LimitScope::Global),
        "agent" => Ok(LimitScope::Agent),
        "channel" => Ok(LimitScope::Channel),
        "user" => Ok(LimitScope::User),
        "person" => Ok(LimitScope::Person),
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
        let node = trim(p.node);
        if p.kind == Provider::Node {
            if node.is_none() {
                return Err(format!("provider '{id}' is a node's model: say which node (its device id)"));
            }
            if trim(p.model.clone()).is_none() {
                return Err(format!("provider '{id}' is a node's model: say which of the node's providers (its id on the node)"));
            }
        }
        let node = if p.kind == Provider::Node { node } else { None };
        checked.push(ProviderConfig { id, kind: p.kind, api_key: trim(p.api_key), base_url: trim(p.base_url), model: trim(p.model), node });
    }
    Ok(checked)
}

/// Whether `id` names a model someone can pick: a provider or a combo (P90).
fn is_model(id: &str, providers: &[ProviderConfig], combos: &[ComboConfig]) -> bool {
    providers.iter().any(|p| p.id == id) || combos.iter().any(|c| c.id == id)
}

/// The risk categories a screen sent by id (P122), each once, or why one isn't a category.
pub fn categories_from_ids(ids: &[String]) -> Result<Vec<warden_core::autonomy::Category>, String> {
    let mut categories = Vec::with_capacity(ids.len());
    for id in ids {
        let category = warden_core::autonomy::Category::parse(id).ok_or_else(|| format!("'{id}' is not a kind of action an agent can be asked to get approved"))?;
        if !categories.contains(&category) {
            categories.push(category);
        }
    }
    Ok(categories)
}

/// Trims each agent and refuses a nameless or repeated one, or one whose default model names
/// neither a provider nor a combo.
pub fn check_agents(agents: Vec<AgentConfig>, providers: &[ProviderConfig], combos: &[ComboConfig]) -> Result<Vec<AgentConfig>, String> {
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
            if !is_model(pid, providers, combos) {
                return Err(format!("agent '{id}' has an unknown default provider '{pid}'"));
            }
        }
        if warden_core::autonomy::Autonomy::from_level(a.autonomy).is_none() {
            return Err(format!("agent '{id}' has autonomy {}: pick a level from 1 to 4", a.autonomy));
        }
        // P120: a blank role or superior is none, and a stray space doesn't make a different id.
        let role = a.role.as_deref().and_then(non_empty);
        let reports_to = a.reports_to.as_deref().and_then(non_empty);
        // P123: the models it may pick for its delegations, trimmed and without repeats, in the order given (the first is the default).
        // A member's agent has none: only the owner limits models. Whether each still exists is looked at when a task is delegated.
        let delegation_models = if a.owner.is_none() { check_delegation_models(&a.delegation_models) } else { Vec::new() };
        checked.push(AgentConfig { id, provider_id, role, reports_to, delegation_models, ..a });
    }
    crate::org::check_hierarchy(&checked)?;
    Ok(checked)
}

/// The models an agent may pick for its delegations (P123), trimmed and without repeats, in the order given (the first is the default).
/// Whether each still exists is looked at by the caller: a save prunes what is gone, the organization edit refuses it.
pub fn check_delegation_models(models: &[String]) -> Vec<String> {
    let mut checked: Vec<String> = Vec::new();
    for model in models.iter().filter_map(|m| non_empty(m)) {
        if !checked.contains(&model) {
            checked.push(model);
        }
    }
    checked
}

/// The model policies (P123), trimmed: a unique name that isn't also a provider's or a combo's, answered by one that is, and with a
/// description of at most `MAX_POLICY_DESCRIPTION` characters (what an agent reads to know when to pick it).
pub fn check_policies(policies: Vec<ModelPolicyConfig>, providers: &[ProviderConfig], combos: &[ComboConfig]) -> Result<Vec<ModelPolicyConfig>, String> {
    let mut checked: Vec<ModelPolicyConfig> = Vec::with_capacity(policies.len());
    for policy in policies {
        let id = policy.id.trim().to_string();
        if id.is_empty() {
            return Err("every model policy needs a name".to_string());
        }
        if is_model(&id, providers, combos) {
            return Err(format!("model policy '{id}' has the same name as a provider or a combo"));
        }
        if checked.iter().any(|p| p.id == id) {
            return Err(format!("duplicate model policy name: {id}"));
        }
        let model = policy.model.trim().to_string();
        if !is_model(&model, providers, combos) {
            return Err(format!("model policy '{id}' names '{model}', which is not a configured provider or combo"));
        }
        let description = policy.description.trim().to_string();
        if description.chars().count() > MAX_POLICY_DESCRIPTION || description.contains('\n') {
            return Err(format!("the description of model policy '{id}' must be one line of at most {MAX_POLICY_DESCRIPTION} characters"));
        }
        checked.push(ModelPolicyConfig { id, model, description });
    }
    Ok(checked)
}

const MAX_POLICY_DESCRIPTION: usize = 200;

/// The combos (P90), trimmed: a unique name that isn't also a provider's, and at least one
/// provider, each one configured and none twice.
pub fn check_combos(combos: Vec<ComboConfig>, providers: &[ProviderConfig]) -> Result<Vec<ComboConfig>, String> {
    let mut checked: Vec<ComboConfig> = Vec::with_capacity(combos.len());
    for combo in combos {
        let id = combo.id.trim().to_string();
        if id.is_empty() {
            return Err("every combo needs a name".to_string());
        }
        if providers.iter().any(|p| p.id == id) {
            return Err(format!("combo '{id}' has the same name as a provider"));
        }
        if checked.iter().any(|c| c.id == id) {
            return Err(format!("duplicate combo name: {id}"));
        }
        let mut members: Vec<String> = Vec::with_capacity(combo.providers.len());
        for member in combo.providers {
            let member = member.trim().to_string();
            if member.is_empty() {
                continue;
            }
            if !providers.iter().any(|p| p.id == member) {
                return Err(format!("combo '{id}' names '{member}', which is not one of the configured providers"));
            }
            if members.contains(&member) {
                return Err(format!("combo '{id}' lists '{member}' twice"));
            }
            members.push(member);
        }
        if members.is_empty() {
            return Err(format!("combo '{id}' needs at least one provider"));
        }
        checked.push(ComboConfig { id, providers: members });
    }
    Ok(checked)
}

/// An empty `active` means none; anything else must be a provider or a combo.
pub fn check_active_provider(active: &str, providers: &[ProviderConfig], combos: &[ComboConfig]) -> Result<Option<String>, String> {
    let active = non_empty(active);
    if let Some(id) = &active {
        if !is_model(id, providers, combos) {
            return Err(format!("active model '{id}' is neither a configured provider nor a combo"));
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
    for (var, what) in [
        ("WARDEN_ENABLE_SHELL", "the shell setting"),
        ("WARDEN_DELEGATE_MAX_DEPTH", "the delegation depth"),
        ("WARDEN_MAX_DELEGATED_CALLS", "the delegated-calls ceiling"),
        ("WARDEN_MAX_PARALLEL_JOBS", "the parallel jobs"),
    ] {
        if std::env::var(var).is_ok_and(|v| !v.is_empty()) {
            notes.push(format!("{var} is set in the hub's environment and wins over {what} saved here."));
        }
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
                node: p.node.clone().unwrap_or_default(),
            })
            .collect(),
        active_provider: config.active_provider.clone().unwrap_or_default(),
        combos: config.combos.iter().map(|c| ComboDto { id: c.id.clone(), providers: c.providers.clone() }).collect(),
        model_policies: config.model_policies.iter().map(|p| ModelPolicyDto { id: p.id.clone(), model: p.model.clone(), description: p.description.clone() }).collect(),
        // P84: members' own agents are theirs — the owner's screen neither shows nor saves them.
        agents: config
            .agents
            .iter()
            .filter(|a| a.owner.is_none())
            .map(|a| AgentSettingsDto {
                original_id: None,
                id: a.id.clone(),
                persona: a.persona.clone(),
                provider_id: a.provider_id.clone().unwrap_or_default(),
                can_delegate_to_agents: a.can_delegate_to_agents,
                can_manage_agents: a.can_manage_agents,
                can_message_agents: a.can_message_agents,
                can_manage_tasks: a.can_manage_tasks,
                allowed_tools: a.allowed_tools.clone(),
                autonomy: a.autonomy,
                approval_required: a.approval_required.iter().map(|c| c.as_str().to_string()).collect(),
                role: a.role.clone(),
                reports_to: a.reports_to.clone(),
                shared_with: a.shared_with.clone(),
                owner: None,
                delegation_models: a.delegation_models.clone(),
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
        git_sync: GitSyncSettingsDto {
            remote_url: config.git_sync.as_ref().map(|g| g.remote_url.clone()).unwrap_or_default(),
            token: secret_status(config.git_sync.as_ref().map(|g| g.token.as_str()).filter(|t| !t.is_empty())),
        },
        bots: bots_settings(config),
        telegram_token: secret_status(config.api_keys.telegram_bot_token.as_deref()),
        advanced: advanced_settings(config),
        machine: machine_settings(config),
        notes,
    }
}

/// What the screens show of `[learning]` and the bots' lists (P118).
pub fn bots_settings(config: &FileConfig) -> BotsSettingsDto {
    BotsSettingsDto {
        learning_enabled: config.learning.enabled,
        learning_provider: config.learning.provider.clone().unwrap_or_default(),
        learning_max_per_day: config.learning.max_per_day,
        learning_bot_chats: config.learning.bot_chats.clone(),
        telegram_allowed_users: config.telegram.allowed_users.clone(),
        whatsapp_allowed_chats: config.whatsapp.allowed_chats.clone(),
        telegram_pairing: config.telegram.pairing,
        whatsapp_pairing: config.whatsapp.pairing,
    }
}

/// Drops blanks and repeats, keeping the order.
fn tidy_list(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && seen.insert(s.clone())).collect()
}

/// Checks a bots block and writes it into `config`: at least one suggestion a day, a learning model
/// that is a provider or a combo, `bot_chats` as `telegram:<id>` / `whatsapp:<id>`, Telegram ids that
/// are positive numbers (a user, not a group) and WhatsApp ids without spaces. Repeats and blanks go.
pub fn apply_bots_settings(config: &mut FileConfig, dto: BotsSettingsDto, providers: &[ProviderConfig], combos: &[ComboConfig]) -> Result<(), String> {
    if dto.learning_max_per_day == 0 {
        return Err("learning needs at least one suggestion a day (set it off instead)".to_string());
    }
    let provider = non_empty(&dto.learning_provider);
    if let Some(id) = &provider {
        if !is_model(id, providers, combos) {
            return Err(format!("learning model '{id}' is neither a configured provider nor a combo"));
        }
    }
    let bot_chats = tidy_list(dto.learning_bot_chats);
    for entry in &bot_chats {
        let valid = entry.split_once(':').is_some_and(|(channel, id)| matches!(channel, "telegram" | "whatsapp") && !id.is_empty() && !id.contains(char::is_whitespace));
        if !valid {
            return Err(format!("'{entry}' is not a bot chat: write telegram:<id> or whatsapp:<id>"));
        }
    }
    if let Some(id) = dto.telegram_allowed_users.iter().find(|id| **id <= 0) {
        return Err(format!("Telegram user id {id} is not a user: ids are positive numbers"));
    }
    let mut seen = HashSet::new();
    let telegram_users: Vec<i64> = dto.telegram_allowed_users.into_iter().filter(|id| seen.insert(*id)).collect();
    let whatsapp_chats = tidy_list(dto.whatsapp_allowed_chats);
    if let Some(chat) = whatsapp_chats.iter().find(|c| c.contains(char::is_whitespace)) {
        return Err(format!("WhatsApp id '{chat}' has spaces: write the number or the whole id"));
    }

    config.learning = LearningSettings { enabled: dto.learning_enabled, provider, max_per_day: dto.learning_max_per_day, bot_chats };
    // The screens don't edit who speaks as which member (`warden bots link` / `pair approve --as`
    // do): carry the map over so a save never unmaps anyone.
    config.telegram = TelegramSettings { allowed_users: telegram_users, pairing: dto.telegram_pairing, members: std::mem::take(&mut config.telegram.members) };
    config.whatsapp = WhatsAppSettings { allowed_chats: whatsapp_chats, pairing: dto.whatsapp_pairing, members: std::mem::take(&mut config.whatsapp.members) };
    Ok(())
}

/// `[git_sync]` after a web save. An empty URL turns git sync off. Only `https://` is accepted
/// from the web: a path or `file://` would let a paired device point the hub's pushes at any
/// directory on its machine.
fn apply_git_sync(edit: GitSyncEditDto, current: Option<GitSyncConfig>) -> Result<Option<GitSyncConfig>, String> {
    let Some(remote_url) = non_empty(&edit.remote_url) else {
        return Ok(None);
    };
    if !remote_url.starts_with("https://") {
        return Err("the git sync remote must be an https:// URL".to_string());
    }
    let token = apply_secret(edit.token, current.map(|g| g.token).filter(|t| !t.is_empty()))
        .ok_or_else(|| "git sync needs an access token for its remote".to_string())?;
    Ok(Some(GitSyncConfig { remote_url, token }))
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
            node: Some(edit.node),
        });
    }
    let providers = check_providers(providers)?;
    let combos = match update.combos {
        Some(dtos) => check_combos(dtos.into_iter().map(|c| ComboConfig { id: c.id, providers: c.providers }).collect(), &providers)?,
        // Untouched by this screen: a provider this save removed leaves its combos, and a combo
        // left empty goes (the active model and the agents are checked against what remains).
        None => config
            .combos
            .drain(..)
            .map(|mut c| {
                c.providers.retain(|id| providers.iter().any(|p| &p.id == id));
                c
            })
            .filter(|c| !c.providers.is_empty())
            .collect(),
    };
    let active_provider = check_active_provider(&update.active_provider, &providers, &combos)?;
    let model_policies = match update.model_policies {
        Some(dtos) => check_policies(dtos.into_iter().map(|p| ModelPolicyConfig { id: p.id, model: p.model, description: p.description }).collect(), &providers, &combos)?,
        // Untouched by this screen: the ones whose model this save removed go with it.
        None => config.model_policies.drain(..).filter(|p| is_model(&p.model, &providers, &combos)).collect(),
    };

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
            can_message_agents: dto.can_message_agents,
            can_manage_tasks: dto.can_manage_tasks,
            allowed_tools: dto.allowed_tools,
            autonomy: dto.autonomy,
            approval_required: categories_from_ids(&dto.approval_required)?,
            role: dto.role,
            reports_to: dto.reports_to,
            owner: None,
            shared_with: crate::users::clean_shares(dto.shared_with, &config.users),
            delegation_models: dto.delegation_models,
        });
    }
    let kept: HashSet<String> = renames.iter().map(|(original, _)| original.clone()).chain(agents.iter().map(|a| a.id.trim().to_string())).collect();
    // P84: the members' own agents aren't on this screen; they're kept as they are, and a name they
    // hold can't be taken by one of the owner's.
    let member_agents: Vec<AgentConfig> = config.agents.iter().filter(|a| a.owner.is_some()).cloned().collect();
    // `check_agents` refuses a repeated name, so an agent of the owner's can't take one of theirs.
    // P120: whoever reported to a renamed agent follows its new name, before the hierarchy is checked.
    let mut agents: Vec<AgentConfig> = agents;
    for (original, new) in &renames {
        crate::org::rename_in_reports(&mut agents, original, new.trim());
    }
    let agents = check_agents(agents.into_iter().chain(member_agents).collect(), &providers, &combos)?;

    let removed: Vec<String> = config.agents.iter().filter(|a| a.owner.is_none()).map(|a| a.id.clone()).filter(|id| !kept.contains(id)).collect();
    let mut scratch = config.agents.clone();
    for id in &removed {
        remove_agent_from(&mut scratch, &mut config.ssh_hosts, id);
        forget_agent_in_nodes(&mut config.nodes, id);
    }
    let node_lists = config.nodes.iter_mut().map(|n| &mut n.agents);
    for list in config.ssh_hosts.iter_mut().map(|h| &mut h.agents).chain(node_lists) {
        for name in list {
            if let Some((_, new)) = renames.iter().find(|(original, _)| original == name) {
                *name = new.clone();
            }
        }
    }

    config.limits = update.limits.map(limits_into_config).transpose()?;
    config.prices = prices_into_config(update.prices)?;
    config.api_keys.tavily = apply_secret(update.tavily_key, config.api_keys.tavily.take());
    config.api_keys.whisper = apply_secret(update.whisper_key, config.api_keys.whisper.take());
    config.api_keys.telegram_bot_token = apply_secret(update.telegram_token, config.api_keys.telegram_bot_token.take());
    if let Some(edit) = update.git_sync {
        config.git_sync = apply_git_sync(edit, config.git_sync.take())?;
    }
    if let Some(advanced) = update.advanced {
        apply_advanced(&mut config, *advanced)?;
    }
    // After the agents above were renamed or removed in the SSH hosts the file had: a list that comes in
    // replaces those, and is checked against the agents as this save leaves them.
    if let Some(machine) = update.machine {
        apply_machine(&mut config, *machine, &agents)?;
    }

    // Same as the desktop's save: once the registry holds a provider, the legacy single-provider
    // fields are only stale duplicate secrets.
    if !providers.is_empty() {
        config.provider = None;
        config.model = None;
        config.api_keys.gemini = None;
        config.api_keys.openai = None;
    }
    if let Some(bots) = update.bots {
        apply_bots_settings(&mut config, bots, &providers, &combos)?;
    }
    config.providers = providers;
    config.active_provider = active_provider;
    config.combos = combos;
    config.model_policies = model_policies;
    config.agents = agents;
    crate::prune_delegation_models(&mut config);
    Ok(config)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_node_model_needs_its_node_and_its_provider_there() {
        let node = |node: Option<&str>, model: Option<&str>| ProviderConfig {
            id: "casa".into(),
            kind: Provider::Node,
            api_key: Some("ignored".into()),
            base_url: None,
            model: model.map(str::to_string),
            node: node.map(str::to_string),
        };
        assert!(check_providers(vec![node(None, Some("ollama"))]).is_err());
        assert!(check_providers(vec![node(Some("node-casa"), None)]).is_err());
        let checked = check_providers(vec![node(Some(" node-casa "), Some("ollama"))]).unwrap();
        assert_eq!((checked[0].node.as_deref(), checked[0].model.as_deref()), (Some("node-casa"), Some("ollama")));
        assert_eq!(provider_kind_from_str("node").unwrap(), Provider::Node);
        assert_eq!(provider_kind_str(Provider::Node), "node");

        // A `node` left on another kind (the kind was switched in a form) doesn't stick.
        let other = ProviderConfig { kind: Provider::Gemini, ..node(Some("node-casa"), Some("gemini-3.5-flash")) };
        assert_eq!(check_providers(vec![other]).unwrap()[0].node, None);

        let toml_text = "[[providers]]\nid = \"casa\"\nkind = \"node\"\nnode = \"node-casa\"\nmodel = \"ollama\"\n";
        let config: FileConfig = toml::from_str(toml_text).unwrap();
        assert_eq!((config.providers[0].kind, config.providers[0].node.as_deref()), (Provider::Node, Some("node-casa")));
    }

    use super::*;
    use crate::SshHostConfig;
    use warden_server_protocol::protocol::{AdvancedSettingsDto, MachineEditDto, ProviderEditDto};

    fn provider(id: &str, key: Option<&str>) -> ProviderConfig {
        ProviderConfig { id: id.into(), kind: Provider::Gemini, api_key: key.map(Into::into), base_url: None, model: None, node: None }
    }

    fn agent(id: &str) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            persona: "p".into(),
            provider_id: None,
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            autonomy: crate::default_autonomy(),
            approval_required: Vec::new(),
            role: None,
            reports_to: None,
            owner: None,
            shared_with: Vec::new(),
            delegation_models: Vec::new(),
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
                .map(|p| ProviderEditDto { original_id: Some(p.id.clone()), id: p.id, kind: p.kind, base_url: p.base_url, model: p.model, api_key: SecretEdit::Keep, node: String::new() })
                .collect(),
            active_provider: view.active_provider,
            agents: view.agents.into_iter().map(|a| AgentSettingsDto { original_id: Some(a.id.clone()), ..a }).collect(),
            tavily_key: SecretEdit::Keep,
            whisper_key: SecretEdit::Keep,
            limits: view.limits,
            prices: view.prices,
            git_sync: None,
            combos: None,
            model_policies: None,
            bots: None,
            telegram_token: SecretEdit::Keep,
            advanced: None,
            machine: None,
        }
    }

    fn bots() -> BotsSettingsDto {
        BotsSettingsDto {
            learning_enabled: true,
            learning_provider: "spare".into(),
            learning_max_per_day: 5,
            learning_bot_chats: vec![" telegram:42 ".into(), "telegram:42".into(), "whatsapp:5511999999999@s.whatsapp.net".into()],
            telegram_allowed_users: vec![42, 7, 42],
            whatsapp_allowed_chats: vec!["5511999999999".into(), " ".into()],
            telegram_pairing: true,
            whatsapp_pairing: false,
        }
    }

    /// P118: the learning and bot lists are saved, tidied, and only when the save carries them.
    #[test]
    fn a_save_with_bots_writes_them_tidied_and_one_without_keeps_them() {
        let mut update = untouched(&sample());
        update.bots = Some(bots());
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.learning, LearningSettings { enabled: true, provider: Some("spare".into()), max_per_day: 5, bot_chats: vec!["telegram:42".into(), "whatsapp:5511999999999@s.whatsapp.net".into()] });
        assert_eq!(saved.telegram.allowed_users, [42, 7]);
        assert_eq!(saved.whatsapp.allowed_chats, ["5511999999999"]);
        assert!(saved.telegram.pairing && !saved.whatsapp.pairing);
        assert_eq!(bots_settings(&saved).learning_bot_chats.len(), 2);

        let view = hub_settings(&saved, Vec::new(), Vec::new());
        assert_eq!(view.bots.telegram_allowed_users, [42, 7]);
        let (learning, telegram, whatsapp) = (saved.learning.clone(), saved.telegram.clone(), saved.whatsapp.clone());
        let update = untouched(&saved);
        let again = apply_hub_settings(saved, update).unwrap();
        assert_eq!(again.learning, learning, "a save that doesn't carry bots keeps them");
        assert_eq!(again.telegram, telegram);
        assert_eq!(again.whatsapp, whatsapp);
    }

    /// P117: who speaks as which member is set by `warden bots`, not by the screens: a save from the web
    /// or the desktop, with or without the bots slice, never unmaps a chat or forgets the hub.
    #[test]
    fn a_save_never_unmaps_a_chat_that_speaks_as_a_member() {
        let mut config = sample();
        config.telegram.members.insert("42".into(), "ana".into());
        config.whatsapp.members.insert("5511999999999".into(), "bia".into());
        config.bot_hub = Some(crate::bot_access::BotHubSettings { url: "ws://192.168.0.5:7420".into() });

        let mut update = untouched(&config);
        update.bots = Some(bots());
        let saved = apply_hub_settings(config, update).unwrap();
        assert_eq!(saved.telegram.member_for(42), Some("ana"), "a save that carries the bots slice keeps the map");
        assert_eq!(saved.whatsapp.members.get("5511999999999").map(String::as_str), Some("bia"));
        assert_eq!(saved.bot_hub.as_ref().map(|h| h.url.as_str()), Some("ws://192.168.0.5:7420"));

        let update = untouched(&saved);
        let again = apply_hub_settings(saved, update).unwrap();
        assert_eq!(again.telegram.member_for(42), Some("ana"), "and so does one that doesn't");
        assert!(again.bot_hub.is_some());
    }

    #[test]
    fn bad_bots_settings_are_refused_and_nothing_is_written() {
        let refuse = |edit: fn(&mut BotsSettingsDto)| {
            let mut dto = bots();
            edit(&mut dto);
            let mut update = untouched(&sample());
            update.bots = Some(dto);
            apply_hub_settings(sample(), update).is_err()
        };
        assert!(refuse(|b| b.learning_max_per_day = 0));
        assert!(refuse(|b| b.learning_provider = "ghost".into()));
        assert!(refuse(|b| b.learning_bot_chats = vec!["signal:1".into()]));
        assert!(refuse(|b| b.learning_bot_chats = vec!["telegram:".into()]));
        assert!(refuse(|b| b.telegram_allowed_users = vec![-100123]));
        assert!(refuse(|b| b.whatsapp_allowed_chats = vec!["55 11 9".into()]));
        assert!(!refuse(|b| b.learning_provider = String::new()), "empty means the active model");
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

    /// P84: the owner's screen neither shows nor loses the members' own agents, and shares are kept
    /// to members that exist.
    #[test]
    fn a_save_keeps_the_members_own_agents_and_cleans_the_shares() {
        let mut config = sample();
        crate::users::add_user(&mut config, "ana", "Ana", "temporary-1").unwrap();
        config.agents.push(AgentConfig { owner: Some("ana".into()), ..agent("anas-helper") });
        let view = hub_settings(&config, Vec::new(), Vec::new());
        assert!(!view.agents.iter().any(|a| a.id == "anas-helper"), "not on the owner's screen");

        let mut update = untouched(&config);
        update.agents[0].shared_with = vec!["ana".into(), "ghost".into(), "ana".into()];
        update.agents.remove(1); // the owner deletes "chef"
        let mut clash = untouched(&config);
        clash.agents.push(AgentSettingsDto { original_id: None, id: "anas-helper".into(), ..clash.agents[0].clone() });
        let saved = apply_hub_settings(config, update).unwrap();
        let ids: Vec<&str> = saved.agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["pirate", "anas-helper"]);
        assert_eq!(saved.agents[0].shared_with, ["ana"], "unknown people and repeats dropped");
        assert_eq!(saved.agents[1].owner.as_deref(), Some("ana"));

        // A name one of Ana's agents holds can't be taken by one of the owner's.
        let mut again = sample();
        crate::users::add_user(&mut again, "ana", "Ana", "temporary-1").unwrap();
        again.agents.push(AgentConfig { owner: Some("ana".into()), ..agent("anas-helper") });
        assert!(apply_hub_settings(again, clash).is_err());
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
            node: String::new(),
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

    /// P119: the Telegram token is shown as set (never itself), and a save keeps, replaces or clears it.
    #[test]
    fn the_telegram_token_is_kept_replaced_or_cleared_and_never_shown() {
        let view = hub_settings(&sample(), Vec::new(), Vec::new());
        assert!(view.telegram_token.set);
        assert!(!serde_json::to_string(&view).unwrap().contains("\"tg\""), "the token reached the screen");
        let token_after = |edit: SecretEdit| {
            let mut update = untouched(&sample());
            update.telegram_token = edit;
            apply_hub_settings(sample(), update).unwrap().api_keys.telegram_bot_token
        };
        assert_eq!(token_after(SecretEdit::Keep).as_deref(), Some("tg"));
        assert_eq!(token_after(SecretEdit::Set(" 123:abc ".into())).as_deref(), Some("123:abc"));
        assert_eq!(token_after(SecretEdit::Clear), None);
    }

    /// P119: a save without the `advanced` and `machine` slices touches none of what they hold, and the
    /// view shows it all.
    #[test]
    fn a_save_without_the_machine_and_advanced_slices_carries_them_over() {
        let mut config = sample();
        config.generated_path = Some("/srv/generated".into());
        config.mcp_servers = vec![crate::McpServerConfig::Stdio { name: "notes".into(), command: "npx".into(), args: Vec::new(), env: [("TOKEN".to_string(), "s3cret".to_string())].into() }];
        config.delegate_max_depth = Some(9);
        config.truthid_public_url = Some("https://hub.example.com".into());
        let view = hub_settings(&config, Vec::new(), Vec::new());
        assert_eq!((view.machine.vault_path.as_str(), view.machine.generated_path.as_str(), view.machine.ssh_hosts.len(), view.machine.mcp_servers.len()), ("/somewhere", "/srv/generated", 2, 1));
        assert_eq!((view.advanced.delegate_max_depth, view.advanced.truthid_public_url.as_str()), (Some(9), "https://hub.example.com"));
        assert!(!serde_json::to_string(&view).unwrap().contains("s3cret"));

        let saved = apply_hub_settings(config, untouched(&sample())).unwrap();
        assert_eq!(saved.generated_path.as_deref(), Some("/srv/generated"));
        assert_eq!(saved.mcp_servers.len(), 1);
        assert_eq!((saved.delegate_max_depth, saved.truthid_public_url.as_deref()), (Some(9), Some("https://hub.example.com")));
    }

    /// P119: the machine slice replaces what it holds, and its SSH hosts are checked against the agents as
    /// this same save leaves them: a host may name an agent renamed in the save, never a removed one.
    #[test]
    fn a_machine_save_replaces_its_parts_and_checks_the_hosts_against_the_agents_it_leaves() {
        let config = sample();
        let machine_of = |config: &FileConfig| {
            let view = machine_settings(config);
            MachineEditDto { enable_shell: view.enable_shell, vault_path: view.vault_path, generated_path: view.generated_path, mcp_servers: Vec::new(), ssh_hosts: view.ssh_hosts, embedded_server: None }
        };
        let mut update = untouched(&config);
        update.agents[0].id = "captain".into(); // pirate renamed
        let mut machine = machine_of(&config);
        machine.enable_shell = false;
        machine.ssh_hosts.truncate(1);
        machine.ssh_hosts[0].agents = vec!["captain".into()];
        update.machine = Some(Box::new(machine));
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.enable_shell, Some(false));
        assert_eq!(saved.ssh_hosts.len(), 1, "the list that came in replaced the file's");
        assert_eq!(saved.ssh_hosts[0].agents, ["captain"]);

        let mut update = untouched(&config);
        update.agents.retain(|a| a.id != "chef");
        let mut machine = machine_of(&config);
        machine.ssh_hosts[1].agents = vec!["chef".into()];
        update.machine = Some(Box::new(machine));
        let error = apply_hub_settings(sample(), update).unwrap_err();
        assert!(error.contains("unknown agent 'chef'"), "{error}");
    }

    /// P119: the delegation and TruthID block is saved only when it comes, and a refused value changes nothing.
    #[test]
    fn the_advanced_block_is_saved_when_it_comes_and_a_refused_one_is_an_error() {
        let mut update = untouched(&sample());
        update.advanced = Some(Box::new(AdvancedSettingsDto { delegate_max_depth: Some(3), max_delegated_calls: Some(50), max_parallel_jobs: None, truthid_network: "base-sepolia".into(), truthid_rpc_url: String::new(), truthid_public_url: "https://hub.example.com".into() }));
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!((saved.delegate_max_depth, saved.max_delegated_calls, saved.max_parallel_jobs), (Some(3), Some(50), None));
        assert_eq!(saved.truthid_public_url.as_deref(), Some("https://hub.example.com"));

        let mut update = untouched(&sample());
        update.advanced = Some(Box::new(AdvancedSettingsDto { delegate_max_depth: Some(50), truthid_network: "base-mainnet".into(), ..AdvancedSettingsDto::default() }));
        assert!(apply_hub_settings(sample(), update).unwrap_err().contains("depth"));
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
    fn an_agent_autonomy_is_a_level_from_one_to_four() {
        for level in 1..=4 {
            assert!(check_agents(vec![AgentConfig { autonomy: level, ..agent("a") }], &[], &[]).is_ok());
        }
        for level in [0, 5, 200] {
            let err = check_agents(vec![AgentConfig { autonomy: level, ..agent("a") }], &[], &[]).unwrap_err();
            assert!(err.contains("autonomy") && err.contains("1 to 4"), "{err}");
        }
    }

    #[test]
    fn the_categories_an_agent_asks_approval_for_round_trip_and_an_unknown_one_is_refused() {
        use warden_core::autonomy::Category;
        let old: FileConfig = toml::from_str("[[agents]]\nid = \"pirate\"\npersona = \"p\"\n").unwrap();
        assert!(old.agents[0].approval_required.is_empty(), "no categories asked for, as before");
        let with: FileConfig = toml::from_str("[[agents]]\nid = \"a\"\npersona = \"p\"\napproval_required = [\"critical_infra\", \"delete_data\"]\n").unwrap();
        assert_eq!(with.agents[0].approval_required, [Category::CriticalInfra, Category::DeleteData]);

        let mut update = untouched(&sample());
        update.agents[0].approval_required = vec!["external_message".into(), "external_message".into(), "publish_code".into()];
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.agents[0].approval_required, [Category::ExternalMessage, Category::PublishCode], "each once, in order");
        assert_eq!(hub_settings(&saved, Vec::new(), Vec::new()).agents[0].approval_required, ["external_message", "publish_code"]);

        let mut bad = untouched(&sample());
        bad.agents[0].approval_required = vec!["everything".into()];
        let err = apply_hub_settings(sample(), bad).unwrap_err();
        assert!(err.contains("'everything'"), "{err}");
    }

    #[test]
    fn a_role_and_a_superior_round_trip_and_a_blank_one_is_none() {
        let old: FileConfig = toml::from_str("[[agents]]\nid = \"pirate\"\npersona = \"p\"\n").unwrap();
        assert_eq!((old.agents[0].role.clone(), old.agents[0].reports_to.clone()), (None, None));

        let mut update = untouched(&sample());
        update.agents[1].role = Some("  First mate ".into());
        update.agents[1].reports_to = Some("pirate".into());
        update.agents[0].role = Some("   ".into());
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!((saved.agents[1].role.as_deref(), saved.agents[1].reports_to.as_deref()), (Some("First mate"), Some("pirate")));
        assert_eq!(saved.agents[0].role, None, "a blank role is none");
        let view = hub_settings(&saved, Vec::new(), Vec::new());
        assert_eq!((view.agents[1].role.as_deref(), view.agents[1].reports_to.as_deref()), (Some("First mate"), Some("pirate")));
    }

    #[test]
    fn a_save_with_a_superior_that_does_not_exist_or_a_circle_is_refused() {
        let mut ghost = untouched(&sample());
        ghost.agents[1].reports_to = Some("ghost".into());
        assert!(apply_hub_settings(sample(), ghost).unwrap_err().contains("doesn't exist"));

        let mut circle = untouched(&sample());
        circle.agents[0].reports_to = Some("chef".into());
        circle.agents[1].reports_to = Some("pirate".into());
        assert!(apply_hub_settings(sample(), circle).unwrap_err().contains("circle"));
    }

    #[test]
    fn renaming_a_superior_carries_the_reports_with_it() {
        let mut base = sample();
        base.agents[1].reports_to = Some("pirate".into());
        let mut update = untouched(&base);
        update.agents[0].id = "captain".into();
        let saved = apply_hub_settings(base, update).unwrap();
        assert_eq!(saved.agents[1].reports_to.as_deref(), Some("captain"));
    }

    #[test]
    fn removing_an_agent_hands_its_reports_to_its_superior() {
        let mut config = sample();
        config.agents.push(agent("cook"));
        config.agents[1].reports_to = Some("pirate".into());
        config.agents[2].reports_to = Some("chef".into());
        crate::remove_agent_references(&mut config, "chef");
        assert_eq!(config.agents.len(), 2);
        assert_eq!(config.agents[1].reports_to.as_deref(), Some("pirate"), "cook now reports to chef's superior");
        assert!(crate::org::check_hierarchy(&config.agents).is_ok());
    }

    #[test]
    fn an_agent_written_before_levels_existed_loads_at_four_and_a_chosen_level_survives_a_save() {
        let old: FileConfig = toml::from_str("[[agents]]\nid = \"pirate\"\npersona = \"p\"\n").unwrap();
        assert_eq!(old.agents[0].autonomy, 4);

        let mut update = untouched(&sample());
        update.agents[0].autonomy = 2;
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.agents[0].autonomy, 2);
        assert_eq!(hub_settings(&saved, Vec::new(), Vec::new()).agents[0].autonomy, 2);
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
            node: String::new(),
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
        assert!(refuse(|u| u.active_provider = "ghost".into()).contains("active model 'ghost'"));
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

    #[test]
    fn git_sync_is_shown_without_its_token_and_saved_only_over_https() {
        let with_git = || {
            let mut config = sample();
            config.git_sync = Some(GitSyncConfig { remote_url: "https://git.example/v.git".into(), token: "ghp_very-long-token-abcd".into() });
            config
        };
        let config = with_git();
        let view = hub_settings(&config, Vec::new(), Vec::new());
        assert_eq!(view.git_sync.remote_url, "https://git.example/v.git");
        assert_eq!(view.git_sync.token, SecretStatusDto { set: true, hint: Some("abcd".into()) });
        assert!(!serde_json::to_string(&view).unwrap().contains("ghp_"));

        // Absent: untouched.
        let saved = apply_hub_settings(with_git(), untouched(&config)).unwrap();
        assert_eq!(saved.git_sync, config.git_sync);

        // A new URL keeps the saved token.
        let mut update = untouched(&config);
        update.git_sync = Some(GitSyncEditDto { remote_url: " https://other.example/v.git ".into(), token: SecretEdit::Keep });
        let saved = apply_hub_settings(with_git(), update).unwrap();
        assert_eq!(saved.git_sync, Some(GitSyncConfig { remote_url: "https://other.example/v.git".into(), token: "ghp_very-long-token-abcd".into() }));

        // An empty URL turns it off.
        let mut update = untouched(&config);
        update.git_sync = Some(GitSyncEditDto { remote_url: String::new(), token: SecretEdit::Keep });
        assert_eq!(apply_hub_settings(with_git(), update).unwrap().git_sync, None);

        // Never a local path, and never without a token.
        for url in ["/srv/vault.git", "file:///srv/vault.git", "ssh://git@host/v.git"] {
            let mut update = untouched(&config);
            update.git_sync = Some(GitSyncEditDto { remote_url: url.into(), token: SecretEdit::Keep });
            assert!(apply_hub_settings(with_git(), update).unwrap_err().contains("https://"), "{url}");
        }
        let mut update = untouched(&sample());
        update.git_sync = Some(GitSyncEditDto { remote_url: "https://git.example/v.git".into(), token: SecretEdit::Keep });
        assert!(apply_hub_settings(sample(), update).unwrap_err().contains("token"));
    }

    #[test]
    fn combos_are_shown_checked_usable_as_the_active_model_and_follow_a_removed_provider() {
        let combo = |id: &str, members: &[&str]| ComboDto { id: id.into(), providers: members.iter().map(|m| m.to_string()).collect() };
        let mut config = sample();
        config.combos = vec![ComboConfig { id: "fast".into(), providers: vec!["main".into(), "spare".into()] }];
        assert_eq!(hub_settings(&config, Vec::new(), Vec::new()).combos, vec![combo("fast", &["main", "spare"])]);

        // A combo can be the active model and an agent's default.
        let mut update = untouched(&config);
        update.combos = Some(vec![combo(" fast ", &[" spare ", "main"])]);
        update.active_provider = "fast".into();
        update.agents[0].provider_id = "fast".into();
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.combos, vec![ComboConfig { id: "fast".into(), providers: vec!["spare".into(), "main".into()] }]);
        assert_eq!(saved.active_provider.as_deref(), Some("fast"));

        for bad in [
            vec![combo("main", &["spare"])],
            vec![combo("x", &[])],
            vec![combo("x", &["ghost"])],
            vec![combo("x", &["main", "main"])],
            vec![combo("x", &["main"]), combo("x", &["spare"])],
            vec![combo(" ", &["main"])],
        ] {
            let mut update = untouched(&config);
            update.combos = Some(bad.clone());
            assert!(apply_hub_settings(sample(), update).is_err(), "{bad:?}");
        }
        let mut update = untouched(&config);
        update.active_provider = "nope".into();
        assert!(apply_hub_settings(sample(), update).is_err());

        // Not sent: kept, minus a provider the same save removed; a combo left empty goes.
        let with = |members: Vec<String>| {
            let mut c = sample();
            c.combos = vec![ComboConfig { id: "fast".into(), providers: members }];
            c
        };
        let mut update = untouched(&config);
        update.providers.retain(|p| p.id != "spare");
        assert_eq!(apply_hub_settings(with(vec!["main".into(), "spare".into()]), update.clone()).unwrap().combos[0].providers, vec!["main".to_string()]);
        assert!(apply_hub_settings(with(vec!["spare".into()]), update).unwrap().combos.is_empty());
    }

    #[test]
    fn model_policies_are_shown_checked_and_kept_when_not_sent_and_an_agents_limit_follows_them() {
        let policy = |id: &str, model: &str, description: &str| ModelPolicyDto { id: id.into(), model: model.into(), description: description.into() };
        let mut config = sample();
        config.model_policies = vec![ModelPolicyConfig { id: "fast".into(), model: "main".into(), description: "simple work".into() }];
        assert_eq!(hub_settings(&config, Vec::new(), Vec::new()).model_policies, vec![policy("fast", "main", "simple work")]);

        // Sent: trimmed and saved; an agent can be limited to one, and the first of its list is kept as it is.
        let mut update = untouched(&config);
        update.model_policies = Some(vec![policy(" fast ", " spare ", " cheap and quick "), policy("deep", "main", "")]);
        update.agents[0].delegation_models = vec![" deep ".into(), "fast".into(), "deep".into(), "ghost-model".into(), "  ".into()];
        let saved = apply_hub_settings(sample(), update).unwrap();
        assert_eq!(saved.model_policies, vec![
            ModelPolicyConfig { id: "fast".into(), model: "spare".into(), description: "cheap and quick".into() },
            ModelPolicyConfig { id: "deep".into(), model: "main".into(), description: String::new() },
        ]);
        assert_eq!(saved.agents[0].delegation_models, ["deep", "fast"], "trimmed, no repeats, and what doesn't exist is dropped");

        for bad in [
            vec![policy("main", "spare", "")],
            vec![policy("x", "ghost", "")],
            vec![policy(" ", "main", "")],
            vec![policy("x", "main", ""), policy("x", "spare", "")],
            vec![policy("x", "main", "two\nlines")],
            vec![policy("x", "main", &"d".repeat(201))],
        ] {
            let mut update = untouched(&config);
            update.model_policies = Some(bad.clone());
            assert!(apply_hub_settings(sample(), update).is_err(), "{bad:?}");
        }

        // Not sent: kept, minus a policy whose model this save removed (and the agent's limit on it).
        let base = || {
            let mut base = sample();
            base.model_policies = vec![ModelPolicyConfig { id: "fast".into(), model: "spare".into(), description: String::new() }];
            base.agents[0].delegation_models = vec!["fast".into(), "main".into()];
            base
        };
        let kept = apply_hub_settings(base(), untouched(&base())).unwrap();
        assert_eq!(kept.model_policies.len(), 1);
        assert_eq!(kept.agents[0].delegation_models, ["fast", "main"]);
        let mut update = untouched(&base());
        update.providers.retain(|p| p.id != "spare");
        let after = apply_hub_settings(base(), update).unwrap();
        assert!(after.model_policies.is_empty());
        assert_eq!(after.agents[0].delegation_models, ["main"]);
    }
}
