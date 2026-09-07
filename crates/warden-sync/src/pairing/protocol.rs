use std::time::Duration;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Own port range for Warden-device-to-Warden-device pairing — distinct from
/// `warden_truthid::protocol::LAN_PORTS` so a pairing session never collides with a TruthID
/// `pin()` session running on the same LAN at the same time.
pub const PAIRING_PORTS: [u16; 5] = [48070, 48071, 48072, 48073, 48074];

/// Generous — this is a human typing an 8-char code from one screen into another device.
pub const PAIRING_TIMEOUT: Duration = Duration::from_secs(300);

/// No `0/O/1/I/L` — characters that are easy to misread when copying a code by eye.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 8;

const PAIRING_HKDF_SALT: &[u8] = b"Warden Pairing";
const PAIRING_HKDF_INFO: &[u8] = b"pairing-auth-v1";
const PROOF_CONTEXT: &[u8] = b"warden-pairing-hello";

pub fn generate_pairing_code() -> String {
    let mut bytes = [0u8; CODE_LEN];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char).collect()
}

/// Derives a symmetric key from the pairing code itself — so a LAN eavesdropper who finds the
/// open port but doesn't know the code (i.e. wasn't shown it) can't compute a valid `code_proof`
/// nor decrypt anything derived from this key.
pub fn derive_code_key(code: &str) -> anyhow::Result<[u8; 32]> {
    let normalized = code.trim().to_uppercase();
    let hk = Hkdf::<Sha256>::new(Some(PAIRING_HKDF_SALT), normalized.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(PAIRING_HKDF_INFO, &mut key).map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
    Ok(key)
}

/// Proves knowledge of the code without ever sending the code itself over the LAN.
pub fn code_proof(code_key: &[u8; 32]) -> String {
    let mut mac = HmacSha256::new_from_slice(code_key).expect("HMAC accepts a key of any size");
    mac.update(PROOF_CONTEXT);
    hex::encode(mac.finalize().into_bytes())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PairingMessage {
    /// Sent by the joiner (the device the code was typed into) as the first frame.
    Hello { v: u8, code_proof: String, joiner_ecies_pub: String, joiner_device_name: String },
    /// Sent by the host — `payload_b64` is `ECIES(joiner_ecies_pub, JSON of KeyMaterialPayload)`.
    KeyMaterial { v: u8, payload_b64: String },
    /// Sent by the host when `code_proof` doesn't match — e.g. a mistyped code.
    Error { reason: String },
    /// Sent by the joiner once it has successfully decrypted `KeyMaterial`.
    Ack { ok: bool },
}

/// Deliberately carries no `last_tx_id`: the joining device hasn't actually fetched or applied
/// any content yet, so its manifest must start at "nothing applied" — otherwise its first `pull`
/// would see a `last_tx_id` that matches the latest published tx and wrongly conclude it's
/// already up to date, even though its vault is empty. `owner_address` alone is enough for `pull`
/// to discover and fetch the real latest snapshot on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyMaterialPayload {
    pub vault_key_b64: String,
    pub owner_address: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_use_only_the_unambiguous_alphabet() {
        for _ in 0..20 {
            let code = generate_pairing_code();
            assert_eq!(code.len(), CODE_LEN);
            assert!(code.chars().all(|c| CODE_ALPHABET.contains(&(c as u8))));
        }
    }

    #[test]
    fn derive_code_key_is_deterministic_and_case_insensitive() {
        let a = derive_code_key("abcd1234").unwrap();
        let b = derive_code_key("ABCD1234").unwrap();
        let c = derive_code_key(" ABCD1234 ").unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn derive_code_key_differs_across_codes() {
        let a = derive_code_key("AAAAAAAA").unwrap();
        let b = derive_code_key("BBBBBBBB").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn code_proof_is_deterministic_for_the_same_key() {
        let key = derive_code_key("ZZZZ9999").unwrap();
        assert_eq!(code_proof(&key), code_proof(&key));
    }
}
