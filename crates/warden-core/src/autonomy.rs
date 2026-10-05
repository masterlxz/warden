//! How much an agent may do without asking (P122). One level per agent, applied in one place — `Orchestrator::run_tool` —
//! on top of whatever each tool already asks for by itself (an SSH host with `require_approval`, `manage_agents`).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::ToolCall;
use crate::tool::{Answer, ApprovalRequest, Approver};

/// `ApprovalRequest.action` of a call that level 3 asks about.
pub const TOOL_CALL_ACTION: &str = "tool_call";

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const DETAIL_LIMIT: usize = 500;

/// From the most careful to the freest, so the lower of two levels is `min`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Autonomy {
    /// 1: answers in text and has no tools at all.
    AnswerOnly,
    /// 2: reads freely, and a call that would change something is refused with a note to propose it instead.
    Suggest,
    /// 3: reads freely, and asks a person before any other call.
    AskFirst,
    /// 4: runs its tools on its own, as an agent without a level always did.
    Autonomous,
}

impl Autonomy {
    pub const DEFAULT_LEVEL: u8 = 4;

    /// The level a config file or a screen writes down (1 to 4); anything else is not a level.
    pub fn from_level(level: u8) -> Option<Self> {
        match level {
            1 => Some(Self::AnswerOnly),
            2 => Some(Self::Suggest),
            3 => Some(Self::AskFirst),
            4 => Some(Self::Autonomous),
            _ => None,
        }
    }

    pub fn level(self) -> u8 {
        self as u8 + 1
    }
}

/// The kinds of action a person may want to approve for an agent that otherwise acts alone (P122, VISAO_AGENTES §38).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Deleting data (an agent, a task, an MCP tool that says it destroys).
    DeleteData,
    /// Spending money: a paid API, a purchase, a spending limit lifted.
    SpendMoney,
    /// Changing infrastructure that matters: a shell, a remote machine, a file copied onto a server.
    CriticalInfra,
    /// A message that leaves for a person or a service outside Warden.
    ExternalMessage,
    /// Publishing code (a push, a release, a deploy).
    PublishCode,
    /// Changing a setting that changes how things behave: a scheduled task.
    ImportantConfig,
    /// Creating or raising an agent.
    ElevatedAgent,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::DeleteData,
        Category::SpendMoney,
        Category::CriticalInfra,
        Category::ExternalMessage,
        Category::PublishCode,
        Category::ImportantConfig,
        Category::ElevatedAgent,
    ];

    /// The id a config file, the wire and the screens use.
    pub fn as_str(self) -> &'static str {
        match self {
            Category::DeleteData => "delete_data",
            Category::SpendMoney => "spend_money",
            Category::CriticalInfra => "critical_infra",
            Category::ExternalMessage => "external_message",
            Category::PublishCode => "publish_code",
            Category::ImportantConfig => "important_config",
            Category::ElevatedAgent => "elevated_agent",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == id.trim())
    }
}

/// Says which risk category a tool call belongs to, if any. Built by the host from what it knows about its tools.
pub type Classifier = Arc<dyn Fn(&ToolCall) -> Option<Category> + Send + Sync>;

/// Decides one tool call. `read_only` names the tools that never change anything and so never need a yes at levels 2
/// and 3. `required` are the categories this agent must have approved even at level 4, and `category` is the one this
/// call belongs to. With nobody to ask (a channel that can't), or no answer in time, a call that needs a yes is refused —
/// it never runs unasked.
pub async fn authorize(
    level: Autonomy,
    read_only: &[String],
    required: &[Category],
    category: Option<Category>,
    approver: Option<&Arc<dyn Approver>>,
    call: &ToolCall,
) -> anyhow::Result<()> {
    authorize_within(level, read_only, required, category, approver, call, APPROVAL_TIMEOUT).await
}

async fn authorize_within(
    level: Autonomy,
    read_only: &[String],
    required: &[Category],
    category: Option<Category>,
    approver: Option<&Arc<dyn Approver>>,
    call: &ToolCall,
    timeout: Duration,
) -> anyhow::Result<()> {
    let needs_category_yes = category.is_some_and(|c| required.contains(&c));
    let why = match level {
        Autonomy::AnswerOnly => anyhow::bail!("this agent answers in text only and has no tools"),
        Autonomy::Autonomous if !needs_category_yes => return Ok(()),
        Autonomy::Autonomous => format!("this kind of action ({}) needs a person's yes for this agent", category.map_or("", Category::as_str)),
        Autonomy::Suggest | Autonomy::AskFirst if read_only.contains(&call.name) => return Ok(()),
        Autonomy::Suggest => anyhow::bail!(
            "'{}' was not run: this agent only suggests (autonomy level 2). Describe what you would do and let the person decide",
            call.name
        ),
        Autonomy::AskFirst => "it needs a person's yes (autonomy level 3)".to_string(),
    };
    let Some(approver) = approver else {
        anyhow::bail!("'{}' was not run: {why} and this channel can't ask", call.name);
    };
    let request = ApprovalRequest { target: call.name.clone(), action: TOOL_CALL_ACTION.to_string(), detail: summarize(&call.arguments), category };
    match tokio::time::timeout(timeout, approver.ask(request, Some(&call.name))).await {
        Ok(Answer::Once | Answer::Always) => Ok(()),
        Ok(Answer::Reject) => anyhow::bail!("'{}' was not run: the person said no", call.name),
        Err(_) => anyhow::bail!("'{}' was not run: nobody answered in time", call.name),
    }
}

fn summarize(arguments: &Value) -> String {
    let text = arguments.to_string();
    if text.chars().count() <= DETAIL_LIMIT {
        return text;
    }
    let cut: String = text.chars().take(DETAIL_LIMIT).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;

    /// Level-only checks: no categories asked for.
    async fn authorize(level: Autonomy, read_only: &[String], approver: Option<&Arc<dyn Approver>>, call: &ToolCall) -> anyhow::Result<()> {
        super::authorize(level, read_only, &[], None, approver, call).await
    }

    struct Says {
        answer: Answer,
        asked: Mutex<Vec<(ApprovalRequest, Option<String>)>>,
    }

    impl Says {
        fn new(answer: Answer) -> Arc<Self> {
            Arc::new(Self { answer, asked: Mutex::new(Vec::new()) })
        }
    }

    #[async_trait]
    impl Approver for Says {
        async fn approve(&self, _request: ApprovalRequest) -> bool {
            self.answer != Answer::Reject
        }

        async fn ask(&self, request: ApprovalRequest, always: Option<&str>) -> Answer {
            self.asked.lock().unwrap().push((request, always.map(str::to_string)));
            self.answer
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), arguments: json!({ "path": "a.txt" }), thought_signature: None }
    }

    fn reads() -> Vec<String> {
        vec!["read_file".to_string()]
    }

    #[test]
    fn a_level_is_one_to_four_and_nothing_else() {
        assert_eq!(Autonomy::from_level(0), None);
        assert_eq!(Autonomy::from_level(5), None);
        for level in 1..=4 {
            assert_eq!(Autonomy::from_level(level).unwrap().level(), level);
        }
        assert!(Autonomy::Suggest < Autonomy::AskFirst);
        assert_eq!(Autonomy::DEFAULT_LEVEL, Autonomy::Autonomous.level());
    }

    #[tokio::test]
    async fn level_four_runs_everything_without_asking() {
        let approver: Arc<dyn Approver> = Says::new(Answer::Reject);
        assert!(authorize(Autonomy::Autonomous, &reads(), Some(&approver), &call("write_file")).await.is_ok());
    }

    #[test]
    fn a_category_is_written_as_its_snake_case_id_and_nothing_else_parses() {
        for category in Category::ALL {
            assert_eq!(Category::parse(category.as_str()), Some(category));
            assert_eq!(serde_json::to_value(category).unwrap(), json!(category.as_str()));
            assert_eq!(serde_json::from_value::<Category>(json!(category.as_str())).unwrap(), category);
        }
        assert_eq!(Category::parse(" critical_infra "), Some(Category::CriticalInfra));
        assert_eq!(Category::parse("everything"), None);
        assert!(serde_json::from_value::<Category>(json!("everything")).is_err());
    }

    #[tokio::test]
    async fn level_four_asks_only_for_a_listed_category_and_says_which() {
        let says = Says::new(Answer::Once);
        let approver: Arc<dyn Approver> = says.clone();
        let required = [Category::CriticalInfra];
        let (read_only, shell) = (reads(), call("shell"));
        let run = |category| super::authorize(Autonomy::Autonomous, &read_only, &required, category, Some(&approver), &shell);

        assert!(run(None).await.is_ok());
        assert!(run(Some(Category::DeleteData)).await.is_ok());
        assert!(says.asked.lock().unwrap().is_empty(), "a category the agent didn't list runs unasked");

        assert!(run(Some(Category::CriticalInfra)).await.is_ok());
        let asked = says.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0.category, Some(Category::CriticalInfra));
        assert_eq!(asked[0].0.action, TOOL_CALL_ACTION);
    }

    #[tokio::test]
    async fn a_listed_category_is_refused_on_a_no_and_when_there_is_nobody_to_ask() {
        let no: Arc<dyn Approver> = Says::new(Answer::Reject);
        let required = [Category::ExternalMessage];
        let call = call("slack__post");
        let refused = super::authorize(Autonomy::Autonomous, &[], &required, Some(Category::ExternalMessage), Some(&no), &call).await;
        assert!(refused.unwrap_err().to_string().contains("said no"));
        let unasked = super::authorize(Autonomy::Autonomous, &[], &required, Some(Category::ExternalMessage), None, &call).await.unwrap_err().to_string();
        assert!(unasked.contains("external_message") && unasked.contains("can't ask"), "{unasked}");
    }

    #[tokio::test]
    async fn level_three_sends_the_category_along_when_the_call_has_one() {
        let says = Says::new(Answer::Once);
        let approver: Arc<dyn Approver> = says.clone();
        super::authorize(Autonomy::AskFirst, &reads(), &[], Some(Category::DeleteData), Some(&approver), &call("write_file")).await.unwrap();
        assert_eq!(says.asked.lock().unwrap()[0].0.category, Some(Category::DeleteData));
    }

    #[tokio::test]
    async fn level_one_refuses_every_call_even_a_read() {
        assert!(authorize(Autonomy::AnswerOnly, &reads(), None, &call("read_file")).await.is_err());
    }

    #[tokio::test]
    async fn level_two_lets_a_read_through_and_refuses_the_rest_with_a_suggestion() {
        assert!(authorize(Autonomy::Suggest, &reads(), None, &call("read_file")).await.is_ok());
        let refused = authorize(Autonomy::Suggest, &reads(), None, &call("write_file")).await.unwrap_err().to_string();
        assert!(refused.contains("only suggests") && refused.contains("write_file"), "{refused}");
    }

    #[tokio::test]
    async fn level_three_asks_before_a_change_and_not_before_a_read() {
        let says = Says::new(Answer::Once);
        let approver: Arc<dyn Approver> = says.clone();
        assert!(authorize(Autonomy::AskFirst, &reads(), Some(&approver), &call("read_file")).await.is_ok());
        assert!(says.asked.lock().unwrap().is_empty());

        assert!(authorize(Autonomy::AskFirst, &reads(), Some(&approver), &call("write_file")).await.is_ok());
        let asked = says.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0.target, "write_file");
        assert_eq!(asked[0].0.action, TOOL_CALL_ACTION);
        assert!(asked[0].0.detail.contains("a.txt"));
        assert_eq!(asked[0].1.as_deref(), Some("write_file"));
    }

    #[tokio::test]
    async fn level_three_runs_nothing_on_a_no_or_without_anyone_to_ask() {
        let no: Arc<dyn Approver> = Says::new(Answer::Reject);
        assert!(authorize(Autonomy::AskFirst, &reads(), Some(&no), &call("write_file")).await.is_err());
        let refused = authorize(Autonomy::AskFirst, &reads(), None, &call("write_file")).await.unwrap_err().to_string();
        assert!(refused.contains("can't ask"), "{refused}");
    }

    #[tokio::test]
    async fn level_three_gives_up_when_nobody_answers() {
        struct Silent;
        #[async_trait]
        impl Approver for Silent {
            async fn approve(&self, _request: ApprovalRequest) -> bool {
                std::future::pending().await
            }
            async fn ask(&self, _request: ApprovalRequest, _always: Option<&str>) -> Answer {
                std::future::pending().await
            }
        }
        let approver: Arc<dyn Approver> = Arc::new(Silent);
        let refused = authorize_within(Autonomy::AskFirst, &reads(), &[], None, Some(&approver), &call("write_file"), Duration::from_millis(30))
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("in time"), "{refused}");
    }

    #[test]
    fn a_long_argument_list_is_cut_in_the_question() {
        let long = summarize(&json!({ "text": "x".repeat(2000) }));
        assert!(long.chars().count() <= DETAIL_LIMIT + 1 && long.ends_with('…'));
    }
}
