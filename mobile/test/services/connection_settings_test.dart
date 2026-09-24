import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/services/connection_settings.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('useTls round-trips through the store (P36)', () async {
    final store = ConnectionSettingsStore();
    await store.save(const ConnectionSettings(host: 'hub.tail1234.ts.net', port: 7420, authKey: 'k', deviceName: 'Phone', useTls: true));

    final loaded = await store.load();
    expect(loaded, isNotNull);
    expect(loaded!.host, 'hub.tail1234.ts.net');
    expect(loaded.useTls, isTrue);
  });

  test('settings saved before TLS existed load with TLS off', () async {
    SharedPreferences.setMockInitialValues({'connection.host': '192.168.1.10', 'connection.authKey': 'k'});

    final loaded = await ConnectionSettingsStore().load();
    expect(loaded!.useTls, isFalse);
  });
}
