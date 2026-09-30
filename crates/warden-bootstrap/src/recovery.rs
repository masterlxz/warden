//! The owner's recovery key and the recovery policies of the workspace (P84, fatia 4, parte B).
//!
//! A member's data key (`member_crypto`) opens with their password or their recovery code. The
//! owner can be given a way to help someone who lost both, by the workspace's **policy**:
//!
//! - `private`: none. Only the password or the code opens the data;
//! - `consent`: the owner **and** the person's code, together. Neither opens it alone;
//! - `company`: the owner alone, with their key — every use is recorded and the person is told.
//!
//! The owner's key is a secp256k1 pair. The hub keeps only the **public** half (enough to prepare
//! each member's data for recovery, whenever the member's key is open at sign-in); the private half
//! is shown once, to the owner, who types it in for each recovery. Someone with only the hub's
//! disk can't recover anything.

use k256::SecretKey;
use serde::{Deserialize, Serialize};
use warden_core::memory::{base32_decode, base32_encode};
use warden_truthid::crypto::{ecies_decrypt, ecies_encrypt, generate_ecies_keypair};
use zeroize::Zeroizing;

use crate::member_crypto::MemberKey;

/// Who besides the person may open their data (TOML `recovery_policy`).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RecoveryPolicy {
    /// Nobody: only the password or the recovery code.
    #[default]
    Private,
    /// The owner's key together with the person's recovery code.
    Consent,
    /// The owner's key alone, recorded and told to the person.
    Company,
}

impl RecoveryPolicy {
    /// How much the person is protected: `Private` most. A change to a lower one needs their yes.
    pub fn strength(self) -> u8 {
        match self {
            Self::Private => 3,
            Self::Consent => 2,
            Self::Company => 1,
        }
    }

    pub fn is_private(&self) -> bool {
        *self == Self::Private
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Consent => "consent",
            Self::Company => "company",
        }
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "private" => Ok(Self::Private),
            "consent" => Ok(Self::Consent),
            "company" => Ok(Self::Company),
            other => anyhow::bail!("'{other}' is not a recovery policy — use private, consent or company"),
        }
    }
}

/// A new owner's key pair: `public_hex` goes in the config file, `secret_text` is shown once.
pub struct EscrowKeys {
    pub public_hex: String,
    pub secret_text: String,
}

pub fn generate_escrow_keypair() -> EscrowKeys {
    let pair = generate_ecies_keypair();
    let bytes = pair.secret.to_bytes();
    let text = base32_encode(bytes.as_slice()).to_ascii_uppercase();
    EscrowKeys { public_hex: pair.public_hex, secret_text: text.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join("-") }
}

/// A short name for a public key, to tell whether a member's data was prepared for the owner's
/// current key (the key can be replaced).
pub fn escrow_id(public_hex: &str) -> String {
    public_hex.trim_start_matches("0x").chars().skip(2).take(12).collect()
}

/// The private key from what the owner wrote down, however it was typed.
fn parse_secret(text: &str) -> anyhow::Result<SecretKey> {
    let clean: String = text.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let bytes = Zeroizing::new(base32_decode(&clean).ok_or_else(|| anyhow::anyhow!("that is not a recovery key"))?);
    SecretKey::from_slice(&bytes).map_err(|_| anyhow::anyhow!("that is not a recovery key"))
}

/// A member's key sealed to the owner's public key: only the private half opens it.
pub fn escrow_seal(key: &MemberKey, public_hex: &str) -> anyhow::Result<Vec<u8>> {
    ecies_encrypt(&key[..], public_hex)
}

/// The member's key, from what `escrow_seal` made and the owner's private key. Fails for another key.
pub fn escrow_open(blob: &[u8], secret_text: &str) -> anyhow::Result<MemberKey> {
    let secret = parse_secret(secret_text)?;
    let opened = Zeroizing::new(ecies_decrypt(blob, &secret).map_err(|_| anyhow::anyhow!("that recovery key doesn't open this member's data"))?);
    let mut key = Zeroizing::new([0u8; 32]);
    anyhow::ensure!(opened.len() == key.len(), "the stored key is damaged");
    key.copy_from_slice(&opened);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::member_crypto::new_key;

    #[test]
    fn a_policy_names_round_trip_and_rank_by_how_much_they_protect() {
        for policy in [RecoveryPolicy::Private, RecoveryPolicy::Consent, RecoveryPolicy::Company] {
            assert_eq!(RecoveryPolicy::parse(policy.as_str()).unwrap(), policy);
        }
        assert_eq!(RecoveryPolicy::parse("  Consent ").unwrap(), RecoveryPolicy::Consent);
        assert!(RecoveryPolicy::parse("everyone").is_err());
        assert!(RecoveryPolicy::Private.strength() > RecoveryPolicy::Consent.strength() && RecoveryPolicy::Consent.strength() > RecoveryPolicy::Company.strength());
        assert!(RecoveryPolicy::default().is_private());
    }

    #[test]
    fn only_the_owners_private_key_opens_a_member_key_sealed_to_the_public_one() {
        let owner = generate_escrow_keypair();
        let key = new_key();
        let blob = escrow_seal(&key, &owner.public_hex).unwrap();
        assert_eq!(*escrow_open(&blob, &owner.secret_text).unwrap(), *key);

        // However the owner typed it.
        let sloppy = owner.secret_text.to_ascii_lowercase().replace('-', " ");
        assert_eq!(*escrow_open(&blob, &sloppy).unwrap(), *key);

        assert!(escrow_open(&blob, &generate_escrow_keypair().secret_text).is_err(), "another key");
        assert!(escrow_open(&blob, "ABCD-EFGH").is_err(), "not a key");
        assert!(escrow_open(b"too short", &owner.secret_text).is_err());
        assert_ne!(blob, escrow_seal(&key, &owner.public_hex).unwrap(), "a fresh seal each time");
    }

    #[test]
    fn the_secret_is_written_for_a_person_and_the_id_names_the_public_key() {
        let owner = generate_escrow_keypair();
        assert_eq!(owner.secret_text.len(), 52 + 12, "{}", owner.secret_text);
        assert!(owner.secret_text.chars().all(|c| c == '-' || c.is_ascii_uppercase() || c.is_ascii_digit()));
        assert_eq!(escrow_id(&owner.public_hex).len(), 12);
        assert_ne!(escrow_id(&owner.public_hex), escrow_id(&generate_escrow_keypair().public_hex));
    }
}
