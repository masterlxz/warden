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
const _channelId = 'chat_messages';
const _channelName = 'Chat messages';
const _channelDescription = "Notifies you when Warden replies while the app isn't in the foreground";

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
Future<void> requestNotificationPermission() async {
  await _plugin
      .resolvePlatformSpecificImplementation<AndroidFlutterLocalNotificationsPlugin>()
      ?.requestNotificationsPermission();
}

Future<void> showChatNotification(ServerMessage message, {required String serverName}) async {
  final (:title, :body) = notificationContentFor(message, serverName: serverName);
  await _plugin.show(
    id: _notificationId,
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
