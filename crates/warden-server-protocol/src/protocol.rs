use serde::{Deserialize, Serialize};
use serde_json::Value;
use warden_core::model::{Attachment, Usage};
use warden_core::skill::Skill;
use warden_core::tool::ToolSpec;

/// A skill (P16) on the wire — what the browser extension's Skills screen lists and edits over
/// `ListSkills`/`SaveSkill`. `agents` is `#[serde(default)]` so a client that never touches the
/// agent restriction (P72 c) can omit it; the server keeps the stored restriction on an edit then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub agents: Vec<String>,
}

impl From<Skill> for SkillDto {
    fn from(skill: Skill) -> Self {
        Self { name: skill.name, description: skill.description, body: skill.body, agents: skill.agents }
    }
}

/// Who said a message in a `History` reply (P40) — the same two roles the persisted conversation
/// ever holds (`warden_bootstrap::ChatRole`, which this crate can't depend on without a cycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryRole {
    User,
    Assistant,
}

/// One persisted message of this device's conversation, as sent back by `History` (P40). Only
/// what a chat transcript renders — usage/generated file paths stay on the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    pub role: HistoryRole,
    pub content: String,
    pub created_at: i64,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// One of a device's conversations in a `ConversationList` (P78) — enough for a sidebar; the
/// messages themselves come from `RequestHistory`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Messages sent from a client (mobile, desktop-as-client, browser extension) to the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClientMessage {
    Hello {
        device_id: String,
        device_name: String,
        /// The hub's shared *pairing* key (P36) — only needed to pair (no `device_token` yet, or
        /// the one held was rejected). May be empty when `device_token` is set.
        #[serde(default)]
        auth_key: String,
        /// The per-device token this hub issued in an earlier `HelloAck` (P36). Keeps working
        /// after the operator rotates the pairing key; stops working once the device is revoked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_token: Option<String>,
        /// Local tools this client can execute on request (Fase 7.4) — e.g. mobile's
        /// `list_files`/`read_file`. `#[serde(default)]` so a client that predates this (or
        /// simply has none configured, like the desktop-as-client) doesn't need to send anything;
        /// `Server` only builds the remote-tool-dispatch machinery when this is non-empty.
        #[serde(default)]
        tools: Vec<ToolSpec>,
    },
    Ping {
        nonce: u64,
    },
    /// A chat turn (Fase 7.3) — answered by the `Orchestrator` `Server` now hosts, appended to one
    /// of this device's conversations. `conversation_id` picks which (P78): an id the hub has never
    /// seen starts a new conversation, titled from this first message; `None` means the device's
    /// default conversation, the only one a client from before P78 ever used.
    Chat {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    /// The result of a `ServerMessage::ToolCallRequest` this client was asked to run (Fase 7.4).
    ToolCallResult {
        call_id: u64,
        result: Value,
    },
    /// The client failed to run a requested tool call (Fase 7.4) — same "carry the real error
    /// text" posture as `ServerMessage::ChatError`.
    ToolCallError {
        call_id: u64,
        message: String,
    },
    /// Asks the server to route a tool call to a *different* connected device (Fase 9.3/9.4) —
    /// unlike `Hello.tools`/`ToolCallRequest` (Fase 7.4, always a round-trip back to the same
    /// connection that advertised the tool), this lets any connected client reach a specific
    /// other one by `target_device_id`. `call_id` is this connection's own id (allocated the same
    /// way `Ping`'s `nonce` is, by the caller) — echoed back on the matching
    /// `ServerMessage::DeviceToolResult`/`DeviceToolError` so concurrent calls stay correlated.
    /// The server never inspects `tool`/`arguments`; only the target device's own code decides
    /// what they mean.
    CallDeviceTool {
        call_id: u64,
        target_device_id: String,
        tool: String,
        arguments: Value,
    },
    /// Skills management (P72) — list the skills in the vault this server hosts. `request_id` is
    /// the caller's own correlation id (same idea as `CallDeviceTool.call_id`), echoed back on the
    /// matching `SkillList`/`SkillError` so concurrent requests stay paired.
    ListSkills {
        request_id: u64,
    },
    /// Creates (`overwrite: false`, refuses a taken name) or edits (`overwrite: true`) a skill.
    /// An edit whose `skill.agents` is empty keeps the agent restriction already on disk — a client
    /// with no UI for it must not silently make a restricted skill global again.
    SaveSkill {
        request_id: u64,
        skill: SkillDto,
        overwrite: bool,
    },
    DeleteSkill {
        request_id: u64,
        name: String,
    },
    /// Asks for this device's persisted conversation (P40) — the one `Chat` turns are appended to,
    /// keyed by `Hello.device_id`, so a client that reconnects (or was restarted) can show what was
    /// already said. Answered by `History`/`HistoryError` with the same `request_id`. `limit` keeps
    /// only the most recent messages; `None` returns all of them.
    RequestHistory {
        request_id: u64,
        #[serde(default)]
        limit: Option<u32>,
        /// Which conversation (P78) — `None` is the device's default one, same as `Chat`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    /// Lists this device's conversations (P78), newest-updated first. Answered by
    /// `ConversationList`/`ConversationError` with the same `request_id`.
    ListConversations {
        request_id: u64,
    },
    /// Renames one of this device's conversations. Answered by `ConversationOk`/`ConversationError`.
    RenameConversation {
        request_id: u64,
        conversation_id: String,
        title: String,
    },
    /// Deletes one of this device's conversations. Answered by `ConversationOk`/`ConversationError`.
    DeleteConversation {
        request_id: u64,
        conversation_id: String,
    },
    /// An unauthenticated presence probe (Fase 9.1 redefined — LAN discovery, not the
    /// authenticated connection Hello starts). No `auth_key`/`device_id` on purpose: the whole
    /// point is finding a hub *before* knowing its credential. Answered by `DiscoverAck` and the
    /// connection closes right after — never reaches `Hello`'s device-registry bookkeeping.
    Discover,
    Goodbye {
        reason: Option<String>,
    },
}

/// Messages sent from the server to a connected client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ServerMessage {
    HelloAck {
        server_name: String,
        /// A newly issued per-device token (P36), present whenever this `Hello` paired with the
        /// pairing key instead of an existing token. The client must store it (keyed by hub and
        /// `device_id`) and send it as `Hello.device_token` from then on — it replaces any token
        /// it held before, which no longer works.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_token: Option<String>,
    },
    AuthError {
        reason: String,
    },
    Pong {
        nonce: u64,
    },
    /// The model's answer to a `Chat` message.
    ChatResponse {
        content: String,
        usage: Option<Usage>,
        /// Media extracted from an MCP tool result during this turn (P64 frente 2 fatia 3).
        /// `#[serde(default)]` so a peer from before this field existed (older client build, or a
        /// stored fixture) still parses.
        #[serde(default)]
        attachments: Vec<Attachment>,
        /// The conversation this answer belongs to (P78) — the `Chat.conversation_id` it answers,
        /// resolved (the default conversation's id when that was `None`). `Chat` carries no request
        /// id, so this is what lets a client with several conversations route a late answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    /// A `Chat` message failed (missing API key, rate limit, provider error, ...) — the raw error
    /// text, since this protocol has no untrusted-public-bot audience to hide it from.
    ChatError {
        message: String,
        /// Same as `ChatResponse.conversation_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    /// Asks a connected client to run one of the tools it advertised in `Hello.tools` (Fase 7.4).
    /// `call_id` is scoped to this connection (a simple counter, mirrors `Ping`'s `nonce`) — the
    /// client echoes it back on `ToolCallResult`/`ToolCallError` so the server can correlate the
    /// reply even if several calls are in flight at once.
    ToolCallRequest {
        call_id: u64,
        tool: String,
        arguments: Value,
    },
    /// Reply to a `ClientMessage::CallDeviceTool` (Fase 9.4) — the target device answered. Same
    /// `call_id` the caller allocated for that request.
    DeviceToolResult {
        call_id: u64,
        result: Value,
    },
    /// A routed `CallDeviceTool` didn't succeed — covers both "the target device isn't connected"
    /// and "the target device ran the tool but it failed", same "one error variant, descriptive
    /// text" posture as `ChatError`; the caller has no separate branch to handle differently
    /// between those two cases anyway.
    DeviceToolError {
        call_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::ListSkills`.
    SkillList {
        request_id: u64,
        skills: Vec<SkillDto>,
    },
    /// Reply to a successful `SaveSkill`/`DeleteSkill`.
    SkillOk {
        request_id: u64,
    },
    /// A `ListSkills`/`SaveSkill`/`DeleteSkill` failed (invalid skill, name taken, no such skill) —
    /// the raw error text, same posture as `ChatError`.
    SkillError {
        request_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::RequestHistory`, oldest message first. Empty when this device never
    /// chatted before.
    History {
        request_id: u64,
        messages: Vec<HistoryMessage>,
    },
    /// The conversation file exists but couldn't be read/parsed — the raw error text, same posture
    /// as `SkillError`.
    HistoryError {
        request_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::ListConversations`.
    ConversationList {
        request_id: u64,
        conversations: Vec<ConversationSummary>,
    },
    /// Reply to a successful `RenameConversation`/`DeleteConversation`.
    ConversationOk {
        request_id: u64,
    },
    /// A `ListConversations`/`RenameConversation`/`DeleteConversation` failed (invalid id, no such
    /// conversation, unreadable directory) — the raw error text, same posture as `SkillError`.
    ConversationError {
        request_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::Discover` — just enough for a sweeping client to show the operator
    /// "which machine is this" and let them pick it, never a secret.
    DiscoverAck {
        server_name: String,
        /// Set when this hub only accepts encrypted connections (P36): the `wss://` URL (a name
        /// its certificate is valid for, e.g. the Tailscale MagicDNS one) a client must use for
        /// `Hello` — a plain `ws://` Hello is refused before the upgrade even completes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secure_url: Option<String>,
    },
    Goodbye {
        reason: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_round_trips_through_json() {
        let msg = ClientMessage::Hello {
            device_id: "dev-1".into(),
            device_name: "Test Device".into(),
            auth_key: "secret".into(),
            device_token: None,
            tools: Vec::new(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret","tools":[]}"#
        );
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn client_hello_with_only_a_device_token_parses() {
        let json = r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","deviceToken":"tok"}"#;
        let msg = serde_json::from_str::<ClientMessage>(json).unwrap();
        assert!(matches!(msg, ClientMessage::Hello { auth_key, device_token: Some(t), .. } if auth_key.is_empty() && t == "tok"));
    }

    #[test]
    fn server_hello_ack_carries_an_issued_token_only_when_there_is_one() {
        let without = ServerMessage::HelloAck { server_name: "Hub".into(), device_token: None };
        assert_eq!(serde_json::to_string(&without).unwrap(), r#"{"type":"helloAck","serverName":"Hub"}"#);

        let with = ServerMessage::HelloAck { server_name: "Hub".into(), device_token: Some("tok".into()) };
        let json = serde_json::to_string(&with).unwrap();
        assert_eq!(json, r#"{"type":"helloAck","serverName":"Hub","deviceToken":"tok"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), with);
    }

    #[test]
    fn client_hello_without_a_tools_field_defaults_to_empty() {
        // A client written before Fase 7.4 (or one with nothing to advertise) never sends `tools`
        // at all — must still parse, not error.
        let json = r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret"}"#;
        let msg = serde_json::from_str::<ClientMessage>(json).unwrap();
        assert!(matches!(msg, ClientMessage::Hello { tools, .. } if tools.is_empty()));
    }

    #[test]
    fn client_hello_with_advertised_tools_round_trips_through_json() {
        let msg = ClientMessage::Hello {
            device_id: "dev-1".into(),
            device_name: "Test Device".into(),
            auth_key: "secret".into(),
            device_token: None,
            tools: vec![ToolSpec {
                name: "list_files".into(),
                description: "List files".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret","tools":[{"name":"list_files","description":"List files","parameters":{"type":"object"}}]}"#
        );
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn client_tool_call_result_round_trips_through_json() {
        let msg = ClientMessage::ToolCallResult { call_id: 7, result: serde_json::json!({"ok": true}) };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"toolCallResult","callId":7,"result":{"ok":true}}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn client_tool_call_error_round_trips_through_json() {
        let msg = ClientMessage::ToolCallError { call_id: 7, message: "boom".into() };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"toolCallError","callId":7,"message":"boom"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_auth_error_round_trips_through_json() {
        let msg = ServerMessage::AuthError {
            reason: "invalid auth key".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"authError","reason":"invalid auth key"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn client_call_device_tool_round_trips_through_json() {
        let msg = ClientMessage::CallDeviceTool {
            call_id: 1,
            target_device_id: "dev-2".into(),
            tool: "vault_read".into(),
            arguments: serde_json::json!({"path": "notes/a.md"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"callDeviceTool","callId":1,"targetDeviceId":"dev-2","tool":"vault_read","arguments":{"path":"notes/a.md"}}"#
        );
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_device_tool_result_round_trips_through_json() {
        let msg = ServerMessage::DeviceToolResult { call_id: 1, result: serde_json::json!({"content": "hi"}) };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"deviceToolResult","callId":1,"result":{"content":"hi"}}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_device_tool_error_round_trips_through_json() {
        let msg = ServerMessage::DeviceToolError { call_id: 1, message: "device 'dev-2' is not connected".into() };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"deviceToolError","callId":1,"message":"device 'dev-2' is not connected"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_chat_response_with_an_attachment_round_trips_through_json() {
        let msg = ServerMessage::ChatResponse {
            content: "here you go".into(),
            usage: None,
            attachments: vec![Attachment { mime_type: "image/png".into(), data: "aGVsbG8=".into() }],
            conversation_id: Some("c1".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"chatResponse","content":"here you go","usage":null,"attachments":[{"mimeType":"image/png","data":"aGVsbG8="}],"conversationId":"c1"}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_chat_response_without_an_attachments_field_defaults_to_empty() {
        // A peer from before this field existed (an older client build, or a stored fixture)
        // never sends `attachments` at all — must still parse, not error.
        let json = r#"{"type":"chatResponse","content":"hi","usage":null}"#;
        let msg = serde_json::from_str::<ServerMessage>(json).unwrap();
        assert!(matches!(msg, ServerMessage::ChatResponse { attachments, .. } if attachments.is_empty()));
    }

    #[test]
    fn client_skill_messages_round_trip_through_json() {
        let list = ClientMessage::ListSkills { request_id: 1 };
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(json, r#"{"type":"listSkills","requestId":1}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), list);

        let save = ClientMessage::SaveSkill {
            request_id: 2,
            skill: SkillDto { name: "review-pr".into(), description: "d".into(), body: "b".into(), agents: vec!["writer".into()] },
            overwrite: true,
        };
        let json = serde_json::to_string(&save).unwrap();
        assert_eq!(
            json,
            r#"{"type":"saveSkill","requestId":2,"skill":{"name":"review-pr","description":"d","body":"b","agents":["writer"]},"overwrite":true}"#
        );
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), save);

        let delete = ClientMessage::DeleteSkill { request_id: 3, name: "review-pr".into() };
        let json = serde_json::to_string(&delete).unwrap();
        assert_eq!(json, r#"{"type":"deleteSkill","requestId":3,"name":"review-pr"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), delete);
    }

    #[test]
    fn save_skill_without_an_agents_field_defaults_to_empty() {
        let json = r#"{"type":"saveSkill","requestId":2,"skill":{"name":"x","description":"d","body":"b"},"overwrite":false}"#;
        let msg = serde_json::from_str::<ClientMessage>(json).unwrap();
        assert!(matches!(msg, ClientMessage::SaveSkill { skill, .. } if skill.agents.is_empty()));
    }

    #[test]
    fn server_skill_messages_round_trip_through_json() {
        let list = ServerMessage::SkillList {
            request_id: 1,
            skills: vec![SkillDto { name: "x".into(), description: "d".into(), body: "b".into(), agents: Vec::new() }],
        };
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(
            json,
            r#"{"type":"skillList","requestId":1,"skills":[{"name":"x","description":"d","body":"b","agents":[]}]}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), list);

        let ok = ServerMessage::SkillOk { request_id: 2 };
        let json = serde_json::to_string(&ok).unwrap();
        assert_eq!(json, r#"{"type":"skillOk","requestId":2}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), ok);

        let err = ServerMessage::SkillError { request_id: 3, message: "no skill named 'x'".into() };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"type":"skillError","requestId":3,"message":"no skill named 'x'"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), err);
    }

    #[test]
    fn history_messages_round_trip_through_json() {
        let request = ClientMessage::RequestHistory { request_id: 1, limit: Some(50), conversation_id: None };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, r#"{"type":"requestHistory","requestId":1,"limit":50}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), request);

        let reply = ServerMessage::History {
            request_id: 1,
            messages: vec![HistoryMessage { role: HistoryRole::User, content: "hi".into(), created_at: 7, attachments: Vec::new() }],
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            json,
            r#"{"type":"history","requestId":1,"messages":[{"role":"user","content":"hi","createdAt":7,"attachments":[]}]}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), reply);

        let err = ServerMessage::HistoryError { request_id: 1, message: "boom".into() };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"type":"historyError","requestId":1,"message":"boom"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), err);
    }

    #[test]
    fn request_history_without_a_limit_field_means_everything() {
        let msg = serde_json::from_str::<ClientMessage>(r#"{"type":"requestHistory","requestId":2}"#).unwrap();
        assert_eq!(msg, ClientMessage::RequestHistory { request_id: 2, limit: None, conversation_id: None });
    }

    #[test]
    fn a_chat_from_before_conversations_existed_has_no_conversation_id() {
        let msg = serde_json::from_str::<ClientMessage>(r#"{"type":"chat","message":"hi"}"#).unwrap();
        assert_eq!(msg, ClientMessage::Chat { message: "hi".into(), conversation_id: None });
        assert_eq!(serde_json::to_string(&msg).unwrap(), r#"{"type":"chat","message":"hi"}"#);

        let with = ClientMessage::Chat { message: "hi".into(), conversation_id: Some("c1".into()) };
        let json = serde_json::to_string(&with).unwrap();
        assert_eq!(json, r#"{"type":"chat","message":"hi","conversationId":"c1"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), with);
    }

    #[test]
    fn conversation_messages_round_trip_through_json() {
        let cases: Vec<(ClientMessage, &str)> = vec![
            (ClientMessage::ListConversations { request_id: 1 }, r#"{"type":"listConversations","requestId":1}"#),
            (
                ClientMessage::RenameConversation { request_id: 2, conversation_id: "c1".into(), title: "Trip".into() },
                r#"{"type":"renameConversation","requestId":2,"conversationId":"c1","title":"Trip"}"#,
            ),
            (
                ClientMessage::DeleteConversation { request_id: 3, conversation_id: "c1".into() },
                r#"{"type":"deleteConversation","requestId":3,"conversationId":"c1"}"#,
            ),
        ];
        for (msg, expected) in cases {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
        }

        let replies: Vec<(ServerMessage, &str)> = vec![
            (
                ServerMessage::ConversationList {
                    request_id: 1,
                    conversations: vec![ConversationSummary { id: "c1".into(), title: "Trip".into(), created_at: 1, updated_at: 2 }],
                },
                r#"{"type":"conversationList","requestId":1,"conversations":[{"id":"c1","title":"Trip","createdAt":1,"updatedAt":2}]}"#,
            ),
            (ServerMessage::ConversationOk { request_id: 2 }, r#"{"type":"conversationOk","requestId":2}"#),
            (
                ServerMessage::ConversationError { request_id: 3, message: "boom".into() },
                r#"{"type":"conversationError","requestId":3,"message":"boom"}"#,
            ),
        ];
        for (msg, expected) in replies {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
        }
    }

    #[test]
    fn client_discover_round_trips_through_json() {
        let msg = ClientMessage::Discover;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"discover"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_discover_ack_round_trips_through_json() {
        let msg = ServerMessage::DiscoverAck { server_name: "Fabio's Desktop".into(), secure_url: None };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"discoverAck","serverName":"Fabio's Desktop"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_discover_ack_carries_the_secure_url_only_when_there_is_one() {
        let msg = ServerMessage::DiscoverAck { server_name: "hub".into(), secure_url: Some("wss://hub.tail1234.ts.net:7420".into()) };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"discoverAck","serverName":"hub","secureUrl":"wss://hub.tail1234.ts.net:7420"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_tool_call_request_round_trips_through_json() {
        let msg = ServerMessage::ToolCallRequest {
            call_id: 3,
            tool: "read_file".into(),
            arguments: serde_json::json!({"path": "abc"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"toolCallRequest","callId":3,"tool":"read_file","arguments":{"path":"abc"}}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }
}
