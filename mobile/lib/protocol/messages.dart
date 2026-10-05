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
    this.deviceToken,
    this.tools = const [],
    this.username,
    this.password,
    this.recoveryCodes = true,
  });

  final String deviceId;
  final String deviceName;

  /// The hub's pairing key (P36) — only needed until this device holds a [deviceToken] for it.
  final String authKey;

  /// The per-device token this hub issued in an earlier `HelloAck` (P36), if any.
  final String? deviceToken;

  /// Local tools this client can run on request (Fase 7.4), e.g.
  /// `MobileFileTool.listFilesSpec`/`readFileSpec` — each a `{name, description, parameters}`
  /// map mirroring `warden_core::tool::ToolSpec`. Empty when nothing is configured (e.g. no
  /// root folder picked yet) — `warden-server` only builds the remote-tool-dispatch machinery
  /// when this is non-empty.
  final List<Map<String, dynamic>> tools;

  /// P84 — pairs as this member (with [password]) instead of with the pairing key. Like the key,
  /// only needed until the hub issues a token.
  final String? username;
  final String? password;

  /// P84 fatia 4 — this app can show a recovery code, so the hub may turn a member's encryption on
  /// (a code nobody sees is a key nobody has). Always true: the screens for it are here.
  final bool recoveryCodes;

  @override
  Map<String, dynamic> toJson() => {
        'type': 'hello',
        'deviceId': deviceId,
        'deviceName': deviceName,
        'authKey': authKey,
        if (deviceToken != null) 'deviceToken': deviceToken,
        'tools': tools,
        if (username != null) 'username': username,
        if (username != null) 'password': password ?? '',
        if (recoveryCodes) 'recoveryCodes': true,
      };
}

/// P84 — the member on this connection picks their own password. Answered by
/// [PasswordChangedMessage] or [UserErrorMessage].
final class ChangePasswordMessage extends ClientMessage {
  const ChangePasswordMessage(this.requestId, this.oldPassword, this.newPassword, {this.recoveryCode});

  final int requestId;
  final String oldPassword;
  final String newPassword;

  /// P84 fatia 4 — after the owner reset the password of someone whose data is encrypted, only the
  /// recovery code opens it again.
  final String? recoveryCode;

  @override
  Map<String, dynamic> toJson() => {
        'type': 'changePassword',
        'requestId': requestId,
        'oldPassword': oldPassword,
        'newPassword': newPassword,
        if (recoveryCode != null) 'recoveryCode': recoveryCode,
      };
}

/// P84 fatia 4 — a new recovery code (the old one stops working), with the password. Answered by
/// [RecoveryCodeMessage] or [UserErrorMessage].
final class RegenerateRecoveryCodeMessage extends ClientMessage {
  const RegenerateRecoveryCodeMessage(this.requestId, this.password);

  final int requestId;
  final String password;

  @override
  Map<String, dynamic> toJson() => {'type': 'regenerateRecoveryCode', 'requestId': requestId, 'password': password};
}

/// P84 fatia 4 parte B — yes to the workspace's recovery policy after a change to a weaker one, with
/// the password. Answered by [RecoveryPolicyAcceptedMessage] or [UserErrorMessage].
final class AcceptRecoveryPolicyMessage extends ClientMessage {
  const AcceptRecoveryPolicyMessage(this.requestId, this.password);

  final int requestId;
  final String password;

  @override
  Map<String, dynamic> toJson() => {'type': 'acceptRecoveryPolicy', 'requestId': requestId, 'password': password};
}

/// P84 fatia 4 parte B — the member has seen the recoveries the owner made. Answered by
/// [RecoveryNoticesAckedMessage].
final class AckRecoveryNoticesMessage extends ClientMessage {
  const AckRecoveryNoticesMessage(this.requestId);

  final int requestId;

  @override
  Map<String, dynamic> toJson() => {'type': 'ackRecoveryNotices', 'requestId': requestId};
}

/// P84 fatia 5 — links the member's TruthID with the owner's invite [code]. Answered by
/// [TruthIdLinkedMessage] or [UserErrorMessage].
final class RedeemInviteMessage extends ClientMessage {
  const RedeemInviteMessage(this.requestId, this.code, this.username);

  final int requestId;
  final String code;
  final String username;

  @override
  Map<String, dynamic> toJson() => {'type': 'redeemInvite', 'requestId': requestId, 'code': code, 'username': username};
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

/// A chat turn (Fase 7.3) — answered by the `Orchestrator` `warden-server` hosts, appended to one of
/// this device's conversations. [conversationId] picks which (P78): an id the hub has never seen
/// starts a new one; null is the device's default conversation. [agentId] (P46/P87) speaks as that
/// configured agent: its persona, skills and tools. [workdir] (P102) is the folder of the hub's machine
/// the new conversation works in — only read when the conversation is created, so it travels with its
/// first message.
final class ChatMessage extends ClientMessage {
  const ChatMessage(this.message, {this.conversationId, this.agentId, this.workdir});

  final String message;
  final String? conversationId;
  final String? agentId;
  final String? workdir;

  @override
  Map<String, dynamic> toJson() => {
        'type': 'chat',
        'message': message,
        if (conversationId != null) 'conversationId': conversationId,
        if (agentId != null) 'agentId': agentId,
        if (workdir != null) 'workdir': workdir,
      };
}

/// P102 — asks for the folders inside [path] on the hub's machine; no path starts where the person may
/// (a member sees only the folders the owner allowed). Answered by [DirListMessage] or [DirErrorMessage]
/// with the same `requestId`.
final class ListDirsMessage extends ClientMessage {
  const ListDirsMessage(this.requestId, {this.path});

  final int requestId;
  final String? path;

  @override
  Map<String, dynamic> toJson() => {'type': 'listDirs', 'requestId': requestId, if (path != null) 'path': path};
}

/// Asks for the hub's settings (P78) — here only for the configured agents' ids (P87), the same
/// way the web's agent selector gets them. Answered by [SettingsMessage] or [SettingsErrorMessage].
final class RequestSettingsMessage extends ClientMessage {
  const RequestSettingsMessage(this.requestId);

  final int requestId;

  @override
  Map<String, dynamic> toJson() => {'type': 'requestSettings', 'requestId': requestId};
}

/// The person's answer to an [ApprovalRequestMessage] (P87).
final class ResolveApprovalMessage extends ClientMessage {
  const ResolveApprovalMessage(this.approvalId, this.approved);

  final int approvalId;
  final bool approved;

  @override
  Map<String, dynamic> toJson() => {'type': 'resolveApproval', 'approvalId': approvalId, 'approved': approved};
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

/// Asks for one of this device's persisted conversations (P40) — answered by a
/// [HistoryServerMessage] or [HistoryErrorMessage] carrying the same `requestId`. `limit` keeps
/// only the most recent messages; null asks for all of them. A null [conversationId] is the
/// default conversation, same as [ChatMessage].
final class RequestHistoryMessage extends ClientMessage {
  const RequestHistoryMessage(this.requestId, {this.limit, this.conversationId});

  final int requestId;
  final int? limit;
  final String? conversationId;

  @override
  Map<String, dynamic> toJson() => {
        'type': 'requestHistory',
        'requestId': requestId,
        'limit': limit,
        if (conversationId != null) 'conversationId': conversationId,
      };
}

/// P78 — lists this device's conversations; answered by [ConversationListMessage] or
/// [ConversationErrorMessage] with the same `requestId`.
final class ListConversationsMessage extends ClientMessage {
  const ListConversationsMessage(this.requestId);

  final int requestId;

  @override
  Map<String, dynamic> toJson() => {'type': 'listConversations', 'requestId': requestId};
}

/// P78 — answered by [ConversationOkMessage] or [ConversationErrorMessage].
final class RenameConversationMessage extends ClientMessage {
  const RenameConversationMessage(this.requestId, this.conversationId, this.title);

  final int requestId;
  final String conversationId;
  final String title;

  @override
  Map<String, dynamic> toJson() =>
      {'type': 'renameConversation', 'requestId': requestId, 'conversationId': conversationId, 'title': title};
}

/// P78 — answered by [ConversationOkMessage] or [ConversationErrorMessage].
final class DeleteConversationMessage extends ClientMessage {
  const DeleteConversationMessage(this.requestId, this.conversationId);

  final int requestId;
  final String conversationId;

  @override
  Map<String, dynamic> toJson() => {'type': 'deleteConversation', 'requestId': requestId, 'conversationId': conversationId};
}

/// One of this device's conversations in a [ConversationListMessage] (P78). Mirrors
/// `warden_server_protocol::protocol::ConversationSummary`.
class ConversationSummary {
  const ConversationSummary({required this.id, required this.title, required this.createdAt, required this.updatedAt, this.agentId, this.workdir});

  final String id;
  final String title;
  final int createdAt;
  final int updatedAt;

  /// The agent this conversation last spoke with (P46/P87), restored when it's opened.
  final String? agentId;

  /// The folder of the hub's machine this conversation works in (P102), fixed when it began.
  final String? workdir;

  static ConversationSummary fromJson(dynamic json) {
    final map = json as Map<String, dynamic>;
    return ConversationSummary(
      id: map['id'] as String,
      title: map['title'] as String,
      createdAt: map['createdAt'] as int,
      updatedAt: map['updatedAt'] as int,
      agentId: map['agentId'] as String?,
      workdir: map['workdir'] as String?,
    );
  }
}

/// One folder in a [DirListMessage] (P102): its name and the path to open or pick.
class DirEntry {
  const DirEntry({required this.name, required this.path});

  final String name;
  final String path;

  static DirEntry fromJson(dynamic json) {
    final map = json as Map<String, dynamic>;
    return DirEntry(name: map['name'] as String, path: map['path'] as String);
  }
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

/// Media extracted from an MCP tool result during a turn (P64 frente 2 fatia 3). Mirrors
/// `warden_core::model::Attachment` — `data` is base64 with no `data:...;base64,` prefix.
class Attachment {
  const Attachment({required this.mimeType, required this.data});

  final String mimeType;
  final String data;

  static Attachment fromJson(dynamic json) {
    final map = json as Map<String, dynamic>;
    return Attachment(mimeType: map['mimeType'] as String, data: map['data'] as String);
  }
}

/// One persisted message in a [HistoryServerMessage] (P40). Mirrors
/// `warden_server_protocol::protocol::HistoryMessage`.
class HistoryEntry {
  const HistoryEntry({required this.fromUser, required this.content, this.attachments = const []});

  /// `role` on the wire is only ever `user` or `assistant`.
  final bool fromUser;
  final String content;
  final List<Attachment> attachments;

  static HistoryEntry fromJson(dynamic json) {
    final map = json as Map<String, dynamic>;
    return HistoryEntry(
      fromUser: map['role'] == 'user',
      content: map['content'] as String,
      attachments: (map['attachments'] as List<dynamic>?)?.map(Attachment.fromJson).toList() ?? const [],
    );
  }
}

/// Messages sent from a warden-server to this client.
sealed class ServerMessage {
  const ServerMessage();

  static ServerMessage fromJson(Map<String, dynamic> json) {
    return switch (json['type']) {
      'helloAck' => HelloAckMessage(
          json['serverName'] as String,
          deviceToken: json['deviceToken'] as String?,
          user: json['user'] == null ? null : UserInfo.fromJson(json['user'] as Map<String, dynamic>),
        ),
      'passwordChanged' => PasswordChangedMessage(json['requestId'] as int, recoveryCode: json['recoveryCode'] as String?),
      'recoveryCode' => RecoveryCodeMessage(json['requestId'] as int, json['code'] as String),
      'recoveryPolicyAccepted' => RecoveryPolicyAcceptedMessage(json['requestId'] as int, recoveryCode: json['recoveryCode'] as String?),
      'recoveryNoticesAcked' => RecoveryNoticesAckedMessage(json['requestId'] as int),
      'truthIdLinked' => TruthIdLinkedMessage(json['requestId'] as int, json['username'] as String),
      'userError' => UserErrorMessage(json['requestId'] as int, json['message'] as String, authRejected: json['authRejected'] as bool? ?? false),
      'authError' => AuthErrorMessage(json['reason'] as String),
      'pong' => PongMessage(json['nonce'] as int),
      'chatResponse' => ChatResponseMessage(
          json['content'] as String,
          Usage.fromJson(json['usage'] as Map<String, dynamic>?),
          attachments: (json['attachments'] as List<dynamic>?)?.map(Attachment.fromJson).toList() ?? const [],
          conversationId: json['conversationId'] as String?,
        ),
      'chatError' => ChatErrorMessage(json['message'] as String, conversationId: json['conversationId'] as String?),
      'toolCallRequest' => ToolCallRequestMessage(
          json['callId'] as int,
          json['tool'] as String,
          json['arguments'] as Map<String, dynamic>,
        ),
      'history' => HistoryServerMessage(
          json['requestId'] as int,
          (json['messages'] as List<dynamic>).map(HistoryEntry.fromJson).toList(),
        ),
      'historyError' => HistoryErrorMessage(json['requestId'] as int, json['message'] as String),
      'conversationList' => ConversationListMessage(
          json['requestId'] as int,
          (json['conversations'] as List<dynamic>).map(ConversationSummary.fromJson).toList(),
        ),
      'dirList' => DirListMessage(
          json['requestId'] as int,
          path: json['path'] as String,
          parent: json['parent'] as String?,
          dirs: (json['dirs'] as List<dynamic>).map(DirEntry.fromJson).toList(),
        ),
      'dirError' => DirErrorMessage(json['requestId'] as int, json['message'] as String),
      'conversationOk' => ConversationOkMessage(json['requestId'] as int),
      'conversationError' => ConversationErrorMessage(json['requestId'] as int, json['message'] as String),
      'goodbye' => GoodbyeServerMessage(json['reason'] as String?),
      'settings' => SettingsMessage(
          json['requestId'] as int,
          [
            for (final agent in ((json['settings'] as Map<String, dynamic>)['agents'] as List<dynamic>? ?? const []))
              (agent as Map<String, dynamic>)['id'] as String,
          ],
        ),
      'settingsError' => SettingsErrorMessage(json['requestId'] as int, json['message'] as String),
      'approvalRequest' => ApprovalRequestMessage(
          json['approvalId'] as int,
          target: json['target'] as String,
          action: json['action'] as String,
          detail: json['detail'] as String,
          category: json['category'] as String?,
        ),
      'approvalCancelled' => ApprovalCancelledMessage(json['approvalId'] as int),
      'conversationsChanged' => ConversationsChangedMessage(json['conversationId'] as String),
      // A message this app doesn't know yet (a newer hub) is skipped, not a broken connection.
      final String other => UnknownServerMessage(other),
      _ => throw const FormatException('ServerMessage without a type'),
    };
  }

  static ServerMessage decode(String text) =>
      fromJson(jsonDecode(text) as Map<String, dynamic>);
}

final class HelloAckMessage extends ServerMessage {
  const HelloAckMessage(this.serverName, {this.deviceToken, this.user});

  final String serverName;

  /// A newly issued device token (P36) — present when this Hello paired with the pairing key.
  /// Replaces whatever token this device held for the hub, which no longer works.
  final String? deviceToken;

  /// P84 — the member this device belongs to; null for the workspace's owner.
  final UserInfo? user;
}

/// Mirrors `UserInfoDto` (P84): a member of the workspace, never their password.
final class UserInfo {
  const UserInfo({
    required this.id,
    required this.name,
    required this.mustChangePassword,
    this.encrypted = false,
    this.needsRecovery = false,
    this.locked = false,
    this.memberPolicy = '',
    this.policyPending = false,
    this.recoveryPolicy = '',
    this.recoveries = const [],
    this.truthid = '',
  });

  /// The username.
  final String id;
  final String name;

  /// Still on the provisional password the owner gave them: the hub only lets them change it.
  final bool mustChangePassword;

  /// P84 fatia 4 — their data is encrypted on the hub with a key only they (or their recovery code) open.
  final bool encrypted;

  /// The owner reset their password, so the data opens only with the recovery code.
  final bool needsRecovery;

  /// The hub doesn't hold their key (it restarted): the data is shut until they sign in with the password.
  final bool locked;

  /// Who besides them may open their data right now — `private`, `consent` or `company` — empty while it
  /// isn't encrypted (parte B).
  final String memberPolicy;

  /// The workspace's policy changed to a weaker one they haven't accepted yet.
  final bool policyPending;

  /// The workspace's recovery policy.
  final String recoveryPolicy;

  /// Every time the owner recovered their data with the workspace's recovery key.
  final List<RecoveryEvent> recoveries;

  /// P84 fatia 5 — the TruthID username they linked; empty if none.
  final String truthid;

  UserInfo withTruthId(String username) => UserInfo(
        id: id,
        name: name,
        mustChangePassword: mustChangePassword,
        encrypted: encrypted,
        needsRecovery: needsRecovery,
        locked: locked,
        memberPolicy: memberPolicy,
        policyPending: policyPending,
        recoveryPolicy: recoveryPolicy,
        recoveries: recoveries,
        truthid: username,
      );

  /// The recoveries they haven't been told about yet.
  List<RecoveryEvent> get unseenRecoveries => [for (final event in recoveries) if (!event.seen) event];

  static UserInfo fromJson(Map<String, dynamic> json) => UserInfo(
        id: json['id'] as String,
        name: json['name'] as String,
        mustChangePassword: json['mustChangePassword'] as bool? ?? false,
        encrypted: json['encrypted'] as bool? ?? false,
        needsRecovery: json['needsRecovery'] as bool? ?? false,
        locked: json['locked'] as bool? ?? false,
        memberPolicy: json['memberPolicy'] as String? ?? '',
        policyPending: json['policyPending'] as bool? ?? false,
        recoveryPolicy: json['recoveryPolicy'] as String? ?? '',
        recoveries: [for (final e in (json['recoveries'] as List<dynamic>? ?? const [])) RecoveryEvent.fromJson(e as Map<String, dynamic>)],
        truthid: json['truthid'] as String? ?? '',
      );

  /// After a password change: the same person, no longer on the provisional password and (if the owner
  /// had reset it) with their data opened again.
  UserInfo withOwnPassword({bool? encrypted}) => UserInfo(
        id: id,
        name: name,
        mustChangePassword: false,
        encrypted: encrypted ?? this.encrypted,
        memberPolicy: memberPolicy,
        policyPending: policyPending,
        recoveryPolicy: recoveryPolicy,
        recoveries: recoveries,
        truthid: truthid,
      );
}

/// Mirrors `RecoveryEventDto` (P84 fatia 4 parte B): the owner recovered someone's data.
final class RecoveryEvent {
  const RecoveryEvent({required this.atMs, required this.kind, required this.seen});

  /// Milliseconds since the epoch.
  final int atMs;

  /// The policy it was done under: `consent` or `company`.
  final String kind;

  /// The person has seen it.
  final bool seen;

  static RecoveryEvent fromJson(Map<String, dynamic> json) => RecoveryEvent(
        atMs: json['atMs'] as int,
        kind: json['kind'] as String,
        seen: json['seen'] as bool? ?? false,
      );
}

/// P84 — the password change went through.
final class PasswordChangedMessage extends ServerMessage {
  const PasswordChangedMessage(this.requestId, {this.recoveryCode});

  final int requestId;

  /// P84 fatia 4 — this change turned encryption on for their data: shown once, they have to write it down.
  final String? recoveryCode;
}

/// A recovery code, shown once: the answer to [RegenerateRecoveryCodeMessage], or ([requestId] 0) sent right
/// after the `HelloAck` when signing in turned encryption on for a member from before.
final class RecoveryCodeMessage extends ServerMessage {
  const RecoveryCodeMessage(this.requestId, this.code);

  final int requestId;
  final String code;
}

/// P84 fatia 4 parte B — the member accepted the policy. [recoveryCode]: entering or leaving `consent`
/// made a new one, shown once.
final class RecoveryPolicyAcceptedMessage extends ServerMessage {
  const RecoveryPolicyAcceptedMessage(this.requestId, {this.recoveryCode});

  final int requestId;
  final String? recoveryCode;
}

final class RecoveryNoticesAckedMessage extends ServerMessage {
  const RecoveryNoticesAckedMessage(this.requestId);

  final int requestId;
}

/// P84 fatia 5 — the member's TruthID is linked.
final class TruthIdLinkedMessage extends ServerMessage {
  const TruthIdLinkedMessage(this.requestId, this.username);

  final int requestId;
  final String username;
}

/// P84 — a people request failed. [authRejected]: the current password was wrong.
final class UserErrorMessage extends ServerMessage {
  const UserErrorMessage(this.requestId, this.message, {this.authRejected = false});

  final int requestId;
  final String message;
  final bool authRejected;
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
  const ChatResponseMessage(this.content, this.usage, {this.attachments = const [], this.conversationId});

  final String content;
  final Usage? usage;
  final List<Attachment> attachments;

  /// Which conversation this answers (P78) — `chat` carries no request id.
  final String? conversationId;
}

final class ChatErrorMessage extends ServerMessage {
  const ChatErrorMessage(this.message, {this.conversationId});

  final String message;

  /// Same as [ChatResponseMessage.conversationId].
  final String? conversationId;
}

/// Asks this client to run one of the tools it advertised in `Hello.tools` (Fase 7.4).
final class ToolCallRequestMessage extends ServerMessage {
  const ToolCallRequestMessage(this.callId, this.tool, this.arguments);

  final int callId;
  final String tool;
  final Map<String, dynamic> arguments;
}

/// Reply to a [RequestHistoryMessage] (P40), oldest message first.
final class HistoryServerMessage extends ServerMessage {
  const HistoryServerMessage(this.requestId, this.messages);

  final int requestId;
  final List<HistoryEntry> messages;
}

/// The server couldn't read this device's conversation (P40).
final class HistoryErrorMessage extends ServerMessage {
  const HistoryErrorMessage(this.requestId, this.message);

  final int requestId;
  final String message;
}

/// Reply to a [ListConversationsMessage] (P78), newest-updated first.
final class ConversationListMessage extends ServerMessage {
  const ConversationListMessage(this.requestId, this.conversations);

  final int requestId;
  final List<ConversationSummary> conversations;
}

/// Reply to [ListDirsMessage] (P102): the folders in [path] (empty for a member's list of allowed folders), and the
/// one above it ([parent] is null at the top of what the person may see).
final class DirListMessage extends ServerMessage {
  const DirListMessage(this.requestId, {required this.path, this.parent, required this.dirs});

  final int requestId;
  final String path;
  final String? parent;
  final List<DirEntry> dirs;
}

/// A [ListDirsMessage] failed (P102): not a folder, outside what the person may see, unreadable.
final class DirErrorMessage extends ServerMessage {
  const DirErrorMessage(this.requestId, this.message);

  final int requestId;
  final String message;
}

/// Reply to a successful rename/delete (P78).
final class ConversationOkMessage extends ServerMessage {
  const ConversationOkMessage(this.requestId);

  final int requestId;
}

/// A list/rename/delete failed (P78) — the raw error text.
final class ConversationErrorMessage extends ServerMessage {
  const ConversationErrorMessage(this.requestId, this.message);

  final int requestId;
  final String message;
}

final class GoodbyeServerMessage extends ServerMessage {
  const GoodbyeServerMessage(this.reason);

  final String? reason;
}

/// Reply to [RequestSettingsMessage], reduced to what this app uses: the agents' ids (P87).
final class SettingsMessage extends ServerMessage {
  const SettingsMessage(this.requestId, this.agentIds);

  final int requestId;
  final List<String> agentIds;
}

final class SettingsErrorMessage extends ServerMessage {
  const SettingsErrorMessage(this.requestId, this.message);

  final int requestId;
  final String message;
}

/// A tool in this device's turn needs the person's yes (P46/P87: an agent creating or editing
/// another, an SSH host with approval, a spending-limit pause). No answer before the hub's
/// deadline (120 s) counts as no.
final class ApprovalRequestMessage extends ServerMessage {
  const ApprovalRequestMessage(this.approvalId, {required this.target, required this.action, required this.detail, this.category});

  final int approvalId;
  final String target;
  final String action;
  final String detail;

  /// P122: the kind of action the agent has to get approved (`critical_infra`...), when the ask comes from that rule;
  /// null from a hub that doesn't send it.
  final String? category;
}

/// The hub stopped waiting for [approvalId]: its dialog closes.
final class ApprovalCancelledMessage extends ServerMessage {
  const ApprovalCancelledMessage(this.approvalId);

  final int approvalId;
}

/// One of this device's conversations changed outside a chat reply — an agent left a note for
/// another, or answered one (P46 `message_agent`).
final class ConversationsChangedMessage extends ServerMessage {
  const ConversationsChangedMessage(this.conversationId);

  final String conversationId;
}

/// A message type this app doesn't handle.
final class UnknownServerMessage extends ServerMessage {
  const UnknownServerMessage(this.type);

  final String type;
}
