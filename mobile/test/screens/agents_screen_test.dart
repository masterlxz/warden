import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/screens/agents_screen.dart';
import 'package:mobile/services/agent_work.dart';
import 'package:mobile/services/server_connection.dart' show HubRequestException;

/// A hub in memory: it holds the agents and the tasks, checks the pairing key like the real one, and records
/// what the screen asked for.
class _FakeBackend implements AgentsBackend {
  _FakeBackend({List<AgentInfo> agents = const [], this.policies = const [], List<AgentTask> tasks = const []})
      : agents = List.of(agents),
        tasks = List.of(tasks);

  static const pairingKey = 'right-key';

  List<AgentInfo> agents;
  final List<ModelPolicy> policies;
  List<AgentTask> tasks;
  final edits = <OrgEdit>[];
  final controls = <String>[];

  @override
  Future<HubAgents> listHubAgents() async => HubAgents(List.of(agents), policies);

  @override
  Future<HubAgents> editAgentOrg(String key, OrgEdit edit) async {
    if (key != pairingKey) throw const HubRequestException('wrong key', authRejected: true);
    edits.add(edit);
    switch (edit) {
      case SetPositionEdit(:final id, :final role, :final reportsTo):
        agents = [
          for (final a in agents)
            if (a.id == id) AgentInfo(id: a.id, role: role, reportsTo: (reportsTo ?? '').isEmpty ? null : reportsTo) else a,
        ];
      case AddReportEdit(:final id, :final reportsTo):
        agents = [...agents, AgentInfo(id: id, reportsTo: reportsTo)];
      case RemoveAgentEdit(:final id):
        agents = [for (final a in agents) if (a.id != id) a];
    }
    return HubAgents(List.of(agents), policies);
  }

  @override
  Future<List<AgentTask>> listAgentTasks() async => List.of(tasks);

  @override
  Future<List<AgentTask>> controlAgentTask(String key, String taskId, String action) async {
    if (key != pairingKey) throw const HubRequestException('wrong key', authRejected: true);
    controls.add('$taskId:$action');
    return List.of(tasks);
  }
}

AgentTask _task(String id, String state, {String? parentId, bool controllable = false, bool pausable = false, String? owner = 'chief', String? result}) => AgentTask(
      id: id,
      group: 'g1',
      owner: owner,
      assignee: id,
      parentId: parentId,
      objective: 'do $id',
      channel: 'cli',
      state: state,
      createdAtMs: 1000,
      controllable: controllable,
      pausable: pausable,
      result: result,
    );

Future<void> _pump(WidgetTester tester, _FakeBackend backend, {void Function(String)? onOpenChat}) async {
  // Pushed like in the chat, so "Chat" on a node has a screen to leave.
  await tester.pumpWidget(
    MaterialApp(
      home: Builder(
        builder: (context) => Scaffold(
          body: ElevatedButton(
            key: const Key('open-agents'),
            onPressed: () => Navigator.of(context).push(
              MaterialPageRoute(builder: (_) => AgentsScreen(backend: backend, onOpenChat: onOpenChat, refreshEvery: null)),
            ),
            child: const Text('open'),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.byKey(const Key('open-agents')));
  await tester.pumpAndSettle();
}

Future<void> _openMenu(WidgetTester tester, String agent, String item) async {
  await tester.tap(find.byKey(Key('menu-$agent')));
  await tester.pumpAndSettle();
  await tester.tap(find.text(item).last);
  await tester.pumpAndSettle();
}

Future<void> _typeKey(WidgetTester tester, String key) async {
  await tester.enterText(find.byKey(const Key('pairing-key-field')), key);
  await tester.pump();
  await tester.tap(find.byKey(const Key('pairing-key-confirm')));
  await tester.pumpAndSettle();
}

void main() {
  group('organization', () {
    final agents = [
      const AgentInfo(id: 'chief', role: 'CTO', canDelegateToAgents: true, delegationModels: ['fast']),
      const AgentInfo(id: 'dev', reportsTo: 'chief', autonomy: 3),
    ];

    testWidgets('shows the tree with roles, badges and the model limit, and the policies under it', (tester) async {
      await _pump(tester, _FakeBackend(agents: agents, policies: const [ModelPolicy(id: 'fast', model: 'gpt-mini', description: 'quick')]));
      expect(find.text('chief'), findsOneWidget);
      expect(find.text('CTO'), findsOneWidget);
      expect(find.text('delegates'), findsOneWidget);
      expect(find.textContaining('Dictated: every task it delegates runs on fast.'), findsOneWidget);
      expect(find.text('1 report'), findsOneWidget);
      expect(find.text('autonomy 3: asks first'), findsOneWidget);
      expect(find.text('fast → gpt-mini'), findsOneWidget);
    });

    testWidgets('moving an agent asks for the pairing key, refuses a wrong one and then applies it', (tester) async {
      final backend = _FakeBackend(agents: agents);
      await _pump(tester, backend);
      await _openMenu(tester, 'dev', 'Edit position');
      await tester.enterText(find.byKey(const Key('role-field')), 'Backend');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();

      await _typeKey(tester, 'wrong');
      expect(find.text('Wrong pairing key.'), findsOneWidget);
      expect(backend.edits, isEmpty);

      await tester.enterText(find.byKey(const Key('pairing-key-field')), _FakeBackend.pairingKey);
      await tester.pump();
      await tester.tap(find.byKey(const Key('pairing-key-confirm')));
      await tester.pumpAndSettle();
      expect(backend.edits.single, isA<SetPositionEdit>());
      expect(find.text('Backend'), findsOneWidget);
    });

    testWidgets('adds an agent at the top once it has a name and instructions', (tester) async {
      final backend = _FakeBackend(agents: agents);
      await _pump(tester, backend);
      await tester.tap(find.byKey(const Key('add-top')));
      await tester.pumpAndSettle();
      expect(tester.widget<FilledButton>(find.byKey(const Key('new-agent-confirm'))).onPressed, isNull);

      await tester.enterText(find.byKey(const Key('new-name-field')), 'reviewer');
      await tester.enterText(find.byKey(const Key('new-persona-field')), 'Reviews code.');
      await tester.pump();
      await tester.tap(find.byKey(const Key('new-agent-confirm')));
      await tester.pumpAndSettle();
      await _typeKey(tester, _FakeBackend.pairingKey);

      expect(backend.edits.single, isA<AddReportEdit>());
      expect(find.text('reviewer'), findsOneWidget);
    });

    testWidgets('removing an agent is confirmed first, and its reports go up', (tester) async {
      final backend = _FakeBackend(agents: agents);
      await _pump(tester, backend);
      await _openMenu(tester, 'chief', 'Remove');
      expect(find.textContaining('Its report goes'), findsOneWidget);
      await tester.tap(find.text('Remove').last);
      await tester.pumpAndSettle();
      await _typeKey(tester, _FakeBackend.pairingKey);

      expect(backend.edits.single, isA<RemoveAgentEdit>());
      expect(find.byKey(const Key('agent-chief')), findsNothing);
      expect(find.byKey(const Key('agent-dev')), findsOneWidget);
    });

    testWidgets('Chat on a node leaves the screen with that agent, and Tasks opens its tasks', (tester) async {
      String? opened;
      final backend = _FakeBackend(agents: agents, tasks: [_task('backend', 'done', owner: 'dev'), _task('docs', 'done', owner: 'writer')]);
      await _pump(tester, backend, onOpenChat: (id) => opened = id);

      await _openMenu(tester, 'dev', 'Tasks');
      expect(find.byKey(const Key('task-backend')), findsOneWidget, reason: 'delegated by dev');
      expect(find.byKey(const Key('task-docs')), findsNothing, reason: 'nobody dev is involved with');
      await tester.tap(find.byKey(const Key('show-all-tasks')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('task-docs')), findsOneWidget);

      await tester.tap(find.byKey(const Key('tab-organization')));
      await tester.pumpAndSettle();
      await _openMenu(tester, 'dev', 'Chat');
      expect(opened, 'dev');
      expect(find.byKey(const Key('open-agents')), findsOneWidget, reason: 'back on the screen that opened it');
    });
  });

  group('tasks', () {
    testWidgets('shows the progress of a batch with its subtasks one level in', (tester) async {
      await _pump(tester, _FakeBackend(tasks: [_task('manager', 'waiting'), _task('helper', 'done', parentId: 'manager')]));
      await tester.tap(find.byKey(const Key('tab-tasks')));
      await tester.pumpAndSettle();
      expect(find.textContaining('1 of 2 finished (50%)'), findsOneWidget);
      expect(find.byKey(const Key('task-manager')), findsOneWidget);
      expect(find.byKey(const Key('task-helper')), findsOneWidget);
    });

    testWidgets('says so when there is nothing yet', (tester) async {
      await _pump(tester, _FakeBackend());
      await tester.tap(find.byKey(const Key('tab-tasks')));
      await tester.pumpAndSettle();
      expect(find.textContaining('Nothing yet.'), findsOneWidget);
    });

    testWidgets('a running task on this hub can be paused, with the pairing key', (tester) async {
      final backend = _FakeBackend(tasks: [_task('a', 'running', controllable: true, pausable: true)]);
      await _pump(tester, backend);
      await tester.tap(find.byKey(const Key('tab-tasks')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('task-a')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('task-a-pause')), findsOneWidget);
      expect(find.byKey(const Key('task-a-cancel')), findsOneWidget);

      await tester.tap(find.byKey(const Key('task-a-pause')));
      await tester.pumpAndSettle();
      await _typeKey(tester, _FakeBackend.pairingKey);
      expect(backend.controls, ['a:pause']);
    });

    testWidgets('a task of another process, or a finished one, has no controls', (tester) async {
      await _pump(tester, _FakeBackend(tasks: [_task('a', 'running'), _task('b', 'done', controllable: true, result: 'ok')]));
      await tester.tap(find.byKey(const Key('tab-tasks')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('task-a')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('task-b')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('task-a-pause')), findsNothing);
      expect(find.byKey(const Key('task-a-cancel')), findsNothing);
      expect(find.byKey(const Key('task-b-cancel')), findsNothing);
      expect(find.text('ok'), findsOneWidget);
    });
  });
}
