//! Encryption at rest for a member's vault (P84, fatia 4): what a file holds, and the names of its
//! folders and files, so the disk of the hub shows neither to whoever opens it.
//!
//! One 32-byte key per member. Two subkeys come out of it by HKDF, so content and names never
//! share a key. Content is AES-256-GCM with a fresh random nonce per write. Names are
//! AES-256-GCM-SIV with a fixed nonce, on purpose: a name must map to the same text every time (a
//! path is looked up by encrypting it), and SIV is the mode that stays safe when a nonce repeats —
//! the only thing it gives away is that two names are equal.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm};
use aes_gcm_siv::Aes256GcmSiv;
use hkdf::Hkdf;
use sha2::Sha256;

/// First bytes of every encrypted file — how a plain file left over from before the vault was
/// encrypted (the migration) is told apart from one already done.
const MAGIC: &[u8; 4] = b"WRD1";
const NONCE_LEN: usize = 12;
/// Every encrypted name starts with this, so a plain name never reads as one.
const NAME_PREFIX: &str = "w1-";
/// A fixed nonce, safe only because names use SIV (see the module docs).
const NAME_NONCE: [u8; NONCE_LEN] = *b"warden-names";
/// Longest name (one folder or file) that still fits a file system's 255 bytes once encrypted and
/// spelled in base32.
pub const MAX_NAME_BYTES: usize = 120;

pub struct VaultCipher {
    content: Aes256Gcm,
    names: Aes256GcmSiv,
}

impl VaultCipher {
    pub fn new(key: &[u8; 32]) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(b"warden member vault"), key);
        let mut content = [0u8; 32];
        let mut names = [0u8; 32];
        hkdf.expand(b"content", &mut content).expect("32 bytes is a valid length");
        hkdf.expand(b"names", &mut names).expect("32 bytes is a valid length");
        Self {
            content: Aes256Gcm::new(&content.into()),
            names: Aes256GcmSiv::new(&names.into()),
        }
    }

    /// True for bytes this module wrote.
    pub fn is_sealed(data: &[u8]) -> bool {
        data.len() >= MAGIC.len() + NONCE_LEN && data.starts_with(MAGIC)
    }

    pub fn seal(&self, plain: &[u8]) -> Vec<u8> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let sealed = self.content.encrypt(&nonce, plain).expect("AES-GCM encryption doesn't fail");
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        out
    }

    /// Fails for bytes that aren't sealed, were changed, or were sealed with another key.
    pub fn open(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(Self::is_sealed(data), "this file is not encrypted with the member key");
        let (nonce, sealed) = data[MAGIC.len()..].split_at(NONCE_LEN);
        self.content.decrypt(nonce.into(), sealed).map_err(|_| anyhow::anyhow!("could not decrypt: wrong key, or the file was changed"))
    }

    /// One folder or file name, encrypted and spelled with lowercase letters and digits only
    /// (safe on file systems that ignore case).
    pub fn seal_name(&self, name: &str) -> anyhow::Result<String> {
        anyhow::ensure!(name.len() <= MAX_NAME_BYTES, "'{name}' is too long: a name can have at most {MAX_NAME_BYTES} bytes");
        let sealed = self.names.encrypt((&NAME_NONCE).into(), name.as_bytes()).expect("AES-GCM-SIV encryption doesn't fail");
        Ok(format!("{NAME_PREFIX}{}", base32_encode(&sealed)))
    }

    /// The name behind an encrypted one, `None` for anything else (a plain name, another key).
    pub fn open_name(&self, sealed: &str) -> Option<String> {
        let bytes = base32_decode(sealed.strip_prefix(NAME_PREFIX)?)?;
        let plain = self.names.decrypt((&NAME_NONCE).into(), bytes.as_slice()).ok()?;
        String::from_utf8(plain).ok()
    }

    /// True for a name this module wrote (by its shape, without decrypting).
    pub fn looks_sealed_name(name: &str) -> bool {
        name.starts_with(NAME_PREFIX)
    }
}

const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 8 / 5 + 1);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in data {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let value = ALPHABET.iter().position(|&a| a == c)? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher(seed: u8) -> VaultCipher {
        VaultCipher::new(&[seed; 32])
    }

    #[test]
    fn content_round_trips_and_is_not_readable() {
        let c = cipher(1);
        let sealed = c.seal("segredo do feijão".as_bytes());
        assert!(VaultCipher::is_sealed(&sealed));
        assert!(!String::from_utf8_lossy(&sealed).contains("segredo"));
        assert_eq!(c.open(&sealed).unwrap(), "segredo do feijão".as_bytes());
        assert_ne!(c.seal(b"x"), c.seal(b"x"), "a fresh nonce each time");
    }

    #[test]
    fn a_changed_file_or_another_key_fails() {
        let c = cipher(1);
        let mut sealed = c.seal(b"nota");
        assert!(cipher(2).open(&sealed).is_err());
        *sealed.last_mut().unwrap() ^= 1;
        assert!(c.open(&sealed).is_err());
        assert!(c.open(b"texto simples").is_err());
        assert!(!VaultCipher::is_sealed(b"WRD1"));
    }

    #[test]
    fn names_round_trip_and_are_stable() {
        let c = cipher(1);
        for name in ["notas", "reunião de março.md", "a", &"x".repeat(MAX_NAME_BYTES)] {
            let sealed = c.seal_name(name).unwrap();
            assert!(VaultCipher::looks_sealed_name(&sealed) && sealed.len() <= 255, "{sealed}");
            assert!(sealed.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'));
            assert_eq!(sealed, c.seal_name(name).unwrap(), "the same name always gives the same text");
            assert_eq!(c.open_name(&sealed).as_deref(), Some(name));
        }
        assert!(c.seal_name(&"x".repeat(MAX_NAME_BYTES + 1)).is_err());
    }

    #[test]
    fn a_name_of_another_key_or_a_plain_one_does_not_open() {
        let sealed = cipher(1).seal_name("notas").unwrap();
        assert_eq!(cipher(2).open_name(&sealed), None);
        assert_eq!(cipher(1).open_name("notas.md"), None);
        assert_eq!(cipher(1).open_name("w1-!!"), None);
    }
}
