use serde::Serialize;

/// OpenAI's TTS endpoint (P28 part 3) — the flip side of `transcribe.rs`: text-to-speech for the
/// desktop chat's response, called on demand by the UI (a per-message play button), never by the
/// model itself.
const API_URL: &str = "https://api.openai.com/v1/audio/speech";

/// `tts-1` (lower latency, good enough quality) and `alloy` (a neutral default voice) — same
/// "fixed, no per-request config yet" scope as `transcribe::MODEL`. Revisit if voice choice ever
/// becomes something the UI exposes.
const MODEL: &str = "tts-1";
const VOICE: &str = "alloy";

#[derive(Serialize)]
struct SpeechRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice: &'a str,
}

/// Synthesizes speech from text, returning the raw audio bytes (mp3, the API's default
/// `response_format`) — unlike `transcribe_audio`, there's no JSON envelope to unwrap.
pub async fn synthesize_speech(api_key: &str, text: &str) -> anyhow::Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let response = client
        .post(API_URL)
        .bearer_auth(api_key)
        .json(&SpeechRequest { model: MODEL, input: text, voice: VOICE })
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("TTS API error ({status}): {body}");
    }

    Ok(response.bytes().await?.to_vec())
}
