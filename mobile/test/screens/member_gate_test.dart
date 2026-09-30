import 'dart:async';
import 'dart:convert';

import 'package:async/async.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/screens/connection_screen.dart';
import 'package:mobile/services/server_connection.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:stream_channel/stream_channel.dart';

/// P84 fatia 4 on the phone: what a member passes through, in order, before the chat. `controller.local`
/// plays warden-server, answering with the `HelloAck` the test gives it.
void main() {
  late StreamChannelController<dynamic> controller;
  late StreamQueue<dynamic> fromClient;
  var helloAck = '';

  setUp(() {
    SharedPreferences.setMockInitialValues({});
    controller = StreamChannelController<dynamic>();
    fromClient = StreamQueue<dynamic>(controller.local.stream);
  });

  Future<ServerConnection> fakeConnector({
    required String host,
    required int port,
    bool secure = false,
    required String deviceId,
    required String deviceName,
    required String authKey,
    String? deviceToken,
    String? username,
    String? password,
    Duration handshakeTimeout = ServerConnection.defaultHandshakeTimeout,
    List<Map<String, dynamic>> toolSpecs = const [],
    Map<String, ToolHandler> toolHandlers = const {},
  }) {
    unawaited(fromClient.next.then((_) {
      controller.local.sink.add(helloAck);
      // The hub pushes the code on its own when signing in turned encryption on.
      if (helloAck.contains('PUSH')) controller.local.sink.add('{"type":"recoveryCode","requestId":0,"code":"PUSHED-CODE"}');
    }));
    return ServerConnection.connectOverChannel(channel: controller.foreign, deviceId: deviceId, deviceName: deviceName, authKey: authKey, username: username, password: password);
  }

  Future<void> signIn(WidgetTester tester) async {
    // Tall enough that the whole form is built (it's a lazy ListView).
    tester.view.physicalSize = const Size(800, 2400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.runAsync(() async {});
    await tester.pumpWidget(MaterialApp(home: ConnectionScreen(connector: fakeConnector)));
    await tester.pump();
    await tester.enterText(find.widgetWithText(TextField, 'Server host'), '10.0.0.1');
    await tester.tap(find.text('Username'));
    await tester.pumpAndSettle();
    await tester.enterText(find.widgetWithText(TextField, 'Username'), 'ana');
    await tester.enterText(find.widgetWithText(TextField, 'Password'), 'temp-pass-1');
    await tester.tap(find.widgetWithText(FilledButton, 'Connect'));
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 100)));
    await tester.pumpAndSettle();
  }

  Future<void> settle(WidgetTester tester) async {
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 80)));
    await tester.pumpAndSettle();
  }

  Future<Map<String, dynamic>> nextSent(WidgetTester tester) async => jsonDecode(await tester.runAsync(() => fromClient.next) as String) as Map<String, dynamic>;

  testWidgets('first sign-in: new password, then the code turned encryption on gives is shown before the chat', (tester) async {
    helloAck = '{"type":"helloAck","serverName":"hub","user":{"id":"ana","name":"Ana","role":"member","mustChangePassword":true}}';
    await signIn(tester);
    expect(find.text('Choose your password'), findsOneWidget);
    expect(find.byKey(const Key('change-password-recovery-code')), findsNothing);

    final fields = find.descendant(of: find.byType(AlertDialog), matching: find.byType(TextField));
    await tester.enterText(fields.at(0), 'temp-pass-1');
    await tester.enterText(fields.at(1), 'her-own-pass');
    await tester.enterText(fields.at(2), 'her-own-pass');
    await tester.tap(find.text('Save'));
    final sent = await nextSent(tester);
    expect(sent['type'], 'changePassword');
    controller.local.sink.add('{"type":"passwordChanged","requestId":${sent['requestId']},"recoveryCode":"FIRST-CODE"}');
    await settle(tester);

    // The code is up, and the chat is not: there's no way past it without saying it was saved.
    expect(find.text('FIRST-CODE'), findsOneWidget);
    expect(find.widgetWithText(AppBar, 'hub'), findsNothing);
    await tester.tap(find.byKey(const Key('recovery-code-saved')));
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await settle(tester);
    expect(find.widgetWithText(AppBar, 'hub'), findsOneWidget);
  });

  testWidgets('a member from before gets the code the hub pushed, then the policy and the recovery notice, then the chat', (tester) async {
    helloAck = '{"type":"helloAck","serverName":"hub","user":{"id":"ana","name":"Ana","role":"member","mustChangePassword":false,"encrypted":true,'
        '"policyPending":true,"recoveryPolicy":"company","recoveries":[{"atMs":1790000000000,"kind":"company","seen":false}],"x":"PUSH"}}';
    await signIn(tester);
    expect(find.text('PUSHED-CODE'), findsOneWidget, reason: 'the pushed code comes first');
    await tester.tap(find.byKey(const Key('recovery-code-saved')));
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await settle(tester);

    expect(find.text('Who can help recover your data changed'), findsOneWidget);
    await tester.enterText(find.byKey(const Key('accept-policy-password')), 'her-own-pass');
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, 'Accept'));
    final accept = await nextSent(tester);
    expect((accept['type'], accept['password']), ('acceptRecoveryPolicy', 'her-own-pass'));
    controller.local.sink.add('{"type":"recoveryPolicyAccepted","requestId":${accept['requestId']}}');
    await settle(tester);

    expect(find.text('Your data was recovered'), findsOneWidget);
    await tester.tap(find.text('Got it'));
    final ack = await nextSent(tester);
    expect(ack['type'], 'ackRecoveryNotices');
    controller.local.sink.add('{"type":"recoveryNoticesAcked","requestId":${ack['requestId']}}');
    await settle(tester);
    expect(find.widgetWithText(AppBar, 'hub'), findsOneWidget);
  });

  testWidgets('a locked member is sent back to sign in with the password', (tester) async {
    helloAck = '{"type":"helloAck","serverName":"hub","user":{"id":"ana","name":"Ana","role":"member","mustChangePassword":false,"encrypted":true,"locked":true}}';
    await signIn(tester);
    expect(find.textContaining('Your data is locked'), findsOneWidget);
    expect(find.widgetWithText(AppBar, 'hub'), findsNothing);
  });

  testWidgets('saying "Not now" to a weaker policy still opens the chat', (tester) async {
    helloAck = '{"type":"helloAck","serverName":"hub","user":{"id":"ana","name":"Ana","role":"member","mustChangePassword":false,"encrypted":true,"policyPending":true,"recoveryPolicy":"consent"}}';
    await signIn(tester);
    expect(find.text('Who can help recover your data changed'), findsOneWidget);
    await tester.tap(find.text('Not now'));
    await settle(tester);
    expect(find.widgetWithText(AppBar, 'hub'), findsOneWidget);
  });
}
