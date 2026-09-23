import 'dart:async';
import 'dart:convert';

import 'package:async/async.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/screens/connection_screen.dart';
import 'package:mobile/services/server_connection.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:stream_channel/stream_channel.dart';

/// P41 — connect, chat, back out with the back button, then resume: the conversation must still be
/// there, and a reply that lands while the chat screen is gone must not be lost. Runs the real
/// `ServerConnection` handshake over an in-process channel (see `server_connection_test.dart`);
/// `controller.local` plays warden-server.
void main() {
  late StreamChannelController<dynamic> controller;
  late StreamQueue<dynamic> fromClient;

  setUp(() {
    SharedPreferences.setMockInitialValues({});
    controller = StreamChannelController<dynamic>();
    fromClient = StreamQueue<dynamic>(controller.local.stream);
  });

  Future<ServerConnection> fakeConnector({
    required String host,
    required int port,
    required String deviceId,
    required String deviceName,
    required String authKey,
    String? deviceToken,
    Duration handshakeTimeout = ServerConnection.defaultHandshakeTimeout,
    List<Map<String, dynamic>> toolSpecs = const [],
    Map<String, ToolHandler> toolHandlers = const {},
  }) {
    // Answer the Hello once it's sent, like a real server would.
    unawaited(fromClient.next.then((_) => controller.local.sink.add('{"type":"helloAck","serverName":"test-hub"}')));
    return ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: deviceId,
      deviceName: deviceName,
      authKey: authKey,
    );
  }

  testWidgets('backing out of the chat and resuming keeps the conversation', (tester) async {
    await tester.runAsync(() async {}); // let SharedPreferences' mock settle
    await tester.pumpWidget(MaterialApp(home: ConnectionScreen(connector: fakeConnector)));
    await tester.pump();

    await tester.enterText(find.widgetWithText(TextField, 'Server host'), '10.0.0.1');
    await tester.enterText(find.widgetWithText(TextField, 'Auth key'), 'secret');
    await tester.tap(find.widgetWithText(FilledButton, 'Connect'));
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 100)));
    await tester.pumpAndSettle();

    // Now in the chat: send a message.
    expect(find.widgetWithText(AppBar, 'test-hub'), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'hello there');
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    expect(find.text('hello there'), findsOneWidget);

    // Back out WITHOUT disconnecting — the reply arrives while no chat screen exists.
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('Resume chat'), findsOneWidget);
    expect(find.widgetWithText(OutlinedButton, 'Disconnect'), findsOneWidget);

    controller.local.sink.add(jsonEncode({'type': 'chatResponse', 'content': 'general kenobi'}));
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 50)));

    await tester.tap(find.text('Resume chat'));
    await tester.pumpAndSettle();

    expect(find.text('hello there'), findsOneWidget);
    expect(find.text('general kenobi'), findsOneWidget);
  });
}
