//! What a client sends along with its words (P78): attachments on a `Chat` turn, and voice
//! recordings to transcribe. Kept out of `server.rs` so it's testable without a socket — same
//! split as `skills.rs`/`conversations.rs`.

use std::path::PathBuf;

use async_trait::async_trait;
use base64::Engine;
use warden_core::model::{Attachment, USER_ATTACHMENT_MIME_TYPES};
use warden_server_protocol::ServerMessage;

/// The most base64 a `Chat` turn's attachments may add up to. A browser sends a whole WebSocket
/// message as one frame and the hub reads frames of up to 16 MiB (tungstenite's default), so this
/// stays under that with room for the rest of the message; the web client resizes images and
/// checks the same limit before sending.
pub const MAX_ATTACHMENTS_BASE64_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_ATTACHMENTS_PER_TURN: usize = 10;

/// Refuses attachments the model can't take (not an image/PDF) or that are too big — the text
/// becomes the turn's `ChatError`. Checked before anything reaches the model or the disk.
pub fn validate_attachments(attachments: &[Attachment]) -> Result<(), String> {
    if attachments.len() > MAX_ATTACHMENTS_PER_TURN {
        return Err(format!("too many attachments ({}, at most {MAX_ATTACHMENTS_PER_TURN} per message)", attachments.len()));
    }
    if let Some(bad) = attachments.iter().find(|a| !USER_ATTACHMENT_MIME_TYPES.contains(&a.mime_type.as_str())) {
        return Err(format!("unsupported attachment type '{}' (images and PDFs only)", bad.mime_type));
    }
    if attachments.iter().any(|a| a.data.is_empty()) {
        return Err("an attachment is empty".to_string());
    }
    let total: usize = attachments.iter().map(|a| a.data.len()).sum();
    if total > MAX_ATTACHMENTS_BASE64_BYTES {
        return Err(format!(
            "attachments too large ({:.1} MB encoded, at most {} MB per message)",
            total as f64 / (1024.0 * 1024.0),
            MAX_ATTACHMENTS_BASE64_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

/// What a turn with no words is titled after, when it starts a conversation.
pub fn title_seed(message: &str, attachments: &[Attachment]) -> String {
    if !message.trim().is_empty() || attachments.is_empty() {
        return message.to_string();
    }
    if attachments.iter().all(|a| a.mime_type.starts_with("image/")) {
        "Image".to_string()
    } else {
        "Document".to_string()
    }
}

/// Turns a voice recording into text for `Transcribe` (P78). A trait so tests don't call Whisper.
#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, audio: Attachment) -> anyhow::Result<String>;
}

/// Whisper with the key from the hub's config file (`api_keys.whisper`, the same key the desktop's
/// mic button uses), read on every call so setting it doesn't need a restart — same as the desktop.
pub struct WhisperTranscriber {
    /// The hub's `--config`; `None` is the default config path.
    config_path: Option<PathBuf>,
}

impl WhisperTranscriber {
    pub fn new(config_path: Option<PathBuf>) -> Self {
        Self { config_path }
    }
}

#[async_trait]
impl Transcriber for WhisperTranscriber {
    async fn transcribe(&self, audio: Attachment) -> anyhow::Result<String> {
        let config = match &self.config_path {
            Some(path) => warden_bootstrap::load_config_from_path(path, true)?,
            None => warden_bootstrap::load_config(None)?,
        };
        let api_key = config
            .api_keys
            .whisper
            .filter(|k| !k.is_empty())
            .ok_or_else(|| anyhow::anyhow!("voice input needs a Whisper API key in this hub's config (api_keys.whisper)"))?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&audio.data)?;
        let filename = warden_core::transcribe::audio_filename_for_mime_type(&audio.mime_type);
        warden_core::transcribe::transcribe_audio(&api_key, bytes, filename).await
    }
}

/// Answers a `Transcribe`. `None` for the transcriber means voice input isn't wired on this hub.
pub async fn handle_transcribe(transcriber: Option<&dyn Transcriber>, request_id: u64, audio: Attachment) -> ServerMessage {
    let Some(transcriber) = transcriber else {
        return ServerMessage::TranscriptionError { request_id, message: "this hub doesn't transcribe voice".to_string() };
    };
    if !audio.mime_type.starts_with("audio/") {
        return ServerMessage::TranscriptionError { request_id, message: format!("'{}' isn't an audio recording", audio.mime_type) };
    }
    match transcriber.transcribe(audio).await {
        Ok(text) => ServerMessage::Transcription { request_id, text },
        Err(err) => ServerMessage::TranscriptionError { request_id, message: format!("{err:#}") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(mime_type: &str, len: usize) -> Attachment {
        Attachment { mime_type: mime_type.to_string(), data: "A".repeat(len) }
    }

    #[test]
    fn images_and_pdfs_within_the_cap_are_accepted() {
        assert!(validate_attachments(&[]).is_ok());
        assert!(validate_attachments(&[attachment("image/jpeg", 10), attachment("application/pdf", 10)]).is_ok());
    }

    #[test]
    fn other_types_empty_data_too_many_or_too_big_are_refused() {
        let err = validate_attachments(&[attachment("text/html", 10)]).unwrap_err();
        assert!(err.contains("text/html"), "{err}");
        assert!(validate_attachments(&[attachment("image/png", 0)]).unwrap_err().contains("empty"));
        let many: Vec<_> = (0..=MAX_ATTACHMENTS_PER_TURN).map(|_| attachment("image/png", 1)).collect();
        assert!(validate_attachments(&many).unwrap_err().contains("too many"));
        let half = MAX_ATTACHMENTS_BASE64_BYTES / 2 + 1;
        assert!(validate_attachments(&[attachment("image/png", half), attachment("image/png", half)]).unwrap_err().contains("too large"));
    }

    #[test]
    fn a_turn_without_words_is_titled_by_what_it_carries() {
        assert_eq!(title_seed("hello", &[attachment("image/png", 1)]), "hello");
        assert_eq!(title_seed("  ", &[attachment("image/png", 1)]), "Image");
        assert_eq!(title_seed("", &[attachment("image/png", 1), attachment("application/pdf", 1)]), "Document");
        assert_eq!(title_seed("", &[]), "");
    }

    struct Fixed(&'static str);

    #[async_trait]
    impl Transcriber for Fixed {
        async fn transcribe(&self, _audio: Attachment) -> anyhow::Result<String> {
            Ok(self.0.to_string())
        }
    }

    #[tokio::test]
    async fn transcribe_answers_with_text_or_an_error() {
        let audio = attachment("audio/webm;codecs=opus", 4);
        assert_eq!(
            handle_transcribe(Some(&Fixed("hi there")), 1, audio.clone()).await,
            ServerMessage::Transcription { request_id: 1, text: "hi there".into() }
        );
        assert!(matches!(handle_transcribe(None, 2, audio).await, ServerMessage::TranscriptionError { request_id: 2, .. }));
        assert!(matches!(
            handle_transcribe(Some(&Fixed("x")), 3, attachment("image/png", 4)).await,
            ServerMessage::TranscriptionError { request_id: 3, .. }
        ));
    }
}
