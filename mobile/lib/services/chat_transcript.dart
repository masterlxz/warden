import 'dart:async';

import 'package:flutter/foundation.dart';

import '../protocol/messages.dart';

enum EntryRole { user, assistant, error }

class ChatEntry {
  const ChatEntry(this.role, this.text, {this.attachments = const []});

  final EntryRole role;
  final String text;
  final List<Attachment> attachments;
}

/// P41 — the in-memory transcript of one server connection, owned by whoever owns the connection
/// (`ConnectionScreen`) rather than by `ChatScreen`'s `State`. Before this, leaving the chat with
/// the back button threw the transcript away with the screen, and any reply that arrived while the
/// screen was gone had no listener at all — so "resume the chat" would have come back empty and
/// possibly missing the answer to the last question. Listening for the connection's lifetime here
/// fixes both. Still memory-only: fetching history from `warden-server` is P40.
///
/// Takes the reply stream and the send function instead of a `ServerConnection` so it can be
/// tested without a WebSocket.
class ChatTranscript extends ChangeNotifier {
  ChatTranscript({required Stream<ServerMessage> chatStream, required this.sendChat}) {
    _subscription = chatStream.listen(_onMessage);
  }

  final void Function(String) sendChat;
  late final StreamSubscription<ServerMessage> _subscription;
  final _entries = <ChatEntry>[];
  bool _waitingForReply = false;

  List<ChatEntry> get entries => List.unmodifiable(_entries);
  bool get waitingForReply => _waitingForReply;

  void _onMessage(ServerMessage msg) {
    switch (msg) {
      case ChatResponseMessage(:final content, :final attachments):
        _entries.add(ChatEntry(EntryRole.assistant, content, attachments: attachments));
      case ChatErrorMessage(:final message):
        _entries.add(ChatEntry(EntryRole.error, message));
      default:
        return;
    }
    _waitingForReply = false;
    notifyListeners();
  }

  /// Records the user's turn and sends it. No-op for blank text or while a reply is still pending
  /// (one turn at a time, same as before). Returns whether anything was sent.
  bool send(String text) {
    final trimmed = text.trim();
    if (trimmed.isEmpty || _waitingForReply) return false;
    _entries.add(ChatEntry(EntryRole.user, trimmed));
    _waitingForReply = true;
    notifyListeners();
    sendChat(trimmed);
    return true;
  }

  @override
  void dispose() {
    _subscription.cancel();
    super.dispose();
  }
}
