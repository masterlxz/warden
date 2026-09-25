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

    test('Hello with a device token (P36)', () {
      const msg = HelloMessage(deviceId: 'dev-1', deviceName: 'Test Device', authKey: '', deviceToken: 'tok');
      expect(
        msg.encode(),
        '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"","deviceToken":"tok","tools":[]}',
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

    test('Chat to a named conversation (P78)', () {
      const msg = ChatMessage('hi', conversationId: 'c1');
      expect(msg.encode(), '{"type":"chat","message":"hi","conversationId":"c1"}');
    });

    test('conversation requests (P78)', () {
      expect(const RequestHistoryMessage(1, limit: 5, conversationId: 'c1').encode(),
          '{"type":"requestHistory","requestId":1,"limit":5,"conversationId":"c1"}');
      expect(const ListConversationsMessage(2).encode(), '{"type":"listConversations","requestId":2}');
      expect(const RenameConversationMessage(3, 'c1', 'Trip').encode(),
          '{"type":"renameConversation","requestId":3,"conversationId":"c1","title":"Trip"}');
      expect(const DeleteConversationMessage(4, 'c1').encode(),
          '{"type":"deleteConversation","requestId":4,"conversationId":"c1"}');
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
      expect(msg.deviceToken, isNull);
    });

    test('HelloAck with an issued device token (P36)', () {
      final msg = ServerMessage.decode('{"type":"helloAck","serverName":"warden-server","deviceToken":"tok"}');
      expect((msg as HelloAckMessage).deviceToken, 'tok');
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
      expect(msg.conversationId, isNull);
    });

    test('ChatResponse and ChatError name their conversation (P78)', () {
      final response = ServerMessage.decode('{"type":"chatResponse","content":"a","usage":null,"conversationId":"c1"}');
      expect((response as ChatResponseMessage).conversationId, 'c1');
      final error = ServerMessage.decode('{"type":"chatError","message":"boom","conversationId":"c2"}');
      expect((error as ChatErrorMessage).conversationId, 'c2');
    });

    test('conversation replies (P78)', () {
      final list = ServerMessage.decode(
        '{"type":"conversationList","requestId":1,"conversations":[{"id":"c1","title":"Trip","createdAt":1,"updatedAt":2}]}',
      ) as ConversationListMessage;
      expect(list.requestId, 1);
      expect(list.conversations.single.id, 'c1');
      expect(list.conversations.single.title, 'Trip');
      expect(list.conversations.single.updatedAt, 2);

      expect((ServerMessage.decode('{"type":"conversationOk","requestId":2}') as ConversationOkMessage).requestId, 2);
      final error = ServerMessage.decode('{"type":"conversationError","requestId":3,"message":"no conversation"}');
      expect((error as ConversationErrorMessage).message, 'no conversation');
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
