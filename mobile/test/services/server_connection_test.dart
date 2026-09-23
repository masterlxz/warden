import 'dart:async';

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
      '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"test-key","tools":[]}',
    );

    controller.local.sink.add('{"type":"helloAck","serverName":"warden-server"}');

    final conn = await future;
    expect(conn.status, isA<Connected>());
    expect((conn.status as Connected).serverName, 'warden-server');
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
      '"tools":[{"name":"list_files","description":"List files","parameters":{"type":"object"}}]}',
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
    expect(await fromClient.next as String, '{"type":"requestHistory","requestId":0,"limit":100}');

    controller.local.sink.add('{"type":"history","requestId":0,"messages":['
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
    controller.local.sink.add('{"type":"historyError","requestId":0,"message":"failed to parse"}');

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
