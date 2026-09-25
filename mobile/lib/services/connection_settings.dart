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
    this.useTls = false,
  });

  final String host;
  final int port;
  final String authKey;
  final String deviceName;

  /// P36 — connect over `wss://` (a TLS-only hub; `host` is then the name its certificate covers).
  final bool useTls;
}

class ConnectionSettingsStore {
  static const _keyHost = 'connection.host';
  static const _keyPort = 'connection.port';
  static const _keyAuthKey = 'connection.authKey';
  static const _keyDeviceName = 'connection.deviceName';
  static const _keyUseTls = 'connection.useTls';
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
      useTls: prefs.getBool(_keyUseTls) ?? false,
    );
  }

  Future<void> save(ConnectionSettings settings) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_keyHost, settings.host);
    await prefs.setInt(_keyPort, settings.port);
    await prefs.setString(_keyAuthKey, settings.authKey);
    await prefs.setString(_keyDeviceName, settings.deviceName);
    await prefs.setBool(_keyUseTls, settings.useTls);
  }

  // P36 — one token per hub (this app has a single device id), so a phone paired with two hubs
  // keeps both.
  static String _deviceTokenKey(String host, int port) => 'connection.deviceToken.$host:$port';

  Future<String?> deviceTokenFor(String host, int port) async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_deviceTokenKey(host, port));
  }

  Future<void> saveDeviceToken(String host, int port, String token) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_deviceTokenKey(host, port), token);
  }

  // P78 — the conversation that was open on each hub, reopened on the next connect.
  static String _lastConversationKey(String host, int port) => 'connection.lastConversation.$host:$port';

  Future<String?> lastConversationFor(String host, int port) async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_lastConversationKey(host, port));
  }

  Future<void> saveLastConversation(String host, int port, String conversationId) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_lastConversationKey(host, port), conversationId);
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
