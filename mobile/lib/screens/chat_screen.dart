import 'dart:async';

import 'package:flutter/material.dart';

import '../protocol/messages.dart';
import '../services/mobile_file_tool.dart';
import '../services/server_connection.dart';

enum _EntryRole { user, assistant, error }

class _ChatEntry {
  const _ChatEntry(this.role, this.text);

  final _EntryRole role;
  final String text;
}

/// Fase 7.3: the real chat UI, built on top of the connection 7.2 proved works. Messages are
/// kept in memory for this screen's lifetime only — the protocol has no "fetch history" message
/// yet, so reopening the app (or reconnecting) starts with an empty transcript even though
/// `warden-server` persisted the conversation on disk. Deliberate, narrow scope for this phase
/// (see PENDING.md); not a silent gap.
class ChatScreen extends StatefulWidget {
  const ChatScreen({super.key, required this.connection});

  final ServerConnection connection;

  @override
  State<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends State<ChatScreen> {
  final _entries = <_ChatEntry>[];
  final _inputController = TextEditingController();
  final _scrollController = ScrollController();

  StreamSubscription<ServerMessage>? _chatSubscription;
  StreamSubscription<ConnectionStatus>? _statusSubscription;

  ConnectionStatus _status = const Disconnected();
  bool _waitingForReply = false;

  @override
  void initState() {
    super.initState();
    _status = widget.connection.status;
    _chatSubscription = widget.connection.chatStream.listen(_onChatMessage);
    _statusSubscription = widget.connection.statusStream.listen((s) {
      if (mounted) setState(() => _status = s);
    });
  }

  @override
  void dispose() {
    _chatSubscription?.cancel();
    _statusSubscription?.cancel();
    _inputController.dispose();
    _scrollController.dispose();
    super.dispose();
  }

  void _onChatMessage(ServerMessage msg) {
    if (!mounted) return;
    setState(() {
      _waitingForReply = false;
      switch (msg) {
        case ChatResponseMessage(:final content):
          _entries.add(_ChatEntry(_EntryRole.assistant, content));
        case ChatErrorMessage(:final message):
          _entries.add(_ChatEntry(_EntryRole.error, message));
        default:
          break;
      }
    });
    _scrollToBottom();
  }

  void _send() {
    final text = _inputController.text.trim();
    if (text.isEmpty || _waitingForReply || _status is! Connected) return;

    setState(() {
      _entries.add(_ChatEntry(_EntryRole.user, text));
      _waitingForReply = true;
    });
    widget.connection.sendChat(text);
    _inputController.clear();
    _scrollToBottom();
  }

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_scrollController.hasClients) return;
      _scrollController.animateTo(
        _scrollController.position.maxScrollExtent,
        duration: const Duration(milliseconds: 200),
        curve: Curves.easeOut,
      );
    });
  }

  Future<void> _disconnect() async {
    await widget.connection.goodbye('user disconnected');
    if (mounted) Navigator.of(context).pop();
  }

  Future<void> _openFilesDialog() async {
    final fileTool = MobileFileTool();
    if (!mounted) return;
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (dialogContext, setDialogState) {
          return AlertDialog(
            title: const Text('Files'),
            content: FutureBuilder<String?>(
              future: fileTool.rootFolderUri(),
              builder: (context, snapshot) {
                if (!snapshot.hasData) return const SizedBox(height: 24, child: Center(child: CircularProgressIndicator(strokeWidth: 2)));
                final uri = snapshot.data;
                return Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(uri == null ? 'No folder configured yet.' : 'Configured folder:\n$uri'),
                    const SizedBox(height: 8),
                    const Text(
                      'Lets the model list and read text files under this folder. '
                      'Takes effect the next time you connect.',
                      style: TextStyle(fontSize: 12),
                    ),
                  ],
                );
              },
            ),
            actions: [
              TextButton(
                onPressed: () async {
                  await fileTool.clearRootFolder();
                  setDialogState(() {});
                },
                child: const Text('Clear'),
              ),
              TextButton(
                onPressed: () async {
                  await fileTool.pickRootFolder();
                  setDialogState(() {});
                },
                child: const Text('Choose folder'),
              ),
              TextButton(
                onPressed: () => Navigator.of(dialogContext).pop(),
                child: const Text('Done'),
              ),
            ],
          );
        },
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final serverName = switch (_status) {
      Connected(:final serverName) => serverName,
      _ => widget.connection.serverName,
    };

    return Scaffold(
      appBar: AppBar(
        title: Text(serverName),
        actions: [
          IconButton(
            onPressed: _openFilesDialog,
            icon: const Icon(Icons.folder_outlined),
            tooltip: 'Files',
          ),
          TextButton(
            onPressed: _disconnect,
            child: const Text('Disconnect'),
          ),
        ],
      ),
      body: Column(
        children: [
          if (_status is! Connected) _DisconnectedBanner(status: _status),
          Expanded(
            child: ListView.builder(
              controller: _scrollController,
              padding: const EdgeInsets.all(12),
              itemCount: _entries.length + (_waitingForReply ? 1 : 0),
              itemBuilder: (context, index) {
                if (index == _entries.length) {
                  return const _ThinkingIndicator();
                }
                return _MessageBubble(entry: _entries[index]);
              },
            ),
          ),
          _InputBar(
            controller: _inputController,
            enabled: !_waitingForReply && _status is Connected,
            onSend: _send,
          ),
        ],
      ),
    );
  }
}

class _DisconnectedBanner extends StatelessWidget {
  const _DisconnectedBanner({required this.status});

  final ConnectionStatus status;

  @override
  Widget build(BuildContext context) {
    final label = switch (status) {
      Disconnected(:final reason) => reason == null ? 'Disconnected' : 'Disconnected: $reason',
      Connecting() => 'Connecting…',
      ConnectionFailure(:final message) => 'Connection error: $message',
      Connected() => '',
    };
    return Container(
      width: double.infinity,
      color: Theme.of(context).colorScheme.errorContainer,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: Text(label, style: TextStyle(color: Theme.of(context).colorScheme.onErrorContainer)),
    );
  }
}

class _ThinkingIndicator extends StatelessWidget {
  const _ThinkingIndicator();

  @override
  Widget build(BuildContext context) {
    return const Padding(
      padding: EdgeInsets.symmetric(vertical: 8, horizontal: 4),
      child: Row(
        children: [
          SizedBox(width: 16, height: 16, child: CircularProgressIndicator(strokeWidth: 2)),
          SizedBox(width: 8),
          Text('Thinking…'),
        ],
      ),
    );
  }
}

class _MessageBubble extends StatelessWidget {
  const _MessageBubble({required this.entry});

  final _ChatEntry entry;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (alignment, background, foreground) = switch (entry.role) {
      _EntryRole.user => (Alignment.centerRight, scheme.primaryContainer, scheme.onPrimaryContainer),
      _EntryRole.assistant => (Alignment.centerLeft, scheme.surfaceContainerHighest, scheme.onSurface),
      _EntryRole.error => (Alignment.centerLeft, scheme.errorContainer, scheme.onErrorContainer),
    };

    return Align(
      alignment: alignment,
      child: Container(
        margin: const EdgeInsets.symmetric(vertical: 4),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        constraints: BoxConstraints(maxWidth: MediaQuery.of(context).size.width * 0.8),
        decoration: BoxDecoration(color: background, borderRadius: BorderRadius.circular(12)),
        child: Text(entry.text, style: TextStyle(color: foreground)),
      ),
    );
  }
}

class _InputBar extends StatelessWidget {
  const _InputBar({required this.controller, required this.enabled, required this.onSend});

  final TextEditingController controller;
  final bool enabled;
  final VoidCallback onSend;

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(8),
        child: Row(
          children: [
            Expanded(
              child: TextField(
                controller: controller,
                enabled: enabled,
                minLines: 1,
                maxLines: 5,
                decoration: const InputDecoration(
                  hintText: 'Message',
                  border: OutlineInputBorder(),
                  contentPadding: EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                ),
                onSubmitted: (_) => enabled ? onSend() : null,
              ),
            ),
            const SizedBox(width: 8),
            IconButton.filled(
              onPressed: enabled ? onSend : null,
              icon: const Icon(Icons.send),
            ),
          ],
        ),
      ),
    );
  }
}
