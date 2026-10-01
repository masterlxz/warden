import 'package:flutter/material.dart';

import '../services/skills_repository.dart';

/// P72 (b) — lists, creates, edits and deletes the skills stored in this device's local vault
/// (`skills/<name>.md`, see `SkillsRepository`). Mirrors the desktop `SkillsView.tsx` minus the
/// "describe it and the AI drafts it" box: the phone is a pure `warden-server` client with no model
/// of its own to ask. Skills written here reach the desktop/server through the normal vault sync
/// (`SyncScreen`), and the model only sees the ones in the vault it runs against.
class SkillsScreen extends StatefulWidget {
  const SkillsScreen({super.key, this.repository = const BridgeSkillsRepository()});

  final SkillsRepository repository;

  @override
  State<SkillsScreen> createState() => _SkillsScreenState();
}

class _SkillsScreenState extends State<SkillsScreen> {
  List<SkillDto>? _skills;
  String? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final skills = await widget.repository.list();
      if (mounted) {
        setState(() {
          _skills = skills;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  Future<void> _openForm({SkillDto? existing}) async {
    final saved = await Navigator.of(context).push<bool>(
      MaterialPageRoute(builder: (_) => SkillFormScreen(repository: widget.repository, existing: existing)),
    );
    if (saved == true) await _load();
  }

  Future<void> _confirmReject(SkillDto skill) => _confirmDelete(skill, verb: 'Reject');

  Future<void> _confirmDelete(SkillDto skill, {String verb = 'Delete'}) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text("$verb '${skill.name}'?"),
        content: const Text('This removes the skill file and can\'t be undone.'),
        actions: [
          TextButton(onPressed: () => Navigator.of(context).pop(false), child: const Text('Cancel')),
          TextButton(onPressed: () => Navigator.of(context).pop(true), child: Text(verb)),
        ],
      ),
    );
    if (confirmed != true) return;
    try {
      await widget.repository.delete(skill.name);
      await _load();
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  /// Accepting is an explicit save with `proposed: false` — the bridge keeps a suggestion pending on
  /// any other save, so editing never activates it by accident.
  Future<void> _accept(SkillDto skill) async {
    try {
      await widget.repository.save(
        SkillDto(name: skill.name, description: skill.description, body: skill.body, proposed: false),
        overwrite: true,
      );
      await _load();
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  Widget _suggestedCard(SkillDto skill) {
    final origin = skill.source;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(child: Text(skill.name, style: Theme.of(context).textTheme.titleMedium)),
                const Chip(label: Text('Suggested'), visualDensity: VisualDensity.compact),
              ],
            ),
            if (skill.description.isNotEmpty) Padding(padding: const EdgeInsets.only(top: 4), child: Text(skill.description)),
            if (origin != null && origin.isNotEmpty) Padding(padding: const EdgeInsets.only(top: 4), child: Text('From: $origin', style: Theme.of(context).textTheme.bodySmall)),
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Wrap(
                spacing: 8,
                children: [
                  FilledButton(onPressed: () => _accept(skill), child: const Text('Accept')),
                  OutlinedButton(onPressed: () => _openForm(existing: skill), child: const Text('Edit')),
                  TextButton(onPressed: () => _confirmReject(skill), child: const Text('Reject')),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final skills = _skills;
    final suggested = (skills ?? const <SkillDto>[]).where((s) => s.proposed).toList();
    final active = (skills ?? const <SkillDto>[]).where((s) => !s.proposed).toList();
    return Scaffold(
      appBar: AppBar(title: const Text('Skills')),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => _openForm(),
        icon: const Icon(Icons.add),
        label: const Text('New skill'),
      ),
      body: _error != null && skills == null
          ? Padding(padding: const EdgeInsets.all(16), child: Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)))
          : skills == null
              ? const Center(child: CircularProgressIndicator())
              : skills.isEmpty
                  ? const Center(
                      child: Padding(
                        padding: EdgeInsets.all(24),
                        child: Text(
                          'No skills yet. Create one here, or ask the AI to write one in chat — they sync with the rest of the vault.',
                          textAlign: TextAlign.center,
                        ),
                      ),
                    )
                  : ListView(
                      padding: const EdgeInsets.fromLTRB(16, 8, 16, 88),
                      children: [
                        if (_error != null)
                          Padding(
                            padding: const EdgeInsets.only(bottom: 8),
                            child: Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
                          ),
                        if (suggested.isNotEmpty) ...[
                          Padding(
                            padding: const EdgeInsets.only(top: 4, bottom: 4),
                            child: Text('Suggested by the AI', style: Theme.of(context).textTheme.titleSmall),
                          ),
                          const Padding(
                            padding: EdgeInsets.only(bottom: 8),
                            child: Text('Not active yet — the AI only uses a suggestion after you accept it.'),
                          ),
                          for (final skill in suggested) _suggestedCard(skill),
                          const SizedBox(height: 16),
                          if (active.isNotEmpty) Text('Your skills', style: Theme.of(context).textTheme.titleSmall),
                        ],
                        for (final skill in active)
                          Card(
                            child: ListTile(
                              title: Text(skill.name),
                              subtitle: Text(skill.description.isEmpty ? '(no description)' : skill.description, maxLines: 2, overflow: TextOverflow.ellipsis),
                              onTap: () => _openForm(existing: skill),
                              trailing: IconButton(
                                icon: const Icon(Icons.delete_outline),
                                tooltip: 'Delete ${skill.name}',
                                onPressed: () => _confirmDelete(skill),
                              ),
                            ),
                          ),
                      ],
                    ),
    );
  }
}

/// New/edit form. `existing == null` is "create" (name editable, refuses a taken name); otherwise
/// the name — which is the filename — is locked and saving overwrites. Pops `true` once saved.
class SkillFormScreen extends StatefulWidget {
  const SkillFormScreen({super.key, required this.repository, this.existing});

  final SkillsRepository repository;
  final SkillDto? existing;

  @override
  State<SkillFormScreen> createState() => _SkillFormScreenState();
}

class _SkillFormScreenState extends State<SkillFormScreen> {
  late final _name = TextEditingController(text: widget.existing?.name ?? '');
  late final _description = TextEditingController(text: widget.existing?.description ?? '');
  late final _body = TextEditingController(text: widget.existing?.body ?? '');
  bool _saving = false;
  String? _error;
  // Editing a suggestion keeps it pending unless this is switched on.
  bool _accept = false;

  bool get _isEdit => widget.existing != null;
  bool get _isSuggestion => widget.existing?.proposed ?? false;

  @override
  void dispose() {
    _name.dispose();
    _description.dispose();
    _body.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      await widget.repository.save(
        SkillDto(
          name: _name.text.trim(),
          description: _description.text,
          body: _body.text,
          proposed: _isSuggestion && !_accept,
          source: widget.existing?.source,
          proposedAt: widget.existing?.proposedAt,
        ),
        overwrite: _isEdit,
      );
      if (mounted) Navigator.of(context).pop(true);
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: Text(_isEdit ? 'Edit skill' : 'New skill')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          TextField(
            controller: _name,
            enabled: !_isEdit,
            autocorrect: false,
            decoration: const InputDecoration(
              labelText: 'Name',
              helperText: 'Lowercase letters, digits and hyphens — it becomes the file name.',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 16),
          TextField(
            controller: _description,
            maxLength: 300,
            decoration: const InputDecoration(
              labelText: 'Description',
              helperText: 'When to use it — the AI reads this every turn.',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 16),
          TextField(
            controller: _body,
            minLines: 8,
            maxLines: null,
            keyboardType: TextInputType.multiline,
            decoration: const InputDecoration(
              labelText: 'Instructions',
              alignLabelWithHint: true,
              border: OutlineInputBorder(),
            ),
          ),
          if (_isSuggestion)
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Accept this skill'),
              subtitle: const Text('Off: it stays a suggestion and the AI still ignores it.'),
              value: _accept,
              onChanged: (v) => setState(() => _accept = v),
            ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
            ),
          const SizedBox(height: 16),
          FilledButton(onPressed: _saving ? null : _save, child: Text(_saving ? 'Saving…' : 'Save')),
        ],
      ),
    );
  }
}
