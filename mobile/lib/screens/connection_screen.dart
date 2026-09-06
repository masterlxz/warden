import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../services/connection_settings.dart';
import '../services/mobile_file_tool.dart';
import '../services/server_connection.dart';
import 'chat_screen.dart';

/// Fase 7.2 scope: prove connectivity to a warden-server over WebSocket.
/// A successful connect now pushes straight into the real chat UI (Fase 7.3, `ChatScreen`) —
/// this screen stays underneath so the user can navigate back to it without hanging up.
class ConnectionScreen extends StatefulWidget {
  const ConnectionScreen({super.key});

  @override
  State<ConnectionScreen> createState() => _ConnectionScreenState();
}

class _ConnectionScreenState extends State<ConnectionScreen> {
  final _hostController = TextEditingController();
  final _portController = TextEditingController(text: '${ConnectionSettingsStore.defaultPort}');
  final _authKeyController = TextEditingController();
  final _deviceNameController = TextEditingController();

  final _settingsStore = ConnectionSettingsStore();
  final _fileTool = MobileFileTool();

  ServerConnection? _connection;
  ConnectionStatus _status = const Disconnected();
  StreamSubscription<ConnectionStatus>? _statusSubscription;

  @override
  void initState() {
    super.initState();
    _deviceNameController.text = ConnectionSettingsStore.defaultDeviceName();
    _loadSavedSettings();
  }

  Future<void> _loadSavedSettings() async {
    final saved = await _settingsStore.load();
    if (saved != null) {
      setState(() {
        _hostController.text = saved.host;
        _portController.text = '${saved.port}';
        _authKeyController.text = saved.authKey;
        _deviceNameController.text = saved.deviceName;
      });
      return;
    }
    // No persisted settings yet: prefill a dev-friendly default so the app
    // is usable out of the box against a locally-run warden-server, without
    // ever silently overriding a value the user already saved. 10.0.2.2 is
    // the Android emulator's special alias for the host machine's loopback.
    final hint = _debugAndroidHostHint();
    if (hint != null) {
      setState(() => _hostController.text = hint);
    }
  }

  String? _debugAndroidHostHint() {
    if (kDebugMode && defaultTargetPlatform == TargetPlatform.android) {
      return '10.0.2.2';
    }
    return null;
  }

  Future<void> _connect() async {
    final host = _hostController.text.trim();
    final port = int.tryParse(_portController.text.trim());
    final authKey = _authKeyController.text;
    final deviceName = _deviceNameController.text.trim();

    if (host.isEmpty || port == null || authKey.isEmpty || deviceName.isEmpty) {
      setState(() => _status = const ConnectionFailure('Fill in host, port, auth key and device name'));
      return;
    }

    setState(() => _status = const Connecting());

    try {
      final deviceId = await _settingsStore.getOrCreateDeviceId();
      final hasRootFolder = await _fileTool.rootFolderUri() != null;
      final connection = await ServerConnection.connect(
        host: host,
        port: port,
        deviceId: deviceId,
        deviceName: deviceName,
        authKey: authKey,
        // Opt-in, same spirit as the desktop's `enable_shell`: only advertise (and answer) the
        // file tools once the user has picked a root folder for them to operate in.
        toolSpecs: hasRootFolder ? const [MobileFileTool.listFilesSpec, MobileFileTool.readFileSpec] : const [],
        toolHandlers: hasRootFolder ? {'list_phone_files': _fileTool.listFiles, 'read_phone_file': _fileTool.readFile} : const {},
      );
      await _settingsStore.save(ConnectionSettings(
        host: host,
        port: port,
        authKey: authKey,
        deviceName: deviceName,
      ));

      await _statusSubscription?.cancel();
      _statusSubscription = connection.statusStream.listen((s) {
        if (mounted) setState(() => _status = s);
      });

      setState(() {
        _connection = connection;
        _status = connection.status;
      });

      if (mounted) {
        await Navigator.of(context).push(
          MaterialPageRoute(builder: (_) => ChatScreen(connection: connection)),
        );
      }
    } on HandshakeException catch (e) {
      setState(() => _status = ConnectionFailure(e.message));
    } catch (e) {
      setState(() => _status = ConnectionFailure(e.toString()));
    }
  }

  Future<void> _disconnect() async {
    await _connection?.goodbye('user disconnected');
  }

  @override
  void dispose() {
    _statusSubscription?.cancel();
    _connection?.goodbye('screen closed');
    _hostController.dispose();
    _portController.dispose();
    _authKeyController.dispose();
    _deviceNameController.dispose();
    super.dispose();
  }

  bool get _isConnected => _status is Connected;
  bool get _isBusy => _status is Connecting;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Warden — Server Connection')),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: ListView(
          children: [
            TextField(
              controller: _hostController,
              enabled: !_isConnected && !_isBusy,
              decoration: const InputDecoration(labelText: 'Server host'),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _portController,
              enabled: !_isConnected && !_isBusy,
              keyboardType: TextInputType.number,
              decoration: const InputDecoration(labelText: 'Port'),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _authKeyController,
              enabled: !_isConnected && !_isBusy,
              obscureText: true,
              decoration: const InputDecoration(labelText: 'Auth key'),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _deviceNameController,
              enabled: !_isConnected && !_isBusy,
              decoration: const InputDecoration(labelText: 'Device name'),
            ),
            const SizedBox(height: 24),
            _StatusIndicator(status: _status),
            const SizedBox(height: 24),
            FilledButton(
              onPressed: _isBusy
                  ? null
                  : _isConnected
                      ? _disconnect
                      : _connect,
              child: Text(_isConnected ? 'Disconnect' : 'Connect'),
            ),
          ],
        ),
      ),
    );
  }
}

class _StatusIndicator extends StatelessWidget {
  const _StatusIndicator({required this.status});

  final ConnectionStatus status;

  @override
  Widget build(BuildContext context) {
    final (label, color) = switch (status) {
      Disconnected(:final reason) => (reason == null ? 'Disconnected' : 'Disconnected: $reason', Colors.grey),
      Connecting() => ('Connecting…', Colors.orange),
      Connected(:final serverName) => ('Connected to $serverName', Colors.green),
      ConnectionFailure(:final message) => ('Error: $message', Colors.red),
    };
    return Row(
      children: [
        Icon(Icons.circle, size: 12, color: color),
        const SizedBox(width: 8),
        Expanded(child: Text(label, style: Theme.of(context).textTheme.bodyLarge)),
      ],
    );
  }
}
