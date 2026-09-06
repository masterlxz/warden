import 'dart:convert';

/// Mirrors `crates/warden-server/src/protocol.rs`. Messages sent from this
/// client to a warden-server, over the WS + JSON protocol (Fase 9.2 / 7.2).
///
/// Wire shape: internally-tagged JSON with a `type` field, both the tag and
/// every field name are camelCase (`#[serde(tag = "type", rename_all =
/// "camelCase", rename_all_fields = "camelCase")]` on the Rust side).
sealed class ClientMessage {
  const ClientMessage();

  Map<String, dynamic> toJson();

  String encode() => jsonEncode(toJson());
}

final class HelloMessage extends ClientMessage {
  const HelloMessage({
    required this.deviceId,
    required this.deviceName,
    required this.authKey,
  });

  final String deviceId;
  final String deviceName;
  final String authKey;

  @override
  Map<String, dynamic> toJson() => {
        'type': 'hello',
        'deviceId': deviceId,
        'deviceName': deviceName,
        'authKey': authKey,
      };
}

final class PingMessage extends ClientMessage {
  const PingMessage(this.nonce);

  final int nonce;

  @override
  Map<String, dynamic> toJson() => {'type': 'ping', 'nonce': nonce};
}

final class GoodbyeMessage extends ClientMessage {
  const GoodbyeMessage([this.reason]);

  final String? reason;

  @override
  Map<String, dynamic> toJson() => {'type': 'goodbye', 'reason': reason};
}

/// Messages sent from a warden-server to this client.
sealed class ServerMessage {
  const ServerMessage();

  static ServerMessage fromJson(Map<String, dynamic> json) {
    return switch (json['type']) {
      'helloAck' => HelloAckMessage(json['serverName'] as String),
      'authError' => AuthErrorMessage(json['reason'] as String),
      'pong' => PongMessage(json['nonce'] as int),
      'goodbye' => GoodbyeServerMessage(json['reason'] as String?),
      final other => throw FormatException('Unknown ServerMessage type: $other'),
    };
  }

  static ServerMessage decode(String text) =>
      fromJson(jsonDecode(text) as Map<String, dynamic>);
}

final class HelloAckMessage extends ServerMessage {
  const HelloAckMessage(this.serverName);

  final String serverName;
}

final class AuthErrorMessage extends ServerMessage {
  const AuthErrorMessage(this.reason);

  final String reason;
}

final class PongMessage extends ServerMessage {
  const PongMessage(this.nonce);

  // Dart's `int` is a real 64-bit signed integer on the VM (Android/iOS),
  // matching Rust's u64 for any realistic monotonic counter value. This
  // stops holding if Flutter Web is ever targeted (`int` becomes a JS
  // double there) — not a concern for the mobile-only scope of Fase 7.
  final int nonce;
}

final class GoodbyeServerMessage extends ServerMessage {
  const GoodbyeServerMessage(this.reason);

  final String? reason;
}
