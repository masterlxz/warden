use serde::{Deserialize, Serialize};
use warden_core::model::Usage;

/// Messages sent from a client (mobile, desktop-as-client, browser extension) to the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClientMessage {
    Hello {
        device_id: String,
        device_name: String,
        auth_key: String,
    },
    Ping {
        nonce: u64,
    },
    /// A chat turn (Fase 7.3) — answered by the `Orchestrator` `Server` now hosts, keyed by this
    /// connection's `device_id` (one conversation per device, same pattern as Telegram's
    /// `chat_id`/WhatsApp's JID).
    Chat {
        message: String,
    },
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
    },
    /// A `Chat` message failed (missing API key, rate limit, provider error, ...) — the raw error
    /// text, since this protocol has no untrusted-public-bot audience to hide it from.
    ChatError {
        message: String,
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
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret"}"#
        );
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
}
