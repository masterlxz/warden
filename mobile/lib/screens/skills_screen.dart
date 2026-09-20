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

  Future<void> _confirmDelete(SkillDto skill) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text("Delete '${skill.name}'?"),
        content: const Text('This removes the skill file and can\'t be undone.'),
        actions: [
          TextButton(onPressed: () => Navigator.of(context).pop(false), child: const Text('Cancel')),
          TextButton(onPressed: () => Navigator.of(context).pop(true), child: const Text('Delete')),
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

  @override
  Widget build(BuildContext context) {
    final skills = _skills;
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
                        for (final skill in skills)
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

  bool get _isEdit => widget.existing != null;

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
        SkillDto(name: _name.text.trim(), description: _description.text, body: _body.text),
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
