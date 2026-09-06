import 'dart:convert';

import 'package:saf_stream/saf_stream.dart';
import 'package:saf_util/saf_util.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Read-only file access on the phone (Fase 7.4) — the model's `list_phone_files`/`read_phone_file` tools,
/// scoped to one root folder the user picks once (Settings-style, mirrors the desktop's
/// `vault_path`: no per-call picker interrupting the model mid-turn).
///
/// Built on `saf_util`/`saf_stream` (Android's Storage Access Framework) rather than the older
/// `shared_storage` package — that one is discontinued with no successor listed on pub.dev, and
/// tracing its own community fork (`mg_shared_storage`) led here: `saf_util`/`saf_stream` are the
/// actively maintained pair from the same author for exactly this (pick+persist a directory,
/// list it, read file bytes). Android only — no iOS equivalent exists for SAF, matching this
/// app's current Android-only testing posture (`PENDING.md` P39).
class MobileFileTool {
  static const _keyRootUri = 'mobileFileTool.rootUri';

  final _safUtil = SafUtil();
  final _safStream = SafStream();

  /// Opens the system folder picker and persists the chosen folder's URI (grants a persistable
  /// read permission, so it survives app restarts/device reboots) — `null` if the user cancelled.
  Future<String?> pickRootFolder() async {
    final picked = await _safUtil.pickDirectory(persistablePermission: true);
    if (picked == null) return null;
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_keyRootUri, picked.uri);
    return picked.uri;
  }

  Future<String?> rootFolderUri() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_keyRootUri);
  }

  Future<void> clearRootFolder() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.remove(_keyRootUri);
  }

  /// `list_phone_files` tool handler. `args['path']` is either empty/absent (list the configured root)
  /// or a URI a previous `list_phone_files` call already returned as an entry's `path` — the model
  /// never constructs a path itself, it only ever echoes one back, same "opaque handle" spirit
  /// MCP resource URIs already use.
  Future<Map<String, dynamic>> listFiles(Map<String, dynamic> args) async {
    final root = await rootFolderUri();
    if (root == null) {
      throw StateError('No folder configured — pick one in the Files settings first');
    }
    final path = args['path'] as String?;
    final targetUri = (path == null || path.isEmpty) ? root : path;

    final entries = await _safUtil.list(targetUri);
    return {
      'entries': entries
          .map((e) => {
                'name': e.name,
                'type': e.isDir ? 'directory' : 'file',
                'path': e.uri,
              })
          .toList(),
    };
  }

  /// `read_phone_file` tool handler. `args['path']` must be a `path` from a `list_phone_files` entry.
  /// Non-UTF-8 content (binary files) fails clearly rather than garbling bytes into the chat.
  Future<Map<String, dynamic>> readFile(Map<String, dynamic> args) async {
    final path = args['path'] as String?;
    if (path == null || path.isEmpty) {
      throw ArgumentError("missing required 'path' argument");
    }
    final bytes = await _safStream.readFileBytes(path);
    try {
      return {'content': utf8.decode(bytes)};
    } on FormatException {
      throw StateError("'$path' is not a readable text file (not valid UTF-8)");
    }
  }

  /// Wire shape sent in `Hello.tools` (Fase 7.4) — mirrors
  /// `crates/warden-core/src/tool/file_tools.rs`'s `ToolSpec` shape, scoped to this tool's
  /// opaque-path semantics rather than the vault's human-readable relative paths.
  static const listFilesSpec = {
    'name': 'list_phone_files',
    'description': 'List files and folders under the configured root folder on the phone. '
        "Omit 'path' to list the root; pass a 'path' from a previous list_phone_files entry to list a subfolder.",
    'parameters': {
      'type': 'object',
      'properties': {
        'path': {'type': 'string', 'description': "A 'path' from a previous list_phone_files entry, or omitted for the root"},
      },
    },
  };

  static const readFileSpec = {
    'name': 'read_phone_file',
    'description': 'Read the text content of a file on the phone by the path returned from list_phone_files. '
        'Only works for text files, not binary/media files.',
    'parameters': {
      'type': 'object',
      'properties': {
        'path': {'type': 'string', 'description': "A 'path' from a list_phone_files entry"},
      },
      'required': ['path'],
    },
  };
}
