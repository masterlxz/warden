import 'dart:async';
import 'dart:convert';

import 'package:async/async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/services/server_connection.dart';
import 'package:stream_channel/stream_channel.dart';

// Runs the REAL handshake/heartbeat/goodbye logic in ServerConnection against
// a real (non-network) StreamChannelController pair — not a mocked service —
// mirroring the "real server + real client" spirit of
// crates/warden-server/tests/handshake.rs. `controller.foreign` plays the
// role of `ServerConnection`'s channel (what a real WebSocketChannel would
// be); `controller.local` plays the role of the network peer (warden-server)
// that each test drives directly.
void main() {
  test('hubUri picks wss:// only for a TLS hub (P36)', () {
    expect(hubUri('192.168.1.10', 7420, secure: false).toString(), 'ws://192.168.1.10:7420');
    expect(hubUri('hub.tail1234.ts.net', 7420, secure: true).toString(), 'wss://hub.tail1234.ts.net:7420');
  });

  test('successful handshake reaches Connected with the server name', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );

    final sentHello = await fromClient.next as String;
    expect(
      sentHello,
      '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"test-key","tools":[],"recoveryCodes":true}',
    );

    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');

    final conn = await future;
    expect(conn.status, isA<Connected>());
    expect((conn.status as Connected).serverName, 'warden-server');
  });

  test('a member pairs with username and password and changes the provisional one (P84)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'ana-phone',
      deviceName: 'Ana phone',
      authKey: '',
      username: 'ana',
      password: 'provisional-1',
    );

    final hello = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect((hello['username'], hello['password'], hello['authKey']), ('ana', 'provisional-1', ''));
    controller.local.sink.add(
      '{"type":"helloAck","serverName":"hub","deviceToken":"tok","user":{"id":"ana","name":"Ana","role":"member","mustChangePassword":true}}',
    );
    final conn = await future;
    expect((conn.user?.id, conn.user?.mustChangePassword, conn.issuedDeviceToken), ('ana', true, 'tok'));

    // A wrong provisional password comes back as a PasswordException.
    final wrong = conn.changePassword('nope', 'her-own-pass');
    final first = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect((first['type'], first['oldPassword'], first['newPassword']), ('changePassword', 'nope', 'her-own-pass'));
    controller.local.sink.add('{"type":"userError","requestId":${first['requestId']},"message":"the current password is wrong","authRejected":true}');
    await expectLater(wrong, throwsA(isA<PasswordException>().having((e) => e.wrongPassword, 'wrongPassword', true)));

    final right = conn.changePassword('provisional-1', 'her-own-pass');
    final second = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    controller.local.sink.add('{"type":"passwordChanged","requestId":${second['requestId']}}');
    await right;
  });

  test('a member gets the recovery code, regenerates it, accepts a policy and links a TruthID (P84)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);
    final future = ServerConnection.connectOverChannel(channel: controller.foreign, deviceId: 'd', deviceName: 'D', authKey: '', username: 'ana', password: 'provisional-1');
    final hello = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect(hello['recoveryCodes'], true, reason: 'the phone can show the code, so the hub may turn encryption on');
    controller.local.sink.add('{"type":"helloAck","serverName":"hub","user":{"id":"ana","name":"Ana","role":"member","mustChangePassword":true,"needsRecovery":true}}');
    // The hub pushes a code on its own right after the ack: kept for a screen to take, once.
    controller.local.sink.add('{"type":"recoveryCode","requestId":0,"code":"PUSHED"}');
    final conn = await future;
    await Future<void>.delayed(Duration.zero);
    expect(conn.takeUnclaimedRecoveryCode(), 'PUSHED');
    expect(conn.takeUnclaimedRecoveryCode(), isNull);

    // After an owner's reset, the recovery code goes with the new password; the answer may carry a code.
    final change = conn.changePassword('temp', 'her-own-pass', recoveryCode: 'OLD-CODE');
    final m1 = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect(m1['recoveryCode'], 'OLD-CODE');
    controller.local.sink.add('{"type":"passwordChanged","requestId":${m1['requestId']},"recoveryCode":"FRESH"}');
    expect(await change, 'FRESH');
    expect((conn.user?.mustChangePassword, conn.user?.encrypted), (false, true));

    final regenerate = conn.regenerateRecoveryCode('her-own-pass');
    final m2 = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect((m2['type'], m2['password']), ('regenerateRecoveryCode', 'her-own-pass'));
    controller.local.sink.add('{"type":"recoveryCode","requestId":${m2['requestId']},"code":"NEW"}');
    expect(await regenerate, 'NEW');

    final wrong = conn.acceptRecoveryPolicy('nope');
    final m3 = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect(m3['type'], 'acceptRecoveryPolicy');
    controller.local.sink.add('{"type":"userError","requestId":${m3['requestId']},"message":"your password doesn\'t open your data","authRejected":true}');
    await expectLater(wrong, throwsA(isA<PasswordException>().having((e) => e.wrongPassword, 'wrong', true)));
    final accepted = conn.acceptRecoveryPolicy('her-own-pass');
    final m4 = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    controller.local.sink.add('{"type":"recoveryPolicyAccepted","requestId":${m4['requestId']}}');
    expect(await accepted, isNull);

    final ack = conn.ackRecoveryNotices();
    final m5 = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect(m5['type'], 'ackRecoveryNotices');
    controller.local.sink.add('{"type":"recoveryNoticesAcked","requestId":${m5['requestId']}}');
    await ack;

    final link = conn.redeemInvite('ana:secret', '@ana.silva');
    final m6 = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect((m6['type'], m6['code'], m6['username']), ('redeemInvite', 'ana:secret', '@ana.silva'));
    controller.local.sink.add('{"type":"truthIdLinked","requestId":${m6['requestId']},"username":"ana.silva"}');
    expect(await link, 'ana.silva');
    expect(conn.user?.truthid, 'ana.silva');
  });

  test('the owner has no user in HelloAck', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);
    final future = ServerConnection.connectOverChannel(channel: controller.foreign, deviceId: 'dev-1', deviceName: 'D', authKey: 'k');
    final hello = jsonDecode(await fromClient.next as String) as Map<String, dynamic>;
    expect(hello.containsKey('username'), false);
    controller.local.sink.add('{"type":"helloAck","serverName":"hub"}');
    expect((await future).user, isNull);
  });

  test('wrong auth key surfaces as a HandshakeException', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'wrong-key',
    );

    await fromClient.next; // consume Hello
    controller.local.sink.add('{"type":"authError","reason":"invalid auth key"}');
    await controller.local.sink.close(); // mirrors the native WS close (code 1008) right after

    await expectLater(
      future,
      throwsA(
        isA<HandshakeException>().having(
          (e) => e.message,
          'message',
          contains('authentication rejected'),
        ),
      ),
    );
  });

  test('pingNow sends a Ping and a matching Pong updates the connected status', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final statusUpdates = StreamQueue<ConnectionStatus>(conn.statusStream);

    conn.pingNow();
    final sentPing = await fromClient.next as String;
    expect(sentPing, '{"type":"ping","nonce":0}');

    controller.local.sink.add('{"type":"pong","nonce":0}');

    final status = await statusUpdates.next;
    expect(status, isA<Connected>());
    expect((status as Connected).lastPongAt, isNotNull);
  });

  test('sending Goodbye leads to a clean Disconnected once the socket closes', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final statusUpdates = StreamQueue<ConnectionStatus>(conn.statusStream);

    unawaited(conn.goodbye('bye'));
    final sentGoodbye = await fromClient.next as String;
    expect(sentGoodbye, '{"type":"goodbye","reason":"bye"}');

    // The real server never acknowledges Goodbye — it just stops reading and
    // the connection drops. Simulate that by closing the peer's send side.
    await controller.local.sink.close();

    final status = await statusUpdates.next;
    expect(status, isA<Disconnected>());
    expect((status as Disconnected).reason, isNull); // clean, not an error
  });

  test('sendChat sends a Chat message and a ChatResponse arrives on chatStream', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final chatUpdates = StreamQueue<ServerMessage>(conn.chatStream);

    conn.sendChat('hello there');
    final sentChat = await fromClient.next as String;
    expect(sentChat, '{"type":"chat","message":"hello there"}');

    controller.local.sink.add('{"type":"chatResponse","content":"ahoy","usage":null}');

    final reply = await chatUpdates.next;
    expect(reply, isA<ChatResponseMessage>());
    expect((reply as ChatResponseMessage).content, 'ahoy');
  });

  test('a ChatError arrives on chatStream without disrupting the connection', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final chatUpdates = StreamQueue<ServerMessage>(conn.chatStream);

    conn.sendChat('hello there');
    await fromClient.next; // consume the Chat frame
    controller.local.sink.add('{"type":"chatError","message":"provider unavailable"}');

    final reply = await chatUpdates.next;
    expect(reply, isA<ChatErrorMessage>());
    expect((reply as ChatErrorMessage).message, 'provider unavailable');
    expect(conn.status, isA<Connected>());
  });

  test('advertised tools are sent in Hello', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
      toolSpecs: const [
        {'name': 'list_files', 'description': 'List files', 'parameters': {'type': 'object'}},
      ],
    );

    final sentHello = await fromClient.next as String;
    expect(
      sentHello,
      '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"test-key",'
      '"tools":[{"name":"list_files","description":"List files","parameters":{"type":"object"}}],"recoveryCodes":true}',
    );

    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    await future;
  });

  test('a ToolCallRequest for a registered handler replies with ToolCallResult', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
      toolHandlers: {
        'list_files': (args) async => {'entries': []},
      },
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    await future;

    controller.local.sink.add('{"type":"toolCallRequest","callId":1,"tool":"list_files","arguments":{}}');

    final sentResult = await fromClient.next as String;
    expect(sentResult, '{"type":"toolCallResult","callId":1,"result":{"entries":[]}}');
  });

  test('a ToolCallRequest for an unregistered tool replies with ToolCallError', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    await future;

    controller.local.sink.add('{"type":"toolCallRequest","callId":1,"tool":"list_files","arguments":{}}');

    final sentError = await fromClient.next as String;
    expect(sentError, "{\"type\":\"toolCallError\",\"callId\":1,\"message\":\"no local handler registered for tool 'list_files'\"}");
  });

  test('a handler that throws replies with ToolCallError carrying the exception message', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
      toolHandlers: {
        'read_file': (args) async => throw StateError('file not found'),
      },
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    await future;

    controller.local.sink.add('{"type":"toolCallRequest","callId":9,"tool":"read_file","arguments":{"path":"x"}}');

    final sentError = await fromClient.next as String;
    expect(sentError, contains('"type":"toolCallError","callId":9'));
    expect(sentError, contains('file not found'));
  });

  test('fetchHistory sends RequestHistory and resolves with the matching History reply (P40)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final history = conn.fetchHistory(limit: 100);
    expect(await fromClient.next as String, '{"type":"requestHistory","requestId":1,"limit":100}');

    controller.local.sink.add('{"type":"history","requestId":1,"messages":['
        '{"role":"user","content":"hi","createdAt":1,"attachments":[]},'
        '{"role":"assistant","content":"hello","createdAt":2,"attachments":[{"mimeType":"image/png","data":"aGk="}]}]}');

    final entries = await history;
    expect(entries.map((e) => e.fromUser), [true, false]);
    expect(entries.map((e) => e.content), ['hi', 'hello']);
    expect(entries.last.attachments.single.mimeType, 'image/png');
  });

  test('a HistoryError reply fails fetchHistory with the server message (P40)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final history = conn.fetchHistory();
    await fromClient.next; // RequestHistory
    controller.local.sink.add('{"type":"historyError","requestId":1,"message":"failed to parse"}');

    await expectLater(
      history,
      throwsA(isA<HistoryException>().having((e) => e.message, 'message', 'failed to parse')),
    );
    expect(conn.status, isA<Connected>());
  });

  test('a connection that drops fails a pending fetchHistory instead of hanging (P40)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final history = conn.fetchHistory();
    await fromClient.next; // RequestHistory
    await controller.local.sink.close();

    await expectLater(history, throwsA(isA<HistoryException>()));
  });

  test('conversation requests resolve with their replies, and an error carries the hub message (P78)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final list = conn.listConversations();
    expect(await fromClient.next as String, '{"type":"listConversations","requestId":1}');
    controller.local.sink.add(
        '{"type":"conversationList","requestId":1,"conversations":[{"id":"c1","title":"Trip","createdAt":1,"updatedAt":2}]}');
    expect((await list).single.title, 'Trip');

    final rename = conn.renameConversation('c1', 'Lisbon');
    expect(await fromClient.next as String, '{"type":"renameConversation","requestId":2,"conversationId":"c1","title":"Lisbon"}');
    controller.local.sink.add('{"type":"conversationOk","requestId":2}');
    await rename;

    final delete = conn.deleteConversation('gone');
    expect(await fromClient.next as String, '{"type":"deleteConversation","requestId":3,"conversationId":"gone"}');
    controller.local.sink.add('{"type":"conversationError","requestId":3,"message":"no conversation with id \'gone\'"}');
    await expectLater(delete, throwsA(isA<ConversationException>().having((e) => e.message, 'message', contains('gone'))));

    conn.sendChat('hi', conversationId: 'c1');
    expect(await fromClient.next as String, '{"type":"chat","message":"hi","conversationId":"c1"}');
  });

  test('a member asks for the organization by their session: the tree with the access, an edit with no key, a refusal as an exception (P120)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final list = conn.listAgentOrg();
    expect(await fromClient.next as String, '{"type":"listAgentOrg","requestId":1}');
    controller.local.sink.add('{"type":"agentOrgList","requestId":1,"agents":[{"id":"chief","role":"CTO"},{"id":"dev","reportsTo":"chief"}],"access":"edit"}');
    final org = await list;
    expect(org.access, 'edit');
    expect(org.agents.map((a) => a.id).toList(), ['chief', 'dev']);

    final edit = conn.editAgentOrgAsMember(const RemoveAgentEdit('dev'));
    expect(await fromClient.next as String, '{"type":"editAgentOrg","requestId":2,"edit":{"kind":"remove","id":"dev"}}');
    controller.local.sink.add('{"type":"agentOrgList","requestId":2,"agents":[{"id":"chief","role":"CTO"}],"access":"edit"}');
    expect((await edit).agents.map((a) => a.id).toList(), ['chief']);

    final refused = conn.listAgentOrg();
    expect(await fromClient.next as String, '{"type":"listAgentOrg","requestId":3}');
    controller.local.sink.add('{"type":"settingsError","requestId":3,"message":"no access","conflict":false,"authRejected":false}');
    await expectLater(refused, throwsA(isA<HubRequestException>().having((e) => e.message, 'message', 'no access')));
  });

  test('the level the owner saves reaches the member as it happens, through the stream (P120)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);
    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final told = StreamQueue<String>(conn.orgAccessChanges);
    controller.local.sink.add('{"type":"orgAccessChanged","access":"edit"}');
    controller.local.sink.add('{"type":"orgAccessChanged","access":"none"}');
    expect(await told.next, 'edit');
    expect(await told.next, 'none');
    await told.cancel();
  });

  test('the folder list is asked for and answered by request id, and a refusal becomes an exception (P102)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;

    final top = conn.listDirs();
    expect(await fromClient.next as String, '{"type":"listDirs","requestId":1}');
    controller.local.sink.add('{"type":"dirList","requestId":1,"path":"","dirs":[{"name":"work","path":"/srv/work"}]}');
    final listing = await top;
    expect(listing.path, isEmpty);
    expect(listing.dirs.single.path, '/srv/work');

    final inside = conn.listDirs('/srv/work');
    expect(await fromClient.next as String, '{"type":"listDirs","requestId":2,"path":"/srv/work"}');
    controller.local.sink.add('{"type":"dirList","requestId":2,"path":"/srv/work","parent":"","dirs":[]}');
    expect((await inside).parent, '');

    final refused = conn.listDirs('/etc');
    expect(await fromClient.next as String, '{"type":"listDirs","requestId":3,"path":"/etc"}');
    controller.local.sink.add('{"type":"dirError","requestId":3,"message":"that folder is not one of yours"}');
    await expectLater(refused, throwsA(isA<ConversationException>().having((e) => e.message, 'message', contains('not one of yours'))));

    conn.sendChat('hi', conversationId: 'c1', workdir: '/srv/work');
    expect(await fromClient.next as String, '{"type":"chat","message":"hi","conversationId":"c1","workdir":"/srv/work"}');
  });

  test('a device token issued in HelloAck is exposed to the caller (P36)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: 'test-key',
    );
    await fromClient.next; // Hello
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server","deviceToken":"tok"}');

    expect((await future).issuedDeviceToken, 'tok');
  });

  test('being revoked mid-session reports the reason, not "closed unexpectedly" (P36)', () async {
    final controller = StreamChannelController<dynamic>();
    final fromClient = StreamQueue<dynamic>(controller.local.stream);

    final future = ServerConnection.connectOverChannel(
      channel: controller.foreign,
      deviceId: 'dev-1',
      deviceName: 'Test Device',
      authKey: '',
      deviceToken: 'tok',
    );
    expect(await fromClient.next as String, contains('"deviceToken":"tok"'));
    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');
    final conn = await future;
    expect(conn.issuedDeviceToken, isNull);

    final failed = conn.statusStream.firstWhere((s) => s is ConnectionFailure);
    controller.local.sink.add('{"type":"authError","reason":"device revoked"}');
    await controller.local.sink.close();
    await failed;

    expect((conn.status as ConnectionFailure).message, 'authentication rejected: device revoked');
  });
}
