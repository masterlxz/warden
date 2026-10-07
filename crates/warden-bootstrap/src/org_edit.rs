//! Editing the organization of the owner's agents from the tree (P120): a person moves an agent, gives it a role, adds a report or
//! removes one, and nothing else of the settings is touched. The same hierarchy checks as a settings save (`check_hierarchy`: no
//! circle, no missing superior), applied to a copy first, so a refused edit changes nothing.

use warden_core::autonomy::Category;
use warden_server_protocol::protocol::AgentOrgEdit;

use crate::manage_agents::{check_id, check_persona, check_role, ASK_FIRST_LEVEL};
use crate::{org, remove_agent_references, AgentConfig, FileConfig, SAFE_AGENT_TOOLS};

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
            });
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
