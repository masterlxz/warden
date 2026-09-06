use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hkdf::Hkdf;
use k256::ecdh::diffie_hellman;
use k256::{PublicKey, SecretKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const COMPRESSED_PUBKEY_LEN: usize = 33;

const PIN_CONTENT_HKDF_SALT: &[u8] = b"TruthID Pin Content";
const PIN_CONTENT_HKDF_INFO: &[u8] = b"content-key-v1";

/// Derives the phase-1 (content push) AES-256 key from the session id, mirroring
/// `sdk/dart/lib/src/internal/pin_content_cipher.dart::derivePinContentKey` exactly:
/// HKDF-SHA256(ikm = raw session-id bytes, salt = "TruthID Pin Content", info = "content-key-v1").
pub fn derive_pin_content_key(session_id_hex: &str) -> anyhow::Result<[u8; 32]> {
    let ikm = hex::decode(session_id_hex)?;
    let hk = Hkdf::<Sha256>::new(Some(PIN_CONTENT_HKDF_SALT), &ikm);
    let mut key = [0u8; 32];
    hk.expand(PIN_CONTENT_HKDF_INFO, &mut key)
        .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
    Ok(key)
}

/// `nonce(12) || ciphertext || tag(16)`, AES-256-GCM — same wire format as
/// `pin_content_cipher.dart::encryptPinContent`.
pub fn encrypt_pin_content(plaintext: &[u8], key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce_bytes: [u8; NONCE_LEN] = rand_bytes();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| anyhow::anyhow!("AES-GCM encryption failed"))?;
    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

pub fn decrypt_pin_content(blob: &[u8], key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    if blob.len() < NONCE_LEN + TAG_LEN {
        anyhow::bail!("blob too short to be a valid pin-content payload");
    }
    let (nonce_bytes, rest) = blob.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), rest)
        .map_err(|_| anyhow::anyhow!("AES-GCM decryption failed"))
}

/// A one-off secp256k1 keypair for a request session — the private half stays with the caller,
/// the compressed public half goes into the QR payload as `ephemeralPubKey`.
pub struct EciesKeyPair {
    pub secret: SecretKey,
    pub public_hex: String,
}

pub fn generate_ecies_keypair() -> EciesKeyPair {
    let secret = SecretKey::random(&mut OsRng);
    let public_hex = hex::encode(secret.public_key().to_sec1_bytes());
    EciesKeyPair { secret, public_hex }
}

fn parse_public_key(hex_str: &str) -> anyhow::Result<PublicKey> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str)?;
    PublicKey::from_sec1_bytes(&bytes).map_err(|_| anyhow::anyhow!("invalid secp256k1 public key"))
}

fn derive_aes_key(shared_secret: &[u8]) -> [u8; 32] {
    Sha256::digest(shared_secret).into()
}

/// Generic ECIES (secp256k1 ECDH + SHA-256 + AES-256-GCM), same algorithm and blob format as
/// `mobile/lib/services/ecies_service.dart` / `sdk/dart/lib/src/internal/ecies.dart`:
/// `ephemeral_pubkey(33, compressed) || nonce(12) || ciphertext || tag(16)`. The AES key is
/// always `SHA-256(ECDH secret)`, never the raw secret.
pub fn ecies_encrypt(plaintext: &[u8], recipient_pub_hex: &str) -> anyhow::Result<Vec<u8>> {
    let recipient_pub = parse_public_key(recipient_pub_hex)?;
    let ephemeral_secret = SecretKey::random(&mut OsRng);
    let ephemeral_pub_bytes = ephemeral_secret.public_key().to_sec1_bytes();

    let shared = diffie_hellman(
        ephemeral_secret.to_nonzero_scalar(),
        recipient_pub.as_affine(),
    );
    let aes_key = derive_aes_key(shared.raw_secret_bytes().as_slice());

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&aes_key));
    let nonce_bytes: [u8; NONCE_LEN] = rand_bytes();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| anyhow::anyhow!("AES-GCM encryption failed"))?;

    let mut blob = Vec::with_capacity(ephemeral_pub_bytes.len() + NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&ephemeral_pub_bytes);
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

pub fn ecies_decrypt(blob: &[u8], recipient_secret: &SecretKey) -> anyhow::Result<Vec<u8>> {
    if blob.len() < COMPRESSED_PUBKEY_LEN + NONCE_LEN + TAG_LEN {
        anyhow::bail!("blob too short to be a valid ECIES payload");
    }
    let (ephemeral_pub_bytes, rest) = blob.split_at(COMPRESSED_PUBKEY_LEN);
    let ephemeral_pub = PublicKey::from_sec1_bytes(ephemeral_pub_bytes)
        .map_err(|_| anyhow::anyhow!("invalid ephemeral public key in ECIES blob"))?;

    let shared = diffie_hellman(recipient_secret.to_nonzero_scalar(), ephemeral_pub.as_affine());
    let aes_key = derive_aes_key(shared.raw_secret_bytes().as_slice());

    let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&aes_key));
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| anyhow::anyhow!("ECIES AES-GCM decryption failed"))
}

fn rand_bytes<const N: usize>() -> [u8; N] {
    use rand_core::RngCore;
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkdf_matches_known_rfc5869_vector() {
        // RFC 5869 Appendix A, Test Case 1 (HKDF-SHA256).
        let ikm = hex::decode("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b").unwrap();
        let salt = hex::decode("000102030405060708090a0b0c").unwrap();
        let info = hex::decode("f0f1f2f3f4f5f6f7f8f9").unwrap();
        let expected = "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865";

        let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
        let mut okm = [0u8; 42];
        hk.expand(&info, &mut okm).unwrap();
        assert_eq!(hex::encode(okm), expected);
    }

    #[test]
    fn derive_pin_content_key_rejects_invalid_hex() {
        assert!(derive_pin_content_key("not-hex").is_err());
    }

    #[test]
    fn pin_content_round_trips() {
        // 32 hex chars = 16 raw bytes, matching a real session id's length.
        let session_id = "00112233445566778899aabbccddeeff";
        let key = derive_pin_content_key(session_id).unwrap();
        let plaintext = b"warden vault blob";
        let blob = encrypt_pin_content(plaintext, &key).unwrap();
        assert_eq!(blob.len(), NONCE_LEN + plaintext.len() + TAG_LEN);
        let decrypted = decrypt_pin_content(&blob, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn ecies_round_trips_between_two_keypairs() {
        let recipient = generate_ecies_keypair();
        let plaintext = b"pin result payload";

        let blob = ecies_encrypt(plaintext, &recipient.public_hex).unwrap();
        assert_eq!(
            blob.len(),
            COMPRESSED_PUBKEY_LEN + NONCE_LEN + plaintext.len() + TAG_LEN
        );

        let decrypted = ecies_decrypt(&blob, &recipient.secret).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn ecies_decrypt_fails_with_the_wrong_key() {
        let recipient = generate_ecies_keypair();
        let other = generate_ecies_keypair();
        let blob = ecies_encrypt(b"secret", &recipient.public_hex).unwrap();
        assert!(ecies_decrypt(&blob, &other.secret).is_err());
    }
}
