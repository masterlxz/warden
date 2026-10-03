//! Checking a provider's key without spending a conversation (P10): a `GET` of the provider's model list, which
//! every one of them answers only to a key it accepts, and which costs no tokens. What came back is told in a few
//! words and never with the response body (it can echo the request), nor with the address that was called (which may
//! be somewhere inside the person's network): a test is for finding out if the key works, not for reading the provider.

use std::time::Duration;

use reqwest::redirect::Policy;

/// How long a check waits for the provider, in all.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// What checking a provider's key came to.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyCheck {
    /// The provider accepted the key.
    Accepted,
    /// The provider answered, but not in a way that vouches for the key: a local server with no model list, say.
    Unverifiable(String),
    /// The provider turned the key down.
    Rejected(String),
    /// The key is fine as far as is known, but the provider is limiting it right now.
    RateLimited(String),
    /// The provider is failing on its side (a 5xx).
    ProviderDown(String),
    /// Nothing answered: no connection, or too slow.
    Unreachable(String),
    /// This kind of provider has no key to check (a model lent by a node, a combo).
    Unsupported(String),
}

impl KeyCheck {
    /// A short word for each outcome, which clients map to a mark and a color.
    pub fn kind(&self) -> &'static str {
        match self {
            KeyCheck::Accepted => "ok",
            KeyCheck::Unverifiable(_) => "unverifiable",
            KeyCheck::Rejected(_) => "rejected",
            KeyCheck::RateLimited(_) => "rate_limited",
            KeyCheck::ProviderDown(_) => "provider_down",
            KeyCheck::Unreachable(_) => "unreachable",
            KeyCheck::Unsupported(_) => "unsupported",
        }
    }

    /// One sentence for the person.
    pub fn message(&self) -> String {
        match self {
            KeyCheck::Accepted => "The provider accepted the key.".to_string(),
            KeyCheck::Unverifiable(why) | KeyCheck::Rejected(why) | KeyCheck::RateLimited(why) | KeyCheck::ProviderDown(why) | KeyCheck::Unreachable(why) | KeyCheck::Unsupported(why) => why.clone(),
        }
    }

    /// Whether the key is known to work.
    pub fn is_ok(&self) -> bool {
        matches!(self, KeyCheck::Accepted)
    }
}

/// A client for a check: it gives up after `CHECK_TIMEOUT` and never follows a redirect, so a provider address (or a
/// lookalike) can't bounce the request, key included, to somewhere else.
pub(super) fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(CHECK_TIMEOUT).redirect(Policy::none()).build().unwrap_or_default()
}

/// Sends `request` and tells what the answer means for the key. `lenient`: a server that has no model list (a local
/// OpenAI-compatible one) isn't a failure, only a key that can't be verified.
pub(super) async fn check_models(provider: &str, request: reqwest::RequestBuilder, lenient: bool) -> KeyCheck {
    match request.send().await {
        Err(err) => KeyCheck::Unreachable(if err.is_timeout() { format!("{provider} didn't answer in time.") } else { format!("Couldn't connect to {provider}.") }),
        Ok(response) => {
            let status = response.status().as_u16();
            // Gemini says a bad key with a 400 and a body that names it: that is the only body read, and only to look for it.
            let body = if status == 400 { response.text().await.unwrap_or_default().to_lowercase() } else { String::new() };
            classify(provider, status, &body, lenient)
        }
    }
}

fn classify(provider: &str, status: u16, body_if_400: &str, lenient: bool) -> KeyCheck {
    match status {
        200..=299 => KeyCheck::Accepted,
        401 | 403 => KeyCheck::Rejected(format!("{provider} rejected the key.")),
        400 if body_if_400.contains("api key") || body_if_400.contains("api_key") => KeyCheck::Rejected(format!("{provider} rejected the key.")),
        404 | 405 if lenient => KeyCheck::Unverifiable(format!("{provider} answered, but has no model list to check the key against.")),
        429 => KeyCheck::RateLimited(format!("{provider} is limiting this key right now; it was not rejected.")),
        500..=599 => KeyCheck::ProviderDown(format!("{provider} is failing on its side ({status}).")),
        other => KeyCheck::Unverifiable(format!("{provider} answered {other}, which doesn't say whether the key works.")),
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A server on a free local port that answers every request with one canned response and remembers each request
    //! it was sent, so a test can look at the path and headers a provider used.

    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub struct Fake {
        pub addr: String,
        pub requests: Arc<Mutex<Vec<String>>>,
    }

    impl Fake {
        /// The request head of the first request, lower-cased (header names are case-insensitive).
        pub fn first(&self) -> String {
            self.requests.lock().unwrap().first().cloned().unwrap_or_default().to_lowercase()
        }
    }

    pub async fn serve(status: u16, extra_headers: &str, body: &str) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let (seen, response) = (requests.clone(), format!("HTTP/1.1 {status} X\r\n{extra_headers}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()));
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let (seen, response) = (seen.clone(), response.clone());
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 8192];
                    let mut head = String::new();
                    while !head.contains("\r\n\r\n") {
                        let Ok(read) = socket.read(&mut buffer).await else { return };
                        if read == 0 {
                            break;
                        }
                        head.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    }
                    seen.lock().unwrap().push(head);
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Fake { addr, requests }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_outcome_has_a_kind_and_only_an_accepted_key_is_ok() {
        let all = [
            (KeyCheck::Accepted, "ok", true),
            (KeyCheck::Unverifiable("x".into()), "unverifiable", false),
            (KeyCheck::Rejected("x".into()), "rejected", false),
            (KeyCheck::RateLimited("x".into()), "rate_limited", false),
            (KeyCheck::ProviderDown("x".into()), "provider_down", false),
            (KeyCheck::Unreachable("x".into()), "unreachable", false),
            (KeyCheck::Unsupported("x".into()), "unsupported", false),
        ];
        for (check, kind, ok) in all {
            assert_eq!((check.kind(), check.is_ok()), (kind, ok));
        }
        assert_eq!(KeyCheck::Rejected("nope".into()).message(), "nope");
    }

    #[test]
    fn a_status_is_read_for_what_it_says_about_the_key() {
        let kind = |status, body: &str, lenient| classify("P", status, body, lenient).kind();
        assert_eq!(kind(200, "", false), "ok");
        assert_eq!(kind(401, "", false), "rejected");
        assert_eq!(kind(403, "", false), "rejected");
        assert_eq!(kind(400, "api key not valid. please pass a valid api key.", false), "rejected", "Gemini says a bad key with a 400");
        assert_eq!(kind(400, "something else is wrong", false), "unverifiable", "a 400 that doesn't name the key says nothing about it");
        assert_eq!(kind(404, "", true), "unverifiable", "a local server with no model list");
        assert_eq!(kind(404, "", false), "unverifiable", "any other surprise is not a verdict on the key either");
        assert_eq!(kind(429, "", false), "rate_limited");
        assert_eq!(kind(503, "", false), "provider_down");
        assert_eq!(kind(302, "", false), "unverifiable");
    }

    #[tokio::test]
    async fn nothing_listening_is_unreachable_and_the_address_stays_out_of_the_message() {
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap().to_string();
        drop(probe); // the port is free again, and nobody answers on it
        let check = check_models("Local", client().get(format!("http://{addr}/v1/models")), true).await;
        assert_eq!(check.kind(), "unreachable");
        assert!(!check.message().contains(&addr), "{}", check.message());
    }

    use crate::model::anthropic::AnthropicProvider;
    use crate::model::gemini::GeminiProvider;
    use crate::model::openai::OpenAiProvider;
    use crate::model::{ChatStream, Message, ModelProvider};
    use crate::tool::ToolSpec;

    const BODY_THAT_ECHOES: &str = r#"{"error":{"message":"Incorrect API key provided: sk-secret-key-123"}}"#;

    /// Nothing the check reports may carry the key it sent, or what the provider said back.
    fn assert_leaks_nothing(check: &KeyCheck) {
        let message = check.message();
        assert!(!message.contains("sk-secret-key-123") && !message.contains("Incorrect API key"), "{message}");
        assert!(!message.contains("127.0.0.1"), "{message}");
    }

    #[tokio::test]
    async fn openai_lists_its_models_with_a_bearer_key_and_every_answer_is_told_without_the_body() {
        let ok = fake::serve(200, "", r#"{"data":[]}"#).await;
        let check = OpenAiProvider::with_base_url("sk-secret-key-123", "m", format!("http://{}/v1", ok.addr)).check_key().await;
        assert_eq!(check, KeyCheck::Accepted);
        let head = ok.first();
        assert!(head.starts_with("get /v1/models "), "{head}");
        assert!(head.contains("authorization: bearer sk-secret-key-123"), "{head}");

        for (status, kind) in [(401, "rejected"), (403, "rejected"), (429, "rate_limited"), (503, "provider_down"), (404, "unverifiable")] {
            let server = fake::serve(status, "", BODY_THAT_ECHOES).await;
            let check = OpenAiProvider::with_base_url("sk-secret-key-123", "m", format!("http://{}/v1", server.addr)).check_key().await;
            assert_eq!(check.kind(), kind, "status {status}");
            assert_leaks_nothing(&check);
        }
    }

    #[tokio::test]
    async fn a_local_server_that_needs_no_key_is_asked_without_one() {
        let ok = fake::serve(200, "", "{}").await;
        let check = OpenAiProvider::with_base_url("", "m", format!("http://{}/v1", ok.addr)).check_key().await;
        assert_eq!(check, KeyCheck::Accepted);
        assert!(!ok.first().contains("authorization"), "an empty key sends no bearer: {}", ok.first());
    }

    #[tokio::test]
    async fn gemini_lists_its_models_with_the_key_header_and_a_bad_key_comes_back_as_a_400() {
        let ok = fake::serve(200, "", "{}").await;
        let check = GeminiProvider::with_models_url("sk-secret-key-123", "m", format!("http://{}/v1beta/models", ok.addr)).check_key().await;
        assert_eq!(check, KeyCheck::Accepted);
        let head = ok.first();
        assert!(head.starts_with("get /v1beta/models "), "{head}");
        assert!(head.contains("x-goog-api-key: sk-secret-key-123"), "{head}");
        assert!(!head.contains("?key="), "the key travels in a header, never in the address: {head}");

        let bad = fake::serve(400, "", r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key."}}"#).await;
        let check = GeminiProvider::with_models_url("sk-secret-key-123", "m", format!("http://{}/v1beta/models", bad.addr)).check_key().await;
        assert_eq!(check.kind(), "rejected");
        assert!(!check.message().contains("Please pass"), "{}", check.message());
    }

    #[tokio::test]
    async fn anthropic_lists_its_models_with_the_key_and_the_version_header() {
        let ok = fake::serve(200, "", "{}").await;
        let check = AnthropicProvider::with_base_url("sk-secret-key-123", "m", format!("http://{}/v1", ok.addr)).check_key().await;
        assert_eq!(check, KeyCheck::Accepted);
        let head = ok.first();
        assert!(head.starts_with("get /v1/models "), "{head}");
        assert!(head.contains("x-api-key: sk-secret-key-123") && head.contains("anthropic-version: 2023-06-01"), "{head}");

        for (status, kind) in [(401, "rejected"), (429, "rate_limited"), (500, "provider_down")] {
            let server = fake::serve(status, "", BODY_THAT_ECHOES).await;
            let check = AnthropicProvider::with_base_url("sk-secret-key-123", "m", format!("http://{}/v1", server.addr)).check_key().await;
            assert_eq!(check.kind(), kind, "status {status}");
            assert_leaks_nothing(&check);
        }
    }

    #[tokio::test]
    async fn a_provider_with_no_key_to_check_says_so_and_the_check_sends_nothing() {
        struct NoKey;
        #[async_trait::async_trait]
        impl ModelProvider for NoKey {
            async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                anyhow::bail!("a key check never chats")
            }
        }
        assert_eq!(NoKey.check_key().await.kind(), "unsupported");
        assert!(!NoKey.check_key().await.is_ok());
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed_so_the_key_never_goes_where_it_was_bounced() {
        let target = fake::serve(200, "", "{}").await;
        let bouncer = fake::serve(302, &format!("location: http://{}/stolen\r\n", target.addr), "").await;
        let check = check_models("P", client().get(format!("http://{}/models", bouncer.addr)).bearer_auth("secret-key"), false).await;
        assert_eq!(check.kind(), "unverifiable", "a 302 is not an accepted key");
        assert!(target.requests.lock().unwrap().is_empty(), "the request never reached the place it was redirected to");
    }
}
