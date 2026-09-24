import 'dart:convert';

/// The two fields the desktop Workspace screen's "Pareamento por QR" section embeds in its QR
/// (Fase 9.7, `desktop/src-tauri/src/workspace_cmds.rs::hub_pairing_qr_svg`) — everything
/// `ConnectionScreen` would otherwise need typed by hand except the device name, which stays
/// client-chosen.
class HubPairingPayload {
  const HubPairingPayload({required this.host, required this.port, required this.authKey, this.useTls = false});

  final String host;
  final int port;
  final String authKey;

  /// `serverUrl` was `wss://` — a TLS-only hub (P36).
  final bool useTls;
}

/// Decodes a scanned QR's raw text into a [HubPairingPayload], or `null` if it isn't one (not
/// JSON, missing a field, or `serverUrl` isn't a `ws://`/`wss://` `host:port` URI) — pulled out as a pure
/// function so it's testable without a camera or platform channel, same split as
/// `chat_notifications.dart`'s `shouldNotifyFor`/`notificationContentFor`.
HubPairingPayload? parseHubPairingQr(String raw) {
  final Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } on FormatException {
    return null;
  }
  if (decoded is! Map<String, dynamic>) return null;

  final serverUrl = decoded['serverUrl'];
  final authKey = decoded['authKey'];
  if (serverUrl is! String || authKey is! String || authKey.isEmpty) return null;

  final uri = Uri.tryParse(serverUrl);
  if (uri == null || uri.host.isEmpty || !uri.hasPort) return null;
  if (uri.scheme != 'ws' && uri.scheme != 'wss') return null;

  return HubPairingPayload(host: uri.host, port: uri.port, authKey: authKey, useTls: uri.scheme == 'wss');
}
