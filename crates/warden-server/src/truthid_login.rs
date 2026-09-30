//! Signing in as a member with their TruthID (P84 fatia 5, P113). A browser's `Hello` with `truthid_login`
//! gets a challenge to show as a QR; the member's TruthID app signs it and posts the answer to this hub's
//! own `https://` address (`POST /auth/truthid`, `handle_callback`), which checks it and lets the waiting
//! connection in as whoever that TruthID is linked to. The proof is the signature of a device the TruthID
//! registry on Base says is active and belongs to the identity the owner's invite linked — nothing is
//! said about which member it is until all of that holds.
//!
//! This only authenticates. A member's data key opens with their password or recovery code, never with
//! this, so after the hub restarted a TruthID session is "locked" until the password opens it once.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::oneshot;
use warden_bootstrap::load_config_from_path;
use warden_truthid::identity::device_identity;
use warden_truthid::login::{verify_auth_response, AuthChallenge, AuthResponse};

use crate::settings::SettingsHost;
use crate::web_ui::{write_response, RequestHead};

/// The path the TruthID app posts its answer to.
pub const CALLBACK_PATH: &str = "/auth/truthid";
/// How long the hub keeps a challenge open for the answer: the phone has 30 s to scan it and then creates
/// an on-chain session before it posts, which takes a while.
pub const ANSWER_WINDOW_MS: u64 = 120_000;
/// How long the QR itself is good: the TruthID app refuses a challenge older than this.
pub const QR_VALID_MS: u64 = 30_000;
/// Challenges waiting at once; one more is refused, so a stranger can't fill the hub's memory.
const MAX_PENDING: usize = 32;
const MAX_BODY: usize = 16 * 1024;
const BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

struct Pending {
    challenge: AuthChallenge,
    done: oneshot::Sender<String>,
}

/// The logins waiting for a phone, by nonce. Shared by every connection and the callback.
#[derive(Default)]
pub struct TruthIdLogins {
    pending: Mutex<HashMap<String, Pending>>,
}

impl TruthIdLogins {
    /// Starts waiting for the answer to `challenge`. The receiver gets the member's username when it comes.
    pub fn begin(&self, challenge: AuthChallenge) -> Result<oneshot::Receiver<String>, &'static str> {
        let mut pending = self.pending.lock().unwrap();
        if pending.len() >= MAX_PENDING {
            return Err("too many TruthID logins are waiting — try again in a moment");
        }
        let (done, receiver) = oneshot::channel();
        pending.insert(challenge.nonce.clone(), Pending { challenge, done });
        Ok(receiver)
    }

    /// Stops waiting (the connection closed or ran out of time).
    pub fn forget(&self, nonce: &str) {
        self.pending.lock().unwrap().remove(nonce);
    }

    /// Takes the challenge for `nonce`: an answer gets one try, right or wrong.
    fn take(&self, nonce: &str) -> Option<Pending> {
        self.pending.lock().unwrap().remove(nonce)
    }

    #[cfg(test)]
    pub fn waiting(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The host of `https://host[:port]/...`, which is what the challenge names as its origin.
pub fn origin_of(public_url: &str) -> Option<&str> {
    let rest = public_url.strip_prefix("https://")?;
    let host = rest.split('/').next().unwrap_or_default();
    (!host.is_empty()).then_some(host)
}

/// Answers `POST /auth/truthid`. Every refusal says only "invalid": what was wrong goes to the log.
pub(crate) async fn handle_callback<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, head: &RequestHead, logins: &TruthIdLogins, settings: Option<&dyn SettingsHost>) -> std::io::Result<()> {
    if head.method != "POST" {
        return reply(stream, "405 Method Not Allowed", "use POST").await;
    }
    let Some(body) = read_body(stream, head).await else {
        return reply(stream, "400 Bad Request", "invalid").await;
    };
    let Ok(response) = serde_json::from_slice::<AuthResponse>(&body) else {
        return reply(stream, "400 Bad Request", "invalid").await;
    };
    // One try per challenge, taken before anything else is checked.
    let Some(pending) = logins.take(&response.nonce) else {
        eprintln!("warden-server: a TruthID answer for a challenge nobody is waiting on (used, expired or made up)");
        return reply(stream, "400 Bad Request", "invalid").await;
    };
    let Some(settings) = settings else {
        return reply(stream, "404 Not Found", "invalid").await;
    };
    let device = match verify_auth_response(&pending.challenge, &response, ANSWER_WINDOW_MS, now_ms()) {
        Ok(address) => address,
        Err(why) => {
            eprintln!("warden-server: a TruthID answer was refused: {why}");
            return reply(stream, "401 Unauthorized", "invalid").await;
        }
    };
    let config = match load_config_from_path(&settings.config_path(), false) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("warden-server: couldn't read the config to check a TruthID: {err:#}");
            return reply(stream, "500 Internal Server Error", "invalid").await;
        }
    };
    let rpc_url = config.truthid_rpc_url.clone().unwrap_or_else(|| config.truthid_network.default_rpc_url().to_string());
    let identity = match device_identity(&rpc_url, config.truthid_network, &device).await {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            eprintln!("warden-server: a TruthID answer came from a device the registry doesn't know or has revoked ({device})");
            return reply(stream, "401 Unauthorized", "invalid").await;
        }
        Err(err) => {
            eprintln!("warden-server: couldn't ask the TruthID registry about a device: {err:#}");
            return reply(stream, "502 Bad Gateway", "invalid").await;
        }
    };
    let Some(member) = config.users.iter().find(|u| u.truthid.as_ref().is_some_and(|t| t.identity_id == identity)) else {
        eprintln!("warden-server: a TruthID answer was for an identity ({identity}) no member linked");
        return reply(stream, "401 Unauthorized", "invalid").await;
    };
    // The connection may have given up meanwhile; then the answer has nowhere to go.
    if pending.done.send(member.id.clone()).is_err() {
        return reply(stream, "410 Gone", "invalid").await;
    }
    eprintln!("warden-server: '{}' signed in with their TruthID", member.id);
    reply(stream, "200 OK", "ok").await
}

async fn reply<S: AsyncWrite + Unpin>(stream: &mut S, status: &str, text: &str) -> std::io::Result<()> {
    write_response(stream, status, &[], "application/json", format!("{{\"result\":\"{text}\"}}").as_bytes(), false).await
}

/// The JSON body: a small one, sent with a `Content-Length`.
async fn read_body<S: AsyncRead + Unpin>(stream: &mut S, head: &RequestHead) -> Option<Vec<u8>> {
    let length: usize = head.header("content-length")?.trim().parse().ok()?;
    if length > MAX_BODY {
        return None;
    }
    let mut body = head.raw[head.body_start.min(head.raw.len())..].to_vec();
    body.truncate(length);
    let start = body.len();
    if start < length {
        body.resize(length, 0);
        tokio::time::timeout(BODY_TIMEOUT, stream.read_exact(&mut body[start..])).await.ok()?.ok()?;
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_origin_is_the_host_of_the_https_address() {
        assert_eq!(origin_of("https://hub.tailnet.ts.net"), Some("hub.tailnet.ts.net"));
        assert_eq!(origin_of("https://hub.example.com:7420/base"), Some("hub.example.com:7420"));
        assert_eq!(origin_of("http://hub.example.com"), None, "the phone refuses anything but https");
        assert_eq!(origin_of("https://"), None);
    }

    #[test]
    fn a_challenge_is_answered_once_and_the_hub_holds_a_limited_number() {
        let logins = TruthIdLogins::default();
        let first = AuthChallenge::new("h", 1);
        let nonce = first.nonce.clone();
        let _rx = logins.begin(first).unwrap();
        assert!(logins.take(&nonce).is_some());
        assert!(logins.take(&nonce).is_none(), "a second answer finds nothing");
        let _held: Vec<_> = (0..MAX_PENDING).map(|_| logins.begin(AuthChallenge::new("h", 1)).unwrap()).collect();
        assert!(logins.begin(AuthChallenge::new("h", 1)).is_err());
        assert_eq!(logins.waiting(), MAX_PENDING);
    }
}
