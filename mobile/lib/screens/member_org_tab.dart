import 'dart:async';

import 'package:flutter/material.dart';

import '../protocol/messages.dart';
import '../services/agent_work.dart';
import '../services/server_connection.dart' show HubRequestException;
import 'agents_screen.dart' show NewAgentDialog, PositionDialog;

/// P120 — the organization tab of a member of the hub. The tree is the owner's, and what the member does with it is the
/// access the owner gave them: none (a note and nothing else), view (the tree, with no way to change it) or edit (a role and a
/// superior, a new report, a removal — by their own session, with no pairing key). It starts from the access the hub sent
/// at sign-in and follows the hub, which says so the moment the owner saves a new level; it also asks again when the tab opens,
/// when the app comes back to the front and on a pull (a change made in the config file is not pushed). The hub checks the access
/// at every request anyway. The agents' powers are not part of what a
/// member sees, and neither are their chats and tasks (the owner's agents are not always theirs).
class MemberOrganizationTab extends StatefulWidget {
  const MemberOrganizationTab({super.key, required this.backend, this.initialAccess = OrgAccess.none});

  final MemberOrgBackend backend;

  /// The access the hub sent at sign-in; what the tab shows until it has asked.
  final OrgAccess initialAccess;

  @override
  State<MemberOrganizationTab> createState() => _MemberOrganizationTabState();
}

class _MemberOrganizationTabState extends State<MemberOrganizationTab> with WidgetsBindingObserver {
  late OrgAccess _access = widget.initialAccess;
  List<AgentInfo>? _agents;
  String? _error;
  bool _busy = false;

  StreamSubscription<String>? _told;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _told = widget.backend.orgAccessChanges.listen(_onTold);
    _load();
  }

  @override
  void dispose() {
    _told?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  /// The hub says the owner just changed the access: take it at once. Gaining it needs the tree, so it is read; losing it drops the
  /// tree and any message about a refused edit.
  void _onTold(String access) {
    if (!mounted) return;
    final next = orgAccessOf(access);
    setState(() {
      _access = next;
      if (next == OrgAccess.none) {
        _agents = null;
        _error = null;
      }
    });
    if (next != OrgAccess.none) _load(quiet: true);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) _load(quiet: true);
  }

  /// Reads the tree and, with it, the access. The hub's refusal is "none"; a timeout or a closed connection says nothing about
  /// the access, so it leaves what the tab shows and (unless [quiet]) says so. [quiet] never touches the message on screen.
  Future<void> _load({bool quiet = false}) async {
    try {
      final org = await widget.backend.listAgentOrg();
      if (!mounted) return;
      setState(() {
        _agents = org.agents;
        _access = orgAccessOf(org.access);
        if (!quiet) _error = null;
      });
    } on HubRequestException {
      if (mounted) {
        setState(() {
          _access = OrgAccess.none;
          _agents = null;
        });
      }
    } catch (e) {
      if (mounted && !quiet) setState(() => _error = e.toString());
    }
  }

  /// Applies [edit] and shows the tree as the hub holds it afterwards. A refusal (a circle, a name already taken, the access
  /// taken back) is shown as the hub words it, and the access is looked at again.
  Future<void> _apply(OrgEdit edit) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final org = await widget.backend.editAgentOrgAsMember(edit);
      if (mounted) {
        setState(() {
          _agents = org.agents;
          _access = orgAccessOf(org.access);
        });
      }
    } on HubRequestException catch (e) {
      if (mounted) setState(() => _error = e.message);
      await _load(quiet: true);
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _edit(AgentInfo agent, List<AgentInfo> all) async {
    final picked = await showDialog<({String role, String superior})>(
      context: context,
      builder: (_) => PositionDialog(agent: agent, choices: superiorChoices(all, agent.id)),
    );
    if (picked == null || !mounted) return;
    await _apply(SetPositionEdit(agent.id, role: picked.role, reportsTo: picked.superior));
  }

  Future<void> _add(String? under) async {
    final picked = await showDialog<({String id, String role, String persona})>(
      context: context,
      builder: (_) => NewAgentDialog(under: under),
    );
    if (picked == null || !mounted) return;
    await _apply(AddReportEdit(picked.id, picked.persona, role: picked.role, reportsTo: under));
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
    await _apply(RemoveAgentEdit(agent.id));
  }

  List<Widget> _rows(List<OrgNode> nodes, List<AgentInfo> all, int depth) {
    final rows = <Widget>[];
    for (final node in nodes) {
      final agent = node.agent;
      rows.add(
        Padding(
          padding: EdgeInsets.only(left: depth * 16.0),
          child: Card(
            key: Key('member-agent-${agent.id}'),
            child: ListTile(
              title: Text(agent.id),
              subtitle: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (agent.role != null && agent.role!.isNotEmpty) Text(agent.role!),
                  if (node.children.isNotEmpty) Text(node.children.length == 1 ? '1 report' : '${node.children.length} reports', style: Theme.of(context).textTheme.bodySmall),
                ],
              ),
              trailing: _access != OrgAccess.edit
                  ? null
                  : PopupMenuButton<String>(
                      key: Key('member-menu-${agent.id}'),
                      enabled: !_busy,
                      onSelected: (action) {
                        switch (action) {
                          case 'edit':
                            _edit(agent, all);
                          case 'add':
                            _add(agent.id);
                          case 'remove':
                            _remove(node);
                        }
                      },
                      itemBuilder: (_) => const [
                        PopupMenuItem(value: 'edit', child: Text('Edit position')),
                        PopupMenuItem(value: 'add', child: Text('Add a report')),
                        PopupMenuItem(value: 'remove', child: Text('Remove')),
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
    final errorStyle = TextStyle(color: Theme.of(context).colorScheme.error);
    if (_access == OrgAccess.none) {
      return RefreshIndicator(
        onRefresh: _load,
        child: ListView(
          padding: const EdgeInsets.all(16),
          children: [
            const Text(
              key: Key('member-org-none'),
              "The workspace's owner hasn't given you access to the organization of the agents.",
            ),
            if (_error != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(_error!, style: errorStyle)),
          ],
        ),
      );
    }
    final agents = _agents;
    if (agents == null) {
      return Center(
        child: _error != null ? Padding(padding: const EdgeInsets.all(16), child: Text(_error!, style: errorStyle)) : const CircularProgressIndicator(),
      );
    }
    final tree = buildOrg(agents);
    final editable = _access == OrgAccess.edit;
    return RefreshIndicator(
      onRefresh: _load,
      child: ListView(
        padding: const EdgeInsets.all(12),
        children: [
          Text(
            editable
                ? "Who reports to whom among the workspace's agents. The owner let you change the hierarchy: the role and the superior of an agent, a new report, a removal. What the agents may do is the owner's."
                : "Who reports to whom among the workspace's agents. You can look at the tree; changing it is the owner's.",
            key: const Key('member-org-hint'),
            style: Theme.of(context).textTheme.bodySmall,
          ),
          if (_error != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(_error!, style: errorStyle)),
          const SizedBox(height: 8),
          if (tree.isEmpty) const Text('No agents yet.') else ..._rows(tree, agents, 0),
          if (editable)
            TextButton.icon(
              key: const Key('member-add-top'),
              onPressed: _busy ? null : () => _add(null),
              icon: const Icon(Icons.add),
              label: const Text('Add an agent at the top'),
            ),
        ],
      ),
    );
  }
}
