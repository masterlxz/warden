//! Reads a TruthID identity from the `IdentityRegistry` contract on Base: `getIdentity(username)`
//! through a plain JSON-RPC `eth_call` (`sdk/typescript/src/contracts.ts` in the TruthID repo has
//! the address and the ABI). The ABI is encoded by hand because one view call isn't worth an
//! Ethereum dependency.

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Network {
    #[default]
    BaseMainnet,
    BaseSepolia,
}

impl Network {
    pub fn identity_registry(self) -> &'static str {
        match self {
            Network::BaseMainnet => "0x97787D6EE3EfD76962dc7E3Bf143E659D9961962",
            Network::BaseSepolia => "0xb56DbCB7580c097d3f64808064C2d5609dD6B243",
        }
    }

    pub fn default_rpc_url(self) -> &'static str {
        match self {
            Network::BaseMainnet => "https://mainnet.base.org",
            Network::BaseSepolia => "https://sepolia.base.org",
        }
    }
}

/// What `getIdentity` answers for a username that exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub id: u64,
    pub username: String,
    /// The controller's address, `0x` + 40 lowercase hex digits.
    pub controller: String,
}

const SIGNATURE: &str = "getIdentity(string)";

fn word(value: usize) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&(value as u64).to_be_bytes());
    out
}

/// The `eth_call` data: the 4-byte selector, then the one `string` argument.
pub fn encode_get_identity(username: &str) -> Vec<u8> {
    let mut data = Keccak256::digest(SIGNATURE.as_bytes())[..4].to_vec();
    data.extend_from_slice(&word(32));
    data.extend_from_slice(&word(username.len()));
    data.extend_from_slice(username.as_bytes());
    data.resize(data.len() + (32 - username.len() % 32) % 32, 0);
    data
}

fn read_word(data: &[u8], at: usize) -> anyhow::Result<&[u8; 32]> {
    data.get(at..at + 32)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| anyhow!("the answer is shorter than its own layout"))
}

fn read_usize(data: &[u8], at: usize) -> anyhow::Result<usize> {
    let word = read_word(data, at)?;
    if word[..24].iter().any(|byte| *byte != 0) {
        bail!("a number in the answer is too big");
    }
    Ok(u64::from_be_bytes(word[24..].try_into().unwrap()) as usize)
}

/// Decodes the `(uint256 id, string username, address controller, bool exists)` tuple. `Ok(None)`
/// is an identity that doesn't exist.
pub fn decode_identity(data: &[u8]) -> anyhow::Result<Option<Identity>> {
    // The tuple has a dynamic member, so it's behind an offset, and its string is behind another
    // (relative to where the tuple starts).
    let tuple = read_usize(data, 0)?;
    let exists = read_usize(data, tuple + 96)? != 0;
    if !exists {
        return Ok(None);
    }
    let id = read_usize(data, tuple)?;
    let controller = read_word(data, tuple + 64)?;
    let string_at = tuple + read_usize(data, tuple + 32)?;
    let len = read_usize(data, string_at)?;
    let bytes = data
        .get(string_at + 32..string_at + 32 + len)
        .ok_or_else(|| anyhow!("the username runs past the end of the answer"))?;
    Ok(Some(Identity {
        id: id as u64,
        username: String::from_utf8(bytes.to_vec()).context("the username isn't UTF-8")?,
        controller: format!("0x{}", hex::encode(&controller[12..])),
    }))
}

#[derive(Deserialize)]
struct RpcAnswer {
    result: Option<String>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    message: String,
}

/// Asks the registry for `username`. `Ok(None)`: no such identity.
pub async fn resolve_identity(rpc_url: &str, network: Network, username: &str) -> anyhow::Result<Option<Identity>> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [
            { "to": network.identity_registry(), "data": format!("0x{}", hex::encode(encode_get_identity(username))) },
            "latest"
        ],
    });
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
    let answer: RpcAnswer = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .context("couldn't reach the Base RPC")?
        .error_for_status()?
        .json()
        .await
        .context("the Base RPC didn't answer JSON-RPC")?;
    if let Some(error) = answer.error {
        // The contract may revert for an unknown username instead of returning `exists: false`.
        if error.message.to_lowercase().contains("revert") {
            return Ok(None);
        }
        bail!("the Base RPC refused the call: {}", error.message);
    }
    let result = answer.result.ok_or_else(|| anyhow!("the Base RPC answered without a result"))?;
    let data = hex::decode(result.trim_start_matches("0x")).context("the answer isn't hex")?;
    decode_identity(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real selector of `getIdentity(string)`, worked out independently: keccak256 of the signature.
    #[test]
    fn selector_is_keccak_of_the_signature() {
        let data = encode_get_identity("ana");
        assert_eq!(&data[..4], &Keccak256::digest(b"getIdentity(string)")[..4]);
        // selector + offset + length + one padded word of text
        assert_eq!(data.len(), 4 + 32 + 32 + 32);
        assert_eq!(&data[4 + 32 + 32..][..3], b"ana");
    }

    #[test]
    fn keccak256_of_empty_matches_the_known_vector() {
        assert_eq!(
            hex::encode(Keccak256::digest(b"")),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    fn answer(id: u64, username: &str, controller: [u8; 20], exists: bool) -> Vec<u8> {
        let mut out = word(32).to_vec();
        out.extend_from_slice(&word(id as usize));
        out.extend_from_slice(&word(128));
        let mut address = [0u8; 32];
        address[12..].copy_from_slice(&controller);
        out.extend_from_slice(&address);
        out.extend_from_slice(&word(exists as usize));
        out.extend_from_slice(&word(username.len()));
        out.extend_from_slice(username.as_bytes());
        out.resize(out.len() + (32 - username.len() % 32) % 32, 0);
        out
    }

    #[test]
    fn decodes_an_existing_identity() {
        let identity = decode_identity(&answer(7, "ana.silva", [0xab; 20], true)).unwrap().unwrap();
        assert_eq!(identity.id, 7);
        assert_eq!(identity.username, "ana.silva");
        assert_eq!(identity.controller, format!("0x{}", "ab".repeat(20)));
    }

    #[test]
    fn a_missing_identity_is_none_and_garbage_is_an_error() {
        assert!(decode_identity(&answer(0, "", [0; 20], false)).unwrap().is_none());
        assert!(decode_identity(&[0u8; 10]).is_err());
    }

    #[tokio::test]
    async fn resolves_through_a_fake_rpc() {
        use axum::{routing::post, Json, Router};
        let reply = format!("0x{}", hex::encode(answer(3, "bia", [1; 20], true)));
        let app = Router::new().route(
            "/",
            post(move |Json(request): Json<serde_json::Value>| {
                let reply = reply.clone();
                async move {
                    assert_eq!(request["method"], "eth_call");
                    assert_eq!(request["params"][0]["to"], Network::BaseMainnet.identity_registry());
                    Json(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": reply }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let identity = resolve_identity(&url, Network::BaseMainnet, "bia").await.unwrap().unwrap();
        assert_eq!((identity.id, identity.username.as_str()), (3, "bia"));
    }

    /// Against the real registry on Base: `cargo test -p warden-truthid -- --ignored real_registry`.
    /// A made-up username must come back as "no such identity", not as an error.
    #[tokio::test]
    #[ignore = "needs the internet"]
    async fn real_registry_says_none_for_an_unknown_username() {
        let network = Network::BaseMainnet;
        let answer = resolve_identity(network.default_rpc_url(), network, "warden-no-such-user-9f3a1c").await.unwrap();
        assert!(answer.is_none(), "{answer:?}");
    }
}
