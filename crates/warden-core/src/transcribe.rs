use serde::Deserialize;

/// OpenAI's Whisper endpoint (P28 part 2) — dedicated speech-to-text, used regardless of which
/// `ModelProvider` is active for chat, so voice input works the same no matter which of the 3
/// chat providers (OpenAI, Anthropic, Gemini) is selected. Not a `ModelProvider`/`Tool`: this
/// runs as a pre-processing step before `Orchestrator::handle_message` ever sees the turn, the
/// model never invokes it itself.
const API_URL: &str = "https://api.openai.com/v1/audio/transcriptions";

/// `whisper-1` is the long-established, broadly compatible model on this endpoint — same "fixed,
/// no per-request config yet" scope as `AnthropicProvider::MAX_TOKENS`. Revisit if
/// `gpt-4o-transcribe` turns out to be worth the switch.
const MODEL: &str = "whisper-1";

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

/// Transcribes an audio recording to text. `filename` only needs a plausible extension (e.g.
/// `"audio.webm"`) — the API infers the format from it, no separate mime-type field.
pub async fn transcribe_audio(api_key: &str, audio_bytes: Vec<u8>, filename: &str) -> anyhow::Result<String> {
    let part = reqwest::multipart::Part::bytes(audio_bytes).file_name(filename.to_string());
    let form = reqwest::multipart::Form::new().text("model", MODEL).part("file", part);

    let client = reqwest::Client::new();
    let response = client.post(API_URL).bearer_auth(api_key).multipart(form).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Whisper API error ({status}): {body}");
    }

    let parsed: TranscriptionResponse = response.json().await?;
    Ok(parsed.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcription_response_deserializes_from_the_wire_shape() {
        let parsed: TranscriptionResponse = serde_json::from_str(r#"{"text": "hello world"}"#).unwrap();
        assert_eq!(parsed.text, "hello world");
    }
}
