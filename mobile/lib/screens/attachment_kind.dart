/// How `_AttachmentPreview` (`chat_screen.dart`) should render an [Attachment] by its
/// `mimeType` — pulled out as a pure function so the dispatch is testable without building a
/// widget tree or mocking any platform channel, same split as
/// `services/hub_pairing_qr.dart::parseHubPairingQr` and
/// `services/chat_notifications.dart::shouldNotifyFor`.
enum AttachmentKind { image, audio, video, unsupported }

AttachmentKind attachmentKindFor(String mimeType) {
  if (mimeType.startsWith('image/')) return AttachmentKind.image;
  if (mimeType.startsWith('audio/')) return AttachmentKind.audio;
  if (mimeType.startsWith('video/')) return AttachmentKind.video;
  return AttachmentKind.unsupported;
}
