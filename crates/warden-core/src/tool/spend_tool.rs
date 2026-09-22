use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::budget::TurnBudget;
use crate::tool::{Tool, ToolSpec};

/// Lets the model read the spending meter (P4): every limit that applies to it, how much of each
/// is used and how much is left. It is the on-demand half of the meter — the orchestrator also
/// volunteers a line once a limit is close — so an agent deciding whether to start more work, or
/// to hand it to sub-agents, can look instead of guessing. Read-only.
///
/// Registered once at startup, unbound; the turn binds a copy to its `TurnBudget` (`with_budget`),
/// which is also how a sub-agent gets one. Unbound, or bound to a turn with no limits configured,
/// it is not offered to the model at all.
pub struct BudgetTool {
    budget: Option<Arc<TurnBudget>>,
}

impl BudgetTool {
    pub fn new() -> Self {
        Self { budget: None }
    }
}

impl Default for BudgetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BudgetTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "budget".to_string(),
            description: "Shows the spending limits that apply to you: for each, how many tokens and dollars are \
                used in its time window, how many are left, and when room starts coming back. Check it before \
                starting expensive work (long research, many sub-agents, background jobs) and when you are told \
                a limit is close. When a limit runs out the turn is paused and the user has to allow more, so \
                prefer finishing with what you have over starting something you can't afford."
                .to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    fn is_available(&self) -> bool {
        self.budget.as_ref().is_some_and(|b| b.spend().is_some())
    }

    fn with_budget(&self, budget: &Arc<TurnBudget>) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self { budget: Some(budget.clone()) }))
    }

    async fn call(&self, _args: Value) -> anyhow::Result<Value> {
        let spend = self
            .budget
            .as_ref()
            .and_then(|b| b.spend())
            .ok_or_else(|| anyhow::anyhow!("no spending limit applies to this turn"))?;
        let limits = spend.status();
        let mut out = json!({ "limits": limits });
        if limits.is_empty() {
            out["note"] = json!("no spending limit applies to you");
        }
        if let Some(error) = spend.ledger_error() {
            out["warning"] = json!(format!("spending could not be recorded, so these numbers may be low: {error}"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::SpendTurn;
    use crate::model::Usage;
    use crate::spend::{Limit, MemoryStore, PriceTable, Scope, SpendContext, SpendGuard};

    fn bound_to(limits: Vec<Limit>, agent: Option<&str>) -> Arc<dyn Tool> {
        let guard = Arc::new(SpendGuard::new(Arc::new(MemoryStore::default()), limits, PriceTable::default()));
        let ctx = SpendContext::new("cli").with_agent(agent.map(String::from));
        guard.record(&ctx, "m", &Usage { prompt_tokens: 300, completion_tokens: 100, total_tokens: 400 });
        let budget = TurnBudget::for_turn(None, Some(SpendTurn::new(guard, ctx, None)));
        BudgetTool::new().with_budget(&budget).unwrap()
    }

    #[test]
    fn it_is_hidden_unless_the_turn_has_limits() {
        assert!(!BudgetTool::new().is_available());
        let no_limits = TurnBudget::for_turn(Some(5), None);
        assert!(!BudgetTool::new().with_budget(&no_limits).unwrap().is_available());
        assert!(bound_to(vec![], None).is_available());
    }

    #[tokio::test]
    async fn it_reports_used_and_left_for_the_limits_that_apply() {
        let tool = bound_to(
            vec![
                Limit::new("day", Scope::Global, 24).with_max_tokens(1000),
                Limit::new("other-agent", Scope::Agent("someone-else".into()), 24).with_max_tokens(10),
            ],
            Some("chief"),
        );
        let out = tool.call(json!({})).await.unwrap();
        let limits = out["limits"].as_array().unwrap();
        assert_eq!(limits.len(), 1, "another agent's limit is none of its business");
        assert_eq!(limits[0]["id"], "day");
        assert_eq!((limits[0]["used_tokens"].as_u64(), limits[0]["remaining_tokens"].as_u64()), (Some(400), Some(600)));
        assert_eq!(limits[0]["exceeded"], false);
    }

    #[tokio::test]
    async fn with_no_applicable_limit_it_says_so() {
        let out = bound_to(vec![Limit::new("x", Scope::Agent("nobody".into()), 1).with_max_tokens(1)], None)
            .call(json!({}))
            .await
            .unwrap();
        assert_eq!(out["limits"], json!([]));
        assert!(out["note"].as_str().unwrap().contains("no spending limit"));
    }
}
