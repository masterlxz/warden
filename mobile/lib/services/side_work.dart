import '../protocol/messages.dart';

// P121 — what an agent did outside its channel: the notes it traded with other agents ("A → B", `message_agent`) and the runs nobody was
// watching (scheduled tasks and webhooks, `task-*`). Pure; the sheet only draws what comes out of here. Mirrors `web/src/hub/agentWork.ts`.
// The ids and the title are the hub's (`activity.rs`).

/// The start of the id of a conversation between two agents (`message_agent::thread_id`).
const notePrefix = 'agents-';

/// The start of the id of a scheduled task's conversation (`tasks::CONVERSATION_PREFIX`); a webhook's is [hookPrefix].
const runPrefix = 'task-';
const hookPrefix = 'task-hook-';
const _arrow = ' → ';

enum SideWorkKind { noteOut, noteIn, runTask, runHook }

/// One conversation of the agent outside its channel. [other] is the colleague of a note.
class SideWorkItem {
  const SideWorkItem({required this.id, required this.title, required this.kind, required this.updatedAt, this.other});

  final String id;
  final String title;
  final SideWorkKind kind;
  final String? other;
  final int updatedAt;
}

/// The two sides of a conversation "A → B", or null when the title has no arrow.
(String, String)? _ends(String title) {
  final at = title.indexOf(_arrow);
  if (at < 0) return null;
  return (title.substring(0, at).trim(), title.substring(at + _arrow.length).trim());
}

/// The conversations of [agent] outside its channel, newest first: the notes it left or got and the runs it made. A thread (P125) is not
/// one, it shows from the message it came from.
List<SideWorkItem> sideWork(List<ConversationSummary> conversations, String agent) {
  final items = <SideWorkItem>[];
  for (final c in conversations) {
    if (c.parent != null) continue;
    if (c.id.startsWith(notePrefix)) {
      final pair = _ends(c.title);
      if (pair == null || pair.$1 == pair.$2) continue;
      if (pair.$1 == agent) {
        items.add(SideWorkItem(id: c.id, title: c.title, kind: SideWorkKind.noteOut, other: pair.$2, updatedAt: c.updatedAt));
      } else if (pair.$2 == agent) {
        items.add(SideWorkItem(id: c.id, title: c.title, kind: SideWorkKind.noteIn, other: pair.$1, updatedAt: c.updatedAt));
      }
    } else if (c.id.startsWith(runPrefix) && c.agentId == agent) {
      items.add(SideWorkItem(
        id: c.id,
        title: c.title,
        kind: c.id.startsWith(hookPrefix) ? SideWorkKind.runHook : SideWorkKind.runTask,
        updatedAt: c.updatedAt,
      ));
    }
  }
  items.sort((a, b) => b.updatedAt != a.updatedAt ? b.updatedAt.compareTo(a.updatedAt) : a.id.compareTo(b.id));
  return items;
}

/// What the list says about a conversation: `to ana`, `from ana`, `scheduled task`, `webhook`.
String sideWorkLabel(SideWorkItem item) => switch (item.kind) {
      SideWorkKind.noteOut => 'to ${item.other}',
      SideWorkKind.noteIn => 'from ${item.other}',
      SideWorkKind.runTask => 'scheduled task',
      SideWorkKind.runHook => 'webhook',
    };

/// What the channel's button says: `Notes and runs (3)`, or without the number when there are none.
String sideWorkButtonLabel(int count) => count == 0 ? 'Notes and runs' : 'Notes and runs ($count)';
