import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../services/chat_transcript.dart';
import '../services/connection_settings.dart';
import '../services/hub_pairing_qr.dart';
import '../services/mobile_file_tool.dart';
import '../services/server_connection.dart';
import '../src/rust/api/discovery.dart';
import 'chat_screen.dart';
import 'qr_scan_screen.dart';
import 'skills_screen.dart';
import 'sync_screen.dart';

/// Fase 7.2 scope: prove connectivity to a warden-server over WebSocket.
/// A successful connect now pushes straight into the real chat UI (Fase 7.3, `ChatScreen`) —
/// this screen stays underneath so the user can navigate back to it without hanging up.
class ConnectionScreen extends StatefulWidget {
  const ConnectionScreen({super.key, this.connector = ServerConnection.connect});

  /// How a connection gets opened — `ServerConnection.connect` (a real WebSocket) in the app, a
  /// fake channel in tests, so the connect → chat → back → resume flow (P41) can be exercised
  /// without a network.
  final ServerConnector connector;

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
  ChatTranscript? _transcript;
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

    if (host.isEmpty || port == null || deviceName.isEmpty) {
      setState(() => _status = const ConnectionFailure('Fill in host, port, auth key and device name'));
      return;
    }
    // P36 — once paired, the device token is enough; the auth key only matters for pairing.
    final deviceToken = await _settingsStore.deviceTokenFor(host, port);
    if (authKey.isEmpty && deviceToken == null) {
      setState(() => _status = const ConnectionFailure('Fill in host, port, auth key and device name'));
      return;
    }

    setState(() => _status = const Connecting());

    try {
      final deviceId = await _settingsStore.getOrCreateDeviceId();
      final hasRootFolder = await _fileTool.rootFolderUri() != null;
      final connection = await widget.connector(
        host: host,
        port: port,
        deviceId: deviceId,
        deviceName: deviceName,
        authKey: authKey,
        deviceToken: deviceToken,
        // Opt-in, same spirit as the desktop's `enable_shell`: only advertise (and answer) the
        // file tools once the user has picked a root folder for them to operate in.
        toolSpecs: hasRootFolder ? const [MobileFileTool.listFilesSpec, MobileFileTool.readFileSpec] : const [],
        toolHandlers: hasRootFolder ? {'list_phone_files': _fileTool.listFiles, 'read_phone_file': _fileTool.readFile} : const {},
      );
      final issuedToken = connection.issuedDeviceToken;
      if (issuedToken != null) {
        await _settingsStore.saveDeviceToken(host, port, issuedToken);
      }
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

      _transcript?.dispose();
      setState(() {
        _connection = connection;
        _transcript = ChatTranscript(
          chatStream: connection.chatStream,
          sendChat: connection.sendChat,
          // P40 — the last 100 messages are plenty to pick a phone conversation back up, and keep
          // the reply small even when older turns carry base64 image attachments.
          fetchHistory: () => connection.fetchHistory(limit: 100),
        );
        _status = connection.status;
      });

      await _openChat();
    } on HandshakeException catch (e) {
      setState(() => _status = ConnectionFailure(e.message));
    } catch (e) {
      setState(() => _status = ConnectionFailure(e.toString()));
    }
  }

  // Fase 9.1 (redefined) — sweeps the LAN instead of asking the user to already know the IP.
  // Reuses the same `discover_hubs` Rust already exposes to the desktop's WorkspaceView, over the
  // mobile bridge. Only ever fills host/port — the auth key is never part of a hub's reply, so it
  // stays manual on purpose, same security boundary as the desktop and the QR pairing flow. Sweeps
  // whatever port is already typed in the form (same parsing `_connect()` uses) so a hub started
  // with `--listen` on a non-default port is still discoverable — falls back to the default only
  // when the field is empty/invalid.
  Future<void> _discoverHubs() async {
    if (!mounted) return;
    final port = int.tryParse(_portController.text.trim()) ?? ConnectionSettingsStore.defaultPort;
    final result = await showModalBottomSheet<DiscoveredHubDto>(
      context: context,
      isScrollControlled: true,
      builder: (context) => _DiscoveredHubsSheet(
        future: bridgeDiscoverHubs(port: port),
      ),
    );
    if (result == null || !mounted) return;
    setState(() {
      _hostController.text = result.host;
      _portController.text = '${result.port}';
    });
  }

  Future<void> _scanQr() async {
    final payload = await Navigator.of(context).push<HubPairingPayload>(
      MaterialPageRoute(builder: (_) => const QrScanScreen()),
    );
    if (payload == null || !mounted) return;
    setState(() {
      _hostController.text = payload.host;
      _portController.text = '${payload.port}';
      _authKeyController.text = payload.authKey;
    });
  }

  /// P41 — (re)opens the chat for the live connection. The transcript belongs to this screen, not to
  /// `ChatScreen`, so backing out of the chat and coming back here keeps the conversation.
  Future<void> _openChat() async {
    final connection = _connection;
    final transcript = _transcript;
    if (connection == null || transcript == null || !mounted) return;
    await Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => ChatScreen(connection: connection, transcript: transcript)),
    );
  }

  Future<void> _disconnect() async {
    await _connection?.goodbye('user disconnected');
  }

  @override
  void dispose() {
    _statusSubscription?.cancel();
    _transcript?.dispose();
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
      appBar: AppBar(
        title: const Text('Warden — Server Connection'),
        actions: [
          // Fase 9.1 (redefined) — sweeps the LAN for a warden-server hub and fills in host/port
          // from the pick, instead of typing an IP by hand. Only useful before connecting.
          if (!_isConnected && !_isBusy)
            IconButton(
              icon: const Icon(Icons.wifi_find),
              tooltip: 'Discover hubs on the local network',
              onPressed: _discoverHubs,
            ),
          // Fase 9.7 — scans the desktop Workspace screen's pairing QR to fill in host/port/auth
          // key below, instead of typing them by hand. Only useful before connecting.
          if (!_isConnected && !_isBusy)
            IconButton(
              icon: const Icon(Icons.qr_code_scanner),
              tooltip: 'Scan QR to fill in connection',
              onPressed: _scanQr,
            ),
          // Fase 4.4 — sync doesn't depend on being connected to warden-server (it only talks to
          // a paired device over LAN and to Arweave/TruthID), so it's reachable independent of
          // this screen's connection state.
          IconButton(
            icon: const Icon(Icons.sync),
            tooltip: 'Sync',
            onPressed: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const SyncScreen())),
          ),
          // P72 (b) — skills live in this device's local vault, independent of the server connection.
          IconButton(
            icon: const Icon(Icons.auto_awesome),
            tooltip: 'Skills',
            onPressed: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const SkillsScreen())),
          ),
        ],
      ),
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
            // P41 — a live connection can be resumed after backing out of the chat, instead of the
            // only way back being disconnect + reconnect.
            if (_isConnected) ...[
              FilledButton(onPressed: _openChat, child: const Text('Resume chat')),
              const SizedBox(height: 12),
              OutlinedButton(onPressed: _disconnect, child: const Text('Disconnect')),
            ] else
              FilledButton(onPressed: _isBusy ? null : _connect, child: const Text('Connect')),
          ],
        ),
      ),
    );
  }
}

class _DiscoveredHubsSheet extends StatelessWidget {
  const _DiscoveredHubsSheet({required this.future});

  final Future<List<DiscoveredHubDto>> future;

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: FutureBuilder<List<DiscoveredHubDto>>(
          future: future,
          builder: (context, snapshot) {
            if (snapshot.connectionState != ConnectionState.done) {
              return const Padding(
                padding: EdgeInsets.symmetric(vertical: 32),
                child: Center(child: CircularProgressIndicator()),
              );
            }
            if (snapshot.hasError) {
              return Padding(
                padding: const EdgeInsets.symmetric(vertical: 16),
                child: Text('Discovery failed: ${snapshot.error}', style: const TextStyle(color: Colors.red)),
              );
            }
            final hubs = snapshot.data!;
            if (hubs.isEmpty) {
              return const Padding(
                padding: EdgeInsets.symmetric(vertical: 16),
                child: Text('No hub answered on the local network.'),
              );
            }
            return Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                const Padding(
                  padding: EdgeInsets.only(bottom: 8),
                  child: Text('Hubs found', style: TextStyle(fontWeight: FontWeight.bold)),
                ),
                for (final hub in hubs)
                  ListTile(
                    title: Text(hub.serverName),
                    subtitle: Text('${hub.host}:${hub.port}'),
                    onTap: () => Navigator.of(context).pop(hub),
                  ),
              ],
            );
          },
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
