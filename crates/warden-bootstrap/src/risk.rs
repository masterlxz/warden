//! Which risk category a tool call belongs to (P122), for the agents that must have a kind of action approved even when
//! they otherwise act alone (`AgentConfig.approval_required`). Three sources, the first that says something wins:
//! the user's own map (`[[tool_categories]]`), the table of Warden's own tools below, and what an MCP server says about
//! its tool (`annotations`, advice rather than a guarantee).

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use warden_core::autonomy::{Category, Classifier};
use warden_core::model::ToolCall;
use warden_core::tool::{RiskHints, Tool};

/// One entry of the user's own map (TOML `[[tool_categories]]`): a tool, by the name the model sees, and the category
/// its calls belong to. The way to cover a tool Warden knows nothing about (a Slack or a payments MCP server).
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolCategoryConfig {
    pub tool: String,
    pub category: Category,
}

/// The category of one of Warden's own tools, from its name and what the model asked for. A name carrying a prefix
/// (`<device>__browser_click_element`, `<node>__...`) counts as the tool after the last `__`.
pub fn builtin_category(name: &str, arguments: &Value) -> Option<Category> {
    let base = name.rsplit_once("__").map_or(name, |(_, tool)| tool);
    let action = arguments.get("action").and_then(Value::as_str).unwrap_or_default();
    match base {
        "shell" | "ssh_exec" | "ssh_upload" | "ssh_download" | "node_shell" | "node_write_file" | "code_task" => Some(Category::CriticalInfra),
        "manage_agents" => match action {
            "list" => None,
            "delete" => Some(Category::DeleteData),
            _ => Some(Category::ElevatedAgent),
        },
        "manage_tasks" => match action {
            "list" => None,
            "delete" => Some(Category::DeleteData),
            _ => Some(Category::ImportantConfig),
        },
        "browser_click_element" | "browser_navigate" => Some(Category::ExternalMessage),
        _ => None,
    }
}

/// What a server's own hints say, when neither the user's map nor the table did. Only an explicit hint counts, and a
/// tool that says it only reads is never classified.
pub fn category_from_hints(hints: &RiskHints) -> Option<Category> {
    if hints.read_only == Some(true) {
        return None;
    }
    if hints.destructive == Some(true) {
        return Some(Category::DeleteData);
    }
    (hints.open_world == Some(true)).then_some(Category::ExternalMessage)
}

/// The classifier for an agent's turn: `tools` are the ones the orchestrator has (their `risk_hints` are read once,
/// here), `user_map` is `[[tool_categories]]`.
pub fn build_classifier(user_map: &[ToolCategoryConfig], tools: &[Arc<dyn Tool>]) -> Classifier {
    let user: HashMap<String, Category> = user_map.iter().map(|entry| (entry.tool.clone(), entry.category)).collect();
    let hints: HashMap<String, Category> = tools
        .iter()
        .filter_map(|tool| Some((tool.spec().name, category_from_hints(&tool.risk_hints()?)?)))
        .collect();
    Arc::new(move |call: &ToolCall| {
        user.get(&call.name).copied().or_else(|| builtin_category(&call.name, &call.arguments)).or_else(|| hints.get(&call.name).copied())
    })
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;
    use warden_core::tool::ToolSpec;

    use super::*;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), arguments, thought_signature: None }
    }

    /// A tool that only says what its provider says about it.
    struct Hinted(&'static str, Option<RiskHints>);

    #[async_trait]
    impl Tool for Hinted {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: self.0.to_string(), description: String::new(), parameters: json!({}) }
        }

        async fn call(&self, _args: Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }

        fn risk_hints(&self) -> Option<RiskHints> {
            self.1
        }
    }

    #[test]
    fn warden_tools_that_reach_a_machine_are_critical_infrastructure() {
        for name in ["shell", "ssh_exec", "ssh_upload", "ssh_download", "node_shell", "node_write_file", "pc__shell", "code_task"] {
            assert_eq!(builtin_category(name, &json!({})), Some(Category::CriticalInfra), "{name}");
        }
        for name in ["read_file", "write_file", "use_skill", "list_nodes", "node_read_file", "delegate_task", "message_agent"] {
            assert_eq!(builtin_category(name, &json!({})), None, "{name}");
        }
    }

    #[test]
    fn managing_agents_and_tasks_depends_on_the_action() {
        let by = |tool: &str, action: &str| builtin_category(tool, &json!({ "action": action }));
        assert_eq!(by("manage_agents", "list"), None);
        assert_eq!(by("manage_agents", "create"), Some(Category::ElevatedAgent));
        assert_eq!(by("manage_agents", "update"), Some(Category::ElevatedAgent));
        assert_eq!(by("manage_agents", "delete"), Some(Category::DeleteData));
        assert_eq!(by("manage_tasks", "list"), None);
        assert_eq!(by("manage_tasks", "create"), Some(Category::ImportantConfig));
        assert_eq!(by("manage_tasks", "delete"), Some(Category::DeleteData));
    }

    #[test]
    fn the_browser_tools_that_act_on_a_page_send_things_out() {
        assert_eq!(builtin_category("browser_click_element", &json!({})), Some(Category::ExternalMessage));
        assert_eq!(builtin_category("laptop__browser_navigate", &json!({})), Some(Category::ExternalMessage));
        assert_eq!(builtin_category("browser_read_page", &json!({})), None);
    }

    #[test]
    fn a_server_hint_counts_only_when_it_is_explicit_and_the_tool_does_not_say_it_only_reads() {
        let hints = |read_only, destructive, open_world| RiskHints { read_only, destructive, open_world };
        assert_eq!(category_from_hints(&hints(Some(false), Some(true), None)), Some(Category::DeleteData));
        assert_eq!(category_from_hints(&hints(None, Some(true), Some(true))), Some(Category::DeleteData));
        assert_eq!(category_from_hints(&hints(Some(false), Some(false), Some(true))), Some(Category::ExternalMessage));
        assert_eq!(category_from_hints(&hints(Some(true), Some(true), Some(true))), None);
        assert_eq!(category_from_hints(&hints(None, None, None)), None);
        assert_eq!(category_from_hints(&hints(Some(false), Some(false), Some(false))), None);
    }

    #[test]
    fn the_users_map_beats_the_table_and_the_table_beats_a_servers_hints() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Hinted("slack__post", Some(RiskHints { read_only: Some(false), destructive: Some(false), open_world: Some(true) }))),
            Arc::new(Hinted("wipe", Some(RiskHints { read_only: Some(false), destructive: Some(true), open_world: None }))),
            Arc::new(Hinted("shell", Some(RiskHints { read_only: Some(true), destructive: None, open_world: None }))),
            Arc::new(Hinted("quiet", None)),
        ];
        let map = [
            ToolCategoryConfig { tool: "wipe".into(), category: Category::SpendMoney },
            ToolCategoryConfig { tool: "pay".into(), category: Category::SpendMoney },
            ToolCategoryConfig { tool: "git_push".into(), category: Category::PublishCode },
        ];
        let classify = build_classifier(&map, &tools);
        let of = |name: &str| classify(&call(name, json!({})));
        assert_eq!(of("slack__post"), Some(Category::ExternalMessage), "from the server's hints");
        assert_eq!(of("wipe"), Some(Category::SpendMoney), "the user's map wins over the hints");
        assert_eq!(of("shell"), Some(Category::CriticalInfra), "the table wins over a hint that says read-only");
        assert_eq!(of("pay"), Some(Category::SpendMoney), "a tool only the map knows");
        assert_eq!(of("git_push"), Some(Category::PublishCode));
        assert_eq!(of("quiet"), None);
        assert_eq!(of("ghost"), None);
    }

    #[test]
    fn the_map_reads_from_toml_and_refuses_a_category_that_does_not_exist() {
        let entries: Vec<ToolCategoryConfig> =
            toml::from_str::<HashMap<String, Vec<ToolCategoryConfig>>>("[[tool_categories]]\ntool = \"pay\"\ncategory = \"spend_money\"\n").unwrap().remove("tool_categories").unwrap();
        assert_eq!(entries, [ToolCategoryConfig { tool: "pay".into(), category: Category::SpendMoney }]);
        assert!(toml::from_str::<HashMap<String, Vec<ToolCategoryConfig>>>("[[tool_categories]]\ntool = \"pay\"\ncategory = \"spending\"\n").is_err());
    }
}
