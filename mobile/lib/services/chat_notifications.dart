import 'package:flutter/widgets.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';

import '../protocol/messages.dart';

/// Fase 7.5 — local notification when a chat reply arrives while the app isn't in the foreground.
/// Deliberately NOT true push (no FCM/APNs, no server-side involvement): the app process must
/// still be alive with its WebSocket connection open, since `ServerConnection` is what actually
/// receives the reply. Chosen over FCM to avoid the project's first third-party cloud dependency
/// (a Firebase project/credentials) — see `ARCHITECTURE.md`.
///
/// One notification, not one per message: reusing this fixed id means a new reply replaces the
/// previous one in the tray instead of stacking up while the user is away.
const _notificationId = 1;

/// P87 — an approval gets its own notification, so a reply arriving after it doesn't replace it.
const _approvalNotificationId = 2;
const _channelId = 'chat_messages';
const _channelName = 'Chat messages';
const _channelDescription = "Notifies you when Warden replies while the app isn't in the foreground";

/// P121 — what agents say on their own (`message_user`): a channel of its own, and ids from here up, one per agent.
const _agentNotificationBase = 1000;
const _agentChannelId = 'agent_messages';
const _agentChannelName = 'Agent messages';
const _agentChannelDescription = 'Notifies you when an agent writes to you in its channel';

const _bodyMaxChars = 200;

final _plugin = FlutterLocalNotificationsPlugin();

/// The app is only ever showing the chat screen when it's actually in the foreground — there's no
/// other in-app screen reachable while connected (see `PENDING.md` P41), so `resumed` alone is
/// enough to know "the user is looking at this reply already, no need to notify".
bool shouldNotifyFor(AppLifecycleState state) => state != AppLifecycleState.resumed;

/// Title/body for a chat notification. Pulled out as a pure function so it's testable without a
/// platform channel — mirrors `warden-cli`'s `wrap_spans`/`card_width` split (pure formatting
/// logic tested directly, the actual terminal/notification write stays a thin, untested wrapper).
({String title, String body}) notificationContentFor(ServerMessage message, {required String serverName}) {
  final (title, rawBody) = switch (message) {
    ChatResponseMessage(:final content) => (serverName, content),
    ChatErrorMessage(:final message) => ('$serverName — Error', message),
    ApprovalRequestMessage(:final action, :final target, :final detail, :final category) => (
        '$serverName — Approval needed',
        '$action: $target${category == null ? '' : ' [$category]'}${detail.isEmpty ? '' : ' — $detail'}'
      ),
    _ => (serverName, ''),
  };
  return (title: title, body: _truncate(rawBody));
}

String _truncate(String text) {
  final trimmed = text.trim();
  if (trimmed.length <= _bodyMaxChars) return trimmed;
  return '${trimmed.substring(0, _bodyMaxChars)}…';
}

/// Registers the Android notification channel. Called once from `main.dart`, same spot as
/// `RustLib.init()` — every other channel-touching call assumes this already ran.
Future<void> initializeChatNotifications() async {
  await _plugin.initialize(
    settings: const InitializationSettings(
      android: AndroidInitializationSettings('@mipmap/ic_launcher'),
    ),
  );
}

/// Requests the Android 13+ runtime notification permission. Fire-and-forget by design — if
/// denied, notifications just silently don't show, no error UI (v1 scope). A no-op on older
/// Android versions where the permission doesn't exist.
/// Best-effort: the caller fires this without awaiting (`ChatScreen.initState`), so a failure here
/// would otherwise surface as an unhandled async error — e.g. the plugin having no platform
/// implementation registered, as in widget tests. Missing permission just means no notifications.
Future<void> requestNotificationPermission() async {
  try {
    await _plugin
        .resolvePlatformSpecificImplementation<AndroidFlutterLocalNotificationsPlugin>()
        ?.requestNotificationsPermission();
  } catch (e) {
    debugPrint('mobile: could not request notification permission: $e');
  }
}

/// P121 — an agent started a message in its channel while the person wasn't looking at it. One notification per agent (a new message of
/// the same agent replaces the last in the tray), titled with the agent's name, in a channel of its own so it can be turned off apart.
Future<void> showAgentMessageNotification(String agent, String text) async {
  try {
    await _plugin.show(
      id: _agentNotificationBase + (agent.hashCode & 0x3ff),
      title: agent,
      body: _truncate(text),
      notificationDetails: const NotificationDetails(
        android: AndroidNotificationDetails(
          _agentChannelId,
          _agentChannelName,
          channelDescription: _agentChannelDescription,
          importance: Importance.high,
          priority: Priority.high,
        ),
      ),
    );
  } catch (e) {
    // No notification is not an error the person can act on; the unread mark is on the screen anyway.
    debugPrint('mobile: could not show an agent notification: $e');
  }
}

Future<void> showChatNotification(ServerMessage message, {required String serverName}) async {
  final (:title, :body) = notificationContentFor(message, serverName: serverName);
  await _plugin.show(
    id: message is ApprovalRequestMessage ? _approvalNotificationId : _notificationId,
    title: title,
    body: body,
    notificationDetails: const NotificationDetails(
      android: AndroidNotificationDetails(
        _channelId,
        _channelName,
        channelDescription: _channelDescription,
        importance: Importance.high,
        priority: Priority.high,
      ),
    ),
  );
}
