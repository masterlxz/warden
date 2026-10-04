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
        '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret","tools":[],"recoveryCodes":true}',
      );
    });

    test('Hello with a device token (P36)', () {
      const msg = HelloMessage(deviceId: 'dev-1', deviceName: 'Test Device', authKey: '', deviceToken: 'tok');
      expect(
        msg.encode(),
        '{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"","deviceToken":"tok","tools":[],"recoveryCodes":true}',
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
        '"tools":[{"name":"list_files","description":"List files","parameters":{"type":"object"}}],"recoveryCodes":true}',
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

    test('an unknown type is skipped, not an error', () {
      final msg = ServerMessage.decode('{"type":"somethingElse"}');
      expect(msg, isA<UnknownServerMessage>());
      expect((msg as UnknownServerMessage).type, 'somethingElse');
      expect(() => ServerMessage.decode('{"text":"no type"}'), throwsA(isA<FormatException>()));
    });

    test('agents, approvals and changed conversations (P87)', () {
      expect(const ChatMessage('hi', conversationId: 'c1', agentId: 'chief').toJson(),
          {'type': 'chat', 'message': 'hi', 'conversationId': 'c1', 'agentId': 'chief'});
      expect(const ChatMessage('hi').toJson(), {'type': 'chat', 'message': 'hi'});
      expect(const RequestSettingsMessage(4).toJson(), {'type': 'requestSettings', 'requestId': 4});
      expect(const ResolveApprovalMessage(3, true).toJson(), {'type': 'resolveApproval', 'approvalId': 3, 'approved': true});

      final settings = ServerMessage.decode(
          '{"type":"settings","requestId":4,"version":"v","secretsWritable":false,"settings":{"agents":[{"id":"chief","persona":"p"},{"id":"poet","persona":"q"}],"other":1}}');
      expect((settings as SettingsMessage).agentIds, ['chief', 'poet']);

      final ask = ServerMessage.decode('{"type":"approvalRequest","approvalId":3,"target":"poet","action":"create_agent","detail":"d"}');
      expect(ask, isA<ApprovalRequestMessage>());
      expect((ask as ApprovalRequestMessage).target, 'poet');
      expect((ServerMessage.decode('{"type":"approvalCancelled","approvalId":3}') as ApprovalCancelledMessage).approvalId, 3);
      expect((ServerMessage.decode('{"type":"conversationsChanged","conversationId":"c"}') as ConversationsChangedMessage).conversationId, 'c');

      final list = ServerMessage.decode(
          '{"type":"conversationList","requestId":1,"conversations":[{"id":"c1","title":"T","createdAt":1,"updatedAt":2,"agentId":"chief"}]}');
      expect((list as ConversationListMessage).conversations.single.agentId, 'chief');
    });
  });

  group('P102 working folder messages', () {
    // The literals are the ones `warden-server-protocol`'s own serialization test asserts.
    test('the client messages are the ones the hub expects', () {
      expect(const ListDirsMessage(1).encode(), '{"type":"listDirs","requestId":1}');
      expect(const ListDirsMessage(2, path: '/srv').encode(), '{"type":"listDirs","requestId":2,"path":"/srv"}');
      expect(const ChatMessage('hi', conversationId: 'c1', workdir: '/srv/work').toJson(),
          {'type': 'chat', 'message': 'hi', 'conversationId': 'c1', 'workdir': '/srv/work'});
      expect(const ChatMessage('hi').toJson().containsKey('workdir'), isFalse, reason: 'no folder, nothing on the wire');
    });

    test('the folder list, its top, and its error', () {
      final list = ServerMessage.decode('{"type":"dirList","requestId":2,"path":"/srv","parent":"/","dirs":[{"name":"work","path":"/srv/work"}]}')
          as DirListMessage;
      expect(list.requestId, 2);
      expect(list.path, '/srv');
      expect(list.parent, '/');
      expect(list.dirs.single.name, 'work');
      expect(list.dirs.single.path, '/srv/work');

      final top = ServerMessage.decode('{"type":"dirList","requestId":3,"path":"","dirs":[]}') as DirListMessage;
      expect(top.path, isEmpty);
      expect(top.parent, isNull, reason: 'the top of what the person may see has no parent');
      expect(top.dirs, isEmpty);

      final error = ServerMessage.decode('{"type":"dirError","requestId":4,"message":"m"}') as DirErrorMessage;
      expect(error.requestId, 4);
      expect(error.message, 'm');
    });

    test('a conversation summary carries its folder when it has one', () {
      final list = ServerMessage.decode(
          '{"type":"conversationList","requestId":1,"conversations":[{"id":"c1","title":"T","createdAt":1,"updatedAt":2,"workdir":"/srv/work"},{"id":"c2","title":"U","createdAt":1,"updatedAt":2}]}')
          as ConversationListMessage;
      expect(list.conversations.first.workdir, '/srv/work');
      expect(list.conversations.last.workdir, isNull);
    });
  });

  group('P84 recovery and TruthID messages', () {
    test('the client messages are the ones the hub expects', () {
      expect(const ChangePasswordMessage(1, 'old', 'new').encode(), '{"type":"changePassword","requestId":1,"oldPassword":"old","newPassword":"new"}');
      expect(
        const ChangePasswordMessage(1, 'old', 'new', recoveryCode: 'AAAA').encode(),
        '{"type":"changePassword","requestId":1,"oldPassword":"old","newPassword":"new","recoveryCode":"AAAA"}',
      );
      expect(const RegenerateRecoveryCodeMessage(2, 'pw').encode(), '{"type":"regenerateRecoveryCode","requestId":2,"password":"pw"}');
      expect(const AcceptRecoveryPolicyMessage(3, 'pw').encode(), '{"type":"acceptRecoveryPolicy","requestId":3,"password":"pw"}');
      expect(const AckRecoveryNoticesMessage(4).encode(), '{"type":"ackRecoveryNotices","requestId":4}');
      expect(const RedeemInviteMessage(5, 'ana:x', 'ana.silva').encode(), '{"type":"redeemInvite","requestId":5,"code":"ana:x","username":"ana.silva"}');
    });

    test('the server messages decode', () {
      expect(ServerMessage.decode('{"type":"passwordChanged","requestId":1,"recoveryCode":"CODE"}'), isA<PasswordChangedMessage>().having((m) => m.recoveryCode, 'code', 'CODE'));
      expect(ServerMessage.decode('{"type":"passwordChanged","requestId":1}'), isA<PasswordChangedMessage>().having((m) => m.recoveryCode, 'code', isNull));
      expect(ServerMessage.decode('{"type":"recoveryCode","requestId":0,"code":"C"}'), isA<RecoveryCodeMessage>().having((m) => m.requestId, 'id', 0));
      expect(ServerMessage.decode('{"type":"recoveryPolicyAccepted","requestId":2}'), isA<RecoveryPolicyAcceptedMessage>());
      expect(ServerMessage.decode('{"type":"recoveryNoticesAcked","requestId":3}'), isA<RecoveryNoticesAckedMessage>());
      expect(ServerMessage.decode('{"type":"truthIdLinked","requestId":4,"username":"ana.silva"}'), isA<TruthIdLinkedMessage>().having((m) => m.username, 'username', 'ana.silva'));
    });

    test('a HelloAck user carries the state of their data', () {
      final ack = ServerMessage.decode(
        '{"type":"helloAck","serverName":"hub","user":{"id":"ana","name":"Ana","role":"member","mustChangePassword":false,'
        '"encrypted":true,"needsRecovery":true,"locked":true,"memberPolicy":"private","policyPending":true,"recoveryPolicy":"company",'
        '"recoveries":[{"atMs":5,"kind":"company","seen":false},{"atMs":4,"kind":"consent","seen":true}],"truthid":"ana.silva"}}',
      ) as HelloAckMessage;
      final user = ack.user!;
      expect((user.encrypted, user.needsRecovery, user.locked, user.policyPending), (true, true, true, true));
      expect((user.memberPolicy, user.recoveryPolicy, user.truthid), ('private', 'company', 'ana.silva'));
      expect(user.unseenRecoveries.map((e) => e.atMs), [5]);
      // A hub from before knows none of it.
      final old = (ServerMessage.decode('{"type":"helloAck","serverName":"hub","user":{"id":"ana","name":"Ana","role":"member"}}') as HelloAckMessage).user!;
      expect((old.encrypted, old.locked, old.recoveries.isEmpty, old.truthid), (false, false, true, ''));
    });
  });
}
