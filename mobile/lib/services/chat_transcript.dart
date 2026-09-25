import 'dart:async';

import 'package:flutter/foundation.dart';

import '../protocol/messages.dart';
import 'device_id.dart';

enum EntryRole { user, assistant, error }

class ChatEntry {
  const ChatEntry(this.role, this.text, {this.attachments = const []});

  final EntryRole role;
  final String text;
  final List<Attachment> attachments;
}

/// What [ChatTranscript] needs from the hub — `ServerConnection` in the app, a fake in tests, so
/// the transcript can be tested without a WebSocket.
abstract interface class ConversationBackend {
  void sendChat(String message, {String? conversationId});
  Future<List<HistoryEntry>> fetchHistory({int? limit, String? conversationId});
  Future<List<ConversationSummary>> listConversations();
  Future<void> renameConversation(String conversationId, String title);
  Future<void> deleteConversation(String conversationId);
}

/// P41 — the in-memory transcript of one server connection, owned by whoever owns the connection
/// (`ConnectionScreen`) rather than by `ChatScreen`'s `State`. Before this, leaving the chat with
/// the back button threw the transcript away with the screen, and any reply that arrived while the
/// screen was gone had no listener at all. Listening for the connection's lifetime here fixes both.
///
/// P40/P78 — the hub keeps several conversations per device; this holds their list, which one is
/// open ([activeConversationId]) and that one's transcript, loaded from the hub when it opens. A
/// turn waits for its answer per conversation, so switching away and back while the model is still
/// answering shows the question again (the hub only saves a turn once it's answered) and the
/// answer lands in the right conversation (`ChatResponse.conversationId`).
class ChatTranscript extends ChangeNotifier {
  ChatTranscript({
    required Stream<ServerMessage> chatStream,
    required this.backend,
    String? lastConversationId,
    this.onConversationOpened,
    this.historyLimit = 100,
    String Function()? newConversationId,
  }) : _newConversationId = newConversationId ?? generateDeviceId {
    _activeId = lastConversationId ?? _newConversationId();
    _subscription = chatStream.listen(_onMessage);
    unawaited(_start());
  }

  final ConversationBackend backend;

  /// Called with the id of every conversation opened, so the caller can reopen it next time.
  final void Function(String conversationId)? onConversationOpened;
  final int historyLimit;
  final String Function() _newConversationId;
  late final StreamSubscription<ServerMessage> _subscription;

  final _entries = <ChatEntry>[];
  var _conversations = <ConversationSummary>[];
  late String _activeId;

  /// Turns sent and not answered yet, by conversation id.
  final _pending = <String, String>{};
  String? _conversationsError;
  bool _disposed = false;

  List<ChatEntry> get entries => List.unmodifiable(_entries);
  List<ConversationSummary> get conversations => List.unmodifiable(_conversations);
  String get activeConversationId => _activeId;

  /// The open conversation's title — null for a new one the hub doesn't have yet.
  String? get activeTitle {
    for (final c in _conversations) {
      if (c.id == _activeId) return c.title;
    }
    return null;
  }

  bool get waitingForReply => _pending.containsKey(_activeId);
  bool isAnswering(String conversationId) => _pending.containsKey(conversationId);

  /// Why the conversation list couldn't be loaded, if it couldn't.
  String? get conversationsError => _conversationsError;

  /// The list, then the conversation opened last time (or the most recent, if that one is gone; a
  /// new one when there are none), then its transcript.
  Future<void> _start() async {
    final list = await refreshConversations();
    if (_disposed) return;
    if (list != null && !list.any((c) => c.id == _activeId) && !_pending.containsKey(_activeId)) {
      _activeId = list.isEmpty ? _newConversationId() : list.first.id;
      notifyListeners();
    }
    onConversationOpened?.call(_activeId);
    await _loadHistory(_activeId);
  }

  /// Re-reads the conversation list. A conversation started here whose first turn is still in
  /// flight isn't on the hub yet, so it stays listed until it is. Null when the hub couldn't answer.
  Future<List<ConversationSummary>?> refreshConversations() async {
    List<ConversationSummary> list;
    try {
      list = await backend.listConversations();
    } catch (e) {
      if (_disposed) return null;
      _conversationsError = "Couldn't load conversations: $e";
      notifyListeners();
      return null;
    }
    if (_disposed) return null;
    _conversations = [
      ..._conversations.where((c) => _pending.containsKey(c.id) && !list.any((l) => l.id == c.id)),
      ...list,
    ];
    _conversationsError = null;
    notifyListeners();
    return list;
  }

  Future<void> _loadHistory(String conversationId) async {
    List<ChatEntry> loaded;
    try {
      final history = await backend.fetchHistory(limit: historyLimit, conversationId: conversationId);
      loaded = [
        for (final m in history)
          ChatEntry(m.fromUser ? EntryRole.user : EntryRole.assistant, m.content, attachments: m.attachments),
      ];
    } catch (e) {
      loaded = [ChatEntry(EntryRole.error, "Couldn't load earlier messages: $e")];
    }
    // Disposed, or another conversation was opened meanwhile.
    if (_disposed || _activeId != conversationId) return;
    final waiting = _pending[conversationId];
    _entries
      ..clear()
      ..addAll(loaded)
      ..addAll([if (waiting != null) ChatEntry(EntryRole.user, waiting)]);
    notifyListeners();
  }

  /// Shows [conversationId]: empty right away, then its transcript once the hub answers.
  void open(String conversationId) {
    if (conversationId == _activeId) return;
    _activeId = conversationId;
    _entries.clear();
    notifyListeners();
    onConversationOpened?.call(conversationId);
    unawaited(_loadHistory(conversationId));
  }

  /// Opens an empty conversation, created on the hub by its first message. Does nothing when the
  /// open one is already that.
  void startNew() {
    final alreadyNew = !_conversations.any((c) => c.id == _activeId) && _entries.isEmpty;
    if (!alreadyNew) open(_newConversationId());
  }

  /// Throws when the hub refuses (e.g. an empty title).
  Future<void> rename(String conversationId, String title) async {
    await backend.renameConversation(conversationId, title);
    await refreshConversations();
  }

  /// Throws when the hub refuses. Deleting the open conversation opens the most recent one left.
  Future<void> delete(String conversationId) async {
    await backend.deleteConversation(conversationId);
    final list = await refreshConversations();
    if (conversationId == _activeId) {
      open(list != null && list.isNotEmpty ? list.first.id : _newConversationId());
    }
  }

  void _onMessage(ServerMessage msg) {
    final (entry, conversationId) = switch (msg) {
      ChatResponseMessage(:final content, :final attachments, :final conversationId) =>
        (ChatEntry(EntryRole.assistant, content, attachments: attachments), conversationId),
      ChatErrorMessage(:final message, :final conversationId) => (ChatEntry(EntryRole.error, message), conversationId),
      _ => (null, null),
    };
    if (entry == null) return;
    final id = conversationId ?? _activeId;
    _pending.remove(id);
    if (id == _activeId) _entries.add(entry);
    notifyListeners();
    // New title/order — and a conversation started here now exists on the hub.
    unawaited(refreshConversations());
  }

  /// Records the user's turn and sends it to the open conversation. No-op for blank text or while
  /// that conversation is still waiting on its reply. Returns whether anything was sent.
  bool send(String text) {
    final trimmed = text.trim();
    if (trimmed.isEmpty || waitingForReply) return false;
    final id = _activeId;
    _entries.add(ChatEntry(EntryRole.user, trimmed));
    _pending[id] = trimmed;
    if (!_conversations.any((c) => c.id == id)) {
      final now = DateTime.now().millisecondsSinceEpoch;
      _conversations = [ConversationSummary(id: id, title: titleFrom(trimmed), createdAt: now, updatedAt: now), ..._conversations];
    }
    notifyListeners();
    backend.sendChat(trimmed, conversationId: id);
    return true;
  }

  @override
  void dispose() {
    _disposed = true;
    _subscription.cancel();
    super.dispose();
  }
}

/// The title the hub gives a new conversation (`title_from` in `warden-bootstrap`): whitespace
/// collapsed, cut to 40 characters with an ellipsis. Shown until the hub's own list comes back.
String titleFrom(String message) {
  final collapsed = message.split(RegExp(r'\s+')).where((w) => w.isNotEmpty).join(' ');
  final chars = collapsed.runes.toList();
  return chars.length > 40 ? '${String.fromCharCodes(chars.take(40))}…' : collapsed;
}
