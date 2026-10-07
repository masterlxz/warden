import 'package:shared_preferences/shared_preferences.dart';

import '../protocol/messages.dart';
import 'chat_transcript.dart' show isAgentChannel;

/// P121 — which agent channels have something the person has not seen. Pure, so it is tested without a screen. Mirrors
/// `web/src/hub/unread.ts`.
///
/// The hub does not keep a "read" mark: each device remembers, per channel, the time of the last change it showed (a [SeenMap]). A
/// channel is unread when it changed after that and is not the one in front. The first time a device runs there is nothing to compare
/// with, so what is there then counts as seen ([baseline]); after that a channel with no mark is a new one, and unread.
typedef SeenMap = Map<String, int>;

/// The first run on a device: every channel that exists counts as seen, so the first list does not light up all of them.
SeenMap baseline(List<ConversationSummary> conversations) => {
      for (final c in conversations)
        if (isAgentChannel(c.id)) c.id: c.updatedAt,
    };

/// Whether [channel] has changes the device has not shown: it changed after the last one it showed, and it is not in front right now.
bool isUnread(ConversationSummary channel, SeenMap seen, String? watchingId) =>
    channel.id != watchingId && channel.updatedAt > (seen[channel.id] ?? 0);

/// The ids of the channels with something unseen.
List<String> unreadIds(List<ConversationSummary> conversations, SeenMap seen, String? watchingId) => [
      for (final c in conversations)
        if (isAgentChannel(c.id) && isUnread(c, seen, watchingId)) c.id,
    ];

/// [seen] with [channel] shown up to its latest change. The same map when nothing moves, so a save can be skipped.
SeenMap markSeen(SeenMap seen, ConversationSummary channel) =>
    (seen[channel.id] ?? 0) >= channel.updatedAt ? seen : {...seen, channel.id: channel.updatedAt};

/// Where the marks are kept between runs, so a message that came while the app was closed is still unread when it opens.
abstract interface class ChannelSeenStore {
  /// The marks kept, or null when there are none yet (the first run).
  Future<SeenMap?> load();

  Future<void> save(SeenMap seen);
}

/// [ChannelSeenStore] in the app's preferences, one set of marks per hub.
class PrefsChannelSeenStore implements ChannelSeenStore {
  PrefsChannelSeenStore(this.host, this.port);

  final String host;
  final int port;

  String get _key => 'channels.seen.$host:$port';

  @override
  Future<SeenMap?> load() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getStringList(_key);
    if (raw == null) return null;
    final seen = <String, int>{};
    for (final entry in raw) {
      final at = entry.lastIndexOf('=');
      final millis = at < 0 ? null : int.tryParse(entry.substring(at + 1));
      if (millis != null) seen[entry.substring(0, at)] = millis;
    }
    return seen;
  }

  @override
  Future<void> save(SeenMap seen) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setStringList(_key, [for (final e in seen.entries) '${e.key}=${e.value}']);
  }
}
