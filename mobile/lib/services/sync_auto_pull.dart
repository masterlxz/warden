import 'package:flutter/widgets.dart';

/// P71 fatia 2 — pulled out as pure functions so they're testable without a platform channel,
/// same split as `chat_notifications.dart`'s `shouldNotifyFor`/`notificationContentFor`.

/// True only on the transition INTO resumed (never while already resumed, never on other
/// transitions) — avoids pulling on every lifecycle event and avoids a duplicate pull on the
/// very first frame (`ChatScreen`'s `_lifecycleState` starts as `resumed` already).
bool shouldAutoPullOnResume(AppLifecycleState previous, AppLifecycleState current) {
  return current == AppLifecycleState.resumed && previous != AppLifecycleState.resumed;
}

/// Builds the same kind of human message the desktop's `SyncView.tsx` builds from a
/// `PullResultDto` — null when nothing changed (nothing worth telling the user about).
String? autoPullMessageFor({
  required int filesWritten,
  required int filesDeleted,
  required bool configUpdated,
}) {
  if (filesWritten == 0 && filesDeleted == 0 && !configUpdated) return null;
  final parts = <String>[
    if (filesWritten > 0) '$filesWritten arquivo(s) atualizado(s)',
    if (filesDeleted > 0) '$filesDeleted arquivo(s) removido(s)',
    if (configUpdated) 'config.toml atualizado',
  ];
  return parts.join(', ');
}
