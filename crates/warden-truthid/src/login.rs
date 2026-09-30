//! TruthID's login protocol, the verifier's side (`sdk/typescript/src/client.ts` in the TruthID repo):
//! the site makes a challenge and shows it in a QR, the user's phone signs it (`personal_sign` over its
//! JSON) and posts the answer to the site's own `https://` callback, and the site recovers the address
//! that signed, which has to be the device it names. Whether that device is still active, and whose it
//! is, is one more question, for the `DeviceRegistry` on Base (`identity::device_identity`).
//!
//! Nothing here touches the network or the clock by itself: `now` is passed in.

use anyhow::{anyhow, bail, Context};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

/// What the phone signs. The field order is the SDK's (`type`, `nonce`, `issuedAt`, `origin`): the
/// phone signs `jsonEncode` of the map it got from the QR, so the text the site signed-over has to be
/// this one, compact, in this order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallenge {
    #[serde(rename = "type")]
    pub kind: String,
    pub nonce: String,
    #[serde(rename = "issuedAt")]
    pub issued_at: u64,
    pub origin: String,
}

impl AuthChallenge {
    /// A fresh challenge for `origin` (the site's host), with a random nonce. `now_ms` is when it was issued.
    pub fn new(origin: &str, now_ms: u64) -> Self {
        Self { kind: "challenge".to_string(), nonce: random_uuid(), issued_at: now_ms, origin: origin.to_string() }
    }

    /// The exact text the phone signs.
    pub fn signed_text(&self) -> String {
        serde_json::to_string(self).expect("a challenge is plain strings and a number")
    }

    /// What goes into the QR: `{ action, challenge, callbackUrl }`, as the SDK's example builds it.
    pub fn qr_payload(&self, callback_url: &str) -> String {
        serde_json::json!({ "action": "truthid-auth", "challenge": self, "callbackUrl": callback_url }).to_string()
    }
}

/// A version 4 UUID, which is what the SDK uses for a nonce.
fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = hex::encode(bytes);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// What the phone posts back (`AuthResponse` in the SDK). `sessionSignature` is for the on-chain session
/// the phone already created; the verifier doesn't need it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    pub approved: bool,
    pub nonce: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub device_address: String,
}

/// `keccak256("\x19Ethereum Signed Message:\n" + len + message)`, what `personal_sign` signs.
fn personal_message_digest(message: &str) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message.as_bytes());
    hasher.finalize().into()
}

/// The address (`0x` + 40 lowercase hex digits) that signed `message` with `personal_sign`, from a 65-byte
/// `r || s || v` signature in hex. `v` may be 27/28 or 0/1, as the SDKs accept.
pub fn recover_personal_sign_address(message: &str, signature_hex: &str) -> anyhow::Result<String> {
    let raw = hex::decode(signature_hex.trim_start_matches("0x")).context("the signature isn't hex")?;
    if raw.len() != 65 {
        bail!("a signature is 65 bytes (r || s || v)");
    }
    let signature = Signature::from_slice(&raw[..64]).map_err(|_| anyhow!("the signature is malformed"))?;
    let v = if raw[64] >= 27 { raw[64] - 27 } else { raw[64] };
    let recovery = RecoveryId::from_byte(v).ok_or_else(|| anyhow!("the signature's recovery byte is wrong"))?;
    let key = VerifyingKey::recover_from_prehash(&personal_message_digest(message), &signature, recovery).map_err(|_| anyhow!("no key signed this"))?;
    Ok(address_of(&key))
}

/// The Ethereum address of a public key: the last 20 bytes of keccak256 of the uncompressed point, without its prefix.
pub fn address_of(key: &VerifyingKey) -> String {
    let point = key.to_encoded_point(false);
    let hash = Keccak256::digest(&point.as_bytes()[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}

/// The phone's side: `personal_sign` over the challenge's JSON with the device key, as the TruthID app does
/// (`r || s || v`, v = 27/28), as the answer it would post. For tests and tools that stand in for the phone.
pub fn sign_challenge(key: &k256::ecdsa::SigningKey, challenge: &AuthChallenge) -> AuthResponse {
    let (signature, recovery) = key.sign_prehash_recoverable(&personal_message_digest(&challenge.signed_text())).expect("a 32-byte digest signs");
    let mut raw = signature.to_bytes().to_vec();
    raw.push(recovery.to_byte() + 27);
    AuthResponse { approved: true, nonce: challenge.nonce.clone(), signature: format!("0x{}", hex::encode(raw)), device_address: address_of(key.verifying_key()) }
}

/// Why an answer isn't accepted, worded for the log and never for the person who sent it.
#[derive(Debug, PartialEq, Eq)]
pub enum Rejection {
    Declined,
    Expired,
    WrongNonce,
    BadSignature,
    WrongDevice,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Rejection::Declined => "the user declined",
            Rejection::Expired => "the challenge expired",
            Rejection::WrongNonce => "the nonce doesn't match the challenge",
            Rejection::BadSignature => "the signature is malformed",
            Rejection::WrongDevice => "the signature isn't from the device it names",
        })
    }
}

/// Checks the phone's answer to `challenge`: approved, inside `ttl_ms` of when it was issued, the same
/// nonce, and a signature by the device it names. Returns that device's address (lowercase).
pub fn verify_auth_response(challenge: &AuthChallenge, response: &AuthResponse, ttl_ms: u64, now_ms: u64) -> Result<String, Rejection> {
    if !response.approved {
        return Err(Rejection::Declined);
    }
    if now_ms.saturating_sub(challenge.issued_at) > ttl_ms {
        return Err(Rejection::Expired);
    }
    if response.nonce != challenge.nonce {
        return Err(Rejection::WrongNonce);
    }
    let signer = recover_personal_sign_address(&challenge.signed_text(), &response.signature).map_err(|_| Rejection::BadSignature)?;
    if !signer.eq_ignore_ascii_case(&response.device_address) {
        return Err(Rejection::WrongDevice);
    }
    Ok(signer)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use k256::ecdsa::SigningKey;

    /// A device key from a fixed secret, for tests that stand in for the phone.
    pub fn device_key(seed: u8) -> SigningKey {
        SigningKey::from_slice(&[seed; 32]).unwrap()
    }

    pub fn phone_answer(key: &SigningKey, challenge: &AuthChallenge) -> AuthResponse {
        sign_challenge(key, challenge)
    }

    fn challenge() -> AuthChallenge {
        AuthChallenge { kind: "challenge".into(), nonce: "4b5c2f4e-1111-4222-8333-444455556666".into(), issued_at: 1_700_000_000_000, origin: "hub.example.ts.net".into() }
    }

    #[test]
    fn the_challenge_is_the_text_the_sdk_signs_and_the_qr_is_the_sdks_payload() {
        assert_eq!(
            challenge().signed_text(),
            r#"{"type":"challenge","nonce":"4b5c2f4e-1111-4222-8333-444455556666","issuedAt":1700000000000,"origin":"hub.example.ts.net"}"#
        );
        let qr: serde_json::Value = serde_json::from_str(&challenge().qr_payload("https://hub.example.ts.net/auth/truthid")).unwrap();
        assert_eq!(qr["action"], "truthid-auth");
        assert_eq!(qr["callbackUrl"], "https://hub.example.ts.net/auth/truthid");
        assert_eq!(qr["challenge"]["issuedAt"], 1_700_000_000_000u64);
    }

    #[test]
    fn a_new_challenge_has_a_version_4_uuid_nonce_that_is_never_the_same() {
        let (a, b) = (AuthChallenge::new("h", 5), AuthChallenge::new("h", 5));
        assert_ne!(a.nonce, b.nonce);
        let parts: Vec<&str> = a.nonce.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
        assert!(parts[2].starts_with('4') && "89ab".contains(parts[3].chars().next().unwrap()));
    }

    /// The address of the secret key `0x0101…01` — worked out independently (web3/eth-account give the same).
    #[test]
    fn the_address_of_a_key_is_the_keccak_of_its_public_key() {
        // The well known key 1 (the generator point) has the address 0x7e5f4552091a69125d5dfcb7b8c2659029395bdf.
        let one = SigningKey::from_slice(&{
            let mut k = [0u8; 32];
            k[31] = 1;
            k
        })
        .unwrap();
        assert_eq!(address_of(one.verifying_key()), "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
    }

    #[test]
    fn a_signature_recovers_to_the_device_that_made_it() {
        let key = device_key(7);
        let answer = phone_answer(&key, &challenge());
        assert_eq!(recover_personal_sign_address(&challenge().signed_text(), &answer.signature).unwrap(), address_of(key.verifying_key()));
        // v as 0/1 works too, and the 0x prefix is optional.
        let mut raw = hex::decode(answer.signature.trim_start_matches("0x")).unwrap();
        raw[64] -= 27;
        assert_eq!(recover_personal_sign_address(&challenge().signed_text(), &hex::encode(raw)).unwrap(), address_of(key.verifying_key()));
    }

    #[test]
    fn an_answer_is_accepted_only_when_everything_holds() {
        let key = device_key(7);
        let c = challenge();
        let good = phone_answer(&key, &c);
        let now = c.issued_at + 5_000;
        assert_eq!(verify_auth_response(&c, &good, 120_000, now).unwrap(), address_of(key.verifying_key()));

        assert_eq!(verify_auth_response(&c, &AuthResponse { approved: false, ..good.clone() }, 120_000, now), Err(Rejection::Declined));
        assert_eq!(verify_auth_response(&c, &good, 120_000, c.issued_at + 120_001), Err(Rejection::Expired));
        assert_eq!(verify_auth_response(&c, &AuthResponse { nonce: "other".into(), ..good.clone() }, 120_000, now), Err(Rejection::WrongNonce));
        // A signature over another challenge, or by another device than the one named.
        let other = AuthChallenge { nonce: "x".into(), ..c.clone() };
        assert_eq!(verify_auth_response(&c, &AuthResponse { signature: phone_answer(&key, &other).signature, ..good.clone() }, 120_000, now), Err(Rejection::WrongDevice));
        let stranger = device_key(9);
        assert_eq!(verify_auth_response(&c, &AuthResponse { device_address: address_of(stranger.verifying_key()), ..good.clone() }, 120_000, now), Err(Rejection::WrongDevice));
        assert_eq!(verify_auth_response(&c, &AuthResponse { signature: "0x1234".into(), ..good.clone() }, 120_000, now), Err(Rejection::BadSignature));
        assert_eq!(verify_auth_response(&c, &AuthResponse { signature: "zz".into(), ..good }, 120_000, now), Err(Rejection::BadSignature));
    }

    #[test]
    fn the_phones_json_reads_into_a_response() {
        let response: AuthResponse = serde_json::from_str(r#"{"approved":true,"nonce":"n","signature":"0xab","deviceAddress":"0xCd","sessionSignature":"0xef"}"#).unwrap();
        assert_eq!((response.approved, response.nonce.as_str(), response.device_address.as_str()), (true, "n", "0xCd"));
        // A refusal carries only the nonce.
        let no: AuthResponse = serde_json::from_str(r#"{"approved":false,"nonce":"n"}"#).unwrap();
        assert!(!no.approved && no.signature.is_empty());
    }

    /// Made by the TruthID phone's own code (`device_key_service.dart`'s `signChallenge`, web3dart) for the
    /// key `0x0707…07` and the challenge above, and read back by the TruthID Dart SDK's
    /// `recoverPersonalSignatureAddress` — a vector from the real implementations, not from this file.
    const DART_SIGNATURE: &str = "0x94862eeae4ff0e126be99248e0fe8027151279174f1caa931bb4852e0699dbfa5f56c2bf378162cfddd1ba772c0ce3c4d4776977b2d8a2e613b508898f66dce11c";
    const DART_ADDRESS: &str = "0x4a62316623ad457f02cdc5d997ded67a383ec569";

    #[test]
    fn it_reads_what_the_truthid_phone_signs_and_signs_the_same_bytes() {
        let c = challenge();
        assert_eq!(recover_personal_sign_address(&c.signed_text(), DART_SIGNATURE).unwrap(), DART_ADDRESS);
        let key = device_key(7);
        assert_eq!(address_of(key.verifying_key()), DART_ADDRESS);
        // Signing is deterministic (RFC 6979), so the same key over the same text gives the phone's bytes.
        assert_eq!(phone_answer(&key, &c).signature, DART_SIGNATURE);
        let answer = AuthResponse { approved: true, nonce: c.nonce.clone(), signature: DART_SIGNATURE.into(), device_address: "0x4a62316623ad457F02cDC5D997deD67a383EC569".into() };
        assert!(verify_auth_response(&c, &answer, 120_000, c.issued_at + 1).is_ok(), "the phone's own answer is accepted, checksummed address and all");
    }
}
