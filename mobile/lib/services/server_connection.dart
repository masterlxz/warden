import 'dart:async';

import 'package:meta/meta.dart';
import 'package:stream_channel/stream_channel.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import '../protocol/messages.dart';
import 'agent_work.dart';
import 'chat_transcript.dart';

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

class HistoryException implements Exception {
  HistoryException(this.message);

  final String message;

  @override
  String toString() => 'HistoryException: $message';
}

/// A conversation list/rename/delete (P78) failed, timed out, or lost its connection.
class ConversationException implements Exception {
  ConversationException(this.message);

  final String message;

  @override
  String toString() => message;
}

/// The hub refused a change to the organization or to a task (P120, P123). [authRejected]: the pairing key was wrong and
/// nothing changed.
class HubRequestException implements Exception {
  const HubRequestException(this.message, {this.authRejected = false});

  final String message;
  final bool authRejected;

  @override
  String toString() => message;
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
/// A local tool this client can run when the server asks (Fase 7.4) — `args` is whatever JSON
/// object the model passed as the tool call's arguments. Return the JSON-encodable result, or
/// throw (any exception) to send a `ToolCallErrorMessage` back instead.
typedef ToolHandler = Future<Object?> Function(Map<String, dynamic> args);

/// The hub's WebSocket URL — `wss://` for a TLS-only hub (P36), where `host` is the name its
/// certificate covers (e.g. `hub.tail1234.ts.net`), not an IP.
Uri hubUri(String host, int port, {required bool secure}) => Uri(scheme: secure ? 'wss' : 'ws', host: host, port: port);

/// Signature of `ServerConnection.connect`, so callers can be handed a different way to open a
/// connection (see `ConnectionScreen.connector`).
typedef ServerConnector = Future<ServerConnection> Function({
  required String host,
  required int port,
  bool secure,
  required String deviceId,
  required String deviceName,
  required String authKey,
  String? deviceToken,
  String? username,
  String? password,
  Duration handshakeTimeout,
  List<Map<String, dynamic>> toolSpecs,
  Map<String, ToolHandler> toolHandlers,
});

class ServerConnection implements ConversationBackend, AgentsBackend, MemberOrgBackend {
  ServerConnection._(this._channel, this._subscription, this.serverName, this.issuedDeviceToken, this.user, this._toolHandlers) {
    _setStatus(Connected(serverName));
    _startHeartbeat();
    _subscription
      ..onData((dynamic frame) => _onMessage(ServerMessage.decode(frame as String)))
      ..onError((Object err) {
        _heartbeatTimer?.cancel();
        _failPendingHistory(err.toString());
        _setStatus(ConnectionFailure(err.toString()));
      })
      ..onDone(() {
        _heartbeatTimer?.cancel();
        _failPendingHistory('Connection closed before the hub answered');
        final rejected = _rejectedReason;
        _setStatus(
          _goodbyeSent
              ? const Disconnected() // clean, user-initiated — not an error
              : rejected != null
                  ? ConnectionFailure('authentication rejected: $rejected')
                  : const ConnectionFailure('Connection closed unexpectedly'),
        );
      });
  }

  final StreamChannel<dynamic> _channel;
  final StreamSubscription<dynamic> _subscription;
  final String serverName;

  /// The device token the hub issued in this connection's `HelloAck` (P36), if it issued one —
  /// the caller must persist it and pass it as `deviceToken` from then on.
  final String? issuedDeviceToken;

  /// P84 — the member this device belongs to, as the hub said in `HelloAck`; null for the owner.
  ///
  /// Kept up to date with what this connection changes about them (a password change that turned
  /// encryption on, a TruthID linked).
  UserInfo? user;
  final Map<String, ToolHandler> _toolHandlers;

  final _statusController = StreamController<ConnectionStatus>.broadcast();
  ConnectionStatus _status = const Connecting();
  ConnectionStatus get status => _status;
  Stream<ConnectionStatus> get statusStream => _statusController.stream;

  // Carries ChatResponseMessage/ChatErrorMessage (Fase 7.3) and ConversationsChangedMessage
  // (P87) — everything else stays internal to the handshake/heartbeat machinery above.
  final _chatController = StreamController<ServerMessage>.broadcast();
  Stream<ServerMessage> get chatStream => _chatController.stream;

  // P87 — ApprovalRequestMessage/ApprovalCancelledMessage, for the chat screen's dialog.
  final _approvalController = StreamController<ServerMessage>.broadcast();
  Stream<ServerMessage> get approvalStream => _approvalController.stream;

  // P120 — what the owner just set as this member's access to the organization (`none`, `view`, `edit`), pushed by the hub.
  final _orgAccessController = StreamController<String>.broadcast();
  @override
  Stream<String> get orgAccessChanges => _orgAccessController.stream;

  // P40 — in-flight `fetchHistory` calls, keyed by the `requestId` the reply echoes back.
  final _pendingHistory = <int, Completer<List<HistoryEntry>>>{};
  // P78 — in-flight conversation list/rename/delete calls, same keying.
  final _pendingConversation = <int, Completer<ServerMessage>>{};
  // P84 fatia 4 — the recovery code the hub pushed on its own after the HelloAck, until a screen takes it.
  String? _unclaimedRecoveryCode;
  // Starts at 1: a `RecoveryCode` with request id 0 is the one the hub pushes on its own (P84 fatia 4).
  int _nextRequestId = 1;

  Timer? _heartbeatTimer;
  int _nextNonce = 0;
  int? _pendingPingNonce;
  bool _goodbyeSent = false;

  // P36 — set when the hub turns this connection away mid-session (the device was revoked), so
  // the close that follows reports why instead of "closed unexpectedly".
  String? _rejectedReason;

  static const defaultHandshakeTimeout = Duration(seconds: 10);

  // Mobile carrier NATs commonly drop idle TCP connections silently within
  // a few minutes; 30s keeps well under that without waking the radio too
  // often. The server never enforces or initiates a heartbeat itself — this
  // cadence exists purely so the client can notice a dead connection.
  static const heartbeatInterval = Duration(seconds: 30);

  static Future<ServerConnection> connect({
    required String host,
    required int port,
    bool secure = false,
    required String deviceId,
    required String deviceName,
    required String authKey,
    String? deviceToken,
    String? username,
    String? password,
    Duration handshakeTimeout = defaultHandshakeTimeout,
    List<Map<String, dynamic>> toolSpecs = const [],
    Map<String, ToolHandler> toolHandlers = const {},
  }) {
    final channel = WebSocketChannel.connect(hubUri(host, port, secure: secure));
    return _handshake(
      channel,
      deviceId: deviceId,
      deviceName: deviceName,
      authKey: authKey,
      deviceToken: deviceToken,
      username: username,
      password: password,
      handshakeTimeout: handshakeTimeout,
      toolSpecs: toolSpecs,
      toolHandlers: toolHandlers,
    );
  }

  @visibleForTesting
  static Future<ServerConnection> connectOverChannel({
    required StreamChannel<dynamic> channel,
    required String deviceId,
    required String deviceName,
    required String authKey,
    String? deviceToken,
    String? username,
    String? password,
    Duration handshakeTimeout = defaultHandshakeTimeout,
    List<Map<String, dynamic>> toolSpecs = const [],
    Map<String, ToolHandler> toolHandlers = const {},
  }) =>
      _handshake(
        channel,
        deviceId: deviceId,
        deviceName: deviceName,
        authKey: authKey,
        deviceToken: deviceToken,
        username: username,
        password: password,
        handshakeTimeout: handshakeTimeout,
        toolSpecs: toolSpecs,
        toolHandlers: toolHandlers,
      );

  static Future<ServerConnection> _handshake(
    StreamChannel<dynamic> channel, {
    required String deviceId,
    required String deviceName,
    required String authKey,
    required String? deviceToken,
    String? username,
    String? password,
    required Duration handshakeTimeout,
    required List<Map<String, dynamic>> toolSpecs,
    required Map<String, ToolHandler> toolHandlers,
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
      deviceToken: deviceToken,
      tools: toolSpecs,
      username: username,
      password: password,
    ).encode());

    try {
      final reply = await firstFrame.future;
      switch (reply) {
        case HelloAckMessage(:final serverName, deviceToken: final issued, :final user):
          return ServerConnection._(channel, subscription, serverName, issued, user, toolHandlers);
        case AuthErrorMessage(:final reason):
          await subscription.cancel();
          throw HandshakeException('authentication rejected: $reason');
        case PongMessage() ||
              GoodbyeServerMessage() ||
              ChatResponseMessage() ||
              ChatErrorMessage() ||
              ToolCallRequestMessage() ||
              HistoryServerMessage() ||
              HistoryErrorMessage() ||
              ConversationListMessage() ||
              ConversationOkMessage() ||
              AgentChannelMessage() ||
              DirListMessage() ||
              DirErrorMessage() ||
              SettingsMessage() ||
              SettingsSavedMessage() ||
              SettingsErrorMessage() ||
              AgentOrgListMessage() ||
              AgentTaskListMessage() ||
              ActivityListMessage() ||
              TaskErrorMessage() ||
              ApprovalRequestMessage() ||
              ApprovalCancelledMessage() ||
              ConversationsChangedMessage() ||
              OrgAccessChangedMessage() ||
              UnknownServerMessage() ||
              PasswordChangedMessage() ||
              RecoveryCodeMessage() ||
              RecoveryPolicyAcceptedMessage() ||
              RecoveryNoticesAckedMessage() ||
              TruthIdLinkedMessage() ||
              UserErrorMessage() ||
              ConversationErrorMessage():
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
      case ConversationsChangedMessage():
        _chatController.add(msg);
      case OrgAccessChangedMessage(:final access):
        _orgAccessController.add(access);
      case ApprovalRequestMessage():
      case ApprovalCancelledMessage():
        _approvalController.add(msg);
      case SettingsMessage(:final requestId) ||
            SettingsSavedMessage(:final requestId) ||
            SettingsErrorMessage(:final requestId) ||
            AgentOrgListMessage(:final requestId) ||
            AgentTaskListMessage(:final requestId) ||
            ActivityListMessage(:final requestId) ||
            TaskErrorMessage(:final requestId):
        _pendingConversation.remove(requestId)?.complete(msg);
      case PasswordChangedMessage(:final requestId) ||
            RecoveryPolicyAcceptedMessage(:final requestId) ||
            RecoveryNoticesAckedMessage(:final requestId) ||
            TruthIdLinkedMessage(:final requestId) ||
            UserErrorMessage(:final requestId):
        _pendingConversation.remove(requestId)?.complete(msg);
      case RecoveryCodeMessage(:final requestId, :final code):
        if (requestId == 0) {
          // Sent by the hub on its own right after the HelloAck, before any screen listens.
          _unclaimedRecoveryCode = code;
        } else {
          _pendingConversation.remove(requestId)?.complete(msg);
        }
      case UnknownServerMessage():
        break;
      case ToolCallRequestMessage(:final callId, :final tool, :final arguments):
        // Fire-and-forget: each call runs independently, so a slow one (e.g. reading a large
        // file) never blocks this connection's heartbeat/chat handling in the meantime.
        unawaited(_handleToolCallRequest(callId, tool, arguments));
      case HistoryServerMessage(:final requestId, :final messages):
        _pendingHistory.remove(requestId)?.complete(messages);
      case HistoryErrorMessage(:final requestId, :final message):
        _pendingHistory.remove(requestId)?.completeError(HistoryException(message));
      case ConversationListMessage(:final requestId) ||
            ConversationOkMessage(:final requestId) ||
            AgentChannelMessage(:final requestId) ||
            DirListMessage(:final requestId):
        _pendingConversation.remove(requestId)?.complete(msg);
      case DirErrorMessage(:final requestId, :final message):
        _pendingConversation.remove(requestId)?.completeError(ConversationException(message));
      case ConversationErrorMessage(:final requestId, :final message):
        _pendingConversation.remove(requestId)?.completeError(ConversationException(message));
      case AuthErrorMessage(:final reason):
        _rejectedReason = reason;
      case HelloAckMessage():
        // Only ever valid as the first frame, already consumed by _handshake.
        break;
    }
  }

  Future<void> _handleToolCallRequest(int callId, String tool, Map<String, dynamic> arguments) async {
    final handler = _toolHandlers[tool];
    if (handler == null) {
      _channel.sink.add(ToolCallErrorMessage(callId, "no local handler registered for tool '$tool'").encode());
      return;
    }
    try {
      final result = await handler(arguments);
      _channel.sink.add(ToolCallResultMessage(callId, result).encode());
    } catch (e) {
      _channel.sink.add(ToolCallErrorMessage(callId, e.toString()).encode());
    }
  }

  /// Sends one chat turn to [conversationId] (P78; null is the default conversation). The reply
  /// arrives asynchronously on [chatStream] as either a [ChatResponseMessage] or a
  /// [ChatErrorMessage], tagged with the same conversation id.
  @override
  void sendChat(String message, {String? conversationId, String? agentId, String? workdir, ThreadParent? threadOf}) {
    _channel.sink.add(ChatMessage(message, conversationId: conversationId, agentId: agentId, workdir: workdir, threadOf: threadOf).encode());
  }

  /// P102 — the folders inside [path] on the hub's machine (no path: where the person may start). Throws a
  /// [ConversationException] when the hub refuses (outside what the person may see, not a folder, unreadable).
  @override
  Future<DirListMessage> listDirs([String? path]) async {
    final reply = await _conversationRequest((requestId) => ListDirsMessage(requestId, path: path));
    if (reply is DirListMessage) return reply;
    throw ConversationException('Unexpected reply to the folder list: $reply');
  }

  /// P87 — the configured agents' ids, from the hub's settings (the web's selector reads the same).
  /// Throws a [ConversationException] when the hub can't answer.
  @override
  Future<List<String>> listAgentIds() async {
    final reply = await _conversationRequest(RequestSettingsMessage.new);
    return switch (reply) {
      SettingsMessage(:final agentIds) => agentIds,
      SettingsErrorMessage(:final message) => throw ConversationException(message),
      _ => const [],
    };
  }

  /// P120, P123 — the agents as the hub's settings hold them (roles, superiors, model limits) and the model policies.
  /// Throws a [ConversationException] when the hub can't answer.
  @override
  Future<HubAgents> listHubAgents() async {
    final reply = await _conversationRequest(RequestSettingsMessage.new);
    return switch (reply) {
      SettingsMessage(:final agents, :final modelPolicies, :final modelIds) => HubAgents(agents, modelPolicies, modelIds),
      SettingsErrorMessage(:final message) => throw ConversationException(message),
      _ => throw ConversationException('Unexpected reply to the settings request: $reply'),
    };
  }

  /// P120 — one change to the organization, with the pairing key. A change starts the hub's orchestrator again, so it
  /// waits longer than the other requests. Throws a [HubRequestException] (`authRejected` on a wrong key).
  @override
  Future<HubAgents> editAgentOrg(String pairingKey, OrgEdit edit) async {
    final reply = await _conversationRequest(
      (requestId) => EditAgentOrgMessage(requestId, pairingKey, edit),
      timeout: const Duration(seconds: 120),
    );
    return switch (reply) {
      SettingsSavedMessage(:final agents, :final modelPolicies, :final modelIds) => HubAgents(agents, modelPolicies, modelIds),
      SettingsErrorMessage(:final message, :final authRejected) => throw HubRequestException(message, authRejected: authRejected),
      _ => throw ConversationException('Unexpected reply to the organization change: $reply'),
    };
  }

  /// P120 — the tree of the organization as the hub shows it to a member: only the id, the role and the superior of each
  /// agent, and the access they have (`view` or `edit`). Throws a [HubRequestException] when the owner gave no access (the
  /// hub's refusal), and a [ConversationException] when the hub can't be asked (a timeout, a closed connection).
  @override
  Future<MemberOrg> listAgentOrg() async {
    final reply = await _conversationRequest(ListAgentOrgMessage.new);
    return _memberOrgOf(reply);
  }

  /// P120 — the same as [editAgentOrg], for a member with the `edit` access: their session is the authorization, so there
  /// is no pairing key. Only a role and a superior, a new report and a removal are theirs. Returns the tree as it is now.
  @override
  Future<MemberOrg> editAgentOrgAsMember(OrgEdit edit) async {
    final reply = await _conversationRequest(
      (requestId) => EditAgentOrgMessage.asMember(requestId, edit),
      timeout: const Duration(seconds: 120),
    );
    return _memberOrgOf(reply);
  }

  MemberOrg _memberOrgOf(ServerMessage reply) => switch (reply) {
        AgentOrgListMessage(:final agents, :final access) => MemberOrg(agents, access),
        SettingsErrorMessage(:final message, :final authRejected) => throw HubRequestException(message, authRejected: authRejected),
        _ => throw ConversationException('Unexpected reply to the organization request: $reply'),
      };

  /// P123 — the work agents delegated to each other, newest first.
  @override
  Future<List<AgentTask>> listAgentTasks() async {
    final reply = await _conversationRequest(ListAgentTasksMessage.new);
    return switch (reply) {
      AgentTaskListMessage(:final tasks) => tasks,
      TaskErrorMessage(:final message) => throw ConversationException(message),
      _ => throw ConversationException('Unexpected reply to the task list: $reply'),
    };
  }

  /// P121 — the feed of activity: what happened among the agents, newest first.
  @override
  Future<List<ActivityEvent>> listActivity() async {
    final reply = await _conversationRequest(ListActivityMessage.new);
    return switch (reply) {
      ActivityListMessage(:final events) => events,
      _ => throw ConversationException('Unexpected reply to the activity list: $reply'),
    };
  }

  /// P123 — pauses, resumes or stops ([action]: `pause`, `resume` or `cancel`) a task running on the hub, with the pairing
  /// key. Returns the updated list. Throws a [HubRequestException] (`authRejected` on a wrong key, or the task doesn't run there).
  @override
  Future<List<AgentTask>> controlAgentTask(String pairingKey, String taskId, String action) async {
    final reply = await _conversationRequest((requestId) => ControlAgentTaskMessage(requestId, pairingKey, taskId, action));
    return switch (reply) {
      AgentTaskListMessage(:final tasks) => tasks,
      TaskErrorMessage(:final message, :final authRejected) => throw HubRequestException(message, authRejected: authRejected),
      _ => throw ConversationException('Unexpected reply to the task control: $reply'),
    };
  }

  /// P84 — the member on this connection picks their own password. Throws a [PasswordException]
  /// (with `wrongPassword` when the current one didn't match) if the hub refuses.
  /// Returns the recovery code when this change turned encryption on for their data (shown once). After the
  /// owner reset the password of someone whose data is encrypted, [recoveryCode] is what opens it again.
  Future<String?> changePassword(String oldPassword, String newPassword, {String? recoveryCode}) async {
    final reply = await _conversationRequest((requestId) => ChangePasswordMessage(requestId, oldPassword, newPassword, recoveryCode: recoveryCode));
    return switch (reply) {
      UserErrorMessage(:final message, :final authRejected) => throw PasswordException(message, wrongPassword: authRejected),
      PasswordChangedMessage(:final recoveryCode) => _afterPasswordChange(recoveryCode),
      _ => null,
    };
  }

  String? _afterPasswordChange(String? recoveryCode) {
    user = user?.withOwnPassword(encrypted: (user?.encrypted ?? false) || recoveryCode != null);
    return recoveryCode;
  }

  /// P84 fatia 4 — the recovery code the hub sent right after the sign-in (it turned encryption on for a
  /// member from before). Taken once: null if there's none or it was already shown.
  String? takeUnclaimedRecoveryCode() {
    final code = _unclaimedRecoveryCode;
    _unclaimedRecoveryCode = null;
    return code;
  }

  /// A new recovery code, with the password; the old one stops working. Throws a [PasswordException].
  Future<String> regenerateRecoveryCode(String password) async {
    final reply = await _conversationRequest((requestId) => RegenerateRecoveryCodeMessage(requestId, password));
    return switch (reply) {
      RecoveryCodeMessage(:final code) => code,
      UserErrorMessage(:final message, :final authRejected) => throw PasswordException(message, wrongPassword: authRejected),
      _ => throw PasswordException('unexpected answer from the hub'),
    };
  }

  /// Yes to a weaker recovery policy. Returns the new recovery code when entering or leaving `consent` made
  /// one (shown once). Throws a [PasswordException].
  Future<String?> acceptRecoveryPolicy(String password) async {
    final reply = await _conversationRequest((requestId) => AcceptRecoveryPolicyMessage(requestId, password));
    return switch (reply) {
      UserErrorMessage(:final message, :final authRejected) => throw PasswordException(message, wrongPassword: authRejected),
      RecoveryPolicyAcceptedMessage(:final recoveryCode) => recoveryCode,
      _ => null,
    };
  }

  /// The member has seen the recoveries the owner made.
  Future<void> ackRecoveryNotices() async {
    final reply = await _conversationRequest(AckRecoveryNoticesMessage.new);
    if (reply case UserErrorMessage(:final message)) throw PasswordException(message);
  }

  String _afterLink(String username) {
    user = user?.withTruthId(username);
    return username;
  }

  /// P84 fatia 5 — links the member's TruthID with the owner's invite. Returns the username the registry has.
  Future<String> redeemInvite(String code, String username) async {
    final reply = await _conversationRequest((requestId) => RedeemInviteMessage(requestId, code, username));
    return switch (reply) {
      TruthIdLinkedMessage(:final username) => _afterLink(username),
      UserErrorMessage(:final message) => throw PasswordException(message),
      _ => throw PasswordException('unexpected answer from the hub'),
    };
  }

  /// P87 — the person's answer to an [ApprovalRequestMessage].
  void resolveApproval(int approvalId, bool approved) {
    _channel.sink.add(ResolveApprovalMessage(approvalId, approved).encode());
  }

  /// P40 — fetches one of this device's conversations as persisted by the server (null is the
  /// default one), oldest first, keeping only the last [limit] messages. Throws a
  /// [HistoryException] if the server couldn't read it, didn't answer within [timeout], or the
  /// connection dropped before the reply arrived.
  @override
  Future<List<HistoryEntry>> fetchHistory({int? limit, String? conversationId, Duration timeout = const Duration(seconds: 15)}) {
    final requestId = _nextRequestId++;
    final completer = Completer<List<HistoryEntry>>();
    _pendingHistory[requestId] = completer;
    _channel.sink.add(RequestHistoryMessage(requestId, limit: limit, conversationId: conversationId).encode());
    return completer.future.timeout(timeout, onTimeout: () {
      _pendingHistory.remove(requestId);
      throw HistoryException('No history reply within ${timeout.inSeconds}s');
    });
  }

  /// P78 — this device's conversations, newest-updated first.
  @override
  Future<List<ConversationSummary>> listConversations() async {
    final reply = await _conversationRequest(ListConversationsMessage.new);
    return reply is ConversationListMessage ? reply.conversations : const [];
  }

  /// P121 — the id of the conversation that is [agentId]'s channel with this person: the hub makes it, always the same, so
  /// there is one per agent. It exists on the hub from the first message sent to it.
  @override
  Future<String> openAgentChannel(String agentId) async {
    final reply = await _conversationRequest((requestId) => OpenAgentChannelMessage(requestId, agentId));
    if (reply is AgentChannelMessage) return reply.conversationId;
    throw ConversationException('Unexpected reply to the agent channel: $reply');
  }

  @override
  Future<void> renameConversation(String conversationId, String title) =>
      _conversationRequest((requestId) => RenameConversationMessage(requestId, conversationId, title));

  @override
  Future<void> deleteConversation(String conversationId) =>
      _conversationRequest((requestId) => DeleteConversationMessage(requestId, conversationId));

  Future<ServerMessage> _conversationRequest(
    ClientMessage Function(int requestId) build, {
    Duration timeout = const Duration(seconds: 15),
  }) {
    final requestId = _nextRequestId++;
    final completer = Completer<ServerMessage>();
    _pendingConversation[requestId] = completer;
    _channel.sink.add(build(requestId).encode());
    return completer.future.timeout(timeout, onTimeout: () {
      _pendingConversation.remove(requestId);
      throw ConversationException('The hub did not answer within ${timeout.inSeconds}s');
    });
  }

  void _failPendingHistory(String reason) {
    for (final completer in _pendingHistory.values) {
      completer.completeError(HistoryException(reason));
    }
    _pendingHistory.clear();
    for (final completer in _pendingConversation.values) {
      completer.completeError(ConversationException(reason));
    }
    _pendingConversation.clear();
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

/// P84 — the hub refused a password change. [wrongPassword]: the current password didn't match.
class PasswordException implements Exception {
  const PasswordException(this.message, {this.wrongPassword = false});

  final String message;
  final bool wrongPassword;

  @override
  String toString() => message;
}
