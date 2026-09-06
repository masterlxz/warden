import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';

// Keeps this file honest against `crates/warden-server/src/protocol.rs` —
// every literal JSON string here matches what the Rust side's own
// serialization tests assert.
void main() {
  group('ClientMessage encoding', () {
    test('Hello', () {
      const msg = HelloMessage(deviceId: 'dev-1', deviceName: 'Test Device', authKey: 'secret');
      expect(
        msg.encode(),
        '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret"}',
      );
    });

    test('Ping', () {
      const msg = PingMessage(42);
      expect(msg.encode(), '{"type":"ping","nonce":42}');
    });

    test('Goodbye with a reason', () {
      const msg = GoodbyeMessage('user closed app');
      expect(msg.encode(), '{"type":"goodbye","reason":"user closed app"}');
    });

    test('Goodbye without a reason serializes reason as null, not omitted', () {
      const msg = GoodbyeMessage();
      expect(msg.encode(), '{"type":"goodbye","reason":null}');
    });
  });

  group('ServerMessage decoding', () {
    test('HelloAck', () {
      final msg = ServerMessage.decode('{"type":"helloAck","serverName":"warden-server"}');
      expect(msg, isA<HelloAckMessage>());
      expect((msg as HelloAckMessage).serverName, 'warden-server');
    });

    test('AuthError', () {
      final msg = ServerMessage.decode('{"type":"authError","reason":"invalid auth key"}');
      expect(msg, isA<AuthErrorMessage>());
      expect((msg as AuthErrorMessage).reason, 'invalid auth key');
    });

    test('Pong echoes the exact nonce', () {
      final msg = ServerMessage.decode('{"type":"pong","nonce":42}');
      expect(msg, isA<PongMessage>());
      expect((msg as PongMessage).nonce, 42);
    });

    test('Goodbye with a null reason', () {
      final msg = ServerMessage.decode('{"type":"goodbye","reason":null}');
      expect(msg, isA<GoodbyeServerMessage>());
      expect((msg as GoodbyeServerMessage).reason, isNull);
    });

    test('unknown type throws FormatException', () {
      expect(
        () => ServerMessage.decode('{"type":"somethingElse"}'),
        throwsA(isA<FormatException>()),
      );
    });
  });
}
