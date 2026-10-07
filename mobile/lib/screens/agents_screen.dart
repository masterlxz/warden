import 'dart:async';

import 'package:flutter/material.dart';

import '../protocol/messages.dart';
import '../services/agent_work.dart';
import '../services/server_connection.dart' show HubRequestException;

const _autonomy = {1: 'only answers', 2: 'suggests', 3: 'asks first', 4: 'acts alone'};

/// P120, P123 — the organization of the agents and the work they hand each other, on the phone: the tree (a role and a
/// superior for each agent, add or remove one, open a chat with it or its tasks) and the background tasks (state, progress,
/// cost, pause, resume, stop). Every change asks for the hub's pairing key, which is never kept. The model policies and the
/// models limit of each agent are edited here too, through the same narrow edit as the tree. Mirrors the web's two screens.
class AgentsScreen extends StatefulWidget {
  const AgentsScreen({
    super.key,
    required this.backend,
    this.onOpenChat,
    this.refreshEvery = const Duration(seconds: 3),
  });

  final AgentsBackend backend;

  /// Starts a conversation with this agent; the screen closes after it. Null hides the action.
  final void Function(String agentId)? onOpenChat;

  /// How often the task list reloads while it is open. Null never reloads (tests).
  final Duration? refreshEvery;

  @override
  State<AgentsScreen> createState() => _AgentsScreenState();
}

class _AgentsScreenState extends State<AgentsScreen> with SingleTickerProviderStateMixin {
  late final TabController _tabs = TabController(length: 2, vsync: this);

  /// The agent whose tasks the Tasks tab shows (null: all).
  String? _taskAgent;

  @override
  void dispose() {
    _tabs.dispose();
    super.dispose();
  }

  void _openTasks(String agent) {
    setState(() => _taskAgent = agent);
    _tabs.animateTo(1);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Agents'),
        bottom: TabBar(
          controller: _tabs,
          tabs: const [Tab(key: Key('tab-organization'), text: 'Organization'), Tab(key: Key('tab-tasks'), text: 'Tasks')],
        ),
      ),
      body: TabBarView(
        controller: _tabs,
        children: [
          OrganizationTab(
            backend: widget.backend,
            onOpenChat: widget.onOpenChat == null
                ? null
                : (id) {
                    Navigator.of(context).pop();
                    widget.onOpenChat!(id);
                  },
            onOpenTasks: _openTasks,
          ),
          AgentTasksTab(
            backend: widget.backend,
            agent: _taskAgent,
            onClearAgent: () => setState(() => _taskAgent = null),
            refreshEvery: widget.refreshEvery,
          ),
        ],
      ),
    );
  }
}

/// Asks for the pairing key and runs [submit] with it. A wrong key keeps the dialog open with a message; any other failure
/// closes it and goes to [onError]. Resolves true when [submit] succeeded.
Future<bool> askPairingKey(
  BuildContext context, {
  required String title,
  required String message,
  required Future<void> Function(String pairingKey) submit,
  required void Function(String error) onError,
}) async {
  final done = await showDialog<bool>(
    context: context,
    barrierDismissible: false,
    builder: (_) => _PairingKeyDialog(title: title, message: message, submit: submit, onError: onError),
  );
  return done ?? false;
}

class _PairingKeyDialog extends StatefulWidget {
  const _PairingKeyDialog({required this.title, required this.message, required this.submit, required this.onError});

  final String title;
  final String message;
  final Future<void> Function(String pairingKey) submit;
  final void Function(String error) onError;

  @override
  State<_PairingKeyDialog> createState() => _PairingKeyDialogState();
}

class _PairingKeyDialogState extends State<_PairingKeyDialog> {
  final _controller = TextEditingController();
  String? _keyError;
  bool _busy = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  Future<void> _confirm() async {
    setState(() {
      _busy = true;
      _keyError = null;
    });
    try {
      await widget.submit(_controller.text);
      if (mounted) Navigator.of(context).pop(true);
    } on HubRequestException catch (e) {
      if (!mounted) return;
      if (e.authRejected) {
        setState(() {
          _busy = false;
          _keyError = 'Wrong pairing key.';
        });
      } else {
        Navigator.of(context).pop(false);
        widget.onError(e.message);
      }
    } catch (e) {
      if (!mounted) return;
      Navigator.of(context).pop(false);
      widget.onError(e.toString());
    }
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.title),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(widget.message),
          const SizedBox(height: 12),
          TextField(
            key: const Key('pairing-key-field'),
            controller: _controller,
            autofocus: true,
            obscureText: true,
            enabled: !_busy,
            decoration: InputDecoration(labelText: "Hub's pairing key", helperText: 'Asked on every change; never kept.', errorText: _keyError),
            onSubmitted: (_) => _controller.text.trim().isEmpty || _busy ? null : _confirm(),
          ),
        ],
      ),
      actions: [
        TextButton(onPressed: _busy ? null : () => Navigator.of(context).pop(false), child: const Text('Cancel')),
        FilledButton(
          key: const Key('pairing-key-confirm'),
          onPressed: _busy ? null : () => _controller.text.trim().isEmpty ? null : _confirm(),
          child: Text(_busy ? 'Wait…' : 'Confirm'),
        ),
      ],
    );
  }
}

// ---- Organization ----

/// The role and the superior of an agent. Owns its controller, so it is disposed only after the dialog's exit
/// animation (disposing it right after `showDialog` returns would break that animation).
class _PositionDialog extends StatefulWidget {
  const _PositionDialog({required this.agent, required this.choices});

  final AgentInfo agent;
  final List<AgentInfo> choices;

  @override
  State<_PositionDialog> createState() => _PositionDialogState();
}

class _PositionDialogState extends State<_PositionDialog> {
  late final TextEditingController _role = TextEditingController(text: widget.agent.role ?? '');
  late String _superior = widget.choices.any((a) => a.id == widget.agent.reportsTo) ? widget.agent.reportsTo! : '';

  @override
  void dispose() {
    _role.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text('Position of ${widget.agent.id}'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            key: const Key('role-field'),
            controller: _role,
            maxLength: 80,
            decoration: const InputDecoration(labelText: 'Role', hintText: 'e.g. backend lead'),
          ),
          DropdownButtonFormField<String>(
            key: const Key('superior-field'),
            initialValue: _superior,
            decoration: InputDecoration(labelText: 'Reports to', helperText: 'Whoever reports to ${widget.agent.id} goes with it.'),
            items: [
              const DropdownMenuItem(value: '', child: Text('Nobody (top of the tree)')),
              for (final a in widget.choices) DropdownMenuItem(value: a.id, child: Text(a.id)),
            ],
            onChanged: (value) => setState(() => _superior = value ?? ''),
          ),
        ],
      ),
      actions: [
        TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Cancel')),
        FilledButton(onPressed: () => Navigator.of(context).pop((role: _role.text, superior: _superior)), child: const Text('Save')),
      ],
    );
  }
}

/// The models an agent may pick when it delegates (P123): checked, with a default (the first). Nothing checked leaves the choice open.
class _LimitDialog extends StatefulWidget {
  const _LimitDialog({required this.agent, required this.candidates, required this.policyIds});

  final AgentInfo agent;
  final List<String> candidates;
  final Set<String> policyIds;

  @override
  State<_LimitDialog> createState() => _LimitDialogState();
}

class _LimitDialogState extends State<_LimitDialog> {
  late final Set<String> _checked = {...widget.agent.delegationModels};
  late String _default = widget.agent.delegationModels.isEmpty ? '' : widget.agent.delegationModels.first;

  List<String> get _chosen => [
        for (final id in widget.candidates)
          if (_checked.contains(id)) id,
      ];

  @override
  Widget build(BuildContext context) {
    final chosen = _chosen;
    final shownDefault = chosen.contains(_default) ? _default : (chosen.isEmpty ? '' : chosen.first);
    return AlertDialog(
      title: Text('Models of ${widget.agent.id}'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'The models it can pick for the tasks it delegates. None checked leaves the choice open; with one, it dictates the model.',
              style: Theme.of(context).textTheme.bodySmall,
            ),
            if (widget.candidates.isEmpty) const Text('The hub has no providers or policies to pick from.'),
            for (final id in widget.candidates)
              CheckboxListTile(
                key: Key('limit-$id'),
                dense: true,
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                title: Text(widget.policyIds.contains(id) ? '$id (policy)' : id),
                value: _checked.contains(id),
                onChanged: (on) => setState(() => on == true ? _checked.add(id) : _checked.remove(id)),
              ),
            if (chosen.length > 1)
              DropdownButtonFormField<String>(
                key: const Key('limit-default'),
                initialValue: shownDefault,
                decoration: InputDecoration(labelText: 'Default', helperText: 'What a task gets when ${widget.agent.id} does not pick.'),
                items: [for (final id in chosen) DropdownMenuItem(value: id, child: Text(id))],
                onChanged: (value) => setState(() => _default = value ?? ''),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Cancel')),
        FilledButton(
          key: const Key('limit-save'),
          onPressed: () => Navigator.of(context).pop(limitModels(widget.candidates, _checked, shownDefault)),
          child: const Text('Save'),
        ),
      ],
    );
  }
}

/// A model policy (P123): a name an agent that delegates can say, what answers it, and when to pick it. Owns its controllers, so they are
/// disposed after the dialog's exit animation.
class _PolicyDialog extends StatefulWidget {
  const _PolicyDialog({required this.original, required this.modelIds, required this.initialId});

  final ModelPolicy? original;
  final List<String> modelIds;
  final String initialId;

  @override
  State<_PolicyDialog> createState() => _PolicyDialogState();
}

class _PolicyDialogState extends State<_PolicyDialog> {
  late final TextEditingController _id = TextEditingController(text: widget.initialId);
  late final TextEditingController _description = TextEditingController(text: widget.original?.description ?? '');
  late String _model = widget.modelIds.contains(widget.original?.model) ? widget.original!.model : (widget.modelIds.isEmpty ? '' : widget.modelIds.first);

  @override
  void dispose() {
    _id.dispose();
    _description.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final ready = _id.text.trim().isNotEmpty && _model.isNotEmpty;
    return AlertDialog(
      title: Text(widget.original == null ? 'New policy' : 'Policy ${widget.original!.id}'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              key: const Key('policy-name-field'),
              controller: _id,
              maxLength: 64,
              decoration: const InputDecoration(labelText: 'Name'),
              onChanged: (_) => setState(() {}),
            ),
            DropdownButtonFormField<String>(
              key: const Key('policy-model-field'),
              initialValue: _model.isEmpty ? null : _model,
              decoration: const InputDecoration(labelText: 'Answered by'),
              items: [for (final id in widget.modelIds) DropdownMenuItem(value: id, child: Text(id))],
              onChanged: (value) => setState(() => _model = value ?? ''),
            ),
            TextField(
              key: const Key('policy-description-field'),
              controller: _description,
              maxLength: 200,
              decoration: const InputDecoration(labelText: 'When to pick it (one line)', hintText: 'e.g. simple, cheap work'),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Cancel')),
        FilledButton(
          key: const Key('policy-save'),
          onPressed: ready ? () => Navigator.of(context).pop(ModelPolicy(id: _id.text, model: _model, description: _description.text)) : null,
          child: const Text('Save policy'),
        ),
      ],
    );
  }
}

/// A new agent: name, role and what it does. Starts careful (the hub decides what that means).
class _NewAgentDialog extends StatefulWidget {
  const _NewAgentDialog({required this.under});

  final String? under;

  @override
  State<_NewAgentDialog> createState() => _NewAgentDialogState();
}

class _NewAgentDialogState extends State<_NewAgentDialog> {
  final _id = TextEditingController();
  final _role = TextEditingController();
  final _persona = TextEditingController();

  @override
  void dispose() {
    _id.dispose();
    _role.dispose();
    _persona.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final ready = _id.text.trim().isNotEmpty && _persona.text.trim().isNotEmpty;
    return AlertDialog(
      title: Text(widget.under == null ? 'New agent at the top' : 'New agent under ${widget.under}'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              key: const Key('new-name-field'),
              controller: _id,
              maxLength: 64,
              decoration: const InputDecoration(labelText: 'Name', hintText: 'e.g. reviewer'),
              onChanged: (_) => setState(() {}),
            ),
            TextField(
              key: const Key('new-role-field'),
              controller: _role,
              maxLength: 80,
              decoration: const InputDecoration(labelText: 'Role (optional)'),
            ),
            TextField(
              key: const Key('new-persona-field'),
              controller: _persona,
              minLines: 2,
              maxLines: 5,
              decoration: const InputDecoration(
                labelText: 'What it does',
                helperText: 'Starts careful: read-only tools, asks before any change, and cannot delegate or manage agents until a person turns that on.',
                helperMaxLines: 4,
              ),
              onChanged: (_) => setState(() {}),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Cancel')),
        FilledButton(
          key: const Key('new-agent-confirm'),
          onPressed: ready ? () => Navigator.of(context).pop((id: _id.text, role: _role.text, persona: _persona.text)) : null,
          child: const Text('Add agent'),
        ),
      ],
    );
  }
}

class OrganizationTab extends StatefulWidget {
  const OrganizationTab({super.key, required this.backend, required this.onOpenTasks, this.onOpenChat});

  final AgentsBackend backend;
  final void Function(String agentId) onOpenTasks;
  final void Function(String agentId)? onOpenChat;

  @override
  State<OrganizationTab> createState() => _OrganizationTabState();
}

class _OrganizationTabState extends State<OrganizationTab> {
  HubAgents? _hub;
  String? _error;

  /// The hub's tasks, for the activity line of each node (an extra: without them the card just shows no line).
  List<AgentTask> _tasks = const [];

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final hub = await widget.backend.listHubAgents();
      if (mounted) {
        setState(() {
          _hub = hub;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
    try {
      final tasks = await widget.backend.listAgentTasks();
      if (mounted) setState(() => _tasks = tasks);
    } catch (_) {
      // The activity is an extra.
    }
  }

  /// What [agent] has been up to, on one line; null with no task of its own.
  String? _activity(String agent) {
    final activity = activityOf(_tasks, agent);
    return activity == null ? null : activityLine(activity, DateTime.now().millisecondsSinceEpoch);
  }

  /// Asks for the key and applies [edit]; the screen shows what the hub holds afterwards.
  Future<void> _apply(OrgEdit edit, String what) async {
    setState(() => _error = null);
    await askPairingKey(
      context,
      title: what,
      message: 'The hub restarts with the change.',
      submit: (key) async {
        final hub = await widget.backend.editAgentOrg(key, edit);
        if (mounted) setState(() => _hub = hub);
      },
      onError: (error) {
        if (mounted) setState(() => _error = error);
      },
    );
  }

  Future<void> _edit(AgentInfo agent, List<AgentInfo> all) async {
    final picked = await showDialog<({String role, String superior})>(
      context: context,
      builder: (_) => _PositionDialog(agent: agent, choices: superiorChoices(all, agent.id)),
    );
    if (picked == null || !mounted) return;
    await _apply(SetPositionEdit(agent.id, role: picked.role, reportsTo: picked.superior), 'Change ${agent.id}');
  }

  Future<void> _add(String? under) async {
    final picked = await showDialog<({String id, String role, String persona})>(
      context: context,
      builder: (_) => _NewAgentDialog(under: under),
    );
    if (picked == null || !mounted) return;
    await _apply(AddReportEdit(picked.id, picked.persona, role: picked.role, reportsTo: under), 'Add ${picked.id}');
  }

  /// The models [agent] may pick when it delegates (P123).
  Future<void> _limit(AgentInfo agent) async {
    final hub = _hub;
    if (hub == null) return;
    final picked = await showDialog<List<String>>(
      context: context,
      builder: (_) => _LimitDialog(agent: agent, candidates: delegationCandidates(hub.modelIds, hub.modelPolicies), policyIds: {for (final p in hub.modelPolicies) p.id}),
    );
    if (picked == null || !mounted) return;
    await _apply(SetDelegationModelsEdit(agent.id, picked), 'Models of ${agent.id}');
  }

  /// A model policy (P123): [original] is the one being edited, null for a new one.
  Future<void> _policy(ModelPolicy? original) async {
    final hub = _hub;
    if (hub == null) return;
    final picked = await showDialog<ModelPolicy>(
      context: context,
      builder: (_) => _PolicyDialog(
        original: original,
        modelIds: hub.modelIds,
        initialId: original?.id ?? nextPolicyId(delegationCandidates(hub.modelIds, hub.modelPolicies)),
      ),
    );
    if (picked == null || !mounted) return;
    await _apply(SetModelPoliciesEdit(policiesWith(hub.modelPolicies, picked, original?.id)), 'Save the policy ${picked.id.trim()}');
  }

  Future<void> _removePolicy(ModelPolicy policy) async {
    final hub = _hub;
    if (hub == null) return;
    await _apply(SetModelPoliciesEdit(policiesWithout(hub.modelPolicies, policy.id)), 'Remove the policy ${policy.id}');
  }

  Future<void> _remove(OrgNode node) async {
    final agent = node.agent;
    final count = node.children.length;
    final goesTo = agent.reportsTo ?? 'nobody (the top of the tree)';
    final ok = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text('Remove ${agent.id}?'),
        content: Text(
          '${count == 0 ? '' : '${count == 1 ? 'Its report goes' : 'Its $count reports go'} to report to $goesTo. '}'
          'The conversations stay; this can\'t be undone.',
        ),
        actions: [
          TextButton(onPressed: () => Navigator.of(dialogContext).pop(false), child: const Text('Keep')),
          FilledButton(onPressed: () => Navigator.of(dialogContext).pop(true), child: const Text('Remove')),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    await _apply(RemoveAgentEdit(agent.id), 'Remove ${agent.id}');
  }

  List<String> _badges(AgentInfo a) => [
        if (a.canDelegateToAgents) 'delegates',
        if (a.canManageAgents) 'manages agents',
        if (a.canMessageAgents) 'leaves notes',
        if (a.canManageTasks) 'schedules tasks',
        if (a.autonomy != 4) 'autonomy ${a.autonomy}: ${_autonomy[a.autonomy] ?? ''}'.trim(),
        if (a.approvalRequired.isNotEmpty) 'asks first: ${a.approvalRequired.length} kind${a.approvalRequired.length > 1 ? 's' : ''} of action',
      ];

  List<Widget> _rows(List<OrgNode> nodes, List<AgentInfo> all, int depth) {
    final rows = <Widget>[];
    for (final node in nodes) {
      final agent = node.agent;
      rows.add(
        Padding(
          padding: EdgeInsets.only(left: depth * 16.0),
          child: Card(
            key: Key('agent-${agent.id}'),
            child: ListTile(
              title: Text(agent.id),
              subtitle: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (agent.role != null && agent.role!.isNotEmpty) Text(agent.role!),
                  if (_badges(agent).isNotEmpty) Wrap(spacing: 4, children: [for (final b in _badges(agent)) Chip(label: Text(b), visualDensity: VisualDensity.compact)]),
                  if (agent.canDelegateToAgents) Text(delegationSummary(agent.delegationModels), style: Theme.of(context).textTheme.bodySmall),
                  if (node.children.isNotEmpty) Text(node.children.length == 1 ? '1 report' : '${node.children.length} reports', style: Theme.of(context).textTheme.bodySmall),
                  if (_activity(agent.id) != null) Text(_activity(agent.id)!, key: Key('activity-${agent.id}'), style: Theme.of(context).textTheme.bodySmall),
                ],
              ),
              isThreeLine: true,
              trailing: PopupMenuButton<String>(
                key: Key('menu-${agent.id}'),
                onSelected: (action) {
                  switch (action) {
                    case 'chat':
                      widget.onOpenChat?.call(agent.id);
                    case 'tasks':
                      widget.onOpenTasks(agent.id);
                    case 'edit':
                      _edit(agent, all);
                    case 'models':
                      _limit(agent);
                    case 'add':
                      _add(agent.id);
                    case 'remove':
                      _remove(node);
                  }
                },
                itemBuilder: (_) => [
                  if (widget.onOpenChat != null) const PopupMenuItem(value: 'chat', child: Text('Chat')),
                  const PopupMenuItem(value: 'tasks', child: Text('Tasks')),
                  const PopupMenuItem(value: 'edit', child: Text('Edit position')),
                  if (agent.canDelegateToAgents) const PopupMenuItem(value: 'models', child: Text('Models it can pick')),
                  const PopupMenuItem(value: 'add', child: Text('Add a report')),
                  const PopupMenuItem(value: 'remove', child: Text('Remove')),
                ],
              ),
            ),
          ),
        ),
      );
      rows.addAll(_rows(node.children, all, depth + 1));
    }
    return rows;
  }

  @override
  Widget build(BuildContext context) {
    final hub = _hub;
    if (hub == null) {
      return Center(
        child: _error != null
            ? Padding(padding: const EdgeInsets.all(16), child: Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)))
            : const CircularProgressIndicator(),
      );
    }
    final tree = buildOrg(hub.agents);
    return RefreshIndicator(
      onRefresh: _load,
      child: ListView(
        padding: const EdgeInsets.all(12),
        children: [
          Text(
            'Who reports to whom. An agent that manages or delegates to others reaches only the ones below it. '
            'Anything an agent changes still waits for your yes.',
            style: Theme.of(context).textTheme.bodySmall,
          ),
          if (_error != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error))),
          const SizedBox(height: 8),
          if (tree.isEmpty) const Text('No agents yet.') else ..._rows(tree, hub.agents, 0),
          if (tree.isNotEmpty && tree.every((n) => n.children.isEmpty))
            const Padding(padding: EdgeInsets.symmetric(vertical: 8), child: Text('Nobody reports to anybody yet: use "Edit position" on an agent to pick its superior.')),
          TextButton.icon(key: const Key('add-top'), onPressed: () => _add(null), icon: const Icon(Icons.add), label: const Text('Add an agent at the top')),
          const Divider(),
          Text('Model policies', style: Theme.of(context).textTheme.titleSmall),
          if (hub.modelPolicies.isEmpty) const Text('None: an agent that delegates sees only the model ids.'),
          for (final p in hub.modelPolicies)
            ListTile(
              key: Key('policy-${p.id}'),
              dense: true,
              contentPadding: EdgeInsets.zero,
              title: Text('${p.id} → ${p.model}'),
              subtitle: p.description == null || p.description!.isEmpty ? null : Text(p.description!),
              trailing: PopupMenuButton<String>(
                key: Key('policy-menu-${p.id}'),
                onSelected: (action) => action == 'edit' ? _policy(p) : _removePolicy(p),
                itemBuilder: (_) => const [
                  PopupMenuItem(value: 'edit', child: Text('Edit')),
                  PopupMenuItem(value: 'remove', child: Text('Remove')),
                ],
              ),
            ),
          TextButton.icon(
            key: const Key('new-policy'),
            onPressed: hub.modelIds.isEmpty ? null : () => _policy(null),
            icon: const Icon(Icons.add),
            label: const Text('New policy'),
          ),
        ],
      ),
    );
  }
}

// ---- Tasks ----

String _clock(int ms) {
  final t = DateTime.fromMillisecondsSinceEpoch(ms);
  String two(int n) => n.toString().padLeft(2, '0');
  return '${two(t.day)}/${two(t.month)} ${two(t.hour)}:${two(t.minute)}';
}

class AgentTasksTab extends StatefulWidget {
  const AgentTasksTab({super.key, required this.backend, required this.agent, required this.onClearAgent, this.refreshEvery});

  final AgentsBackend backend;
  final String? agent;
  final VoidCallback onClearAgent;
  final Duration? refreshEvery;

  @override
  State<AgentTasksTab> createState() => _AgentTasksTabState();
}

class _AgentTasksTabState extends State<AgentTasksTab> {
  List<AgentTask>? _tasks;
  String? _error;
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _load();
    final every = widget.refreshEvery;
    if (every != null) _timer = Timer.periodic(every, (_) => _load());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final tasks = await widget.backend.listAgentTasks();
      if (mounted) {
        setState(() {
          _tasks = tasks;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  Future<void> _control(AgentTask task, String action) async {
    setState(() => _error = null);
    await askPairingKey(
      context,
      title: '${taskActionLabel[action]} the task of ${task.assignee}',
      message: action == 'cancel' ? 'Its subtasks stop too.' : 'Only a task running on this hub can be controlled.',
      submit: (key) async {
        final tasks = await widget.backend.controlAgentTask(key, task.id, action);
        if (mounted) setState(() => _tasks = tasks);
      },
      onError: (error) {
        if (mounted) setState(() => _error = error);
      },
    );
  }

  Widget _header(TaskGroup g) {
    final c = g.counts;
    final parts = [
      if (c['done']! > 0) '${c['done']} done',
      if (c['running']! > 0) '${c['running']} running',
      if (c['waiting']! > 0) '${c['waiting']} waiting for an agent',
      if (c['paused']! > 0) '${c['paused']} paused',
      if (c['pending']! > 0) '${c['pending']} pending',
      if (c['failed']! > 0) '${c['failed']} failed',
      if (c['cancelled']! > 0) '${c['cancelled']} stopped',
    ];
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 12, 4, 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(children: [
            Text(g.owner ?? 'An agent', style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(width: 8),
            Text(_clock(g.createdAtMs), style: Theme.of(context).textTheme.bodySmall),
            if (g.active) ...[const SizedBox(width: 8), const Text('working')],
          ]),
          const SizedBox(height: 4),
          LinearProgressIndicator(value: g.percent / 100),
          const SizedBox(height: 4),
          Text(
            '${g.finished} of ${g.total} finished (${g.percent}%) · ${parts.join(', ')}${g.totalTokens > 0 ? ' · ${formatTokens(g.totalTokens)} tokens' : ''}',
            style: Theme.of(context).textTheme.bodySmall,
          ),
        ],
      ),
    );
  }

  Widget _row(TaskRow row) {
    final task = row.task;
    final duration = durationLabel(task, DateTime.now().millisecondsSinceEpoch);
    final detail = task.state == 'done' ? task.result : task.error;
    final meta = [
      if (task.model != null) task.model!,
      ?duration,
      if (task.totalTokens != null) '${formatTokens(task.totalTokens!)} tok',
    ].join(' · ');
    return Padding(
      padding: EdgeInsets.only(left: row.depth * 16.0),
      child: Card(
        key: Key('task-${task.id}'),
        child: ExpansionTile(
          leading: Text(taskStateMark[task.state] ?? taskStateMark['pending']!, semanticsLabel: taskStateLabel[task.state]),
          title: Text(task.assignee),
          subtitle: Text(
            [task.objective, if (meta.isNotEmpty) meta].join('\n'),
            maxLines: 3,
            overflow: TextOverflow.ellipsis,
          ),
          childrenPadding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
          expandedCrossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Task', style: Theme.of(context).textTheme.labelSmall),
            SelectableText(task.objective),
            const SizedBox(height: 8),
            Text(task.state == 'done' ? 'Result' : 'Why it stopped', style: Theme.of(context).textTheme.labelSmall),
            detail != null && detail.isNotEmpty ? SelectableText(detail) : Text('${taskStateLabel[task.state] ?? task.state}: no result yet.'),
            Wrap(
              spacing: 8,
              children: [
                for (final action in actionsFor(task))
                  TextButton(key: Key('task-${task.id}-$action'), onPressed: () => _control(task, action), child: Text(taskActionLabel[action]!)),
              ],
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final tasks = _tasks;
    final agent = widget.agent;
    if (tasks == null) {
      return Center(
        child: _error != null
            ? Padding(padding: const EdgeInsets.all(16), child: Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)))
            : const CircularProgressIndicator(),
      );
    }
    final groups = groupTasks(agent == null ? tasks : involvingAgent(tasks, agent));
    return RefreshIndicator(
      onRefresh: _load,
      child: ListView(
        padding: const EdgeInsets.all(12),
        children: [
          Text(
            'Tasks the agents handed each other in the background on this hub, where each one is and what it cost.',
            style: Theme.of(context).textTheme.bodySmall,
          ),
          if (agent != null)
            Row(children: [
              Expanded(child: Text('Only the tasks of $agent: the ones it received and the ones it delegated.', style: Theme.of(context).textTheme.bodySmall)),
              TextButton(key: const Key('show-all-tasks'), onPressed: widget.onClearAgent, child: const Text('Show all')),
            ]),
          if (_error != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error))),
          if (groups.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 16),
              child: Text(agent != null ? 'No task involves $agent yet.' : 'Nothing yet. When an agent delegates a task with "background", it shows up here.'),
            ),
          for (final g in groups) ...[_header(g), for (final row in g.rows) _row(row)],
        ],
      ),
    );
  }
}
