//! The Settings screen's "Test key" button for a model provider (P10): `test_provider_key`. It takes the provider
//! as the form has it, key included (typed but maybe not saved yet, like `test_ssh_host` takes an unsaved host), asks
//! the provider for its model list and reports what came back in a few words. It never goes through the orchestrator,
//! so it books nothing in the spend ledger and counts against no limit.

use serde::{Deserialize, Serialize};
use warden_bootstrap::{test_provider, Provider, ProviderConfig};

/// A provider as the Settings form holds it: every "not set" is an empty string, like the rest of this IPC boundary.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProviderToTest {
    id: String,
    kind: Provider,
    api_key: String,
    base_url: String,
    model: String,
    /// Kind "node" only (P93): the node's device id. Such a provider has no key to test.
    #[serde(default)]
    node: String,
}

/// What testing came to: `ok` only when the provider accepted the key. `kind` is the word the screen maps to a mark
/// (`ok`, `unverifiable`, `rejected`, `rate_limited`, `provider_down`, `unreachable`, `unsupported`).
#[derive(Serialize, Debug, PartialEq)]
pub struct KeyTestPayload {
    ok: bool,
    kind: String,
    message: String,
}

fn blank_is_none(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Tests `provider` the way a chat would build it. A blank key is "no key" (so a provider that needs one says so),
/// not an empty key to send.
async fn check(provider: ProviderToTest) -> KeyTestPayload {
    let config = ProviderConfig {
        id: provider.id.trim().to_string(),
        kind: provider.kind,
        api_key: blank_is_none(provider.api_key),
        base_url: blank_is_none(provider.base_url),
        model: blank_is_none(provider.model),
        node: blank_is_none(provider.node),
    };
    let result = test_provider(&config).await;
    KeyTestPayload { ok: result.is_ok(), kind: result.kind().to_string(), message: result.message() }
}

#[tauri::command]
pub async fn test_provider_key(provider: ProviderToTest) -> KeyTestPayload {
    check(provider).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server on a free local port that answers every request with `status` and a body that echoes a key, from a
    /// thread of its own, and remembers the request heads it got.
    fn answers_with(status: u16) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let heads: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let seen = heads.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                seen.lock().unwrap().push(String::from_utf8_lossy(&buffer[..read]).to_lowercase());
                let body = r#"{"error":"Incorrect API key provided: sk-secret"}"#;
                let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            }
        });
        (format!("http://{addr}/v1"), heads)
    }

    fn form(json: serde_json::Value) -> ProviderToTest {
        serde_json::from_value(json).unwrap()
    }

    /// The names the frontend (`ProviderEntry` in `types.ts`) sends and reads, and the outcomes the button shows.
    #[tokio::test]
    async fn the_form_is_tested_as_typed_and_the_answer_is_told_in_a_few_words() {
        let (ok_url, heads) = answers_with(200);
        let accepted = check(form(serde_json::json!({ "id": "local", "kind": "openai_compatible", "apiKey": " sk-secret ", "baseUrl": ok_url, "model": "m" }))).await;
        assert_eq!((accepted.ok, accepted.kind.as_str()), (true, "ok"));
        let head = heads.lock().unwrap()[0].clone();
        assert!(head.starts_with("get /v1/models ") && head.contains("authorization: bearer sk-secret"), "the typed key, trimmed, is what was sent: {head}");
        assert_eq!(serde_json::to_value(&accepted).unwrap()["kind"], "ok");

        let (bad_url, _) = answers_with(401);
        let rejected = check(form(serde_json::json!({ "id": "local", "kind": "openai_compatible", "apiKey": "sk-secret", "baseUrl": bad_url, "model": "m" }))).await;
        assert_eq!((rejected.ok, rejected.kind.as_str()), (false, "rejected"));
        assert!(!rejected.message.contains("sk-secret") && !rejected.message.contains("Incorrect"), "{}", rejected.message);
    }

    #[tokio::test]
    async fn a_form_that_cannot_be_built_says_what_is_missing_and_a_node_has_no_key() {
        let missing = check(form(serde_json::json!({ "id": "g", "kind": "gemini", "apiKey": "   ", "baseUrl": "", "model": "" }))).await;
        assert_eq!((missing.ok, missing.kind.as_str()), (false, "rejected"));
        assert!(missing.message.contains("no API key"), "a blank key is no key, not an empty one to send: {}", missing.message);

        let node = check(form(serde_json::json!({ "id": "casa", "kind": "node", "apiKey": "", "baseUrl": "", "model": "ollama", "node": "node-casa" }))).await;
        assert_eq!(node.kind, "unsupported");
    }
}
