import 'dart:async';

import 'package:async/async.dart';
import 'package:flutter_test/flutter_test.dart';
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
      '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"test-key"}',
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
}
