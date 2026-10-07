import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:audioplayers/audioplayers.dart';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:video_player/video_player.dart';

import '../protocol/messages.dart';
import '../services/chat_notifications.dart';
import '../services/chat_transcript.dart';
import '../services/mobile_file_tool.dart';
import '../services/server_connection.dart';
import '../services/sync_auto_pull.dart';
import '../services/vault_paths.dart';
import '../src/rust/api/sync.dart' as sync_bridge;
import 'agents_screen.dart';
import 'attachment_kind.dart';
import 'folder_picker.dart';

/// Fase 7.3: the real chat UI, built on top of the connection 7.2 proved works. The transcript is
/// kept in memory by the [ChatTranscript] the caller passes in (P41: it outlives this screen, so
/// leaving with the back button and resuming keeps the conversation), which since P40 also starts
/// with the conversation `warden-server` persisted for this device, so reopening the app (or
/// reconnecting) picks up where it left off.
class ChatScreen extends StatefulWidget {
  const ChatScreen({super.key, required this.connection, required this.transcript});

  final ServerConnection connection;
  final ChatTranscript transcript;

  @override
  State<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends State<ChatScreen> with WidgetsBindingObserver {
  final _inputController = TextEditingController();
  final _scrollController = ScrollController();

  StreamSubscription<ServerMessage>? _chatSubscription;
  StreamSubscription<ConnectionStatus>? _statusSubscription;
  StreamSubscription<ServerMessage>? _approvalSubscription;

  /// P87 — approvals the hub is waiting on, oldest first, and the one whose dialog is open.
  final _approvals = <ApprovalRequestMessage>[];
  int? _approvalShown;

  ConnectionStatus _status = const Disconnected();
  AppLifecycleState _lifecycleState = AppLifecycleState.resumed;
  bool _autoPulling = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    unawaited(requestNotificationPermission());
    _status = widget.connection.status;
    _chatSubscription = widget.connection.chatStream.listen(_onChatMessage);
    _statusSubscription = widget.connection.statusStream.listen((s) {
      if (mounted) setState(() => _status = s);
    });
    _approvalSubscription = widget.connection.approvalStream.listen(_onApproval);
  }

  /// P87 — a tool in this device's turn needs the person's yes (an agent creating another, an SSH
  /// host with approval, a spending-limit pause). One dialog at a time; the hub gives up (and says
  /// so) after 120 s, which closes it.
  void _onApproval(ServerMessage msg) {
    switch (msg) {
      case ApprovalRequestMessage():
        _approvals.add(msg);
        if (shouldNotifyFor(_lifecycleState)) {
          unawaited(showChatNotification(msg, serverName: widget.connection.serverName));
        }
        _showNextApproval();
      case ApprovalCancelledMessage(:final approvalId):
        _approvals.removeWhere((a) => a.approvalId == approvalId);
        if (_approvalShown == approvalId && mounted) Navigator.of(context).pop();
      default:
        break;
    }
  }

  Future<void> _showNextApproval() async {
    if (_approvalShown != null || _approvals.isEmpty || !mounted) return;
    final request = _approvals.first;
    _approvalShown = request.approvalId;
    final approved = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Approval needed'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('${request.action}: ${request.target}', style: const TextStyle(fontWeight: FontWeight.bold)),
            if (request.category != null) ...[
              const SizedBox(height: 4),
              Text('Category: ${request.category} (this agent asks for approval on this)'),
            ],
            if (request.detail.isNotEmpty) ...[const SizedBox(height: 8), Text(request.detail)],
          ],
        ),
        actions: [
          TextButton(onPressed: () => Navigator.of(dialogContext).pop(false), child: const Text('Deny')),
          FilledButton(onPressed: () => Navigator.of(dialogContext).pop(true), child: const Text('Approve')),
        ],
      ),
    );
    // Null: the hub gave up on it first (ApprovalCancelled closed the dialog).
    if (approved != null) widget.connection.resolveApproval(request.approvalId, approved);
    _approvals.removeWhere((a) => a.approvalId == request.approvalId);
    _approvalShown = null;
    _showNextApproval();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _chatSubscription?.cancel();
    _statusSubscription?.cancel();
    _approvalSubscription?.cancel();
    _inputController.dispose();
    _scrollController.dispose();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (shouldAutoPullOnResume(_lifecycleState, state)) {
      unawaited(_autoPullOnResume());
    }
    _lifecycleState = state;
  }

  /// P71 fatia 2 — pulls sync changes silently when the app comes back to the foreground,
  /// mirroring the desktop's periodic `spawn_auto_pull` (Fase 4.7 fatia 1) as closely as a
  /// process without a long-lived background daemon allows. Skips silently when sync was never
  /// paired on this device (`bridgeStatus` is safe to call even then — see
  /// `warden-sync::SyncEngine::status`); any other failure (e.g. no network) is only logged, never
  /// surfaced, same posture as the desktop's `eprintln!`.
  Future<void> _autoPullOnResume() async {
    if (_autoPulling) return;
    _autoPulling = true;
    try {
      final paths = await VaultPaths.resolve();
      final status = sync_bridge.bridgeStatus(
        vaultRoot: paths.vaultRoot,
        configPath: paths.configPath,
        secretsPath: paths.secretsPath,
        manifestPath: paths.manifestPath,
      );
      if (!status.paired) return;
      final result = await sync_bridge.bridgePull(
        vaultRoot: paths.vaultRoot,
        configPath: paths.configPath,
        secretsPath: paths.secretsPath,
        manifestPath: paths.manifestPath,
      );
      final message = autoPullMessageFor(
        filesWritten: result.filesWritten,
        filesDeleted: result.filesDeleted,
        configUpdated: result.configUpdated,
      );
      if (message != null && mounted) {
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('Sync: $message')));
      }
    } catch (e) {
      debugPrint('mobile: auto-pull on resume failed: $e');
    } finally {
      _autoPulling = false;
    }
  }

  /// The transcript itself is updated by `ChatTranscript`; this screen only reacts to a new reply
  /// while it's visible (scroll, local notification when the app is in the background).
  void _onChatMessage(ServerMessage msg) {
    if (!mounted) return;
    _scrollToBottom();
    if (shouldNotifyFor(_lifecycleState)) {
      unawaited(showChatNotification(msg, serverName: widget.connection.serverName));
    }
  }

  void _send() {
    if (_status is! Connected) return;
    if (widget.transcript.send(_inputController.text)) {
      _inputController.clear();
      _scrollToBottom();
    }
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

  /// P120, P123 — the agents screen. "Chat" on a node leaves it and starts an empty conversation as that agent.
  void _openAgents() {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => AgentsScreen(
          backend: widget.connection,
          onOpenChat: (agentId) {
            widget.transcript.startNew();
            widget.transcript.selectAgent(agentId);
          },
        ),
      ),
    );
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
        // P78 — the open conversation's title, with the hub's name under it.
        title: ListenableBuilder(
          listenable: widget.transcript,
          builder: (context, _) => Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(widget.transcript.activeTitle ?? 'New conversation', overflow: TextOverflow.ellipsis),
              Text(
                [serverName, if (widget.transcript.selectedAgentId != null) 'as ${widget.transcript.selectedAgentId}'].join(' · '),
                style: Theme.of(context).textTheme.bodySmall,
                overflow: TextOverflow.ellipsis,
              ),
            ],
          ),
        ),
        actions: [
          // P87 — which configured agent this conversation speaks as.
          ListenableBuilder(
            listenable: widget.transcript,
            builder: (context, _) => _AgentMenu(transcript: widget.transcript),
          ),
          // P120, P123 — the organization of the agents and the work they hand each other.
          IconButton(
            key: const Key('agents-button'),
            onPressed: _openAgents,
            icon: const Icon(Icons.account_tree_outlined),
            tooltip: 'Agents',
          ),
          Builder(
            builder: (context) => IconButton(
              onPressed: () => Scaffold.of(context).openEndDrawer(),
              icon: const Icon(Icons.forum_outlined),
              tooltip: 'Conversations',
            ),
          ),
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
      endDrawer: _ConversationDrawer(transcript: widget.transcript, connected: _status is Connected),
      body: Column(
        children: [
          if (_status is! Connected) _DisconnectedBanner(status: _status),
          // P125 — in a thread, the way back to the conversation it came from.
          ListenableBuilder(
            listenable: widget.transcript,
            builder: (context, _) => widget.transcript.threadParent == null
                ? const SizedBox.shrink()
                : Align(
                    alignment: Alignment.centerLeft,
                    child: TextButton.icon(
                      key: const Key('thread-back'),
                      onPressed: widget.transcript.closeThread,
                      icon: const Icon(Icons.arrow_back),
                      label: const Text('Back to the conversation'),
                    ),
                  ),
          ),
          // P102 — the folder this conversation works in, picked before its first message.
          ListenableBuilder(
            listenable: widget.transcript,
            builder: (context, _) => _FolderBar(transcript: widget.transcript, connected: _status is Connected),
          ),
          Expanded(
            child: ListenableBuilder(
              listenable: widget.transcript,
              builder: (context, _) {
                final entries = widget.transcript.entries;
                final waiting = widget.transcript.waitingForReply;
                // P125 — no thread inside a thread; a message gets the button once the hub has given it an id.
                final inThread = widget.transcript.threadParent != null;
                final threads = widget.transcript.threads;
                return ListView.builder(
                  controller: _scrollController,
                  padding: const EdgeInsets.all(12),
                  itemCount: entries.length + (waiting ? 1 : 0),
                  itemBuilder: (context, index) {
                    if (index == entries.length) {
                      return const _ThinkingIndicator();
                    }
                    final entry = entries[index];
                    final id = entry.id;
                    return _MessageBubble(
                      entry: entry,
                      thread: inThread || id == null || entry.role == EntryRole.error
                          ? null
                          : _ThreadLink(replies: threads[id]?.replies ?? 0, onTap: () => widget.transcript.openThread(id)),
                    );
                  },
                );
              },
            ),
          ),
          ListenableBuilder(
            listenable: widget.transcript,
            builder: (context, _) => _InputBar(
              controller: _inputController,
              enabled: !widget.transcript.waitingForReply && _status is Connected,
              onSend: _send,
            ),
          ),
        ],
      ),
    );
  }
}

/// P102 — the working folder of the open conversation: a button to pick one while the conversation is new (and the
/// hub is reachable), a plain label once it has begun. Hidden for an old conversation that has no folder.
class _FolderBar extends StatelessWidget {
  const _FolderBar({required this.transcript, required this.connected});

  final ChatTranscript transcript;
  final bool connected;

  Future<void> _pick(BuildContext context) async {
    final picked = await showFolderPicker(context, listDirs: transcript.listDirs, initialPath: transcript.workdir);
    if (picked != null) transcript.selectWorkdir(picked);
  }

  @override
  Widget build(BuildContext context) {
    final folder = transcript.workdir;
    if (transcript.canPickFolder) {
      return Align(
        alignment: Alignment.centerLeft,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              ActionChip(
                key: const Key('folder-button'),
                avatar: const Icon(Icons.folder_special_outlined, size: 18),
                label: Text(folder == null ? 'Working folder: none' : folderName(folder)),
                tooltip: folder ?? 'Pick a folder of the hub for the AI to work in',
                onPressed: connected ? () => _pick(context) : null,
              ),
              if (folder != null)
                IconButton(
                  key: const Key('folder-clear'),
                  onPressed: () => transcript.selectWorkdir(null),
                  icon: const Icon(Icons.close, size: 18),
                  tooltip: 'Remove the folder',
                ),
            ],
          ),
        ),
      );
    }
    if (folder == null) return const SizedBox.shrink();
    return Align(
      alignment: Alignment.centerLeft,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Tooltip(
          message: folder,
          child: Chip(
            key: const Key('folder-chip'),
            avatar: const Icon(Icons.folder_special_outlined, size: 18),
            label: Text(folderName(folder)),
          ),
        ),
      ),
    );
  }
}

/// P87 — picks the agent the next turns speak as, like the web's selector. Hidden while the hub has
/// no agents configured (and none is selected); locked while the conversation waits on a reply.
class _AgentMenu extends StatelessWidget {
  const _AgentMenu({required this.transcript});

  final ChatTranscript transcript;

  /// `PopupMenuButton` can't return null as a choice, so "no agent" is this.
  static const _none = '';

  @override
  Widget build(BuildContext context) {
    final selected = transcript.selectedAgentId;
    final ids = transcript.agentIds;
    if (ids.isEmpty && selected == null) return const SizedBox.shrink();
    return PopupMenuButton<String>(
      enabled: !transcript.waitingForReply,
      tooltip: 'Agent',
      icon: Icon(selected == null ? Icons.person_outline : Icons.person),
      initialValue: selected ?? _none,
      onOpened: () => unawaited(transcript.refreshAgents()),
      onSelected: (id) => transcript.selectAgent(id == _none ? null : id),
      itemBuilder: (context) => [
        const PopupMenuItem(value: _none, child: Text('No agent')),
        for (final id in ids) PopupMenuItem(value: id, child: Text(id)),
        if (selected != null && !ids.contains(selected)) PopupMenuItem(value: selected, child: Text('$selected (removed)')),
      ],
    );
  }
}

/// P78 — this device's conversations on the hub: open one, start a new one, rename or delete.
class _ConversationDrawer extends StatelessWidget {
  const _ConversationDrawer({required this.transcript, required this.connected});

  final ChatTranscript transcript;

  /// Rename/delete need the hub; browsing and starting a new (still local) one don't.
  final bool connected;

  Future<void> _rename(BuildContext context, ConversationSummary conversation) async {
    final controller = TextEditingController(text: conversation.title);
    final title = await showDialog<String>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Rename conversation'),
        content: TextField(
          controller: controller,
          autofocus: true,
          maxLength: 120,
          onSubmitted: (value) => Navigator.of(dialogContext).pop(value),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.of(dialogContext).pop(), child: const Text('Cancel')),
          TextButton(onPressed: () => Navigator.of(dialogContext).pop(controller.text), child: const Text('Save')),
        ],
      ),
    );
    controller.dispose();
    if (title == null || title.trim().isEmpty || title.trim() == conversation.title || !context.mounted) return;
    await _run(context, () => transcript.rename(conversation.id, title.trim()));
  }

  Future<void> _delete(BuildContext context, ConversationSummary conversation) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Delete conversation?'),
        content: Text('"${conversation.title}" will be deleted from the hub. This can\'t be undone.'),
        actions: [
          TextButton(onPressed: () => Navigator.of(dialogContext).pop(false), child: const Text('Cancel')),
          TextButton(onPressed: () => Navigator.of(dialogContext).pop(true), child: const Text('Delete')),
        ],
      ),
    );
    if (confirmed != true || !context.mounted) return;
    await _run(context, () => transcript.delete(conversation.id));
  }

  Future<void> _run(BuildContext context, Future<void> Function() action) async {
    final messenger = ScaffoldMessenger.of(context);
    try {
      await action();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(e.toString())));
    }
  }

  @override
  Widget build(BuildContext context) {
    return Drawer(
      child: SafeArea(
        child: ListenableBuilder(
          listenable: transcript,
          builder: (context, _) {
            final conversations = transcript.visibleConversations;
            final error = transcript.conversationsError;
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Padding(
                  padding: const EdgeInsets.all(12),
                  child: FilledButton.icon(
                    onPressed: () {
                      transcript.startNew();
                      Navigator.of(context).pop();
                    },
                    icon: const Icon(Icons.add),
                    label: const Text('New conversation'),
                  ),
                ),
                if (error != null)
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
                    child: Text(error, style: TextStyle(color: Theme.of(context).colorScheme.error)),
                  ),
                if (conversations.isEmpty && error == null)
                  const Padding(
                    padding: EdgeInsets.all(16),
                    child: Text('No conversations yet.', textAlign: TextAlign.center),
                  ),
                Expanded(
                  child: ListView.builder(
                    itemCount: conversations.length,
                    itemBuilder: (context, index) {
                      final conversation = conversations[index];
                      final answering = transcript.isAnswering(conversation.id);
                      return ListTile(
                        selected: conversation.id == transcript.activeConversationId,
                        title: Text(conversation.title.isEmpty ? 'Untitled' : conversation.title, maxLines: 1, overflow: TextOverflow.ellipsis),
                        subtitle: answering ? const Text('Answering…') : null,
                        onTap: () {
                          transcript.open(conversation.id);
                          Navigator.of(context).pop();
                        },
                        trailing: PopupMenuButton<String>(
                          // Nothing on the hub to change yet, or its answer is still on the way.
                          enabled: connected && !answering,
                          tooltip: 'Conversation actions',
                          onSelected: (action) => action == 'rename' ? _rename(context, conversation) : _delete(context, conversation),
                          itemBuilder: (_) => const [
                            PopupMenuItem(value: 'rename', child: Text('Rename')),
                            PopupMenuItem(value: 'delete', child: Text('Delete')),
                          ],
                        ),
                      );
                    },
                  ),
                ),
              ],
            );
          },
        ),
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

/// P125 — what a message offers under it: start a thread, or open the one it has (with its reply count).
class _ThreadLink extends StatelessWidget {
  const _ThreadLink({required this.replies, required this.onTap});

  final int replies;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return TextButton(
      key: const Key('thread-link'),
      style: TextButton.styleFrom(visualDensity: VisualDensity.compact, padding: EdgeInsets.zero, minimumSize: const Size(0, 28)),
      onPressed: onTap,
      child: Text(replies == 0 ? 'Reply in thread' : (replies == 1 ? '1 reply' : '$replies replies')),
    );
  }
}

class _MessageBubble extends StatelessWidget {
  const _MessageBubble({required this.entry, this.thread});

  final ChatEntry entry;
  final _ThreadLink? thread;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (alignment, background, foreground) = switch (entry.role) {
      EntryRole.user => (Alignment.centerRight, scheme.primaryContainer, scheme.onPrimaryContainer),
      EntryRole.assistant => (Alignment.centerLeft, scheme.surfaceContainerHighest, scheme.onSurface),
      EntryRole.error => (Alignment.centerLeft, scheme.errorContainer, scheme.onErrorContainer),
    };

    return Align(
      alignment: alignment,
      child: Container(
        margin: const EdgeInsets.symmetric(vertical: 4),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        constraints: BoxConstraints(maxWidth: MediaQuery.of(context).size.width * 0.8),
        decoration: BoxDecoration(color: background, borderRadius: BorderRadius.circular(12)),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(entry.text, style: TextStyle(color: foreground)),
            for (final attachment in entry.attachments) _AttachmentPreview(attachment: attachment, foreground: foreground),
            ?thread,
          ],
        ),
      ),
    );
  }
}

/// Media extracted from an MCP tool result (P64 frente 2). `image/*` renders via `Image.memory`
/// (fatia 3); `audio/*`/`video/*` play for real too now (P66) — audio straight from bytes
/// (`audioplayers`'s `BytesSource`), video via a temp file (`video_player` has no bytes source).
/// Anything else still falls back to a caption instead of vanishing silently, same
/// graceful-degradation spirit as `MEDIA_REPLY` on the WhatsApp side for *received* media.
class _AttachmentPreview extends StatelessWidget {
  const _AttachmentPreview({required this.attachment, required this.foreground});

  final Attachment attachment;
  final Color foreground;

  @override
  Widget build(BuildContext context) {
    return switch (attachmentKindFor(attachment.mimeType)) {
      AttachmentKind.image => _ImageAttachment(attachment: attachment, foreground: foreground),
      AttachmentKind.audio => _AudioAttachmentPlayer(attachment: attachment, foreground: foreground),
      AttachmentKind.video => _VideoAttachmentPlayer(attachment: attachment, foreground: foreground),
      AttachmentKind.unsupported => _UnsupportedAttachment(attachment: attachment, foreground: foreground),
    };
  }
}

class _UnsupportedAttachment extends StatelessWidget {
  const _UnsupportedAttachment({required this.attachment, required this.foreground});

  final Attachment attachment;
  final Color foreground;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Text(
        '📎 ${attachment.mimeType} attached (playback not supported here yet)',
        style: TextStyle(color: foreground, fontSize: 12, fontStyle: FontStyle.italic),
      ),
    );
  }
}

class _AttachmentError extends StatelessWidget {
  const _AttachmentError({required this.attachment, required this.foreground, required this.verb});

  final Attachment attachment;
  final Color foreground;
  final String verb;

  @override
  Widget build(BuildContext context) {
    return Text(
      'Could not $verb ${attachment.mimeType} attachment',
      style: TextStyle(color: foreground, fontSize: 12, fontStyle: FontStyle.italic),
    );
  }
}

class _ImageAttachment extends StatelessWidget {
  const _ImageAttachment({required this.attachment, required this.foreground});

  final Attachment attachment;
  final Color foreground;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(8),
        child: Image.memory(
          base64Decode(attachment.data),
          errorBuilder: (context, error, stackTrace) =>
              _AttachmentError(attachment: attachment, foreground: foreground, verb: 'decode'),
        ),
      ),
    );
  }
}

/// Plays an `audio/*` attachment straight from the decoded bytes — `BytesSource` needs no temp
/// file, same zero-I/O spirit as `Image.memory` above. Manual play/pause, no autoplay (same
/// deliberate posture as the desktop's TTS `SpeakButton`, P28).
class _AudioAttachmentPlayer extends StatefulWidget {
  const _AudioAttachmentPlayer({required this.attachment, required this.foreground});

  final Attachment attachment;
  final Color foreground;

  @override
  State<_AudioAttachmentPlayer> createState() => _AudioAttachmentPlayerState();
}

class _AudioAttachmentPlayerState extends State<_AudioAttachmentPlayer> {
  final _player = AudioPlayer();
  PlayerState _state = PlayerState.stopped;
  String? _error;
  StreamSubscription<PlayerState>? _stateSubscription;

  @override
  void initState() {
    super.initState();
    _stateSubscription = _player.onPlayerStateChanged.listen((state) {
      if (mounted) setState(() => _state = state);
    });
  }

  @override
  void dispose() {
    _stateSubscription?.cancel();
    _player.dispose();
    super.dispose();
  }

  Future<void> _toggle() async {
    if (_state == PlayerState.playing) {
      await _player.pause();
      return;
    }
    try {
      final bytes = base64Decode(widget.attachment.data);
      await _player.play(BytesSource(bytes));
    } catch (_) {
      if (mounted) setState(() => _error = 'play');
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_error != null) {
      return _AttachmentError(attachment: widget.attachment, foreground: widget.foreground, verb: _error!);
    }
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          IconButton(
            icon: Icon(_state == PlayerState.playing ? Icons.pause_circle_filled : Icons.play_circle_filled,
                color: widget.foreground),
            onPressed: _toggle,
          ),
          Text(widget.attachment.mimeType, style: TextStyle(color: widget.foreground, fontSize: 12)),
        ],
      ),
    );
  }
}

/// Plays a `video/*` attachment — `video_player` has no bytes source, so the decoded bytes are
/// written to a temp file once and cleaned up on dispose.
class _VideoAttachmentPlayer extends StatefulWidget {
  const _VideoAttachmentPlayer({required this.attachment, required this.foreground});

  final Attachment attachment;
  final Color foreground;

  @override
  State<_VideoAttachmentPlayer> createState() => _VideoAttachmentPlayerState();
}

class _VideoAttachmentPlayerState extends State<_VideoAttachmentPlayer> {
  VideoPlayerController? _controller;
  File? _tempFile;
  late final Future<void> _initialization = _initialize();

  static const _extensionBySubtype = {'quicktime': 'mov', 'x-msvideo': 'avi'};

  Future<void> _initialize() async {
    final bytes = base64Decode(widget.attachment.data);
    final subtype = widget.attachment.mimeType.split('/').last;
    final extension = _extensionBySubtype[subtype] ?? subtype;
    final dir = await getTemporaryDirectory();
    final file = File('${dir.path}/warden_attachment_${DateTime.now().microsecondsSinceEpoch}.$extension');
    await file.writeAsBytes(bytes);
    _tempFile = file;

    final controller = VideoPlayerController.file(file);
    await controller.initialize();
    _controller = controller;
  }

  @override
  void dispose() {
    _controller?.dispose();
    _tempFile?.delete().catchError((_) => _tempFile!);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: FutureBuilder<void>(
        future: _initialization,
        builder: (context, snapshot) {
          if (snapshot.hasError || (snapshot.connectionState == ConnectionState.done && _controller == null)) {
            return _AttachmentError(attachment: widget.attachment, foreground: widget.foreground, verb: 'play');
          }
          if (snapshot.connectionState != ConnectionState.done) {
            return SizedBox(
              height: 32,
              width: 32,
              child: CircularProgressIndicator(strokeWidth: 2, color: widget.foreground),
            );
          }
          final controller = _controller!;
          return ClipRRect(
            borderRadius: BorderRadius.circular(8),
            child: AspectRatio(
              aspectRatio: controller.value.aspectRatio,
              child: Stack(
                alignment: Alignment.center,
                children: [
                  VideoPlayer(controller),
                  GestureDetector(
                    onTap: () => setState(() {
                      controller.value.isPlaying ? controller.pause() : controller.play();
                    }),
                    child: AnimatedOpacity(
                      opacity: controller.value.isPlaying ? 0 : 1,
                      duration: const Duration(milliseconds: 200),
                      child: Icon(Icons.play_circle_filled, size: 48, color: widget.foreground),
                    ),
                  ),
                ],
              ),
            ),
          );
        },
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
