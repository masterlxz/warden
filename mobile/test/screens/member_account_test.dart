import 'dart:convert';

import 'package:async/async.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/screens/member_account.dart';
import 'package:mobile/services/server_connection.dart';
import 'package:stream_channel/stream_channel.dart';

// P84 fatia 4 and 5 on the phone: the recovery code can't be dismissed before it's saved, the recovery
// code field shows only after an owner's reset, and the answers of the hub reach the screens.

Future<(ServerConnection, StreamChannelController<dynamic>, StreamQueue<dynamic>)> memberConnection({String user = '{"id":"ana","name":"Ana","role":"member","mustChangePassword":false}'}) async {
  final controller = StreamChannelController<dynamic>();
  final fromClient = StreamQueue<dynamic>(controller.local.stream);
  final future = ServerConnection.connectOverChannel(channel: controller.foreign, deviceId: 'd', deviceName: 'D', authKey: '', username: 'ana', password: 'pw');
  await fromClient.next;
  controller.local.sink.add('{"type":"helloAck","serverName":"hub","user":$user}');
  return (await future, controller, fromClient);
}

Widget host(Widget Function(BuildContext) body) => MaterialApp(home: Scaffold(body: Builder(builder: body)));

void main() {
  testWidgets('the recovery code stays until the member says they saved it', (tester) async {
    await tester.pumpWidget(host((context) => TextButton(onPressed: () => showRecoveryCode(context, 'ABCD-EFGH'), child: const Text('open'))));
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
    expect(find.text('ABCD-EFGH'), findsOneWidget);
    expect(find.text('Your data is now encrypted'), findsOneWidget);

    // Continue is off, the back button and a tap outside don't close it.
    FilledButton button() => tester.widget<FilledButton>(find.widgetWithText(FilledButton, 'Continue'));
    expect(button().onPressed, isNull);
    await tester.tapAt(const Offset(2, 2));
    await tester.pumpAndSettle();
    expect(find.text('ABCD-EFGH'), findsOneWidget);
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(find.text('ABCD-EFGH'), findsOneWidget);

    await tester.tap(find.byKey(const Key('recovery-code-saved')));
    await tester.pump();
    expect(button().onPressed, isNotNull);
    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await tester.pumpAndSettle();
    expect(find.text('ABCD-EFGH'), findsNothing);
  });

  testWidgets('a replacing code says the old one stopped working', (tester) async {
    await tester.pumpWidget(host((context) => TextButton(onPressed: () => showRecoveryCode(context, 'NEW-CODE', replacing: true), child: const Text('open'))));
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
    expect(find.text('Your new recovery code'), findsOneWidget);
  });

  testWidgets('the recovery code field is there only after an owner reset', (tester) async {
    final (conn, _, _) = await memberConnection();
    for (final needs in [false, true]) {
      await tester.pumpWidget(MaterialApp(home: Scaffold(body: ChangePasswordDialog(connection: conn, name: 'Ana', needsRecovery: needs))));
      expect(find.byKey(const Key('change-password-recovery-code')), needs ? findsOneWidget : findsNothing);
    }
    await conn.goodbye('done');
  });

  testWidgets('a reset member has to give the code, and what the hub answers comes back', (tester) async {
    final (conn, controller, fromClient) = await memberConnection();
    PasswordChange? result;
    await tester.pumpWidget(host((context) => TextButton(
          onPressed: () async => result = await showDialog<PasswordChange>(context: context, builder: (_) => ChangePasswordDialog(connection: conn, name: 'Ana', needsRecovery: true)),
          child: const Text('open'),
        )));
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
    final fields = find.byType(TextField);
    // provisional, recovery code, new, again
    await tester.enterText(fields.at(0), 'temp');
    await tester.enterText(fields.at(2), 'her-own-pass');
    await tester.enterText(fields.at(3), 'her-own-pass');
    await tester.tap(find.text('Save'));
    await tester.pump();
    expect(find.text('Give your recovery code to keep your data.'), findsOneWidget, reason: 'no request goes out without the code');

    await tester.enterText(fields.at(1), ' abcd-efgh ');
    await tester.tap(find.text('Save'));
    final sent = jsonDecode(await tester.runAsync(() => fromClient.next) as String) as Map<String, dynamic>;
    expect((sent['type'], sent['recoveryCode']), ('changePassword', 'abcd-efgh'));
    controller.local.sink.add('{"type":"passwordChanged","requestId":${sent['requestId']}}');
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 50)));
    await tester.pumpAndSettle();
    expect(result, isNotNull);
    expect(result!.recoveryCode, isNull);
    await conn.goodbye('done');
  });

  testWidgets('the account screen changes nothing on its own and links a TruthID', (tester) async {
    final (conn, controller, fromClient) = await memberConnection(user: '{"id":"ana","name":"Ana","role":"member","mustChangePassword":false,"encrypted":true}');
    await tester.pumpWidget(MaterialApp(home: AccountScreen(connection: conn)));
    expect(find.text('Link my TruthID'), findsOneWidget);
    expect(find.byKey(const Key('account-new-code')), findsOneWidget, reason: 'encrypted members can ask for a new code');

    await tester.tap(find.byKey(const Key('account-link-truthid')));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('truthid-invite-code')), 'ana:secret');
    await tester.enterText(find.byKey(const Key('truthid-username')), '@ana.silva');
    await tester.tap(find.text('Link'));
    final sent = jsonDecode(await tester.runAsync(() => fromClient.next) as String) as Map<String, dynamic>;
    expect((sent['type'], sent['code']), ('redeemInvite', 'ana:secret'));
    controller.local.sink.add('{"type":"truthIdLinked","requestId":${sent['requestId']},"username":"ana.silva"}');
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 50)));
    await tester.pumpAndSettle();
    expect(find.text('TruthID: @ana.silva'), findsOneWidget);
    await conn.goodbye('done');
  });

  testWidgets('a member without encryption is not offered a new recovery code', (tester) async {
    final (conn, _, _) = await memberConnection();
    await tester.pumpWidget(MaterialApp(home: AccountScreen(connection: conn)));
    expect(find.byKey(const Key('account-new-code')), findsNothing);
    await conn.goodbye('done');
  });
}
