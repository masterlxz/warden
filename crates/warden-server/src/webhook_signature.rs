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
//! - **Slack**: `X-Slack-Signature: v0=<hex>` plus `X-Slack-Request-Timestamp: <unix seconds>`, the HMAC of
//!   `"v0:<t>:<raw body>"`. Like Stripe's, the time is signed and the same window applies.
//!
//! If several are there, the first of GitHub, Stripe, Slack decides — a legitimate service sends one. Every comparison is constant-time
//! (`Mac::verify_slice`), and a header that isn't well formed is simply "not signed": the caller gets the same `401` as
//! for a wrong signature.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// How far from now a Stripe-style timestamp may be, in either direction: Stripe's own default.
pub const STRIPE_TOLERANCE_SECS: i64 = 300;

/// The signature headers a request has, if any: `X-Hub-Signature-256`, `Stripe-Signature`, `X-Slack-Signature` and
/// `X-Slack-Request-Timestamp`.
#[derive(Default, Clone, Copy)]
pub struct SignatureHeaders<'a> {
    pub github: Option<&'a str>,
    pub stripe: Option<&'a str>,
    pub slack: Option<&'a str>,
    pub slack_timestamp: Option<&'a str>,
}

/// Whether the call is signed with `secret`.
pub fn verify(secret: &str, headers: SignatureHeaders<'_>, body: &[u8], now_secs: i64) -> bool {
    if let Some(value) = headers.github {
        verify_github(secret, body, value)
    } else if let Some(value) = headers.stripe {
        verify_stripe(secret, body, value, now_secs)
    } else if let Some(value) = headers.slack {
        headers.slack_timestamp.is_some_and(|timestamp| verify_slack(secret, body, value, timestamp, now_secs))
    } else {
        false
    }
}

/// `v0=<64 hex>` over `"v0:<timestamp>:<body>"`, within `STRIPE_TOLERANCE_SECS` of `now_secs` (Slack's own window).
pub fn verify_slack(secret: &str, body: &[u8], header: &str, timestamp: &str, now_secs: i64) -> bool {
    let Ok(sent_at) = timestamp.trim().parse::<i64>() else {
        return false;
    };
    let Some(expected) = header.trim().strip_prefix("v0=").and_then(decode_hex) else {
        return false;
    };
    if now_secs.abs_diff(sent_at) > STRIPE_TOLERANCE_SECS as u64 {
        return false;
    }
    let mut mac = mac_for(secret);
    mac.update(format!("v0:{sent_at}:").as_bytes());
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
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
    const SLACK_SIGNATURE: &str = "1840d7c7c2bfff1d2fa702afbf2423fbc977c0fd3e2eb549d187586ad94d45c7";
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
        let slack = format!("v0={SLACK_SIGNATURE}");
        let slack_time = STRIPE_TIME.to_string();
        let on = |headers| verify(SECRET, headers, BODY, STRIPE_TIME);
        assert!(on(SignatureHeaders { github: Some(&github), ..Default::default() }));
        assert!(on(SignatureHeaders { stripe: Some(&stripe), ..Default::default() }));
        assert!(on(SignatureHeaders { slack: Some(&slack), slack_timestamp: Some(&slack_time), ..Default::default() }));
        assert!(!on(SignatureHeaders::default()), "no signature at all");
        assert!(!on(SignatureHeaders { slack: Some(&slack), ..Default::default() }), "Slack's time header is part of the proof");
        // A good Stripe signature does not rescue a bad GitHub one: the first header decides.
        assert!(!on(SignatureHeaders { github: Some("sha256=00"), stripe: Some(&stripe), ..Default::default() }));
    }

    #[test]
    fn slack_signs_the_version_the_time_and_the_body() {
        // The example in Slack's own documentation.
        let doc_body = b"token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.47445629121.803a0bc887a14d10d2c447fce8b6703c";
        let doc = "v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503";
        assert!(verify_slack("8f742231b10e8888abcd99yyyzzz85a5", doc_body, doc, "1531420618", 1_531_420_618));

        let header = format!("v0={SLACK_SIGNATURE}");
        let time = STRIPE_TIME.to_string();
        assert!(verify_slack(SECRET, BODY, &header, &time, STRIPE_TIME));
        assert!(verify_slack(SECRET, BODY, &header, &time, STRIPE_TIME + STRIPE_TOLERANCE_SECS), "right at the edge of the window");
        assert!(!verify_slack(SECRET, BODY, &header, &time, STRIPE_TIME + STRIPE_TOLERANCE_SECS + 1), "too old: a replay");
        assert!(!verify_slack(SECRET, BODY, &header, &time, STRIPE_TIME - STRIPE_TOLERANCE_SECS - 1), "from the future");
        assert!(!verify_slack("another secret", BODY, &header, &time, STRIPE_TIME));
        assert!(!verify_slack(SECRET, b"{}", &header, &time, STRIPE_TIME), "a body changed on the way");
        let moved = (STRIPE_TIME + 100).to_string();
        assert!(!verify_slack(SECRET, BODY, &header, &moved, STRIPE_TIME + 100), "the time is signed");
        for bad in ["", "v0=", "v0=zz", SLACK_SIGNATURE, &format!("v1={SLACK_SIGNATURE}")] {
            assert!(!verify_slack(SECRET, BODY, bad, &time, STRIPE_TIME), "'{bad}'");
        }
        assert!(!verify_slack(SECRET, BODY, &header, "abc", STRIPE_TIME));
    }
}
