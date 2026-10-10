//! Editing the organization of the owner's agents from the tree (P120): a person moves an agent, gives it a role, adds a report or
//! removes one, and nothing else of the settings is touched. The same hierarchy checks as a settings save (`check_hierarchy`: no
//! circle, no missing superior), applied to a copy first, so a refused edit changes nothing.
//!
//! The same narrow door takes the two edits of P123 the light clients (phone, browser extension) make without the whole settings form:
//! the models an agent may delegate with, and the named model policies. They reuse the checks of a save (`check_policies`,
//! `check_delegation_models`).

use warden_core::autonomy::Category;
use warden_server_protocol::protocol::AgentOrgEdit;

use crate::manage_agents::{check_id, check_persona, check_role, ASK_FIRST_LEVEL};
use crate::settings::{check_delegation_models, check_policies};
use crate::{org, prune_delegation_models, remove_agent_references, AgentConfig, FileConfig, ModelPolicyConfig, SAFE_AGENT_TOOLS};

fn blank_is_none(value: &Option<String>) -> Option<String> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

/// Applies `edit` to `config`, or says why not and leaves it as it was. Only the owner's agents are in the tree: a member's agent is
/// theirs alone (P84), so it can't be moved, removed or taken as a superior from here.
pub fn apply_org_edit(config: &mut FileConfig, edit: &AgentOrgEdit) -> Result<(), String> {
    let mut agents = config.agents.clone();
    let owners = |agents: &[AgentConfig], id: &str| agents.iter().any(|a| a.owner.is_none() && a.id == id);
    match edit {
        AgentOrgEdit::SetPosition { id, role, reports_to } => {
            let role = check_role(role.as_deref()).map_err(|e| e.to_string())?;
            let superior = blank_is_none(reports_to);
            let agent = agents.iter_mut().find(|a| a.owner.is_none() && &a.id == id).ok_or_else(|| format!("no agent named '{id}'"))?;
            agent.role = role;
            agent.reports_to = superior;
        }
        AgentOrgEdit::AddReport { id, persona, role, reports_to } => {
            check_id(id).map_err(|e| e.to_string())?;
            check_persona(persona).map_err(|e| e.to_string())?;
            let role = check_role(role.as_deref()).map_err(|e| e.to_string())?;
            let superior = blank_is_none(reports_to);
            if agents.iter().any(|a| &a.id == id) {
                return Err(format!("an agent named '{id}' already exists — pick another name"));
            }
            if superior.as_deref().is_some_and(|s| !owners(&agents, s)) {
                return Err(format!("'{}' isn't one of your agents", superior.unwrap_or_default()));
            }
            agents.push(AgentConfig {
                id: id.clone(),
                persona: persona.clone(),
                provider_id: None,
                // Only a person turns these on, in Settings.
                can_delegate_to_agents: false,
                can_manage_agents: false,
                can_message_agents: false,
                can_manage_tasks: false,
                allowed_tools: Some(SAFE_AGENT_TOOLS.iter().map(|t| t.to_string()).collect()),
                // Careful by default, the same as an agent a manager creates: it asks before every change.
                autonomy: ASK_FIRST_LEVEL,
                approval_required: Category::ALL.to_vec(),
                role,
                reports_to: superior,
                owner: None,
                shared_with: Vec::new(),
                delegation_models: Vec::new(),
                // Cautious like an agent a manager creates: only a person turns these on.
                can_start_tasks: false,
                can_create_workers: false,
            });
        }
        AgentOrgEdit::SetDelegationModels { id, models } => {
            let models = check_delegation_models(models);
            let known: Vec<&str> = config.providers.iter().map(|p| p.id.as_str()).chain(config.combos.iter().map(|c| c.id.as_str())).chain(config.model_policies.iter().map(|p| p.id.as_str())).collect();
            if let Some(unknown) = models.iter().find(|m| !known.contains(&m.as_str())) {
                return Err(format!("'{unknown}' isn't a provider, a combo or a model policy"));
            }
            let agent = agents.iter_mut().find(|a| a.owner.is_none() && &a.id == id).ok_or_else(|| format!("no agent named '{id}'"))?;
            agent.delegation_models = models;
        }
        AgentOrgEdit::SetModelPolicies { policies } => {
            let wanted = policies.iter().map(|p| ModelPolicyConfig { id: p.id.clone(), model: p.model.clone(), description: p.description.clone() }).collect();
            let checked = check_policies(wanted, &config.providers, &config.combos)?;
            config.model_policies = checked;
            // A policy that is gone leaves every agent's limit.
            prune_delegation_models(config);
            return Ok(());
        }
        AgentOrgEdit::Remove { id } => {
            if !owners(&agents, id) {
                return Err(format!("no agent named '{id}'"));
            }
            let mut scratch = FileConfig { agents, nodes: config.nodes.clone(), ssh_hosts: config.ssh_hosts.clone(), ..FileConfig::default() };
            remove_agent_references(&mut scratch, id);
            // Whoever reported to it reports to its superior now; the hosts and nodes that listed it forget it.
            org::check_hierarchy(&scratch.agents)?;
            config.nodes = scratch.nodes;
            config.ssh_hosts = scratch.ssh_hosts;
            config.agents = scratch.agents;
            return Ok(());
        }
    }
    org::check_hierarchy(&agents)?;
    config.agents = agents;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_server_protocol::protocol::ModelPolicyDto;

    fn agent(id: &str, boss: Option<&str>) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            persona: format!("{id} persona"),
            provider_id: None,
            can_delegate_to_agents: true,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            autonomy: 4,
            approval_required: Vec::new(),
            role: None,
            reports_to: boss.map(str::to_string),
            owner: None,
            shared_with: Vec::new(),
            delegation_models: Vec::new(),
            can_start_tasks: true,
            can_create_workers: true,
        }
    }

    fn config() -> FileConfig {
        let member = AgentConfig { owner: Some("ana".into()), ..agent("ana-bot", None) };
        FileConfig { agents: vec![agent("chief", None), agent("lead", Some("chief")), agent("dev", Some("lead")), member], ..FileConfig::default() }
    }

    fn position(id: &str, role: Option<&str>, to: Option<&str>) -> AgentOrgEdit {
        AgentOrgEdit::SetPosition { id: id.into(), role: role.map(str::to_string), reports_to: to.map(str::to_string) }
    }

    fn boss_of<'a>(config: &'a FileConfig, id: &str) -> Option<&'a str> {
        config.agents.iter().find(|a| a.id == id).unwrap().reports_to.as_deref()
    }

    #[test]
    fn a_position_sets_the_role_and_the_superior_and_blank_means_none() {
        let mut config = config();
        apply_org_edit(&mut config, &position("dev", Some("  Backend  "), Some("chief"))).unwrap();
        let dev = config.agents.iter().find(|a| a.id == "dev").unwrap();
        assert_eq!((dev.role.as_deref(), dev.reports_to.as_deref()), (Some("Backend"), Some("chief")));

        apply_org_edit(&mut config, &position("dev", Some(" "), Some(""))).unwrap();
        let dev = config.agents.iter().find(|a| a.id == "dev").unwrap();
        assert_eq!((dev.role.as_deref(), dev.reports_to.as_deref()), (None, None));
    }

    #[test]
    fn a_refused_edit_changes_nothing() {
        let mut config = config();
        let before = config.agents.clone();
        // A circle (chief under its own report), itself, a missing superior, a missing agent, a member's agent, a role that isn't one line.
        for edit in [
            position("chief", None, Some("dev")),
            position("lead", None, Some("lead")),
            position("dev", None, Some("ghost")),
            position("ghost", None, None),
            position("ana-bot", Some("x"), None),
            position("dev", Some("two\nlines"), None),
            position("dev", None, Some("ana-bot")),
        ] {
            assert!(apply_org_edit(&mut config, &edit).is_err(), "{edit:?}");
            assert_eq!(config.agents, before, "{edit:?}");
        }
    }

    #[test]
    fn a_new_report_is_careful_by_default_and_sits_under_its_superior() {
        let mut config = config();
        let add = |id: &str, to: Option<&str>| AgentOrgEdit::AddReport { id: id.into(), persona: "Reviews code.".into(), role: Some("Reviewer".into()), reports_to: to.map(str::to_string) };
        apply_org_edit(&mut config, &add("reviewer", Some("lead"))).unwrap();

        let new = config.agents.iter().find(|a| a.id == "reviewer").unwrap();
        assert_eq!((new.reports_to.as_deref(), new.role.as_deref(), new.autonomy, new.owner.as_deref()), (Some("lead"), Some("Reviewer"), 3, None));
        assert!(!new.can_delegate_to_agents && !new.can_manage_agents && !new.can_message_agents && !new.can_manage_tasks);
        assert!(!new.can_start_tasks && !new.can_create_workers);
        assert_eq!(new.approval_required, Category::ALL.to_vec());
        assert!(new.allowed_tools.as_ref().is_some_and(|tools| tools.iter().all(|t| SAFE_AGENT_TOOLS.contains(&t.as_str()))));

        apply_org_edit(&mut config, &add("solo", None)).unwrap();
        assert_eq!(boss_of(&config, "solo"), None);

        let before = config.agents.clone();
        assert!(apply_org_edit(&mut config, &add("dev", None)).unwrap_err().contains("already exists"), "a repeated name");
        assert!(apply_org_edit(&mut config, &add("ana-bot", None)).unwrap_err().contains("already exists"), "a member's name too");
        assert!(apply_org_edit(&mut config, &add("x", Some("ghost"))).unwrap_err().contains("isn't one of your agents"));
        assert!(apply_org_edit(&mut config, &add("y", Some("ana-bot"))).is_err(), "a member's agent is no superior");
        assert!(apply_org_edit(&mut config, &AgentOrgEdit::AddReport { id: " bad".into(), persona: "p".into(), role: None, reports_to: None }).is_err());
        assert!(apply_org_edit(&mut config, &AgentOrgEdit::AddReport { id: "empty".into(), persona: " ".into(), role: None, reports_to: None }).is_err());
        assert_eq!(config.agents, before);
    }

    #[test]
    fn removing_an_agent_hands_its_reports_to_its_superior_and_it_leaves_the_hosts_and_nodes() {
        let mut config = config();
        config.nodes = vec![crate::NodeAccessConfig { id: "laptop".into(), enabled: true, agents: vec!["lead".into(), "dev".into()], require_approval: false }];
        config.ssh_hosts = vec![crate::SshHostConfig {
            id: "box".into(),
            host: "box.example".into(),
            user: "me".into(),
            port: 22,
            identity_file: None,
            enabled: true,
            agents: vec!["lead".into()],
            require_approval: false,
        }];

        apply_org_edit(&mut config, &AgentOrgEdit::Remove { id: "lead".into() }).unwrap();

        assert!(config.agents.iter().all(|a| a.id != "lead"));
        assert_eq!(boss_of(&config, "dev"), Some("chief"), "dev follows lead's superior");
        assert_eq!(config.nodes[0].agents, ["dev"]);
        assert!(!config.ssh_hosts[0].enabled && config.ssh_hosts[0].agents.is_empty(), "a host left with nobody is switched off");
        assert!(config.agents.iter().any(|a| a.id == "ana-bot"), "a member's agent stays");

        let before = config.agents.clone();
        assert!(apply_org_edit(&mut config, &AgentOrgEdit::Remove { id: "ghost".into() }).is_err());
        assert!(apply_org_edit(&mut config, &AgentOrgEdit::Remove { id: "ana-bot".into() }).is_err(), "not from here");
        assert_eq!(config.agents, before);
    }

    fn with_models() -> FileConfig {
        let provider = |id: &str| crate::ProviderConfig { id: id.into(), kind: crate::Provider::Gemini, api_key: None, base_url: None, model: None, node: None };
        FileConfig {
            providers: vec![provider("main"), provider("spare")],
            model_policies: vec![ModelPolicyConfig { id: "fast".into(), model: "main".into(), description: "simple work".into() }],
            ..config()
        }
    }

    fn limit_of<'a>(config: &'a FileConfig, id: &str) -> &'a [String] {
        &config.agents.iter().find(|a| a.id == id).unwrap().delegation_models
    }

    #[test]
    fn an_agents_model_limit_is_trimmed_in_order_and_refuses_what_does_not_exist() {
        let mut config = with_models();
        let limit = |id: &str, models: &[&str]| AgentOrgEdit::SetDelegationModels { id: id.into(), models: models.iter().map(|m| m.to_string()).collect() };

        apply_org_edit(&mut config, &limit("chief", &[" fast ", "spare", "fast", " "])).unwrap();
        assert_eq!(limit_of(&config, "chief"), ["fast", "spare"], "trimmed, no repeats, the first is the default");
        apply_org_edit(&mut config, &limit("chief", &[])).unwrap();
        assert!(limit_of(&config, "chief").is_empty(), "empty opens the choice again");

        let before = config.agents.clone();
        assert!(apply_org_edit(&mut config, &limit("chief", &["ghost"])).unwrap_err().contains("isn't a provider"), "an unknown name is told, not dropped");
        assert!(apply_org_edit(&mut config, &limit("ghost", &["fast"])).is_err(), "no such agent");
        assert!(apply_org_edit(&mut config, &limit("ana-bot", &["fast"])).is_err(), "a member's agent is theirs alone");
        assert_eq!(config.agents, before);
    }

    #[test]
    fn the_model_policies_are_replaced_checked_and_a_gone_one_leaves_every_limit() {
        let mut config = with_models();
        config.agents.iter_mut().find(|a| a.id == "chief").unwrap().delegation_models = vec!["fast".into(), "spare".into()];
        let policy = |id: &str, model: &str, description: &str| ModelPolicyDto { id: id.into(), model: model.into(), description: description.into() };

        apply_org_edit(&mut config, &AgentOrgEdit::SetModelPolicies { policies: vec![policy(" deep ", " spare ", " thinks hard "), policy("fast", "main", "")] }).unwrap();
        assert_eq!(config.model_policies, vec![
            ModelPolicyConfig { id: "deep".into(), model: "spare".into(), description: "thinks hard".into() },
            ModelPolicyConfig { id: "fast".into(), model: "main".into(), description: String::new() },
        ]);
        assert_eq!(limit_of(&config, "chief"), ["fast", "spare"]);

        apply_org_edit(&mut config, &AgentOrgEdit::SetModelPolicies { policies: vec![policy("deep", "spare", "")] }).unwrap();
        assert_eq!(limit_of(&config, "chief"), ["spare"], "the removed policy left the limit");

        let before = (config.model_policies.clone(), config.agents.clone());
        for bad in [
            vec![policy("main", "spare", "")],
            vec![policy("x", "ghost", "")],
            vec![policy(" ", "main", "")],
            vec![policy("x", "main", ""), policy("x", "spare", "")],
            vec![policy("x", "main", "two\nlines")],
        ] {
            assert!(apply_org_edit(&mut config, &AgentOrgEdit::SetModelPolicies { policies: bad.clone() }).is_err(), "{bad:?}");
            assert_eq!((config.model_policies.clone(), config.agents.clone()), before, "{bad:?}");
        }
    }

    #[test]
    fn the_model_edits_round_trip_over_the_wire() {
        let limit = AgentOrgEdit::SetDelegationModels { id: "chief".into(), models: vec!["fast".into()] };
        let json = serde_json::to_string(&limit).unwrap();
        assert_eq!(json, r#"{"kind":"setDelegationModels","id":"chief","models":["fast"]}"#);
        assert_eq!(serde_json::from_str::<AgentOrgEdit>(&json).unwrap(), limit);
        assert_eq!(serde_json::from_str::<AgentOrgEdit>(r#"{"kind":"setDelegationModels","id":"chief"}"#).unwrap(), AgentOrgEdit::SetDelegationModels { id: "chief".into(), models: Vec::new() });

        let policies = AgentOrgEdit::SetModelPolicies { policies: vec![ModelPolicyDto { id: "fast".into(), model: "main".into(), description: "quick".into() }] };
        let json = serde_json::to_string(&policies).unwrap();
        assert_eq!(json, r#"{"kind":"setModelPolicies","policies":[{"id":"fast","model":"main","description":"quick"}]}"#);
        assert_eq!(serde_json::from_str::<AgentOrgEdit>(&json).unwrap(), policies);
        assert_eq!(serde_json::from_str::<AgentOrgEdit>(r#"{"kind":"setModelPolicies"}"#).unwrap(), AgentOrgEdit::SetModelPolicies { policies: Vec::new() });
    }

    #[test]
    fn the_edits_round_trip_over_the_wire() {
        let edit = AgentOrgEdit::AddReport { id: "r".into(), persona: "p".into(), role: None, reports_to: Some("lead".into()) };
        let json = serde_json::to_string(&edit).unwrap();
        assert_eq!(json, r#"{"kind":"addReport","id":"r","persona":"p","reportsTo":"lead"}"#);
        assert_eq!(serde_json::from_str::<AgentOrgEdit>(&json).unwrap(), edit);
        assert_eq!(serde_json::from_str::<AgentOrgEdit>(r#"{"kind":"setPosition","id":"dev"}"#).unwrap(), position("dev", None, None));
        assert_eq!(serde_json::from_str::<AgentOrgEdit>(r#"{"kind":"remove","id":"dev"}"#).unwrap(), AgentOrgEdit::Remove { id: "dev".into() });
    }
}
