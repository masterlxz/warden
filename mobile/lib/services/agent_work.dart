import '../protocol/messages.dart';

/// The agents as the hub's settings hold them, the model policies and the ids of the providers and combos (P120, P123).
class HubAgents {
  const HubAgents(this.agents, this.modelPolicies, [this.modelIds = const []]);

  final List<AgentInfo> agents;
  final List<ModelPolicy> modelPolicies;

  /// What a policy can be answered by.
  final List<String> modelIds;
}

/// What `AgentsScreen` needs from the hub — `ServerConnection` in the app, a fake in tests, like [ConversationBackend].
abstract interface class AgentsBackend {
  Future<HubAgents> listHubAgents();
  Future<HubAgents> editAgentOrg(String pairingKey, OrgEdit edit);
  Future<List<AgentTask>> listAgentTasks();
  Future<List<AgentTask>> controlAgentTask(String pairingKey, String taskId, String action);

  /// P121 — the feed of activity, newest first.
  Future<List<ActivityEvent>> listActivity();
}

/// What a member of the hub (P120) is allowed to do with the organization of the agents, as the owner set it.
enum OrgAccess { none, view, edit }

/// The access the hub sent (`orgAccess`, empty when it is none). A value this app doesn't know counts as none, so it never
/// shows a screen the hub will refuse.
OrgAccess orgAccessOf(String? value) => switch (value) {
      'view' => OrgAccess.view,
      'edit' => OrgAccess.edit,
      _ => OrgAccess.none,
    };

/// The tree as the hub shows it to a member: the owner's agents with only the id, the role and the superior, and the [access]
/// they have (`view` or `edit`).
class MemberOrg {
  const MemberOrg(this.agents, this.access);

  final List<AgentInfo> agents;
  final String access;
}

/// What the organization tab of a member needs from the hub — `ServerConnection` in the app, a fake in tests. The owner's
/// calls are in [AgentsBackend]; these two ask by the member's own session, with no pairing key.
abstract interface class MemberOrgBackend {
  /// Throws a `HubRequestException` when the owner gave no access, and a `ConversationException` when the hub can't be asked.
  Future<MemberOrg> listAgentOrg();
  Future<MemberOrg> editAgentOrgAsMember(OrgEdit edit);
}

// ---- P120: the organization tree. Mirrors `web/src/hub/org.ts`. ----

class OrgNode {
  const OrgNode(this.agent, this.children);

  final AgentInfo agent;
  final List<OrgNode> children;
}

/// The agents as a forest: the ones with no superior at the top, in the order they came, each with its reports under it.
/// An agent whose superior is gone is shown at the top rather than lost, and a circle (which the hub refuses) never loops.
List<OrgNode> buildOrg(List<AgentInfo> agents) {
  final ids = {for (final a in agents) a.id};
  final seen = <String>{};
  OrgNode nodeOf(AgentInfo agent) {
    seen.add(agent.id);
    final children = <OrgNode>[];
    for (final report in agents) {
      if (report.reportsTo == agent.id && !seen.contains(report.id)) children.add(nodeOf(report));
    }
    return OrgNode(agent, children);
  }

  return [
    for (final a in agents)
      if (a.reportsTo == null || a.reportsTo!.isEmpty || !ids.contains(a.reportsTo)) nodeOf(a),
  ];
}

/// Everyone below [id], at any level: who it can't come to report to.
Set<String> descendantsOf(List<AgentInfo> agents, String id) {
  final below = <String>{};
  final queue = [id];
  while (queue.isNotEmpty) {
    final current = queue.removeLast();
    for (final a in agents) {
      if (a.reportsTo == current && !below.contains(a.id)) {
        below.add(a.id);
        queue.add(a.id);
      }
    }
  }
  return below;
}

/// Who [id] may come to report to: everyone but itself and whoever is below it (that would close a circle).
List<AgentInfo> superiorChoices(List<AgentInfo> agents, String id) {
  final below = descendantsOf(agents, id);
  return [
    for (final a in agents)
      if (a.id != id && !below.contains(a.id)) a,
  ];
}

/// What the limit of models of an agent says, in a sentence for its card. Mirrors `delegationSummary` of the web.
String delegationSummary(List<String> models) {
  if (models.isEmpty) return 'Open: the agent picks any model for each task it delegates.';
  if (models.length == 1) return 'Dictated: every task it delegates runs on ${models.first}.';
  return 'Limited to ${models.length} models; ${models.first} is what a task gets when the agent does not pick.';
}

/// Everything an agent may be limited to: the providers and combos, then the policies.
List<String> delegationCandidates(List<String> modelIds, List<ModelPolicy> policies) => [
      for (final id in [...modelIds, ...policies.map((p) => p.id)])
        if (id.trim().isNotEmpty) id,
    ];

/// The list the limit form sends: the default first (when it is checked), then the others in the candidates' order. Nothing checked
/// leaves the choice open.
List<String> limitModels(List<String> candidates, Set<String> checked, String defaultModel) {
  final chosen = [
    for (final id in candidates)
      if (checked.contains(id)) id,
  ];
  if (!checked.contains(defaultModel)) return chosen;
  return [defaultModel, ...chosen.where((id) => id != defaultModel)];
}

/// The policies after saving [policy] (which was named [originalId], or null if new): replaces the old one in place, or adds it at the end.
/// The name and the description are trimmed.
List<ModelPolicy> policiesWith(List<ModelPolicy> policies, ModelPolicy policy, String? originalId) {
  final clean = ModelPolicy(id: policy.id.trim(), model: policy.model.trim(), description: (policy.description ?? '').trim());
  final at = originalId == null ? -1 : policies.indexWhere((p) => p.id == originalId);
  if (at < 0) return [...policies, clean];
  return [for (var i = 0; i < policies.length; i++) i == at ? clean : policies[i]];
}

List<ModelPolicy> policiesWithout(List<ModelPolicy> policies, String id) => [
      for (final p in policies)
        if (p.id != id) p,
    ];

/// The name a new policy starts with: `policy-1`, `policy-2`... free among providers, combos and policies.
String nextPolicyId(List<String> taken) {
  var n = 1;
  while (taken.contains('policy-$n')) {
    n += 1;
  }
  return 'policy-$n';
}

// ---- P123: the work agents delegate to each other. Mirrors `web/src/hub/agentTasks.ts`. ----

const taskStates = ['pending', 'running', 'waiting', 'paused', 'done', 'failed', 'cancelled'];

const taskStateLabel = {
  'pending': 'Pending',
  'running': 'Running',
  'waiting': 'Waiting for an agent',
  'paused': 'Paused',
  'done': 'Done',
  'failed': 'Failed',
  'cancelled': 'Stopped',
};

const taskStateMark = {
  'pending': '○',
  'running': '◐',
  'waiting': '◉',
  'paused': '⏸',
  'done': '✓',
  'failed': '⚠',
  'cancelled': '⏹',
};

const taskActionLabel = {'pause': 'Pause', 'resume': 'Resume', 'cancel': 'Stop'};

/// A task as the list shows it: how many levels below the agent of the turn it is (0 for the ones that agent started).
class TaskRow {
  const TaskRow(this.task, this.depth);

  final AgentTask task;
  final int depth;
}

class TaskGroup {
  const TaskGroup({
    required this.group,
    required this.owner,
    required this.tasks,
    required this.rows,
    required this.counts,
    required this.finished,
    required this.percent,
    required this.totalTokens,
    required this.active,
    required this.createdAtMs,
  });

  final String group;

  /// The agent that delegated, if any.
  final String? owner;

  /// Oldest first, in the order the turn started them.
  final List<AgentTask> tasks;

  /// The same tasks as a flattened tree: each followed by its subtasks.
  final List<TaskRow> rows;
  final Map<String, int> counts;

  /// Done, failed or stopped: nothing left to wait for.
  final int finished;
  final int percent;
  final int totalTokens;

  /// Something in the group is still pending, running, waiting or paused.
  final bool active;
  final int createdAtMs;

  int get total => tasks.length;
}

/// What can be done with this task now: only the one that runs in the hub process that answered. One that has not started
/// can only be stopped; a paused one can be resumed.
List<String> actionsFor(AgentTask task) {
  if (!task.controllable) return const [];
  return switch (task.state) {
    'pending' => const ['cancel'],
    'running' || 'waiting' => task.pausable ? const ['pause', 'cancel'] : const ['cancel'],
    'paused' => const ['resume', 'cancel'],
    _ => const [],
  };
}

List<TaskRow> _treeRows(List<AgentTask> ordered) {
  final ids = {for (final t in ordered) t.id};
  final rows = <TaskRow>[];
  final seen = <String>{};
  void add(AgentTask task, int depth) {
    seen.add(task.id);
    rows.add(TaskRow(task, depth));
    for (final child in ordered) {
      if (child.parentId == task.id && !seen.contains(child.id)) add(child, depth + 1);
    }
  }

  for (final t in ordered) {
    if (t.parentId == null || !ids.contains(t.parentId)) add(t, 0);
  }
  // A circle that should not exist: shown at the top rather than lost.
  for (final t in ordered) {
    if (!seen.contains(t.id)) add(t, 0);
  }
  return rows;
}

/// The tasks grouped by the turn that started them, newest group first. An unknown state counts as pending.
List<TaskGroup> groupTasks(List<AgentTask> tasks) {
  final byGroup = <String, List<AgentTask>>{};
  for (final task in tasks) {
    byGroup.putIfAbsent(task.group, () => []).add(task);
  }
  final groups = <TaskGroup>[];
  byGroup.forEach((group, members) {
    final ordered = [...members]..sort((a, b) => a.createdAtMs.compareTo(b.createdAtMs));
    final counts = {for (final s in taskStates) s: 0};
    for (final task in ordered) {
      final key = counts.containsKey(task.state) ? task.state : 'pending';
      counts[key] = counts[key]! + 1;
    }
    final finished = counts['done']! + counts['failed']! + counts['cancelled']!;
    final owner = ordered.where((t) => t.owner != null).map((t) => t.owner).firstOrNull;
    groups.add(TaskGroup(
      group: group,
      owner: owner,
      tasks: ordered,
      rows: _treeRows(ordered),
      counts: counts,
      finished: finished,
      percent: (finished / ordered.length * 100).round(),
      totalTokens: ordered.fold(0, (sum, t) => sum + (t.totalTokens ?? 0)),
      active: counts['pending']! + counts['running']! + counts['waiting']! + counts['paused']! > 0,
      createdAtMs: ordered.first.createdAtMs,
    ));
  });
  groups.sort((a, b) => b.createdAtMs.compareTo(a.createdAtMs));
  return groups;
}

/// The tasks that involve an agent: the ones it received and the ones it delegated (from a node of the tree).
List<AgentTask> involvingAgent(List<AgentTask> tasks, String agent) => [
      for (final t in tasks)
        if (t.assignee == agent || t.owner == agent) t,
    ];

/// What an agent has been up to, taken only from the tasks the hub keeps (P120): what it was given (`assignee`), how many it delegated
/// (`owner`) and when it last did anything. The log keeps only the most recent ones, and direct chats and notes are not in it.
class AgentActivity {
  const AgentActivity({
    required this.done,
    required this.failed,
    required this.cancelled,
    required this.active,
    required this.tokens,
    required this.delegated,
    required this.lastActiveMs,
  });

  /// Tasks it was given, by where they are.
  final int done;
  final int failed;
  final int cancelled;

  /// Pending, running, waiting for an agent or paused.
  final int active;

  /// Tokens of the tasks it was given that reported any.
  final int tokens;

  /// How many tasks it delegated to others.
  final int delegated;

  /// The last thing that happened to a task of its own (created, started or finished).
  final int lastActiveMs;
}

/// The activity of [agent], or null when no task involves it.
AgentActivity? activityOf(List<AgentTask> tasks, String agent) {
  final received = [
    for (final t in tasks)
      if (t.assignee == agent) t,
  ];
  final delegated = [
    for (final t in tasks)
      if (t.owner == agent) t,
  ];
  if (received.isEmpty && delegated.isEmpty) return null;
  final counts = {for (final s in taskStates) s: 0};
  for (final task in received) {
    final key = counts.containsKey(task.state) ? task.state : 'pending';
    counts[key] = counts[key]! + 1;
  }
  int stamp(AgentTask t) => t.finishedAtMs ?? t.startedAtMs ?? t.createdAtMs;
  return AgentActivity(
    done: counts['done']!,
    failed: counts['failed']!,
    cancelled: counts['cancelled']!,
    active: counts['pending']! + counts['running']! + counts['waiting']! + counts['paused']!,
    tokens: received.fold(0, (sum, t) => sum + (t.totalTokens ?? 0)),
    delegated: delegated.length,
    lastActiveMs: [...received, ...delegated].map(stamp).reduce((a, b) => a > b ? a : b),
  );
}

/// How long ago, in a few words: `just now`, `4 min ago`, `2 h ago`, `3 d ago`.
String agoLabel(int thenMs, int nowMs) {
  final elapsed = ((nowMs - thenMs) / 60000).round();
  final minutes = elapsed < 0 ? 0 : elapsed;
  if (minutes < 1) return 'just now';
  if (minutes < 60) return '$minutes min ago';
  final hours = minutes ~/ 60;
  if (hours < 24) return '$hours h ago';
  return '${hours ~/ 24} d ago';
}

/// The activity on one line of the tree card: `3 done, 1 failed, 2 running · 12.9k tokens · delegated 5 · active 4 min ago`.
String activityLine(AgentActivity a, int nowMs) {
  final parts = [
    if (a.done > 0) '${a.done} done',
    if (a.failed > 0) '${a.failed} failed',
    if (a.cancelled > 0) '${a.cancelled} stopped',
    if (a.active > 0) '${a.active} running',
  ];
  return [
    if (parts.isNotEmpty) parts.join(', '),
    if (a.tokens > 0) '${formatTokens(a.tokens)} tokens',
    if (a.delegated > 0) 'delegated ${a.delegated}',
    'active ${agoLabel(a.lastActiveMs, nowMs)}',
  ].join(' · ');
}

/// `950`, `12.9k`, `1.2M`.
String formatTokens(int n) {
  if (n < 1000) return '$n';
  String compact(double v) {
    final text = v.toStringAsFixed(1);
    return text.endsWith('.0') ? text.substring(0, text.length - 2) : text;
  }

  if (n < 1000000) return '${compact(n / 1000)}k';
  return '${compact(n / 1000000)}M';
}

/// How long a task took (or is taking): `4s`, `2m 05s`, `1h 03m`. Null before it starts.
String? durationLabel(AgentTask task, int nowMs) {
  final started = task.startedAtMs;
  if (started == null) return null;
  final end = task.finishedAtMs ?? nowMs;
  final elapsed = ((end - started) / 1000).round();
  final seconds = elapsed < 0 ? 0 : elapsed;
  if (seconds < 60) return '${seconds}s';
  final minutes = seconds ~/ 60;
  if (minutes < 60) return '${minutes}m ${(seconds % 60).toString().padLeft(2, '0')}s';
  return '${minutes ~/ 60}h ${(minutes % 60).toString().padLeft(2, '0')}m';
}
