import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/services/chat_transcript.dart';

/// The hub, in memory: a list of conversations and each one's history.
class FakeBackend implements ConversationBackend {
  final sent = <(String, String?)>[];
  var conversations = <ConversationSummary>[];
  final histories = <String, List<HistoryEntry>>{};

  /// When set, `fetchHistory` waits on this instead of answering right away.
  Completer<List<HistoryEntry>>? historyGate;
  Object? historyError;
  Object? listError;

  /// The agent each sent turn spoke as, in order (P87).
  final sentAgents = <String?>[];
  var agentIds = <String>[];

  @override
  void sendChat(String message, {String? conversationId, String? agentId}) {
    sent.add((message, conversationId));
    sentAgents.add(agentId);
  }

  @override
  Future<List<String>> listAgentIds() async => agentIds;

  @override
  Future<List<HistoryEntry>> fetchHistory({int? limit, String? conversationId}) async {
    if (historyError != null) throw historyError!;
    final gate = historyGate;
    if (gate != null) return gate.future;
    return histories[conversationId] ?? const [];
  }

  @override
  Future<List<ConversationSummary>> listConversations() async {
    if (listError != null) throw listError!;
    return conversations;
  }

  @override
  Future<void> renameConversation(String conversationId, String title) async {
    conversations = [
      for (final c in conversations)
        c.id == conversationId ? ConversationSummary(id: c.id, title: title, createdAt: c.createdAt, updatedAt: c.updatedAt) : c,
    ];
  }

  @override
  Future<void> deleteConversation(String conversationId) async {
    conversations = conversations.where((c) => c.id != conversationId).toList();
    histories.remove(conversationId);
  }
}

ConversationSummary summary(String id, [String? title]) =>
    ConversationSummary(id: id, title: title ?? 'title $id', createdAt: 0, updatedAt: 0);

/// Lets the transcript's async start (list, then history) run to completion.
Future<void> settle() async {
  for (var i = 0; i < 5; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  late StreamController<ServerMessage> replies;
  late FakeBackend backend;
  late int newIds;
  final opened = <String>[];

  ChatTranscript make({String? last}) {
    final transcript = ChatTranscript(
      chatStream: replies.stream,
      backend: backend,
      lastConversationId: last,
      onConversationOpened: opened.add,
      newConversationId: () => 'new-${newIds++}',
    );
    addTearDown(transcript.dispose);
    return transcript;
  }

  setUp(() {
    replies = StreamController<ServerMessage>.broadcast();
    backend = FakeBackend();
    newIds = 0;
    opened.clear();
  });

  tearDown(() => replies.close());

  test('send records the user turn, sends it to the open conversation, and waits for the reply', () async {
    final transcript = make();
    await settle();

    expect(transcript.send('  hello  '), isTrue);

    expect(backend.sent, [('hello', transcript.activeConversationId)]);
    expect(transcript.waitingForReply, isTrue);
    expect(transcript.entries.single.role, EntryRole.user);
    expect(transcript.entries.single.text, 'hello');
  });

  test('the agent follows the conversation, and a new one keeps the last choice (P87)', () async {
    backend.agentIds = ['chief', 'poet'];
    backend.conversations = [
      ConversationSummary(id: 'c1', title: 'with chief', createdAt: 0, updatedAt: 2, agentId: 'chief'),
      summary('c2'),
    ];
    final transcript = make(last: 'c1');
    await settle();

    expect(transcript.agentIds, ['chief', 'poet']);
    expect(transcript.selectedAgentId, 'chief', reason: 'restored from the conversation');
    transcript.send('hi');
    expect(backend.sentAgents.last, 'chief');

    transcript.open('c2');
    expect(transcript.selectedAgentId, isNull, reason: 'c2 spoke with no agent');
    transcript.selectAgent('poet');
    transcript.startNew();
    expect(transcript.selectedAgentId, 'poet', reason: 'a new conversation keeps the choice');
    transcript.send('new one');
    expect(backend.sentAgents.last, 'poet');
    expect(transcript.conversations.first.agentId, 'poet');
  });

  test('a changed conversation reloads the list and the open transcript (P87)', () async {
    backend.conversations = [summary('c1')];
    backend.histories['c1'] = [const HistoryEntry(fromUser: true, content: 'note from ana')];
    final transcript = make(last: 'c1');
    await settle();
    expect(transcript.entries.single.text, 'note from ana');

    backend.histories['c1'] = [
      const HistoryEntry(fromUser: true, content: 'note from ana'),
      const HistoryEntry(fromUser: false, content: 'answer from bia'),
    ];
    backend.conversations = [summary('c1'), summary('agents-x', 'ana → bia')];
    replies.add(const ConversationsChangedMessage('c1'));
    await settle();

    expect(transcript.entries.map((e) => e.text), ['note from ana', 'answer from bia']);
    expect(transcript.conversations.map((c) => c.id), contains('agents-x'));
  });

  test('blank text and a pending reply are both ignored', () async {
    final transcript = make();
    await settle();

    expect(transcript.send('   '), isFalse);
    transcript.send('first');
    expect(transcript.send('second'), isFalse);

    expect(backend.sent, hasLength(1));
    expect(transcript.entries, hasLength(1));
  });

  test('a reply and an error are appended and clear the waiting state', () async {
    final transcript = make();
    await settle();

    transcript.send('q');
    replies.add(ChatResponseMessage('answer', null, conversationId: transcript.activeConversationId));
    await settle();

    expect(transcript.waitingForReply, isFalse);
    expect(transcript.entries.map((e) => e.role), [EntryRole.user, EntryRole.assistant]);
    expect(transcript.entries.last.text, 'answer');

    transcript.send('q2');
    replies.add(const ChatErrorMessage('boom'));
    await settle();

    expect(transcript.entries.last.role, EntryRole.error);
    expect(transcript.waitingForReply, isFalse);
  });

  test('a reply that arrives with no screen attached is still kept (P41)', () async {
    final transcript = make();
    await settle();

    transcript.send('q');
    replies.add(const ChatResponseMessage('late answer', null));
    await settle();

    expect(transcript.entries.last.text, 'late answer');
  });

  group('history (P40)', () {
    test('starts with the persisted conversation, before anything sent meanwhile', () async {
      backend.conversations = [summary('c1')];
      backend.historyGate = Completer<List<HistoryEntry>>();
      final transcript = make(last: 'c1');
      await settle();

      transcript.send('new question');
      backend.historyGate!.complete(const [
        HistoryEntry(fromUser: true, content: 'old question'),
        HistoryEntry(fromUser: false, content: 'old answer'),
      ]);
      await settle();

      expect(transcript.entries.map((e) => e.text), ['old question', 'old answer', 'new question']);
      expect(transcript.entries.map((e) => e.role), [EntryRole.user, EntryRole.assistant, EntryRole.user]);
      expect(transcript.waitingForReply, isTrue);
    });

    test('a failed fetch shows one error entry and keeps the chat usable', () async {
      backend.historyError = Exception('connection dropped');
      final transcript = make();
      await settle();

      expect(transcript.entries.single.role, EntryRole.error);
      expect(transcript.entries.single.text, contains('connection dropped'));
      expect(transcript.send('still works'), isTrue);
    });

    test('a history that lands after dispose is ignored', () async {
      backend.historyGate = Completer<List<HistoryEntry>>();
      final transcript = ChatTranscript(chatStream: replies.stream, backend: backend);
      await settle();
      transcript.dispose();

      backend.historyGate!.complete(const [HistoryEntry(fromUser: true, content: 'late')]);
      await settle();

      expect(transcript.entries, isEmpty);
    });
  });

  group('conversations (P78)', () {
    test('reopens the last conversation when the hub still has it', () async {
      backend.conversations = [summary('recent'), summary('older')];
      backend.histories['older'] = const [HistoryEntry(fromUser: true, content: 'from older')];
      final transcript = make(last: 'older');
      await settle();

      expect(transcript.activeConversationId, 'older');
      expect(transcript.activeTitle, 'title older');
      expect(transcript.entries.single.text, 'from older');
      expect(opened, ['older']);
    });

    test('falls back to the most recent one, or a new one when there are none', () async {
      backend.conversations = [summary('recent')];
      final withList = make(last: 'deleted-elsewhere');
      await settle();
      expect(withList.activeConversationId, 'recent');

      backend.conversations = [];
      final empty = make();
      await settle();
      expect(empty.activeConversationId, startsWith('new-'));
      expect(empty.activeTitle, isNull);
    });

    test('opening another conversation swaps the transcript', () async {
      backend.conversations = [summary('a'), summary('b')];
      backend.histories['a'] = const [HistoryEntry(fromUser: true, content: 'in a')];
      backend.histories['b'] = const [HistoryEntry(fromUser: true, content: 'in b')];
      final transcript = make(last: 'a');
      await settle();

      transcript.open('b');
      expect(transcript.entries, isEmpty);
      await settle();

      expect(transcript.entries.single.text, 'in b');
      expect(opened.last, 'b');
    });

    test('an answer for a conversation not on screen stays out of it, and the wait is per conversation', () async {
      backend.conversations = [summary('a'), summary('b')];
      final transcript = make(last: 'a');
      await settle();

      transcript.send('question in a');
      transcript.open('b');
      await settle();
      expect(transcript.waitingForReply, isFalse);
      expect(transcript.send('question in b'), isTrue);

      replies.add(const ChatResponseMessage('answer for a', null, conversationId: 'a'));
      await settle();

      expect(transcript.entries.map((e) => e.text), ['question in b']);
      expect(transcript.isAnswering('a'), isFalse);
      expect(transcript.isAnswering('b'), isTrue);
    });

    test('switching back to a conversation still being answered shows the question again', () async {
      backend.conversations = [summary('a'), summary('b')];
      backend.histories['a'] = const [HistoryEntry(fromUser: true, content: 'earlier')];
      final transcript = make(last: 'a');
      await settle();

      transcript.send('still waiting');
      transcript.open('b');
      await settle();
      transcript.open('a');
      await settle();

      expect(transcript.entries.map((e) => e.text), ['earlier', 'still waiting']);
      expect(transcript.waitingForReply, isTrue);
    });

    test('a new conversation is listed as soon as its first message is sent', () async {
      final transcript = make();
      await settle();

      transcript.send('Plan a trip to Lisbon');

      expect(transcript.conversations.single.id, transcript.activeConversationId);
      expect(transcript.activeTitle, 'Plan a trip to Lisbon');
    });

    test('startNew opens an empty conversation, but not twice in a row', () async {
      backend.conversations = [summary('a')];
      final transcript = make(last: 'a');
      await settle();

      transcript.startNew();
      final fresh = transcript.activeConversationId;
      expect(fresh, startsWith('new-'));
      transcript.startNew();
      expect(transcript.activeConversationId, fresh);
    });

    test('rename refreshes the list; deleting the open conversation opens the most recent one left', () async {
      backend.conversations = [summary('a'), summary('b')];
      backend.histories['b'] = const [HistoryEntry(fromUser: true, content: 'in b')];
      final transcript = make(last: 'a');
      await settle();

      await transcript.rename('a', 'Lisbon');
      expect(transcript.activeTitle, 'Lisbon');

      await transcript.delete('a');
      await settle();
      expect(transcript.conversations.map((c) => c.id), ['b']);
      expect(transcript.activeConversationId, 'b');
      expect(transcript.entries.single.text, 'in b');
    });

    test('a list that fails to load is reported and the chat still works', () async {
      backend.listError = Exception('hub is down');
      final transcript = make();
      await settle();

      expect(transcript.conversationsError, contains('hub is down'));
      expect(transcript.send('hello'), isTrue);
    });
  });

  test('titleFrom matches the hub: collapsed whitespace, 40 characters and an ellipsis', () {
    expect(titleFrom('  a   b\nc '), 'a b c');
    expect(titleFrom('x' * 45), '${'x' * 40}…');
  });
}
