//! Signed webhook calls (P105): an HMAC-SHA256 of the request body, made with the webhook's signing secret
//! (`webhook_tokens.rs`), for services that sign what they send and can't be given a header of our choosing. Two
//! formats, the ones in the wild:
//!
//! - **GitHub** (also Gitea and Forgejo): `X-Hub-Signature-256: sha256=<hex>`, the HMAC of the raw body. It carries no
//!   time, so a recorded call could be sent again; `webhooks.rs` refuses a repeated delivery id (`X-GitHub-Delivery`)
//!   for that reason.
//! - **Stripe**: `Stripe-Signature: t=<unix seconds>,v1=<hex>[,v1=<hex>...]`, the HMAC of `"<t>.<raw body>"`. The time is
//!   part of what is signed, so a call older (or newer) than `STRIPE_TOLERANCE_SECS` is refused, which is what stops a
//!   replay. Several `v1` are what a service sends while it rotates its secret: any one that matches is enough.
//!
//! If both headers are there, GitHub's decides — a legitimate service sends one. Every comparison is constant-time
//! (`Mac::verify_slice`), and a header that isn't well formed is simply "not signed": the caller gets the same `401` as
//! for a wrong signature.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// How far from now a Stripe-style timestamp may be, in either direction: Stripe's own default.
pub const STRIPE_TOLERANCE_SECS: i64 = 300;

/// Whether the call is signed with `secret`. `github` and `stripe` are the values of `X-Hub-Signature-256` and
/// `Stripe-Signature`, when the request has them.
pub fn verify(secret: &str, github: Option<&str>, stripe: Option<&str>, body: &[u8], now_secs: i64) -> bool {
    match (github, stripe) {
        (Some(value), _) => verify_github(secret, body, value),
        (None, Some(value)) => verify_stripe(secret, body, value, now_secs),
        (None, None) => false,
    }
}

fn mac_for(secret: &str) -> HmacSha256 {
    // HMAC takes a key of any length, so this can't fail.
    HmacSha256::new_from_slice(secret.as_bytes()).expect("an HMAC key can be any length")
}

/// `sha256=<64 hex>` over the raw body.
pub fn verify_github(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(hex) = header.trim().strip_prefix("sha256=") else {
        return false;
    };
    let Some(expected) = decode_hex(hex) else {
        return false;
    };
    let mut mac = mac_for(secret);
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// `t=<seconds>,v1=<64 hex>` over `"<t>.<body>"`, within `STRIPE_TOLERANCE_SECS` of `now_secs`.
pub fn verify_stripe(secret: &str, body: &[u8], header: &str, now_secs: i64) -> bool {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", value)) => timestamp = value.trim().parse::<i64>().ok(),
            Some(("v1", value)) => signatures.extend(decode_hex(value.trim())),
            _ => {}
        }
    }
    let Some(timestamp) = timestamp else {
        return false;
    };
    if signatures.is_empty() || now_secs.abs_diff(timestamp) > STRIPE_TOLERANCE_SECS as u64 {
        return false;
    }
    let mut signed = timestamp.to_string().into_bytes();
    signed.push(b'.');
    signed.extend_from_slice(body);
    // Every candidate is checked (no early exit on the first), so how many were sent doesn't show in the time.
    signatures.iter().fold(false, |any, candidate| {
        let mut mac = mac_for(secret);
        mac.update(&signed);
        mac.verify_slice(candidate).is_ok() | any
    })
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some((high * 16 + low) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with Python's `hmac`, not with this crate, so the check is against something else.
    const BODY: &[u8] = br#"{"build":42,"status":"failed"}"#;
    const SECRET: &str = "whsec_test_secret";
    const GITHUB_SIGNATURE: &str = "7525e0cfa078cd877bd24fa9303a00c7d7eeb870e18d6a4d07341086519c530d";
    const STRIPE_SIGNATURE: &str = "2bfcbed227da70146bc82772c6b55fcd185879e5e9a65dd17a68caa252f87d0e";
    const STRIPE_TIME: i64 = 1_700_000_000;

    #[test]
    fn github_signs_the_raw_body() {
        // The example in GitHub's own documentation.
        let doc = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(verify_github("It's a Secret to Everybody", b"Hello, World!", doc));

        let header = format!("sha256={GITHUB_SIGNATURE}");
        assert!(verify_github(SECRET, BODY, &header));
        assert!(verify_github(SECRET, BODY, &format!("  {header}  ")), "surrounding spaces don't matter");
        assert!(!verify_github("another secret", BODY, &header), "a wrong secret");
        assert!(!verify_github(SECRET, br#"{"build":43,"status":"failed"}"#, &header), "a body changed on the way");
        assert!(!verify_github(SECRET, b"", &header));
    }

    #[test]
    fn github_headers_that_are_not_signatures_are_refused_not_crashed_on() {
        for bad in ["", "sha256=", "sha256=zz", "sha256=abc", GITHUB_SIGNATURE, "sha1=7525e0cf", "sha256=7525e0cfa078cd877bd24fa9303a00c7d7eeb870e18d6a4d07341086519c530", "sha256=ébé"] {
            assert!(!verify_github(SECRET, BODY, bad), "'{bad}'");
        }
        // Upper-case hex is the same number.
        assert!(verify_github(SECRET, BODY, &format!("sha256={}", GITHUB_SIGNATURE.to_uppercase())));
    }

    #[test]
    fn stripe_signs_the_time_and_the_body() {
        let header = format!("t={STRIPE_TIME},v1={STRIPE_SIGNATURE}");
        assert!(verify_stripe(SECRET, BODY, &header, STRIPE_TIME));
        assert!(verify_stripe(SECRET, BODY, &header, STRIPE_TIME + STRIPE_TOLERANCE_SECS), "right at the edge of the window");
        assert!(verify_stripe(SECRET, BODY, &header, STRIPE_TIME - STRIPE_TOLERANCE_SECS), "a clock a little behind");
        assert!(!verify_stripe(SECRET, BODY, &header, STRIPE_TIME + STRIPE_TOLERANCE_SECS + 1), "too old: a replay");
        assert!(!verify_stripe(SECRET, BODY, &header, STRIPE_TIME - STRIPE_TOLERANCE_SECS - 1), "from the future");
        assert!(!verify_stripe("another secret", BODY, &header, STRIPE_TIME));
        assert!(!verify_stripe(SECRET, b"{}", &header, STRIPE_TIME), "a body changed on the way");
        // The time is signed: moving it to look fresh breaks the signature.
        let moved = format!("t={},v1={STRIPE_SIGNATURE}", STRIPE_TIME + 100);
        assert!(!verify_stripe(SECRET, BODY, &moved, STRIPE_TIME + 100));
    }

    #[test]
    fn stripe_takes_any_of_several_signatures_and_refuses_what_is_malformed() {
        let wrong = "0".repeat(64);
        // While a service rotates its secret it sends the old and the new signature.
        let both = format!("t={STRIPE_TIME},v1={wrong},v1={STRIPE_SIGNATURE}");
        assert!(verify_stripe(SECRET, BODY, &both, STRIPE_TIME));
        assert!(verify_stripe(SECRET, BODY, &format!("v0=ignored, t={STRIPE_TIME} , v1={STRIPE_SIGNATURE}"), STRIPE_TIME), "other keys and spaces are fine");
        assert!(!verify_stripe(SECRET, BODY, &format!("t={STRIPE_TIME},v1={wrong}"), STRIPE_TIME));
        for bad in ["", "t=", "v1=", "t=abc,v1=00", &format!("v1={STRIPE_SIGNATURE}"), &format!("t={STRIPE_TIME}"), &format!("t={STRIPE_TIME},v1=nothex"), &format!("t={STRIPE_TIME},v0={STRIPE_SIGNATURE}")] {
            assert!(!verify_stripe(SECRET, BODY, bad, STRIPE_TIME), "'{bad}'");
        }
    }

    #[test]
    fn verify_uses_the_header_the_request_has_and_github_decides_when_both_are_there() {
        let github = format!("sha256={GITHUB_SIGNATURE}");
        let stripe = format!("t={STRIPE_TIME},v1={STRIPE_SIGNATURE}");
        assert!(verify(SECRET, Some(&github), None, BODY, STRIPE_TIME));
        assert!(verify(SECRET, None, Some(&stripe), BODY, STRIPE_TIME));
        assert!(!verify(SECRET, None, None, BODY, STRIPE_TIME), "no signature at all");
        // A good Stripe signature does not rescue a bad GitHub one: the first header decides.
        assert!(!verify(SECRET, Some("sha256=00"), Some(&stripe), BODY, STRIPE_TIME));
    }
}
