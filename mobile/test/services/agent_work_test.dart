import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/services/agent_work.dart';

// P120, P123 — the pure half of the agents screen. Same cases as the web's and the extension's `org.test.mjs` and
// `agentTasks.test.mjs`.

AgentInfo _agent(String id, [String? reportsTo]) => AgentInfo(id: id, reportsTo: reportsTo);

AgentTask _task(
  String id,
  String group,
  String state, {
  String? owner = 'chief',
  String? parentId,
  int createdAtMs = 1000,
  int? totalTokens,
  int? startedAtMs,
  int? finishedAtMs,
  bool controllable = false,
  bool pausable = false,
}) =>
    AgentTask(
      id: id,
      group: group,
      owner: owner,
      assignee: id,
      parentId: parentId,
      objective: 'do $id',
      channel: 'cli',
      state: state,
      createdAtMs: createdAtMs,
      totalTokens: totalTokens,
      startedAtMs: startedAtMs,
      finishedAtMs: finishedAtMs,
      controllable: controllable,
      pausable: pausable,
    );

List<String> _ids(List<OrgNode> nodes) => [for (final n in nodes) n.agent.id];

void main() {
  group('the organization tree', () {
    test('puts the ones with no superior at the top, in order, with their reports under them', () {
      final tree = buildOrg([_agent('dev', 'boss'), _agent('boss'), _agent('solo'), _agent('qa', 'boss'), _agent('intern', 'dev')]);
      expect(_ids(tree), ['boss', 'solo']);
      expect(_ids(tree[0].children), ['dev', 'qa']);
      expect(_ids(tree[0].children[0].children), ['intern']);
    });

    test('shows an agent whose superior is gone at the top, and never loops on a circle', () {
      expect(_ids(buildOrg([_agent('orphan', 'ghost'), _agent('other')])), ['orphan', 'other']);
      expect(_ids(buildOrg([_agent('a', 'b'), _agent('b', 'a'), _agent('c')])), ['c']);
      expect(buildOrg([]), isEmpty);
    });

    test('an agent cannot be made to report to anyone below it', () {
      final agents = [_agent('boss'), _agent('lead', 'boss'), _agent('dev', 'lead'), _agent('other')];
      expect(descendantsOf(agents, 'boss'), {'lead', 'dev'});
      expect([for (final a in superiorChoices(agents, 'lead')) a.id], ['boss', 'other']);
      expect([for (final a in superiorChoices(agents, 'dev')) a.id], ['boss', 'lead', 'other']);
    });

    test('what the model limit says: open, dictated and limited', () {
      expect(delegationSummary([]), startsWith('Open'));
      expect(delegationSummary(['fast']), 'Dictated: every task it delegates runs on fast.');
      expect(delegationSummary(['fast', 'deep']), startsWith('Limited to 2 models; fast is what'));
    });
  });

  group('grouping the tasks of a turn', () {
    test('counts the finished ones as the progress and sums the tokens', () {
      final group = groupTasks([
        _task('backend', 'g1', 'done', totalTokens: 100, createdAtMs: 1),
        _task('frontend', 'g1', 'done', totalTokens: 50, createdAtMs: 2),
        _task('database', 'g1', 'running', createdAtMs: 3),
        _task('tests', 'g1', 'pending', createdAtMs: 4),
        _task('security', 'g1', 'failed', createdAtMs: 5),
      ]).single;
      expect(group.total, 5);
      expect(group.finished, 3);
      expect(group.percent, 60);
      expect(group.counts, {'pending': 1, 'running': 1, 'waiting': 0, 'paused': 0, 'done': 2, 'failed': 1, 'cancelled': 0});
      expect(group.totalTokens, 150);
      expect(group.active, isTrue);
      expect(group.owner, 'chief');
      expect([for (final t in group.tasks) t.id], ['backend', 'frontend', 'database', 'tests', 'security']);
    });

    test('a batch with nothing left to wait for is not active, and stopped counts as finished', () {
      final group = groupTasks([_task('a', 'g', 'done'), _task('b', 'g', 'cancelled')]).single;
      expect(group.active, isFalse);
      expect(group.percent, 100);
    });

    test('the newest group comes first and groups stay apart', () {
      final groups = groupTasks([
        _task('old', 'g1', 'done', createdAtMs: 10),
        _task('new', 'g2', 'running', createdAtMs: 99),
        _task('old2', 'g1', 'done', createdAtMs: 11),
      ]);
      expect([for (final g in groups) '${g.group}:${g.total}'], ['g2:1', 'g1:2']);
    });

    test('an unknown state counts as pending, and no tasks is no groups', () {
      expect(groupTasks([_task('a', 'g', 'some-new-state')]).single.counts['pending'], 1);
      expect(groupTasks([]), isEmpty);
    });

    test('a group with no delegating agent has no owner', () {
      expect(groupTasks([_task('a', 'g', 'done', owner: null)]).single.owner, isNull);
    });
  });

  group('subtasks', () {
    List<String> shape(TaskGroup g) => [for (final r in g.rows) '${r.task.id}@${r.depth}'];

    test('a task is followed by its subtasks, one level deeper, and the progress counts the whole tree', () {
      final group = groupTasks([
        _task('manager', 'g', 'waiting', createdAtMs: 1),
        _task('helper-a', 'g', 'done', parentId: 'manager', createdAtMs: 2),
        _task('helper-b', 'g', 'running', parentId: 'manager', createdAtMs: 3),
        _task('other', 'g', 'done', createdAtMs: 4),
        _task('deep', 'g', 'pending', parentId: 'helper-b', createdAtMs: 5),
      ]).single;
      expect(shape(group), ['manager@0', 'helper-a@1', 'helper-b@1', 'deep@2', 'other@0']);
      expect(group.total, 5);
      expect(group.finished, 2);
      expect(group.counts['waiting'], 1);
      expect(group.active, isTrue, reason: 'a task waiting for an agent is still active');
    });

    test('a subtask whose parent is missing is shown at the top, and a loop does not hang', () {
      expect(shape(groupTasks([_task('orphan', 'g', 'done', parentId: 'ghost')]).single), ['orphan@0']);
      final loop = groupTasks([
        _task('a', 'g', 'done', parentId: 'b', createdAtMs: 1),
        _task('b', 'g', 'done', parentId: 'a', createdAtMs: 2),
      ]).single;
      expect(loop.rows.length, 2, reason: 'both are shown once');
    });
  });

  group('how a task is shown', () {
    test('tokens are compact', () {
      expect(formatTokens(950), '950');
      expect(formatTokens(12900), '12.9k');
      expect(formatTokens(2000), '2k');
      expect(formatTokens(1200000), '1.2M');
    });

    test('the duration is empty before it starts, and counts to now while it runs', () {
      expect(durationLabel(_task('a', 'g', 'pending'), 99999), isNull);
      expect(durationLabel(_task('a', 'g', 'running', startedAtMs: 1000), 5000), '4s');
      expect(durationLabel(_task('a', 'g', 'done', startedAtMs: 0, finishedAtMs: 125000), 999999), '2m 05s');
      expect(durationLabel(_task('a', 'g', 'done', startedAtMs: 0, finishedAtMs: 3780000), 0), '1h 03m');
    });
  });

  group('the tasks of one agent', () {
    test('it keeps the ones the agent was given and the ones it delegated, and nobody else\'s', () {
      final tasks = [
        _task('backend', 'g1', 'done', owner: 'chief'),
        _task('frontend', 'g1', 'done', owner: 'chief'),
        _task('db', 'g1', 'running', owner: 'backend', parentId: 'backend'),
        _task('docs', 'g2', 'done', owner: 'writer'),
      ];
      expect([for (final t in involvingAgent(tasks, 'backend')) t.id], ['backend', 'db']);
      expect([for (final t in involvingAgent(tasks, 'chief')) t.id], ['backend', 'frontend']);
      expect(involvingAgent(tasks, 'ghost'), isEmpty);
    });
  });

  group('what a person can do to a task', () {
    AgentTask mine(String state) => _task('a', 'g', state, controllable: true, pausable: true);

    test('only a task running in the answering process can be controlled, and the actions follow its state', () {
      expect(actionsFor(mine('pending')), ['cancel']);
      expect(actionsFor(mine('running')), ['pause', 'cancel']);
      expect(actionsFor(mine('waiting')), ['pause', 'cancel']);
      expect(actionsFor(mine('paused')), ['resume', 'cancel']);
      for (final state in ['done', 'failed', 'cancelled']) {
        expect(actionsFor(mine(state)), isEmpty);
      }
      expect(actionsFor(_task('a', 'g', 'running')), isEmpty, reason: 'a task of another process has no controls');
    });

    test('a delegation the agent is waiting on can only be stopped', () {
      AgentTask waitedOn(String state) => _task('a', 'g', state, controllable: true);
      expect(actionsFor(waitedOn('running')), ['cancel']);
      expect(actionsFor(waitedOn('waiting')), ['cancel']);
    });

    test('a paused task still counts as active work', () {
      final group = groupTasks([_task('a', 'g', 'paused'), _task('b', 'g', 'done')]).single;
      expect(group.active, isTrue);
      expect(group.counts['paused'], 1);
      expect(group.finished, 1);
    });
  });
}
