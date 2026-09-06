import 'dart:async';

import 'package:meta/meta.dart';
import 'package:stream_channel/stream_channel.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import '../protocol/messages.dart';

/// Connection state exposed to the UI. Mirrors the lifecycle a Fase 7.2
/// connection screen needs to render — nothing more.
sealed class ConnectionStatus {
  const ConnectionStatus();
}

final class Disconnected extends ConnectionStatus {
  /// Null when the disconnect was user-initiated (clean `Goodbye`) or the
  /// connection was simply never opened. Non-null describes what went wrong.
  const Disconnected([this.reason]);

  final String? reason;
}

final class Connecting extends ConnectionStatus {
  const Connecting();
}

final class Connected extends ConnectionStatus {
  const Connected(this.serverName, {this.lastPongAt});

  final String serverName;
  final DateTime? lastPongAt;
}

final class ConnectionFailure extends ConnectionStatus {
  const ConnectionFailure(this.message);

  final String message;
}

class HandshakeException implements Exception {
  HandshakeException(this.message);

  final String message;

  @override
  String toString() => 'HandshakeException: $message';
}

/// Dart mirror of `crates/warden-server/src/client.rs`'s `ServerConnection`.
///
/// Written against `StreamChannel<dynamic>` rather than `WebSocketChannel`
/// directly — `WebSocketChannel` already *is* a `StreamChannel`, so the
/// handshake/heartbeat state machine here is exercised in tests over a real
/// (non-network) `StreamChannelController` pair instead of a socket, the
/// same "real logic, no network" spirit as the Rust side's
/// `tests/handshake.rs` (real server + real client on 127.0.0.1, not mocks).
///
/// `channel.stream` is single-subscription (true both for a real
/// `WebSocketChannel` and for a `StreamChannelController`'s halves — see
/// `AdapterWebSocketChannel`, which is itself backed by a plain, non-broadcast
/// `StreamController`), so it can only ever be listened to once. The
/// handshake and the ongoing message loop therefore share one
/// `StreamSubscription`, created before `Hello` is sent and handed off (with
/// its callbacks swapped) to the connected `ServerConnection` on success —
/// never a second `.listen()` call on the same stream.
///
/// Deliberately out of scope for Fase 7.2: no automatic reconnect on
/// app-lifecycle transitions (foreground/background) and no retry/backoff
/// on a failed or dropped connection — this phase step only has to prove
/// connectivity, not be resilient. That's an accepted limitation, not a
/// silent gap (see `PENDING.md`).
class ServerConnection {
  ServerConnection._(this._channel, this._subscription, this.serverName) {
    _setStatus(Connected(serverName));
    _startHeartbeat();
    _subscription
      ..onData((dynamic frame) => _onMessage(ServerMessage.decode(frame as String)))
      ..onError((Object err) {
        _heartbeatTimer?.cancel();
        _setStatus(ConnectionFailure(err.toString()));
      })
      ..onDone(() {
        _heartbeatTimer?.cancel();
        _setStatus(
          _goodbyeSent
              ? const Disconnected() // clean, user-initiated — not an error
              : const ConnectionFailure('Connection closed unexpectedly'),
        );
      });
  }

  final StreamChannel<dynamic> _channel;
  final StreamSubscription<dynamic> _subscription;
  final String serverName;

  final _statusController = StreamController<ConnectionStatus>.broadcast();
  ConnectionStatus _status = const Connecting();
  ConnectionStatus get status => _status;
  Stream<ConnectionStatus> get statusStream => _statusController.stream;

  // Carries only ChatResponseMessage/ChatErrorMessage (Fase 7.3) — everything else stays
  // internal to the handshake/heartbeat machinery above.
  final _chatController = StreamController<ServerMessage>.broadcast();
  Stream<ServerMessage> get chatStream => _chatController.stream;

  Timer? _heartbeatTimer;
  int _nextNonce = 0;
  int? _pendingPingNonce;
  bool _goodbyeSent = false;

  static const defaultHandshakeTimeout = Duration(seconds: 10);

  // Mobile carrier NATs commonly drop idle TCP connections silently within
  // a few minutes; 30s keeps well under that without waking the radio too
  // often. The server never enforces or initiates a heartbeat itself — this
  // cadence exists purely so the client can notice a dead connection.
  static const heartbeatInterval = Duration(seconds: 30);

  static Future<ServerConnection> connect({
    required String host,
    required int port,
    required String deviceId,
    required String deviceName,
    required String authKey,
    Duration handshakeTimeout = defaultHandshakeTimeout,
  }) {
    final channel = WebSocketChannel.connect(Uri.parse('ws://$host:$port'));
    return _handshake(
      channel,
      deviceId: deviceId,
      deviceName: deviceName,
      authKey: authKey,
      handshakeTimeout: handshakeTimeout,
    );
  }

  @visibleForTesting
  static Future<ServerConnection> connectOverChannel({
    required StreamChannel<dynamic> channel,
    required String deviceId,
    required String deviceName,
    required String authKey,
    Duration handshakeTimeout = defaultHandshakeTimeout,
  }) =>
      _handshake(
        channel,
        deviceId: deviceId,
        deviceName: deviceName,
        authKey: authKey,
        handshakeTimeout: handshakeTimeout,
      );

  static Future<ServerConnection> _handshake(
    StreamChannel<dynamic> channel, {
    required String deviceId,
    required String deviceName,
    required String authKey,
    required Duration handshakeTimeout,
  }) async {
    if (channel is WebSocketChannel) {
      // Surfaces TCP/connect-time failures before Hello is even sent.
      await channel.ready;
    }

    final firstFrame = Completer<ServerMessage>();
    final subscription = channel.stream.listen(
      (dynamic frame) {
        if (!firstFrame.isCompleted) {
          firstFrame.complete(ServerMessage.decode(frame as String));
        }
      },
      onError: (Object err) {
        if (!firstFrame.isCompleted) firstFrame.completeError(err);
      },
      onDone: () {
        if (!firstFrame.isCompleted) {
          firstFrame.completeError(
            HandshakeException('server closed the connection before replying to Hello'),
          );
        }
      },
    );

    final timeoutTimer = Timer(handshakeTimeout, () {
      if (!firstFrame.isCompleted) {
        firstFrame.completeError(
          HandshakeException('No response to Hello within ${handshakeTimeout.inSeconds}s'),
        );
      }
    });

    channel.sink.add(HelloMessage(
      deviceId: deviceId,
      deviceName: deviceName,
      authKey: authKey,
    ).encode());

    try {
      final reply = await firstFrame.future;
      switch (reply) {
        case HelloAckMessage(:final serverName):
          return ServerConnection._(channel, subscription, serverName);
        case AuthErrorMessage(:final reason):
          await subscription.cancel();
          throw HandshakeException('authentication rejected: $reason');
        case PongMessage() || GoodbyeServerMessage() || ChatResponseMessage() || ChatErrorMessage():
          await subscription.cancel();
          throw HandshakeException('expected HelloAck, got $reply');
      }
    } catch (_) {
      await subscription.cancel();
      rethrow;
    } finally {
      timeoutTimer.cancel();
    }
  }

  void _onMessage(ServerMessage msg) {
    switch (msg) {
      case PongMessage(:final nonce):
        if (nonce == _pendingPingNonce) {
          _pendingPingNonce = null;
          final current = _status;
          if (current is Connected) {
            _setStatus(Connected(current.serverName, lastPongAt: DateTime.now()));
          }
        }
        // Nonce mismatch or an unsolicited pong: nothing in 7.2 depends on
        // strict correlation beyond dead-connection detection, so ignore it.
      case GoodbyeServerMessage():
        // The server never actually sends this today, but handle it for
        // completeness/forward-compatibility.
        _heartbeatTimer?.cancel();
        _setStatus(const Disconnected());
      case ChatResponseMessage():
      case ChatErrorMessage():
        _chatController.add(msg);
      case HelloAckMessage():
      case AuthErrorMessage():
        // Only ever valid as the first frame, already consumed by _handshake.
        break;
    }
  }

  /// Sends one chat turn. The reply arrives asynchronously on [chatStream] as either a
  /// [ChatResponseMessage] or a [ChatErrorMessage].
  void sendChat(String message) {
    _channel.sink.add(ChatMessage(message).encode());
  }

  void _startHeartbeat() {
    _heartbeatTimer = Timer.periodic(heartbeatInterval, (_) => pingNow());
  }

  /// Sends one heartbeat ping immediately. Also invoked by the periodic
  /// timer; exposed so tests can drive heartbeat/pong logic deterministically
  /// without waiting on a real 30s `Timer.periodic`.
  @visibleForTesting
  void pingNow() {
    if (_pendingPingNonce != null) {
      // The previous ping never got a pong within one full interval — most
      // likely a silently-dropped connection (carrier NAT idle timeout).
      // No auto-reconnect in 7.2 (see class doc) — just surface it so the
      // UI can prompt the user.
      _heartbeatTimer?.cancel();
      _setStatus(const ConnectionFailure('Heartbeat timed out — connection may be dead'));
      return;
    }
    final nonce = _nextNonce++;
    _pendingPingNonce = nonce;
    _channel.sink.add(PingMessage(nonce).encode());
  }

  /// Ends the connection cleanly. The server does not acknowledge `Goodbye`
  /// — it just stops reading and drops the connection — so no reply is
  /// awaited here; the `onDone` handler above turns the resulting socket
  /// close into a clean `Disconnected` status because `_goodbyeSent` is set.
  Future<void> goodbye([String? reason]) async {
    _goodbyeSent = true;
    _heartbeatTimer?.cancel();
    _channel.sink.add(GoodbyeMessage(reason).encode());
    await _channel.sink.close();
  }

  void _setStatus(ConnectionStatus s) {
    _status = s;
    _statusController.add(s);
  }
}
