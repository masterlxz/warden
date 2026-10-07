import 'dart:async';

import 'package:flutter/foundation.dart';

import '../protocol/messages.dart';
import 'device_id.dart';

enum EntryRole { user, assistant, error }

class ChatEntry {
  const ChatEntry(this.role, this.text, {this.attachments = const [], this.id});

  final EntryRole role;
  final String text;
  final List<Attachment> attachments;

  /// P125 — the hub's id for the message, which a thread hangs from. Null until the history is read again after the
  /// message is sent or answered.
  final String? id;

  ChatEntry withId(String id) => ChatEntry(role, text, attachments: attachments, id: id);
}

/// P125 — a message's thread: its conversation and how many messages the person sent in it.
class ThreadInfo {
  const ThreadInfo(this.conversationId, this.replies);

  final String conversationId;
  final int replies;
}

/// P125 — the ids of the hub's [history] given to the [entries] on screen that don't have one (the answer that has
/// just arrived). Only when the entries, errors left out (the hub doesn't keep them), are as many as the history:
/// otherwise a turn is on the way and the positions don't match, so the entries stay as they are.
List<ChatEntry> withMessageIds(List<ChatEntry> entries, List<HistoryEntry> history) {
  if (entries.where((e) => e.role != EntryRole.error).length != history.length) return entries;
  var next = 0;
  return [
    for (final e in entries)
      if (e.role == EntryRole.error) e else _withHubId(e, history[next++].id),
  ];
}

ChatEntry _withHubId(ChatEntry entry, String? id) => id != null && entry.id == null ? entry.withId(id) : entry;

/// What [ChatTranscript] needs from the hub — `ServerConnection` in the app, a fake in tests, so
/// the transcript can be tested without a WebSocket.
abstract interface class ConversationBackend {
  void sendChat(String message, {String? conversationId, String? agentId, String? workdir, ThreadParent? threadOf});
  Future<List<String>> listAgentIds();
  Future<DirListMessage> listDirs([String? path]);
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

  /// P87 — the hub's configured agents, and the one the next turn speaks as (null: none). Opening a
  /// conversation restores the agent it last spoke with; a new one keeps the last choice.
  var _agentIds = <String>[];
  String? _agentId;

  /// P102 — the folder of the hub's machine the open conversation works in. Picked before the first
  /// message of a new conversation and fixed after it, so opening a conversation shows the one it has.
  String? _workdir;
  String? _conversationsError;
  bool _disposed = false;

  /// P125 — a thread opened here whose first reply hasn't been sent: the hub only learns of it (and of what it hangs
  /// from) with that reply.
  ({String id, ThreadParent parent})? _threadDraft;

  List<ChatEntry> get entries => List.unmodifiable(_entries);
  List<ConversationSummary> get conversations => List.unmodifiable(_conversations);

  /// P125 — the conversations the list shows: threads are left out, they show from the message they came from.
  List<ConversationSummary> get visibleConversations => List.unmodifiable(_conversations.where((c) => c.parent == null));

  /// P125 — what the open conversation hangs from, when it is a thread (on the hub already, or still a draft).
  ThreadParent? get threadParent {
    final draft = _threadDraft;
    if (draft != null && draft.id == _activeId) return draft.parent;
    for (final c in _conversations) {
      if (c.id == _activeId) return c.parent;
    }
    return null;
  }

  /// P125 — the threads of the open conversation, by the id of the message they hang from. A message has at most one;
  /// if a race between two devices made two, the one with more replies counts.
  Map<String, ThreadInfo> get threads {
    final found = <String, ThreadInfo>{};
    for (final c in _conversations) {
      final parent = c.parent;
      if (parent == null || parent.conversationId != _activeId) continue;
      final known = found[parent.messageId];
      if (known == null || c.replies > known.replies) found[parent.messageId] = ThreadInfo(c.id, c.replies);
    }
    return found;
  }
  String get activeConversationId => _activeId;

  /// The open conversation's title — null for a new one the hub doesn't have yet.
  String? get activeTitle {
    for (final c in _conversations) {
      if (c.id == _activeId) return c.title;
    }
    return null;
  }

  List<String> get agentIds => List.unmodifiable(_agentIds);
  String? get selectedAgentId => _agentId;
  String? get workdir => _workdir;

  /// A folder can be chosen only for a conversation the hub doesn't have yet and that has no message — the
  /// same rule as the web, the desktop and the CLI, so a conversation never moves folder halfway.
  bool get canPickFolder => !_conversations.any((c) => c.id == _activeId) && _entries.isEmpty;

  bool get waitingForReply => _pending.containsKey(_activeId);
  bool isAnswering(String conversationId) => _pending.containsKey(conversationId);

  /// Why the conversation list couldn't be loaded, if it couldn't.
  String? get conversationsError => _conversationsError;

  /// The list, then the conversation opened last time (or the most recent, if that one is gone; a
  /// new one when there are none), then its transcript.
  Future<void> _start() async {
    unawaited(refreshAgents());
    final list = await refreshConversations();
    if (_disposed) return;
    if (list != null && !list.any((c) => c.id == _activeId) && !_pending.containsKey(_activeId)) {
      _activeId = _firstVisible(list)?.id ?? _newConversationId();
    }
    _restoreAgent(_activeId);
    // A folder picked while the list was loading stays; only a conversation the hub has brings its own.
    if (_conversations.any((c) => c.id == _activeId)) _restoreWorkdir(_activeId);
    notifyListeners();
    onConversationOpened?.call(_activeId);
    await _loadHistory(_activeId);
  }

  /// Re-reads the configured agents. A hub that can't answer leaves the selector empty.
  Future<void> refreshAgents() async {
    List<String> ids;
    try {
      ids = await backend.listAgentIds();
    } catch (_) {
      ids = const [];
    }
    if (_disposed) return;
    _agentIds = ids;
    notifyListeners();
  }

  /// Speak as [agentId] from the next turn on (null: no agent).
  void selectAgent(String? agentId) {
    if (agentId == _agentId) return;
    _agentId = agentId;
    notifyListeners();
  }

  /// The folders inside [path] on the hub's machine, for the picker (no path: where the person may start).
  Future<DirListMessage> listDirs([String? path]) => backend.listDirs(path);

  /// Work in [path] from the first message on (null: no folder). Ignored once the conversation has begun.
  void selectWorkdir(String? path) {
    if (!canPickFolder || path == _workdir) return;
    _workdir = path;
    notifyListeners();
  }

  /// The folder [conversationId] works in, when the hub has it; a conversation the hub doesn't have yet starts
  /// with none (unlike the agent, a folder is never carried over from the last one).
  void _restoreWorkdir(String conversationId) {
    _workdir = null;
    for (final c in _conversations) {
      if (c.id == conversationId) {
        _workdir = c.workdir;
        return;
      }
    }
  }

  /// The agent [conversationId] last spoke with, when the hub has it; otherwise the choice stays.
  void _restoreAgent(String conversationId) {
    for (final c in _conversations) {
      if (c.id == conversationId) {
        _agentId = c.agentId;
        return;
      }
    }
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
          ChatEntry(m.fromUser ? EntryRole.user : EntryRole.assistant, m.content, attachments: m.attachments, id: m.id),
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

  /// P125 — the message just answered has no id on screen yet; reads the history again to give it one, so a thread can
  /// hang from it.
  Future<void> _attachMessageIds(String conversationId) async {
    List<HistoryEntry> history;
    try {
      history = await backend.fetchHistory(limit: historyLimit, conversationId: conversationId);
    } catch (_) {
      return;
    }
    if (_disposed || _activeId != conversationId) return;
    final next = withMessageIds(_entries, history);
    if (identical(next, _entries)) return;
    _entries
      ..clear()
      ..addAll(next);
    notifyListeners();
  }

  /// P125 — the first conversation of [list] that is not a thread.
  ConversationSummary? _firstVisible(List<ConversationSummary> list) {
    for (final c in list) {
      if (c.parent == null) return c;
    }
    return null;
  }

  /// P125 — opens the thread of the message [messageId] of the open conversation: the one it has, or an empty one the
  /// first reply creates. Not from inside a thread, and not before the hub has given the message an id.
  void openThread(String messageId) {
    if (threadParent != null) return;
    final existing = threads[messageId];
    if (existing != null) {
      open(existing.conversationId);
      return;
    }
    final id = _newConversationId();
    _threadDraft = (id: id, parent: ThreadParent(conversationId: _activeId, messageId: messageId));
    open(id);
  }

  /// P125 — back to the conversation the open thread came from.
  void closeThread() {
    final parent = threadParent;
    if (parent != null) open(parent.conversationId);
  }

  /// Shows [conversationId]: empty right away, then its transcript once the hub answers.
  void open(String conversationId) {
    if (conversationId == _activeId) return;
    if (_threadDraft != null && _threadDraft!.id != conversationId) _threadDraft = null;
    _activeId = conversationId;
    _entries.clear();
    _restoreAgent(conversationId);
    _restoreWorkdir(conversationId);
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
      open((list == null ? null : _firstVisible(list))?.id ?? _newConversationId());
    }
  }

  void _onMessage(ServerMessage msg) {
    if (msg is ConversationsChangedMessage) {
      // An agent left a note in one of this device's conversations, or answered one (P87).
      unawaited(refreshConversations());
      if (msg.conversationId == _activeId && !_pending.containsKey(_activeId)) unawaited(_loadHistory(_activeId));
      return;
    }
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
    if (id == _activeId && entry.role == EntryRole.assistant) unawaited(_attachMessageIds(id));
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
    // The folder only counts when the conversation is created, which is this message.
    String? creatingIn;
    ThreadParent? threadOf;
    if (!_conversations.any((c) => c.id == id)) {
      // P125 — a thread's first reply carries the link; the hub gives it the parent's folder and project, so no folder.
      final draft = _threadDraft;
      threadOf = draft != null && draft.id == id ? draft.parent : null;
      creatingIn = threadOf == null ? _workdir : null;
      final now = DateTime.now().millisecondsSinceEpoch;
      _conversations = [
        ConversationSummary(
          id: id,
          title: titleFrom(trimmed),
          createdAt: now,
          updatedAt: now,
          agentId: _agentId,
          workdir: creatingIn,
          parent: threadOf,
        ),
        ..._conversations,
      ];
    }
    notifyListeners();
    backend.sendChat(trimmed, conversationId: id, agentId: _agentId, workdir: creatingIn, threadOf: threadOf);
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
