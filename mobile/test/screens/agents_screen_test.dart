import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/screens/agents_screen.dart';
import 'package:mobile/services/agent_work.dart';
import 'package:mobile/services/server_connection.dart' show HubRequestException;

/// A hub in memory: it holds the agents and the tasks, checks the pairing key like the real one, and records
/// what the screen asked for.
class _FakeBackend implements AgentsBackend {
  _FakeBackend({
    List<AgentInfo> agents = const [],
    List<ModelPolicy> policies = const [],
    this.modelIds = const [],
    List<AgentTask> tasks = const [],
    List<ActivityEvent> events = const [],
  })  : agents = List.of(agents),
        policies = List.of(policies),
        tasks = List.of(tasks),
        events = List.of(events);

  static const pairingKey = 'right-key';

  List<AgentInfo> agents;
  List<ModelPolicy> policies;
  final List<String> modelIds;
  List<AgentTask> tasks;
  List<ActivityEvent> events;
  final edits = <OrgEdit>[];
  final controls = <String>[];

  @override
  Future<HubAgents> listHubAgents() async => HubAgents(List.of(agents), List.of(policies), modelIds);

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
      case SetDelegationModelsEdit(:final id, :final models):
        agents = [
          for (final a in agents)
            if (a.id == id) AgentInfo(id: a.id, role: a.role, reportsTo: a.reportsTo, canDelegateToAgents: a.canDelegateToAgents, delegationModels: models) else a,
        ];
      case SetModelPoliciesEdit(policies: final next):
        policies = List.of(next);
    }
    return HubAgents(List.of(agents), List.of(policies), modelIds);
  }

  @override
  Future<List<AgentTask>> listAgentTasks() async => List.of(tasks);

  @override
  Future<List<ActivityEvent>> listActivity() async => List.of(events);

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

Future<void> _pump(
  WidgetTester tester,
  _FakeBackend backend, {
  void Function(String)? onOpenChat,
  void Function(String)? onOpenConversation,
  void Function(String)? onOpenChannel,
}) async {
  // Pushed like in the chat, so "Chat" on a node has a screen to leave.
  await tester.pumpWidget(
    MaterialApp(
      home: Builder(
        builder: (context) => Scaffold(
          body: ElevatedButton(
            key: const Key('open-agents'),
            onPressed: () => Navigator.of(context).push(
              MaterialPageRoute(
                builder: (_) => AgentsScreen(
                  backend: backend,
                  onOpenChat: onOpenChat,
                  onOpenConversation: onOpenConversation,
                  onOpenChannel: onOpenChannel,
                  refreshEvery: null,
                ),
              ),
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
      expect(find.byKey(const Key('new-policy')), findsOneWidget, reason: 'policies are edited here too, but a hub with no model offers none to answer one');
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

  group('activity of each node', () {
    testWidgets('shows what each agent has been up to, from the hub\'s tasks, and nothing for one with no task', (tester) async {
      final backend = _FakeBackend(
        agents: [const AgentInfo(id: 'chief'), const AgentInfo(id: 'dev', reportsTo: 'chief'), const AgentInfo(id: 'idle', reportsTo: 'chief')],
        tasks: [_task('dev', 'done', owner: 'chief')],
      );
      await _pump(tester, backend);
      expect(find.byKey(const Key('activity-dev')), findsOneWidget);
      expect(find.textContaining('1 done'), findsOneWidget);
      expect(find.byKey(const Key('activity-chief')), findsOneWidget);
      expect(find.textContaining('delegated 1'), findsOneWidget);
      expect(find.byKey(const Key('activity-idle')), findsNothing);
    });
  });

  group('model limit and policies', () {
    const delegating = AgentInfo(id: 'chief', canDelegateToAgents: true);
    const fast = ModelPolicy(id: 'fast', model: 'main', description: 'simple work');

    testWidgets('limiting an agent to some models sends them with the first checked as the default', (tester) async {
      final backend = _FakeBackend(agents: [delegating, const AgentInfo(id: 'dev', reportsTo: 'chief')], policies: [fast], modelIds: ['main', 'spare']);
      await _pump(tester, backend);
      expect(find.text('Models it can pick'), findsNothing, reason: 'the menu is closed');
      await _openMenu(tester, 'chief', 'Models it can pick');
      await tester.tap(find.byKey(const Key('limit-spare')));
      await tester.tap(find.byKey(const Key('limit-fast')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('limit-save')));
      await tester.pumpAndSettle();
      await _typeKey(tester, _FakeBackend.pairingKey);

      final edit = backend.edits.single as SetDelegationModelsEdit;
      expect(edit.id, 'chief');
      expect(edit.models, ['spare', 'fast']);
      expect(find.textContaining('Limited to 2 models; spare is what'), findsOneWidget);
    });

    testWidgets('only an agent that can delegate has the models entry', (tester) async {
      await _pump(tester, _FakeBackend(agents: [const AgentInfo(id: 'dev')], modelIds: ['main']));
      await tester.tap(find.byKey(const Key('menu-dev')));
      await tester.pumpAndSettle();
      expect(find.text('Models it can pick'), findsNothing);
    });

    testWidgets('a new policy is named, answered by a model and saved with the key', (tester) async {
      final backend = _FakeBackend(agents: [delegating], modelIds: ['main', 'spare']);
      await _pump(tester, backend);
      await tester.ensureVisible(find.byKey(const Key('new-policy')));
      await tester.tap(find.byKey(const Key('new-policy')));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('policy-name-field')), ' cheap ');
      await tester.enterText(find.byKey(const Key('policy-description-field')), 'quick and cheap');
      await tester.pump();
      await tester.tap(find.byKey(const Key('policy-save')));
      await tester.pumpAndSettle();
      await _typeKey(tester, _FakeBackend.pairingKey);

      final edit = backend.edits.single as SetModelPoliciesEdit;
      expect([for (final p in edit.policies) (p.id, p.model, p.description)], [('cheap', 'main', 'quick and cheap')]);
      expect(find.text('cheap → main'), findsOneWidget);
    });

    testWidgets('a policy can be removed, and the list says so when none is left', (tester) async {
      final backend = _FakeBackend(agents: [delegating], policies: [fast], modelIds: ['main']);
      await _pump(tester, backend);
      await tester.ensureVisible(find.byKey(const Key('policy-menu-fast')));
      await tester.tap(find.byKey(const Key('policy-menu-fast')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Remove').last);
      await tester.pumpAndSettle();
      await _typeKey(tester, _FakeBackend.pairingKey);

      expect((backend.edits.single as SetModelPoliciesEdit).policies, isEmpty);
      expect(find.textContaining('None: an agent that delegates'), findsOneWidget);
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

  group('activity', () {
    final now = DateTime.now().millisecondsSinceEpoch;
    ActivityEvent event(String id, String kind, String actor, {String? target, String text = '', String? taskId, String? conversationId, int ago = 0}) =>
        ActivityEvent(id: id, atMs: now - ago, kind: kind, actor: actor, target: target, text: text, taskId: taskId, conversationId: conversationId);

    final events = [
      event('e3', 'messaged_user', 'pirate', text: 'the disk is full', conversationId: 'channel-1'),
      event('e2', 'note', 'ana', target: 'bia', text: 'review this?', conversationId: 'agents-1', ago: 1000),
      event('e1', 'delegated', 'chief', target: 'dev', text: 'build it', taskId: 'at-1', ago: 2000),
    ];

    Future<void> openTab(WidgetTester tester) async {
      await tester.tap(find.byKey(const Key('tab-activity')));
      await tester.pumpAndSettle();
    }

    testWidgets('lists the events under today, newest first, with their sentence', (tester) async {
      await _pump(tester, _FakeBackend(events: events));
      await openTab(tester);
      expect(find.text('Today'), findsOneWidget);
      expect(find.text('pirate wrote to you'), findsOneWidget);
      expect(find.text('ana left a note for bia'), findsOneWidget);
      expect(find.text('chief delegated a task to dev'), findsOneWidget);
      expect(tester.getTopLeft(find.byKey(const Key('activity-e3'))).dy, lessThan(tester.getTopLeft(find.byKey(const Key('activity-e1'))).dy));
    });

    testWidgets('says so when there is nothing yet', (tester) async {
      await _pump(tester, _FakeBackend());
      await openTab(tester);
      expect(find.textContaining('Nothing yet.'), findsOneWidget);
    });

    testWidgets('the filter keeps the events an agent did or received', (tester) async {
      await _pump(tester, _FakeBackend(events: events));
      await openTab(tester);
      await tester.tap(find.byKey(const Key('activity-filter')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('bia').last);
      await tester.pumpAndSettle();
      expect(find.text('ana left a note for bia'), findsOneWidget);
      expect(find.text('pirate wrote to you'), findsNothing);
    });

    testWidgets('a task event opens the tasks of the agent, narrowed to it', (tester) async {
      await _pump(tester, _FakeBackend(events: events, tasks: [_task('dev', 'done'), _task('other', 'done')]));
      await openTab(tester);
      await tester.tap(find.byKey(const Key('activity-e1')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('show-all-tasks')), findsOneWidget);
    });

    testWidgets('a note opens its conversation and a message the agent started opens its channel, leaving the screen', (tester) async {
      final opened = <String>[];
      await _pump(tester, _FakeBackend(events: events), onOpenConversation: (id) => opened.add('conversation:$id'), onOpenChannel: (agent) => opened.add('channel:$agent'));
      await openTab(tester);
      await tester.tap(find.byKey(const Key('activity-e2')));
      await tester.pumpAndSettle();
      expect(opened, ['conversation:agents-1']);
      expect(find.byKey(const Key('open-agents')), findsOneWidget, reason: 'the agents screen closed');

      await tester.tap(find.byKey(const Key('open-agents')));
      await tester.pumpAndSettle();
      await openTab(tester);
      await tester.tap(find.byKey(const Key('activity-e3')));
      await tester.pumpAndSettle();
      expect(opened, ['conversation:agents-1', 'channel:pirate']);
    });
  });
}
