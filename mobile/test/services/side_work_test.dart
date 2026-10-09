import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/services/side_work.dart';

// P121 — the pure half of what an agent did outside its channel. Same cases as the web's, the desktop's and the extension's `agentWork.test.mjs`.

ConversationSummary _c(String id, String title, int updatedAt, {String? agentId, ThreadParent? parent}) =>
    ConversationSummary(id: id, title: title, createdAt: 1, updatedAt: updatedAt, agentId: agentId, parent: parent);

void main() {
  group('the work of an agent', () {
    test('has the notes it left and received, and the runs it made, newest first', () {
      final list = [
        _c('agents-1', 'pirate → poet', 10),
        _c('agents-2', 'chief → pirate', 30),
        _c('task-7', 'Task', 20, agentId: 'pirate'),
        _c('task-hook-9', 'Webhook', 40, agentId: 'pirate'),
      ];
      final work = sideWork(list, 'pirate');
      expect(work.map((w) => w.id), ['task-hook-9', 'agents-2', 'task-7', 'agents-1']);
      expect(work.map((w) => w.kind), [SideWorkKind.runHook, SideWorkKind.noteIn, SideWorkKind.runTask, SideWorkKind.noteOut]);
      expect(work.map((w) => w.other), [null, 'chief', null, 'poet']);
    });

    test("leaves out other agents' work, the channels, loose conversations and threads", () {
      final list = [
        _c('agents-1', 'chief → poet', 10),
        _c('task-7', 'Task', 20, agentId: 'poet'),
        _c('task-8', 'Task', 20),
        _c('channel-00ff', 'pirate', 30, agentId: 'pirate'),
        _c('c1', 'chat', 30, agentId: 'pirate'),
        _c('task-9', 'Task', 50, agentId: 'pirate', parent: const ThreadParent(conversationId: 'task-9x', messageId: 'm')),
      ];
      expect(sideWork(list, 'pirate'), isEmpty);
    });

    test('a title without the arrow, or with the same agent on both sides, is not a note', () {
      expect(sideWork([_c('agents-1', 'no arrow', 1), _c('agents-2', 'pirate → pirate', 1)], 'pirate'), isEmpty);
    });

    test('the same time keeps a steady order, by id', () {
      final list = [_c('task-b', 'x', 5, agentId: 'a'), _c('task-a', 'x', 5, agentId: 'a')];
      expect(sideWork(list, 'a').map((w) => w.id), ['task-a', 'task-b']);
    });
  });

  group('the words', () {
    test('say who a note is to or from, and what kind of run', () {
      const base = (id: 'x', title: 't', updatedAt: 1);
      final labels = [
        SideWorkItem(id: base.id, title: base.title, updatedAt: base.updatedAt, kind: SideWorkKind.noteOut, other: 'poet'),
        SideWorkItem(id: base.id, title: base.title, updatedAt: base.updatedAt, kind: SideWorkKind.noteIn, other: 'chief'),
        SideWorkItem(id: base.id, title: base.title, updatedAt: base.updatedAt, kind: SideWorkKind.runTask),
        SideWorkItem(id: base.id, title: base.title, updatedAt: base.updatedAt, kind: SideWorkKind.runHook),
      ].map(sideWorkLabel);
      expect(labels, ['to poet', 'from chief', 'scheduled task', 'webhook']);
    });

    test('the button shows the count only when there is something', () {
      expect(sideWorkButtonLabel(0), 'Notes and runs');
      expect(sideWorkButtonLabel(3), 'Notes and runs (3)');
    });
  });
}
