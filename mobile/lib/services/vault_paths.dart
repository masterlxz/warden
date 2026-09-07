import 'dart:io';

import 'package:path/path.dart' as p;
import 'package:path_provider/path_provider.dart';

/// Fase 4.4 — where this device's local vault + sync tracking state live. Mirrors the desktop's
/// use of the OS config dir (`desktop/src-tauri/src/lib.rs::sync_secrets_path`/
/// `sync_manifest_path`, `warden_sync::paths`) rather than a user-visible Documents folder:
/// `getApplicationSupportDirectory()` is the Flutter/`path_provider` equivalent — private to the
/// app, excluded from iCloud/Google auto-backup, not shown to the user in a file browser.
///
/// `config.toml` is never read or written by the mobile app itself (it has no providers/agents of
/// its own — it's a pure `warden-server` client) — it only exists here as an opaque sync target,
/// same as the CLI/desktop treat any file that isn't theirs. `SyncEngine` tolerates it being
/// absent (`diff::config_changed` reads it via `Option`), so there's nothing to create upfront.
class VaultPaths {
  const VaultPaths({
    required this.vaultRoot,
    required this.configPath,
    required this.secretsPath,
    required this.manifestPath,
  });

  final String vaultRoot;
  final String configPath;
  final String secretsPath;
  final String manifestPath;

  static VaultPaths? _cached;

  static Future<VaultPaths> resolve() async {
    final cached = _cached;
    if (cached != null) return cached;

    final supportDir = await getApplicationSupportDirectory();
    final vaultDir = Directory(p.join(supportDir.path, 'vault'));
    await vaultDir.create(recursive: true);

    final resolved = VaultPaths(
      vaultRoot: vaultDir.path,
      configPath: p.join(supportDir.path, 'config.toml'),
      secretsPath: p.join(supportDir.path, 'sync_secrets.json'),
      manifestPath: p.join(supportDir.path, 'sync_manifest.json'),
    );
    _cached = resolved;
    return resolved;
  }
}
