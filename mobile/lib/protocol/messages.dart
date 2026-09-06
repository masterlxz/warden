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
    this.tools = const [],
  });

  final String deviceId;
  final String deviceName;
  final String authKey;

  /// Local tools this client can run on request (Fase 7.4), e.g.
  /// `MobileFileTool.listFilesSpec`/`readFileSpec` — each a `{name, description, parameters}`
  /// map mirroring `warden_core::tool::ToolSpec`. Empty when nothing is configured (e.g. no
  /// root folder picked yet) — `warden-server` only builds the remote-tool-dispatch machinery
  /// when this is non-empty.
  final List<Map<String, dynamic>> tools;

  @override
  Map<String, dynamic> toJson() => {
        'type': 'hello',
        'deviceId': deviceId,
        'deviceName': deviceName,
        'authKey': authKey,
        'tools': tools,
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

/// A chat turn (Fase 7.3) — answered by the `Orchestrator` `warden-server` hosts, keyed by this
/// device's id (one conversation per device, same pattern as Telegram/WhatsApp on the Rust side).
final class ChatMessage extends ClientMessage {
  const ChatMessage(this.message);

  final String message;

  @override
  Map<String, dynamic> toJson() => {'type': 'chat', 'message': message};
}

/// The result of a `ToolCallRequestMessage` this client was asked to run (Fase 7.4).
final class ToolCallResultMessage extends ClientMessage {
  const ToolCallResultMessage(this.callId, this.result);

  final int callId;
  final dynamic result;

  @override
  Map<String, dynamic> toJson() => {'type': 'toolCallResult', 'callId': callId, 'result': result};
}

/// This client failed to run a requested tool call (Fase 7.4).
final class ToolCallErrorMessage extends ClientMessage {
  const ToolCallErrorMessage(this.callId, this.message);

  final int callId;
  final String message;

  @override
  Map<String, dynamic> toJson() => {'type': 'toolCallError', 'callId': callId, 'message': message};
}

/// Token usage for one chat turn, when the provider reported it. Mirrors
/// `warden_core::model::Usage`.
class Usage {
  const Usage({
    required this.promptTokens,
    required this.completionTokens,
    required this.totalTokens,
  });

  final int promptTokens;
  final int completionTokens;
  final int totalTokens;

  static Usage? fromJson(Map<String, dynamic>? json) {
    if (json == null) return null;
    return Usage(
      promptTokens: json['promptTokens'] as int,
      completionTokens: json['completionTokens'] as int,
      totalTokens: json['totalTokens'] as int,
    );
  }
}

/// Messages sent from a warden-server to this client.
sealed class ServerMessage {
  const ServerMessage();

  static ServerMessage fromJson(Map<String, dynamic> json) {
    return switch (json['type']) {
      'helloAck' => HelloAckMessage(json['serverName'] as String),
      'authError' => AuthErrorMessage(json['reason'] as String),
      'pong' => PongMessage(json['nonce'] as int),
      'chatResponse' => ChatResponseMessage(
          json['content'] as String,
          Usage.fromJson(json['usage'] as Map<String, dynamic>?),
        ),
      'chatError' => ChatErrorMessage(json['message'] as String),
      'toolCallRequest' => ToolCallRequestMessage(
          json['callId'] as int,
          json['tool'] as String,
          json['arguments'] as Map<String, dynamic>,
        ),
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

final class ChatResponseMessage extends ServerMessage {
  const ChatResponseMessage(this.content, this.usage);

  final String content;
  final Usage? usage;
}

final class ChatErrorMessage extends ServerMessage {
  const ChatErrorMessage(this.message);

  final String message;
}

/// Asks this client to run one of the tools it advertised in `Hello.tools` (Fase 7.4).
final class ToolCallRequestMessage extends ServerMessage {
  const ToolCallRequestMessage(this.callId, this.tool, this.arguments);

  final int callId;
  final String tool;
  final Map<String, dynamic> arguments;
}

final class GoodbyeServerMessage extends ServerMessage {
  const GoodbyeServerMessage(this.reason);

  final String? reason;
}
