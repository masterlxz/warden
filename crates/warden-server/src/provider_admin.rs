//! Checking a model provider's key from a client (P10): what the web's "Test key" button asks. The check itself is
//! `warden_bootstrap::test_provider`, a model-list request that costs no tokens and never goes through an
//! orchestrator, so it books nothing in the spend ledger.
//!
//! What makes it safe for a hub to do on a client's say-so:
//! - it needs the pairing key (with the same 1 s wait on a wrong one), and is the root's only;
//! - a key typed into the form travels only over an encrypted or local connection, like a key saved from there;
//! - the key the client never had (`Keep`) is the one *saved* for the provider it names, never echoed back;
//! - the hub only asks an address it **already has saved**: a new or changed `base_url` is refused ("save it first"),
//!   so a paired device can't make the hub call somewhere inside its network that the owner never configured;
//! - the answer is a word and a sentence, never the key, the address or what the provider said.
//!
//! The settings lock is held only while the pairing key is checked, not across the call to the provider, which can
//! take seconds and would block every settings save meanwhile.

use warden_bootstrap::settings::provider_kind_from_str;
use warden_bootstrap::{load_config_from_path, test_provider, Provider, ProviderConfig};
use warden_server_protocol::protocol::{ProviderEditDto, SecretEdit};
use warden_server_protocol::ServerMessage;

use crate::settings::{keys_match, SettingsHost, WRONG_KEY_DELAY};

const NO_SETTINGS: &str = "this hub has no settings file, so it has no providers to test";

fn error(request_id: u64, message: impl Into<String>, auth_rejected: bool) -> ServerMessage {
    ServerMessage::UserError { request_id, message: message.into(), auth_rejected }
}

fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Answers `TestProvider`. `secure`: whether this connection may carry a key (`settings::is_secure`).
pub async fn handle_test_provider(
    settings: Option<&dyn SettingsHost>,
    lock: &tokio::sync::Mutex<()>,
    auth_key: &str,
    secure: bool,
    request_id: u64,
    pairing_key: &str,
    edit: ProviderEditDto,
) -> ServerMessage {
    let Some(settings) = settings else { return error(request_id, NO_SETTINGS, false) };
    {
        // Only the pairing key is checked under the lock, so wrong guesses are serialized and slow; the call to the provider isn't.
        let _serialized = lock.lock().await;
        if !keys_match(pairing_key, auth_key) {
            tokio::time::sleep(WRONG_KEY_DELAY).await;
            return error(request_id, "wrong pairing key", true);
        }
    }
    if edit.api_key.is_set() && !secure {
        return error(request_id, "A key typed here can only be tested over an encrypted connection (https://) or from the hub's own machine — nothing was sent.", false);
    }
    let config = match load_config_from_path(&settings.config_path(), false) {
        Ok(config) => config,
        Err(err) => return error(request_id, format!("{err:#}"), false),
    };
    let kind = match provider_kind_from_str(edit.kind.trim()) {
        Ok(kind) => kind,
        Err(message) => return error(request_id, message, false),
    };

    let saved = edit.original_id.as_deref().and_then(|id| config.providers.iter().find(|p| p.id == id));
    let api_key = match &edit.api_key {
        SecretEdit::Keep => saved.and_then(|p| p.api_key.clone()),
        SecretEdit::Set(value) => non_empty(value),
        SecretEdit::Clear => None,
    };
    // Only a compatible server has an address of its own; the others are asked at their provider's.
    let base_url = if kind == Provider::OpenaiCompatible {
        let typed = non_empty(&edit.base_url);
        let saved_url = saved.and_then(|p| p.base_url.as_deref()).and_then(non_empty);
        if typed.is_some() && typed != saved_url {
            return error(request_id, "Save the provider first: the hub only tests an address that is already saved, so a paired device can't make it call a new one.", false);
        }
        if typed.as_deref().is_some_and(|url| !(url.starts_with("http://") || url.starts_with("https://"))) {
            return error(request_id, "The address has to start with http:// or https://.", false);
        }
        typed
    } else {
        None
    };

    let provider = ProviderConfig { id: edit.id.trim().to_string(), kind, api_key, base_url, model: non_empty(&edit.model), node: non_empty(&edit.node) };
    let result = test_provider(&provider).await;
    ServerMessage::ProviderTest { request_id, ok: result.is_ok(), kind: result.kind().to_string(), message: result.message() }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use warden_bootstrap::{save_config, FileConfig};
    use warden_core::orchestrator::Orchestrator;

    use super::*;

    const KEY: &str = "pairing-key-0123456789";

    struct Host(PathBuf);

    #[async_trait]
    impl SettingsHost for Host {
        fn config_path(&self) -> PathBuf {
            self.0.clone()
        }

        async fn build(&self) -> anyhow::Result<Orchestrator> {
            anyhow::bail!("a key test never rebuilds the hub")
        }
    }

    /// A server on a free local port answering every request with `status` and a body that echoes the key it would
    /// have been sent, from a thread of its own; remembers each request head.
    fn answers_with(status: u16) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let heads: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = heads.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                seen.lock().unwrap().push(String::from_utf8_lossy(&buffer[..read]).to_lowercase());
                let body = r#"{"error":"Incorrect API key provided: sk-saved"}"#;
                let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            }
        });
        (format!("http://{addr}/v1"), heads)
    }

    /// A hub whose config saves one `openai_compatible` provider, `local`, at `base_url`, with the key `sk-saved`.
    fn hub_with_local_provider(name: &str, base_url: &str) -> Host {
        let dir = std::env::temp_dir().join(format!("warden-provider-admin-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let config = FileConfig {
            providers: vec![ProviderConfig { id: "local".into(), kind: Provider::OpenaiCompatible, api_key: Some("sk-saved".into()), base_url: Some(base_url.into()), model: Some("m".into()), node: None }],
            ..FileConfig::default()
        };
        save_config(&path, &config).unwrap();
        Host(path)
    }

    fn edit(id: &str, original: Option<&str>, key: SecretEdit, base_url: &str) -> ProviderEditDto {
        ProviderEditDto { original_id: original.map(String::from), id: id.into(), kind: "openai_compatible".into(), base_url: base_url.into(), model: "m".into(), api_key: key, node: String::new() }
    }

    async fn test(host: &Host, secure: bool, key: &str, provider: ProviderEditDto) -> ServerMessage {
        let lock = tokio::sync::Mutex::new(());
        handle_test_provider(Some(host), &lock, KEY, secure, 1, key, provider).await
    }

    fn refusal(reply: ServerMessage) -> (String, bool) {
        match reply {
            ServerMessage::UserError { message, auth_rejected, .. } => (message, auth_rejected),
            other => panic!("a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn keep_tests_the_saved_key_even_for_a_renamed_provider_and_the_answer_leaks_nothing() {
        let (url, heads) = answers_with(200);
        let host = hub_with_local_provider("keep", &url);
        let reply = test(&host, true, KEY, edit("renamed", Some("local"), SecretEdit::Keep, &url)).await;
        let ServerMessage::ProviderTest { ok, kind, .. } = &reply else { panic!("{reply:?}") };
        assert_eq!((*ok, kind.as_str()), (true, "ok"));
        assert!(heads.lock().unwrap()[0].contains("authorization: bearer sk-saved"), "the key the client never had: the saved one, found by the old id");

        let (url, _) = answers_with(401);
        let host = hub_with_local_provider("rejected", &url);
        let reply = test(&host, true, KEY, edit("local", Some("local"), SecretEdit::Keep, &url)).await;
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains(r#""kind":"rejected""#), "{json}");
        assert!(!json.contains("sk-saved") && !json.contains("Incorrect") && !json.contains("127.0.0.1"), "the answer carries no key, no body and no address: {json}");
    }

    #[tokio::test]
    async fn a_typed_key_is_tested_instead_of_the_saved_one_but_only_over_a_secure_connection() {
        let (url, heads) = answers_with(200);
        let host = hub_with_local_provider("typed", &url);
        let (message, auth_rejected) = refusal(test(&host, false, KEY, edit("local", Some("local"), SecretEdit::Set("sk-typed".into()), &url)).await);
        assert!(message.contains("encrypted connection") && !auth_rejected, "{message}");
        assert!(heads.lock().unwrap().is_empty(), "refused before anything was sent");

        let reply = test(&host, true, KEY, edit("local", Some("local"), SecretEdit::Set("  sk-typed ".into()), &url)).await;
        assert!(matches!(reply, ServerMessage::ProviderTest { ok: true, .. }), "{reply:?}");
        assert!(heads.lock().unwrap()[0].contains("authorization: bearer sk-typed"), "what was typed, trimmed, not the saved key");
        // Keep needs no secure connection: nothing typed travels.
        let reply = test(&host, false, KEY, edit("local", Some("local"), SecretEdit::Keep, &url)).await;
        assert!(matches!(reply, ServerMessage::ProviderTest { ok: true, .. }), "{reply:?}");
    }

    #[tokio::test]
    async fn the_hub_only_asks_an_address_it_has_already_saved() {
        let (url, heads) = answers_with(200);
        let host = hub_with_local_provider("ssrf", &url);
        for (what, provider) in [
            ("a changed address", edit("local", Some("local"), SecretEdit::Keep, "http://169.254.169.254/latest")),
            ("a new provider with an address", edit("fresh", None, SecretEdit::Set("k".into()), "http://127.0.0.1:1/v1")),
            ("a saved provider, another address", edit("local", Some("nope"), SecretEdit::Keep, &url)),
        ] {
            let (message, auth_rejected) = refusal(test(&host, true, KEY, provider).await);
            assert!(message.contains("Save the provider first") && !auth_rejected, "{what}: {message}");
        }
        assert!(heads.lock().unwrap().is_empty(), "nothing was sent to the saved address either: it was the others that were refused");
        // The saved address, as saved, is fine; a scheme other than http(s) is not, even when it is "saved".
        let reply = test(&host, true, KEY, edit("local", Some("local"), SecretEdit::Keep, &url)).await;
        assert!(matches!(reply, ServerMessage::ProviderTest { ok: true, .. }));
        let host = hub_with_local_provider("scheme", "ftp://example.com/v1");
        let (message, _) = refusal(test(&host, true, KEY, edit("local", Some("local"), SecretEdit::Keep, "ftp://example.com/v1")).await);
        assert!(message.contains("http:// or https://"), "{message}");
    }

    #[tokio::test]
    async fn a_wrong_pairing_key_is_turned_away_before_anything_else_and_a_hub_without_settings_says_so() {
        let (url, heads) = answers_with(200);
        let host = hub_with_local_provider("key", &url);
        let (_, auth_rejected) = refusal(test(&host, true, "wrong", edit("local", Some("local"), SecretEdit::Keep, &url)).await);
        assert!(auth_rejected);
        assert!(heads.lock().unwrap().is_empty());
        let lock = tokio::sync::Mutex::new(());
        let (message, _) = refusal(handle_test_provider(None, &lock, KEY, true, 2, KEY, edit("local", None, SecretEdit::Keep, "")).await);
        assert!(message.contains("no settings file"), "{message}");
    }
}
