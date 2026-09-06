import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';

// Keeps this file honest against `crates/warden-server/src/protocol.rs` —
// every literal JSON string here matches what the Rust side's own
// serialization tests assert.
void main() {
  group('ClientMessage encoding', () {
    test('Hello with no tools', () {
      const msg = HelloMessage(deviceId: 'dev-1', deviceName: 'Test Device', authKey: 'secret');
      expect(
        msg.encode(),
        '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret","tools":[]}',
      );
    });

    test('Hello with advertised tools', () {
      const msg = HelloMessage(
        deviceId: 'dev-1',
        deviceName: 'Test Device',
        authKey: 'secret',
        tools: [
          {'name': 'list_files', 'description': 'List files', 'parameters': {'type': 'object'}},
        ],
      );
      expect(
        msg.encode(),
        '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret",'
        '"tools":[{"name":"list_files","description":"List files","parameters":{"type":"object"}}]}',
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

    test('Chat', () {
      const msg = ChatMessage('hello there');
      expect(msg.encode(), '{"type":"chat","message":"hello there"}');
    });

    test('ToolCallResult', () {
      const msg = ToolCallResultMessage(7, {'ok': true});
      expect(msg.encode(), '{"type":"toolCallResult","callId":7,"result":{"ok":true}}');
    });

    test('ToolCallError', () {
      const msg = ToolCallErrorMessage(7, 'boom');
      expect(msg.encode(), '{"type":"toolCallError","callId":7,"message":"boom"}');
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

    test('ChatResponse with usage', () {
      final msg = ServerMessage.decode(
        '{"type":"chatResponse","content":"ahoy","usage":{"promptTokens":1,"completionTokens":2,"totalTokens":3}}',
      );
      expect(msg, isA<ChatResponseMessage>());
      final response = msg as ChatResponseMessage;
      expect(response.content, 'ahoy');
      expect(response.usage?.promptTokens, 1);
      expect(response.usage?.completionTokens, 2);
      expect(response.usage?.totalTokens, 3);
    });

    test('ChatResponse without usage', () {
      final msg = ServerMessage.decode('{"type":"chatResponse","content":"ahoy","usage":null}');
      expect(msg, isA<ChatResponseMessage>());
      expect((msg as ChatResponseMessage).usage, isNull);
    });

    test('ChatError', () {
      final msg = ServerMessage.decode('{"type":"chatError","message":"provider unavailable"}');
      expect(msg, isA<ChatErrorMessage>());
      expect((msg as ChatErrorMessage).message, 'provider unavailable');
    });

    test('ToolCallRequest', () {
      final msg = ServerMessage.decode('{"type":"toolCallRequest","callId":3,"tool":"read_file","arguments":{"path":"abc"}}');
      expect(msg, isA<ToolCallRequestMessage>());
      final request = msg as ToolCallRequestMessage;
      expect(request.callId, 3);
      expect(request.tool, 'read_file');
      expect(request.arguments, {'path': 'abc'});
    });

    test('unknown type throws FormatException', () {
      expect(
        () => ServerMessage.decode('{"type":"somethingElse"}'),
        throwsA(isA<FormatException>()),
      );
    });
  });
}
