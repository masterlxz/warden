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

    /// Real vectors generated 2026-09-10 by actually running the TruthID Dart SDK's own crypto
    /// code (`sdk/dart/lib/src/internal/{hkdf,pin_content_cipher}.dart`, `elliptic`'s
    /// `computeSecret`) against fixed inputs — not re-derived by reading source, the Dart SDK was
    /// installed and executed for real. Closes P38's biggest open risk: whether `k256`'s
    /// `diffie_hellman(...).raw_secret_bytes()` (the X-coordinate, big-endian, zero-padded to 32
    /// bytes) is the same convention as `elliptic`'s `computeSecret` (confirmed identical by
    /// reading `elliptic-0.3.12/lib/src/ecdh.dart`, but never run side-by-side until this test).
    /// See `project/PENDING.md` P38 for how this was produced and what it does/doesn't cover.
    #[test]
    fn matches_the_real_dart_sdk_pin_content_key_vector() {
        // `dart run bin/_warden_verify.dart` from `truthid/sdk/dart`, same session id as
        // `pin_content_round_trips` below.
        let key = derive_pin_content_key("00112233445566778899aabbccddeeff").unwrap();
        assert_eq!(hex::encode(key), "0fb347626731fa0f1b34ceff29470170c69bc06c6c3940dedcc433f55938080d");
    }

    #[test]
    fn ecdh_shared_secret_matches_the_real_dart_sdk_elliptic_package() {
        // privA = 1, privB = 2 (both valid secp256k1 scalars) — `elliptic`'s `computeSecret`
        // gives the same shared secret from either side (the X-coordinate of `2*G`), and so does
        // `k256`'s `diffie_hellman`, byte-for-byte against what the real Dart SDK computed.
        let mut priv_a_bytes = [0u8; 32];
        priv_a_bytes[31] = 1;
        let mut priv_b_bytes = [0u8; 32];
        priv_b_bytes[31] = 2;

        let secret_a = SecretKey::from_slice(&priv_a_bytes).unwrap();
        let secret_b = SecretKey::from_slice(&priv_b_bytes).unwrap();

        assert_eq!(
            hex::encode(secret_a.public_key().to_sec1_bytes()),
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );
        assert_eq!(
            hex::encode(secret_b.public_key().to_sec1_bytes()),
            "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
        );

        let shared_a_to_b = diffie_hellman(secret_a.to_nonzero_scalar(), secret_b.public_key().as_affine());
        let shared_b_to_a = diffie_hellman(secret_b.to_nonzero_scalar(), secret_a.public_key().as_affine());
        let expected = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

        assert_eq!(hex::encode(shared_a_to_b.raw_secret_bytes().as_slice()), expected);
        assert_eq!(hex::encode(shared_b_to_a.raw_secret_bytes().as_slice()), expected);
        assert_eq!(
            hex::encode(derive_aes_key(shared_a_to_b.raw_secret_bytes().as_slice())),
            "0135da2f8acf7b9e3090939432e47684eb888ea38c2173054d4eedffdf152ca5"
        );
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
