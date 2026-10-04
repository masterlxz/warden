import 'package:flutter/material.dart';

import '../protocol/messages.dart';

/// P102 — the last segment of a folder path, for the chip ("/srv/work/alpha" → "alpha").
String folderName(String path) {
  final parts = path.split('/').where((p) => p.isNotEmpty);
  return parts.isEmpty ? path : parts.last;
}

typedef ListDirs = Future<DirListMessage> Function([String? path]);

/// Opens the folder browser as a bottom sheet and returns the folder picked, or null when it was cancelled.
/// [initialPath] is where it opens (the current choice); a folder that is gone falls back to the start.
Future<String?> showFolderPicker(BuildContext context, {required ListDirs listDirs, String? initialPath}) {
  return showModalBottomSheet<String>(
    context: context,
    isScrollControlled: true,
    useSafeArea: true,
    builder: (_) => FolderPicker(listDirs: listDirs, initialPath: initialPath),
  );
}

/// Browses the folders of the hub's machine to choose the one a conversation works in (P102). The hub lists
/// folders only; a member sees just the ones the owner allowed, so at the top their list has no path to pick.
class FolderPicker extends StatefulWidget {
  const FolderPicker({super.key, required this.listDirs, this.initialPath});

  final ListDirs listDirs;
  final String? initialPath;

  @override
  State<FolderPicker> createState() => _FolderPickerState();
}

class _FolderPickerState extends State<FolderPicker> {
  DirListMessage? _listing;
  String? _error;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    _openInitial();
  }

  /// Opens once. A remembered folder that is gone falls back to the start instead of an error.
  Future<void> _openInitial() async {
    try {
      DirListMessage listing;
      try {
        listing = await widget.listDirs(widget.initialPath);
      } catch (_) {
        if (widget.initialPath == null) rethrow;
        listing = await widget.listDirs();
      }
      if (!mounted) return;
      setState(() {
        _listing = listing;
        _loading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _loading = false;
      });
    }
  }

  Future<void> _open(String? path) async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final listing = await widget.listDirs(path);
      if (!mounted) return;
      setState(() {
        _listing = listing;
        _loading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final listing = _listing;
    final here = listing?.path ?? '';
    final parent = listing?.parent;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Working folder', style: Theme.of(context).textTheme.titleLarge),
            const SizedBox(height: 4),
            Text(
              'The AI reads and writes in this folder, and the shell starts there (each command asks for your yes). '
              'It applies to the whole conversation and does not change afterwards.',
              style: Theme.of(context).textTheme.bodySmall,
            ),
            const SizedBox(height: 12),
            Text(here.isEmpty ? 'Your folders' : here, key: const Key('folder-here'), style: const TextStyle(fontFamily: 'monospace')),
            if (_error != null) ...[
              const SizedBox(height: 8),
              Text(_error!, key: const Key('folder-error'), style: TextStyle(color: Theme.of(context).colorScheme.error)),
            ],
            const SizedBox(height: 8),
            Flexible(
              child: _loading && listing == null
                  ? const Center(child: Padding(padding: EdgeInsets.all(24), child: CircularProgressIndicator()))
                  : ListView(
                      shrinkWrap: true,
                      children: [
                        if (parent != null)
                          ListTile(
                            key: const Key('folder-up'),
                            leading: const Icon(Icons.arrow_upward),
                            title: const Text('Up'),
                            enabled: !_loading,
                            // An empty parent is the top of a member's list.
                            onTap: () => _open(parent.isEmpty ? null : parent),
                          ),
                        for (final dir in listing?.dirs ?? const <DirEntry>[])
                          ListTile(
                            leading: const Icon(Icons.folder_outlined),
                            title: Text(dir.name),
                            enabled: !_loading,
                            onTap: () => _open(dir.path),
                          ),
                        if (listing != null && listing.dirs.isEmpty && !_loading)
                          const Padding(padding: EdgeInsets.all(12), child: Text('No subfolders.', key: Key('folder-empty'))),
                      ],
                    ),
            ),
            const SizedBox(height: 8),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Cancel')),
                const SizedBox(width: 8),
                FilledButton(
                  key: const Key('folder-use'),
                  onPressed: here.isEmpty || _loading ? null : () => Navigator.of(context).pop(here),
                  child: const Text('Use this folder'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
