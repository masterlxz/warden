import 'dart:io';

import 'package:shared_preferences/shared_preferences.dart';

import 'device_id.dart';

/// Server connection details the user entered, persisted across app runs.
///
/// Stored in plain text via `shared_preferences` (no OS keychain/secure
/// storage) — matches the security posture already established elsewhere in
/// the project (desktop MCP OAuth tokens are plain JSON on disk too).
class ConnectionSettings {
  const ConnectionSettings({
    required this.host,
    required this.port,
    required this.authKey,
    required this.deviceName,
  });

  final String host;
  final int port;
  final String authKey;
  final String deviceName;
}

class ConnectionSettingsStore {
  static const _keyHost = 'connection.host';
  static const _keyPort = 'connection.port';
  static const _keyAuthKey = 'connection.authKey';
  static const _keyDeviceName = 'connection.deviceName';
  static const _keyDeviceId = 'connection.deviceId';

  static const defaultPort = 7420;

  Future<ConnectionSettings?> load() async {
    final prefs = await SharedPreferences.getInstance();
    final host = prefs.getString(_keyHost);
    final authKey = prefs.getString(_keyAuthKey);
    if (host == null || authKey == null) return null;
    return ConnectionSettings(
      host: host,
      port: prefs.getInt(_keyPort) ?? defaultPort,
      authKey: authKey,
      deviceName: prefs.getString(_keyDeviceName) ?? defaultDeviceName(),
    );
  }

  Future<void> save(ConnectionSettings settings) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_keyHost, settings.host);
    await prefs.setInt(_keyPort, settings.port);
    await prefs.setString(_keyAuthKey, settings.authKey);
    await prefs.setString(_keyDeviceName, settings.deviceName);
  }

  Future<String> getOrCreateDeviceId() async {
    final prefs = await SharedPreferences.getInstance();
    final existing = prefs.getString(_keyDeviceId);
    if (existing != null) return existing;
    final id = generateDeviceId();
    await prefs.setString(_keyDeviceId, id);
    return id;
  }

  static String defaultDeviceName() => switch (Platform.operatingSystem) {
        'android' => 'Android Device',
        'ios' => 'iOS Device',
        final other => other,
      };
}
