import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/services/sync_auto_pull.dart';

void main() {
  group('shouldAutoPullOnResume', () {
    test('resumed while already resumed does not pull again', () {
      expect(shouldAutoPullOnResume(AppLifecycleState.resumed, AppLifecycleState.resumed), isFalse);
    });

    test('transitioning away from resumed never pulls', () {
      for (final state in AppLifecycleState.values) {
        if (state == AppLifecycleState.resumed) continue;
        expect(
          shouldAutoPullOnResume(AppLifecycleState.resumed, state),
          isFalse,
          reason: 'expected resumed -> $state not to pull',
        );
      }
    });

    test('every non-resumed state transitioning into resumed pulls', () {
      for (final state in AppLifecycleState.values) {
        if (state == AppLifecycleState.resumed) continue;
        expect(
          shouldAutoPullOnResume(state, AppLifecycleState.resumed),
          isTrue,
          reason: 'expected $state -> resumed to pull',
        );
      }
    });
  });

  group('autoPullMessageFor', () {
    test('nothing changed produces no message', () {
      final message = autoPullMessageFor(filesWritten: 0, filesDeleted: 0, configUpdated: false);
      expect(message, isNull);
    });

    test('only files written', () {
      final message = autoPullMessageFor(filesWritten: 3, filesDeleted: 0, configUpdated: false);
      expect(message, '3 arquivo(s) atualizado(s)');
    });

    test('only files deleted', () {
      final message = autoPullMessageFor(filesWritten: 0, filesDeleted: 2, configUpdated: false);
      expect(message, '2 arquivo(s) removido(s)');
    });

    test('only config updated', () {
      final message = autoPullMessageFor(filesWritten: 0, filesDeleted: 0, configUpdated: true);
      expect(message, 'config.toml atualizado');
    });

    test('all three combine in written, deleted, config order', () {
      final message = autoPullMessageFor(filesWritten: 1, filesDeleted: 2, configUpdated: true);
      expect(message, '1 arquivo(s) atualizado(s), 2 arquivo(s) removido(s), config.toml atualizado');
    });
  });
}
