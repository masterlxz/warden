//! The organization of the owner's agents (P120): each one may have a role and a superior (`AgentConfig.role`,
//! `.reports_to`). For now this is what the screens show and nothing more — it changes none of what an agent may do
//! (delegating, messaging, approvals and autonomy don't read it).

use std::collections::HashSet;

use crate::AgentConfig;

/// One agent in the tree, with the ones that report to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgNode {
    pub id: String,
    pub role: Option<String>,
    pub children: Vec<OrgNode>,
}

/// Why the hierarchy of `agents` can't be saved, or `Ok` when it is a forest: nobody reports to themselves, to a
/// stranger or in a circle, and a member's agent (P84) is outside it.
pub fn check_hierarchy(agents: &[AgentConfig]) -> Result<(), String> {
    for agent in agents {
        if agent.owner.is_some() {
            if agent.role.is_some() || agent.reports_to.is_some() {
                return Err(format!("agent '{}' belongs to a member, so it has no role or superior", agent.id));
            }
            continue;
        }
        let Some(superior) = agent.reports_to.as_deref() else { continue };
        if superior == agent.id {
            return Err(format!("agent '{}' can't report to itself", agent.id));
        }
        match agents.iter().find(|a| a.id == superior) {
            None => return Err(format!("agent '{}' reports to '{superior}', which doesn't exist", agent.id)),
            Some(found) if found.owner.is_some() => return Err(format!("agent '{}' reports to '{superior}', which is a member's agent", agent.id)),
            Some(_) => {}
        }
    }
    for agent in agents.iter().filter(|a| a.owner.is_none()) {
        let mut chain = vec![agent.id.as_str()];
        let mut current = agent;
        while let Some(superior) = current.reports_to.as_deref() {
            if chain.contains(&superior) {
                chain.push(superior);
                return Err(format!("the reporting lines go in a circle: {}", chain.join(" → ")));
            }
            chain.push(superior);
            match agents.iter().find(|a| a.id == superior) {
                Some(next) => current = next,
                None => break,
            }
        }
    }
    Ok(())
}

/// The owner's agents as a forest: the ones with no superior at the top, in the order of the file, each with its
/// reports under it. A member's agent isn't in it, and an agent whose superior is gone is shown at the top rather
/// than lost.
pub fn build_org(agents: &[AgentConfig]) -> Vec<OrgNode> {
    let owners: Vec<&AgentConfig> = agents.iter().filter(|a| a.owner.is_none()).collect();
    let is_listed = |id: &str| owners.iter().any(|a| a.id == id);
    let mut seen = HashSet::new();
    owners
        .iter()
        .filter(|a| a.reports_to.as_deref().is_none_or(|superior| !is_listed(superior)))
        .map(|a| node(a, &owners, &mut seen))
        .collect()
}

fn node(agent: &AgentConfig, owners: &[&AgentConfig], seen: &mut HashSet<String>) -> OrgNode {
    seen.insert(agent.id.clone());
    let mut children = Vec::new();
    for report in owners.iter().filter(|a| a.reports_to.as_deref() == Some(agent.id.as_str())) {
        if !seen.contains(&report.id) {
            children.push(node(report, owners, seen));
        }
    }
    OrgNode { id: agent.id.clone(), role: agent.role.clone(), children }
}

/// The tree as lines of text: `id — role`, with the reports indented under their superior.
pub fn render_org(nodes: &[OrgNode]) -> Vec<String> {
    let mut lines = Vec::new();
    for root in nodes {
        lines.push(label(root));
        render_children(&root.children, "", &mut lines);
    }
    lines
}

fn render_children(children: &[OrgNode], prefix: &str, lines: &mut Vec<String>) {
    for (index, child) in children.iter().enumerate() {
        let last = index + 1 == children.len();
        lines.push(format!("{prefix}{} {}", if last { "└─" } else { "├─" }, label(child)));
        render_children(&child.children, &format!("{prefix}{}", if last { "   " } else { "│  " }), lines);
    }
}

fn label(node: &OrgNode) -> String {
    match &node.role {
        Some(role) => format!("{} — {role}", node.id),
        None => node.id.clone(),
    }
}

/// Every agent below `id`, at any depth, in the order of the file: the scope of authority of `id` (P120). The agent
/// itself, its superior and its peers aren't in it. Only the owner's agents count.
pub fn subordinates_of(agents: &[AgentConfig], id: &str) -> Vec<String> {
    let mut below: HashSet<&str> = HashSet::new();
    let mut queue = vec![id];
    while let Some(current) = queue.pop() {
        for agent in agents.iter().filter(|a| a.owner.is_none() && a.reports_to.as_deref() == Some(current)) {
            if agent.id != id && below.insert(agent.id.as_str()) {
                queue.push(agent.id.as_str());
            }
        }
    }
    agents.iter().filter(|a| below.contains(a.id.as_str())).map(|a| a.id.clone()).collect()
}

/// Whether `id` is part of the organization: it has a superior or someone reports to it. An agent outside it is on
/// its own, and the rules that follow the hierarchy leave it as it was before the hierarchy existed.
pub fn is_in_hierarchy(agents: &[AgentConfig], id: &str) -> bool {
    agents.iter().filter(|a| a.owner.is_none()).any(|a| (a.id == id && a.reports_to.is_some()) || a.reports_to.as_deref() == Some(id))
}

/// An agent is leaving: whoever reported to it reports to its superior instead (or to nobody, if it had none).
/// Call before the agent is dropped from `agents`.
pub fn reparent_reports(agents: &mut [AgentConfig], removed_id: &str) {
    let superior = agents.iter().find(|a| a.id == removed_id).and_then(|a| a.reports_to.clone());
    for agent in agents.iter_mut().filter(|a| a.reports_to.as_deref() == Some(removed_id)) {
        agent.reports_to = superior.clone();
    }
}

/// An agent was renamed: whoever reported to it follows the new name.
pub fn rename_in_reports(agents: &mut [AgentConfig], from: &str, to: &str) {
    for agent in agents.iter_mut().filter(|a| a.reports_to.as_deref() == Some(from)) {
        agent.reports_to = Some(to.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, role: Option<&str>, reports_to: Option<&str>) -> AgentConfig {
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
            role: role.map(str::to_string),
            reports_to: reports_to.map(str::to_string),
            owner: None,
            shared_with: Vec::new(),
            delegation_models: Vec::new(),
        }
    }

    fn members(id: &str) -> AgentConfig {
        AgentConfig { owner: Some("ana".into()), ..agent(id, None, None) }
    }

    #[test]
    fn a_tree_of_reporting_lines_is_fine_and_so_is_no_hierarchy_at_all() {
        assert!(check_hierarchy(&[]).is_ok());
        assert!(check_hierarchy(&[agent("a", None, None), agent("b", None, None)]).is_ok());
        assert!(check_hierarchy(&[agent("boss", None, None), agent("dev", None, Some("boss")), agent("qa", None, Some("boss")), agent("intern", None, Some("dev"))]).is_ok());
    }

    #[test]
    fn nobody_reports_to_themselves_to_a_stranger_or_in_a_circle() {
        let err = check_hierarchy(&[agent("a", None, Some("a"))]).unwrap_err();
        assert!(err.contains("itself"), "{err}");

        let err = check_hierarchy(&[agent("a", None, Some("ghost"))]).unwrap_err();
        assert!(err.contains("'ghost', which doesn't exist"), "{err}");

        let err = check_hierarchy(&[agent("a", None, Some("b")), agent("b", None, Some("a"))]).unwrap_err();
        assert!(err.contains("a → b → a"), "{err}");

        let err = check_hierarchy(&[agent("a", None, Some("c")), agent("b", None, Some("a")), agent("c", None, Some("b"))]).unwrap_err();
        assert!(err.contains("circle"), "{err}");
    }

    #[test]
    fn a_members_agent_is_outside_the_hierarchy() {
        assert!(check_hierarchy(&[agent("boss", None, None), members("hers")]).is_ok());
        let err = check_hierarchy(&[agent("boss", None, None), AgentConfig { role: Some("cook".into()), ..members("hers") }]).unwrap_err();
        assert!(err.contains("member"), "{err}");
        let err = check_hierarchy(&[agent("boss", None, None), AgentConfig { reports_to: Some("boss".into()), ..members("hers") }]).unwrap_err();
        assert!(err.contains("member"), "{err}");
        let err = check_hierarchy(&[agent("dev", None, Some("hers")), members("hers")]).unwrap_err();
        assert!(err.contains("a member's agent"), "{err}");
    }

    #[test]
    fn the_tree_keeps_the_order_of_the_file_and_leaves_a_members_agents_out() {
        let agents = [
            agent("dev", Some("Backend"), Some("boss")),
            agent("boss", Some("Head of engineering"), None),
            members("hers"),
            agent("solo", None, None),
            agent("qa", None, Some("boss")),
            agent("intern", None, Some("dev")),
        ];
        let tree = build_org(&agents);
        assert_eq!(tree.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["boss", "solo"]);
        let boss = &tree[0];
        assert_eq!(boss.role.as_deref(), Some("Head of engineering"));
        assert_eq!(boss.children.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["dev", "qa"]);
        assert_eq!(boss.children[0].children[0].id, "intern");
    }

    #[test]
    fn an_agent_whose_superior_is_gone_is_shown_at_the_top_not_lost() {
        let tree = build_org(&[agent("orphan", None, Some("ghost")), agent("other", None, None)]);
        assert_eq!(tree.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["orphan", "other"]);
    }

    #[test]
    fn a_circle_that_got_past_the_check_does_not_loop_the_tree() {
        let tree = build_org(&[agent("a", None, Some("b")), agent("b", None, Some("a")), agent("c", None, None)]);
        assert_eq!(tree.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["c"], "the circle has no top, so it isn't shown, and it ends");
    }

    #[test]
    fn the_text_tree_indents_the_reports_under_their_superior() {
        let tree = build_org(&[
            agent("boss", Some("Head"), None),
            agent("dev", Some("Backend"), Some("boss")),
            agent("intern", None, Some("dev")),
            agent("qa", None, Some("boss")),
            agent("solo", None, None),
        ]);
        assert_eq!(render_org(&tree), ["boss — Head", "├─ dev — Backend", "│  └─ intern", "└─ qa", "solo"]);
    }

    #[test]
    fn a_leaving_agents_reports_move_up_to_its_superior() {
        let mut agents = vec![agent("boss", None, None), agent("lead", None, Some("boss")), agent("dev", None, Some("lead")), agent("qa", None, Some("lead"))];
        reparent_reports(&mut agents, "lead");
        assert_eq!(agents[2].reports_to.as_deref(), Some("boss"));
        assert_eq!(agents[3].reports_to.as_deref(), Some("boss"));

        let mut top = vec![agent("boss", None, None), agent("dev", None, Some("boss"))];
        reparent_reports(&mut top, "boss");
        assert_eq!(top[1].reports_to, None, "the top one had no superior to hand them to");
    }

    #[test]
    fn the_scope_of_an_agent_is_everyone_below_it_and_nobody_else() {
        let agents = [
            agent("boss", None, None),
            agent("a", None, Some("boss")),
            agent("b", None, Some("boss")),
            agent("a1", None, Some("a")),
            agent("a2", None, Some("a")),
            agent("a11", None, Some("a1")),
            members("hers"),
        ];
        assert_eq!(subordinates_of(&agents, "a"), ["a1", "a2", "a11"], "any depth, in the order of the file");
        assert_eq!(subordinates_of(&agents, "boss"), ["a", "b", "a1", "a2", "a11"]);
        assert!(subordinates_of(&agents, "a11").is_empty(), "a leaf has no scope");
        assert!(subordinates_of(&agents, "b").is_empty(), "a peer's branch isn't mine");
        assert!(subordinates_of(&agents, "ghost").is_empty());
    }

    #[test]
    fn a_circle_that_got_past_the_check_does_not_loop_the_scope() {
        let agents = [agent("a", None, Some("b")), agent("b", None, Some("a"))];
        assert_eq!(subordinates_of(&agents, "a"), ["b"], "a is never in its own scope");
    }

    #[test]
    fn an_agent_is_in_the_hierarchy_when_it_has_a_superior_or_reports() {
        let agents = [agent("boss", None, None), agent("dev", None, Some("boss")), agent("solo", None, None), members("hers")];
        assert!(is_in_hierarchy(&agents, "boss"), "someone reports to it");
        assert!(is_in_hierarchy(&agents, "dev"), "it has a superior");
        assert!(!is_in_hierarchy(&agents, "solo"));
        assert!(!is_in_hierarchy(&agents, "hers"));
        assert!(!is_in_hierarchy(&agents, "ghost"));
    }

    #[test]
    fn a_rename_is_followed_by_the_reports() {
        let mut agents = vec![agent("boss", None, None), agent("dev", None, Some("boss"))];
        rename_in_reports(&mut agents, "boss", "chief");
        assert_eq!(agents[1].reports_to.as_deref(), Some("chief"));
    }
}
