import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/services/channel_unread.dart';
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

  /// The folder each sent turn carried, in order (P102).
  final sentWorkdirs = <String?>[];

  /// The paths the picker asked to list, in order (P102).
  final listedDirs = <String?>[];

  @override
  void sendChat(String message, {String? conversationId, String? agentId, String? workdir, ThreadParent? threadOf}) {
    sent.add((message, conversationId));
    sentAgents.add(agentId);
    sentWorkdirs.add(workdir);
    sentThreadOf.add(threadOf);
  }

  /// The thread link each sent turn carried, in order (P125).
  final sentThreadOf = <ThreadParent?>[];

  /// The hub makes the id of an agent's channel (P121); here it is `channel-<agent>`.
  final askedChannels = <String>[];

  @override
  Future<String> openAgentChannel(String agentId) async {
    askedChannels.add(agentId);
    return 'channel-$agentId';
  }

  @override
  Future<DirListMessage> listDirs([String? path]) async {
    listedDirs.add(path);
    return DirListMessage(1, path: path ?? '', dirs: const []);
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

/// The marks of the agents' channels, in memory (P121).
class FakeSeenStore implements ChannelSeenStore {
  FakeSeenStore({Map<String, int>? initial}) : saved = {...?initial}, _first = initial;

  Map<String, int> saved;
  final Map<String, int>? _first;

  @override
  Future<Map<String, int>?> load() async => _first;

  @override
  Future<void> save(Map<String, int> seen) async => saved = {...seen};
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

  test('a folder is picked before the first message, travels with it only, and is fixed afterwards (P102)', () async {
    final transcript = make();
    await settle();

    expect(transcript.canPickFolder, isTrue);
    expect(transcript.workdir, isNull);
    transcript.selectWorkdir('/srv/work/alpha');
    expect(transcript.workdir, '/srv/work/alpha');

    transcript.send('organize this');
    expect(backend.sentWorkdirs.last, '/srv/work/alpha', reason: 'the first message carries it');
    expect(transcript.conversations.first.workdir, '/srv/work/alpha');
    expect(transcript.canPickFolder, isFalse, reason: 'the conversation has begun');
    transcript.selectWorkdir('/elsewhere');
    transcript.selectWorkdir(null);
    expect(transcript.workdir, '/srv/work/alpha', reason: 'neither changed nor cleared after the first message');

    replies.add(const ChatResponseMessage('done', null));
    await settle();
    backend.conversations = [ConversationSummary(id: transcript.activeConversationId, title: 't', createdAt: 0, updatedAt: 1, workdir: '/srv/work/alpha')];
    await transcript.refreshConversations();
    transcript.send('and now?');
    expect(backend.sentWorkdirs.last, isNull, reason: 'only the message that creates the conversation carries the folder');
  });

  test('no folder travels when none was picked, and clearing one before the first message removes it (P102)', () async {
    final transcript = make();
    await settle();

    transcript.selectWorkdir('/srv/work');
    transcript.selectWorkdir(null);
    expect(transcript.workdir, isNull);
    transcript.send('hi');
    expect(backend.sentWorkdirs.last, isNull);
  });

  test('each conversation shows its own folder, and a new one starts with none (P102)', () async {
    backend.conversations = [
      ConversationSummary(id: 'c1', title: 'in a folder', createdAt: 0, updatedAt: 2, workdir: '/srv/work'),
      summary('c2'),
    ];
    final transcript = make(last: 'c1');
    await settle();

    expect(transcript.workdir, '/srv/work', reason: 'restored from the conversation');
    expect(transcript.canPickFolder, isFalse);

    transcript.open('c2');
    expect(transcript.workdir, isNull, reason: 'c2 has no folder');
    expect(transcript.canPickFolder, isFalse, reason: 'c2 already exists on the hub');

    transcript.selectWorkdir('/not-allowed-here');
    expect(transcript.workdir, isNull);

    transcript.open('c1');
    transcript.startNew();
    expect(transcript.workdir, isNull, reason: 'a folder is never carried over to a new conversation');
    expect(transcript.canPickFolder, isTrue);
  });

  test('the picker lists through the backend (P102)', () async {
    final transcript = make();
    await settle();
    await transcript.listDirs();
    await transcript.listDirs('/srv/work');
    expect(backend.listedDirs, [null, '/srv/work']);
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

  group('what an agent starts in its channel (P121)', () {
    ConversationSummary channelAt(String agent, int updatedAt) =>
        ConversationSummary(id: 'channel-$agent', title: agent, createdAt: 0, updatedAt: updatedAt, agentId: agent);

    final told = <(String, String)>[];

    ChatTranscript withMarks({String? last, FakeSeenStore? store}) {
      final transcript = ChatTranscript(
        chatStream: replies.stream,
        backend: backend,
        lastConversationId: last,
        seenStore: store,
        onAgentMessage: (agent, text) => told.add((agent, text)),
        newConversationId: () => 'new-${newIds++}',
      );
      addTearDown(transcript.dispose);
      return transcript;
    }

    setUp(told.clear);

    test('what is there on the first run counts as seen, and a later message is unread and told once', () async {
      backend.agentIds = ['poet'];
      backend.conversations = [summary('main'), channelAt('poet', 10)];
      final store = FakeSeenStore();
      final transcript = withMarks(last: 'main', store: store);
      await settle();
      expect(transcript.unreadChannelIds, isEmpty, reason: 'the baseline');
      expect(store.saved, {'channel-poet': 10}, reason: 'kept for the next run');

      backend.conversations = [summary('main'), channelAt('poet', 20)];
      backend.histories['channel-poet'] = [const HistoryEntry(id: 'm1', fromUser: false, content: 'The disk is full')];
      replies.add(const ConversationsChangedMessage('channel-poet'));
      await settle();

      expect(transcript.unreadChannelIds, ['channel-poet']);
      expect(transcript.unreadAgents, {'poet'});
      expect(told, [('poet', 'The disk is full')]);
    });

    test('opening the channel reads it, and what comes while it is in front is not told', () async {
      backend.agentIds = ['poet'];
      backend.conversations = [summary('main'), channelAt('poet', 10)];
      final store = FakeSeenStore(initial: {'channel-poet': 1});
      final transcript = withMarks(last: 'main', store: store);
      await settle();
      expect(transcript.unreadChannelIds, ['channel-poet'], reason: 'it changed after the last time it was shown');

      await transcript.openAgentChannel('poet');
      await settle();
      expect(transcript.unreadChannelIds, isEmpty);
      expect(store.saved['channel-poet'], 10);

      backend.conversations = [summary('main'), channelAt('poet', 30)];
      backend.histories['channel-poet'] = [const HistoryEntry(id: 'm2', fromUser: false, content: 'more')];
      replies.add(const ConversationsChangedMessage('channel-poet'));
      await settle();
      expect(transcript.unreadChannelIds, isEmpty, reason: 'it is being read as it arrives');
      expect(told, isEmpty);
    });

    test('with the app in the background even the open channel is unread and told', () async {
      backend.agentIds = ['poet'];
      backend.conversations = [channelAt('poet', 10)];
      final transcript = withMarks(last: 'channel-poet', store: FakeSeenStore());
      await settle();
      await transcript.openAgentChannel('poet');
      await settle();

      transcript.setForeground(false);
      backend.conversations = [channelAt('poet', 40)];
      backend.histories['channel-poet'] = [const HistoryEntry(id: 'm3', fromUser: false, content: 'while you were away')];
      replies.add(const ConversationsChangedMessage('channel-poet'));
      await settle();

      expect(transcript.unreadChannelIds, ['channel-poet']);
      expect(told, [('poet', 'while you were away')]);

      transcript.setForeground(true);
      expect(transcript.unreadChannelIds, isEmpty, reason: 'back on screen, the open channel is read');
    });

    test('a change that is not an agent message, or a loose conversation, tells nobody', () async {
      backend.agentIds = ['poet'];
      backend.conversations = [summary('main'), channelAt('poet', 10)];
      final transcript = withMarks(last: 'main', store: FakeSeenStore());
      await settle();

      backend.conversations = [summary('main'), channelAt('poet', 20)];
      backend.histories['channel-poet'] = [const HistoryEntry(id: 'm4', fromUser: true, content: 'my own words')];
      replies.add(const ConversationsChangedMessage('channel-poet'));
      replies.add(const ConversationsChangedMessage('main'));
      await settle();

      expect(told, isEmpty, reason: 'the last message is the person\'s own, and "main" is no channel');
      expect(transcript.unreadChannelIds, ['channel-poet'], reason: 'still marked: the list changed');
    });
  });

  group('agent channels (P121)', () {
    test('the list leaves the channels out, and the agents are asked for their channel once', () async {
      backend.agentIds = ['poet', 'chief'];
      backend.conversations = [summary('channel-poet', 'hi poet'), summary('main')];
      final transcript = make(last: 'main');
      await settle();

      expect(transcript.visibleConversations.map((c) => c.id), ['main']);
      expect(transcript.channels, {'poet': 'channel-poet', 'chief': 'channel-chief'});
      await transcript.refreshAgents();
      await settle();
      expect(backend.askedChannels, ['poet', 'chief'], reason: 'asked once per agent');
    });

    test('opening one shows it with the agent fixed and no folder; the first message goes to it', () async {
      backend.agentIds = ['poet'];
      backend.conversations = [ConversationSummary(id: 'main', title: 'main', createdAt: 0, updatedAt: 0, workdir: '/srv')];
      final transcript = make(last: 'main');
      await settle();
      expect(transcript.workdir, '/srv');

      await transcript.openAgentChannel('poet');
      await settle();
      expect(transcript.activeConversationId, 'channel-poet');
      expect(transcript.channelAgent, 'poet');
      expect(transcript.selectedAgentId, 'poet');
      expect(transcript.workdir, isNull);

      transcript.send('oi');
      expect(backend.sent.last, ('oi', 'channel-poet'));
      expect(backend.sentAgents.last, 'poet');
      expect(backend.sentWorkdirs.last, isNull);
    });

    test('leaving goes to the first loose conversation, or a new one when there is none', () async {
      backend.agentIds = ['poet'];
      backend.conversations = [summary('channel-poet'), summary('main')];
      final transcript = make(last: 'channel-poet');
      await settle();
      expect(transcript.channelAgent, 'poet');

      transcript.leaveChannel();
      expect(transcript.activeConversationId, 'main');
      expect(transcript.channelAgent, isNull);

      backend.conversations = [summary('channel-poet')];
      final alone = make(last: 'channel-poet');
      await settle();
      alone.leaveChannel();
      expect(alone.activeConversationId, startsWith('new-'));
    });

    test('starts on the first conversation that is not a channel', () async {
      backend.conversations = [summary('channel-poet'), summary('main')];
      final transcript = make();
      await settle();
      expect(transcript.activeConversationId, 'main');
    });
  });

  group('threads (P125)', () {
    ConversationSummary threadOf(String id, String parent, String messageId, int replies) => ConversationSummary(
          id: id,
          title: 'thread $id',
          createdAt: 0,
          updatedAt: 0,
          parent: ThreadParent(conversationId: parent, messageId: messageId),
          replies: replies,
        );

    test('the list leaves the threads out and they show from the message they came from', () async {
      backend.conversations = [summary('main'), threadOf('t1', 'main', 'm1', 2), threadOf('t2', 'other', 'm9', 1)];
      backend.histories['main'] = [const HistoryEntry(id: 'm1', fromUser: true, content: 'q')];
      final transcript = make(last: 'main');
      await settle();

      expect(transcript.visibleConversations.map((c) => c.id), ['main']);
      expect(transcript.threads.keys, ['m1']);
      expect(transcript.threads['m1']!.replies, 2);
      expect(transcript.entries.single.id, 'm1');
      expect(transcript.threadParent, isNull);
    });

    test('opening one that has no thread yet starts an empty conversation; its first reply carries the link and no folder', () async {
      backend.conversations = [ConversationSummary(id: 'main', title: 'main', createdAt: 0, updatedAt: 0, workdir: '/srv')];
      backend.histories['main'] = [const HistoryEntry(id: 'm1', fromUser: false, content: 'a')];
      final transcript = make(last: 'main');
      await settle();

      transcript.openThread('m1');
      expect(transcript.activeConversationId, 'new-0');
      expect(transcript.entries, isEmpty);
      expect(transcript.threadParent?.messageId, 'm1');

      transcript.send('and then?');
      expect(backend.sentThreadOf.last?.conversationId, 'main');
      expect(backend.sentThreadOf.last?.messageId, 'm1');
      expect(backend.sentWorkdirs.last, isNull, reason: 'the hub gives the thread the parent folder');
      expect(transcript.visibleConversations.map((c) => c.id), ['main'], reason: 'a thread never shows in the list, even before the hub has it');
    });

    test('a message that has a thread opens it, and back returns to the conversation', () async {
      backend.conversations = [summary('main'), threadOf('t1', 'main', 'm1', 1)];
      backend.histories['main'] = [const HistoryEntry(id: 'm1', fromUser: false, content: 'a')];
      backend.histories['t1'] = [const HistoryEntry(id: 't1-0', fromUser: true, content: 'side')];
      final transcript = make(last: 'main');
      await settle();

      transcript.openThread('m1');
      await settle();
      expect(transcript.activeConversationId, 't1');
      expect(transcript.entries.single.text, 'side');
      expect(transcript.threadParent?.conversationId, 'main');

      transcript.openThread('t1-0');
      expect(transcript.activeConversationId, 't1', reason: 'no thread inside a thread');

      transcript.closeThread();
      await settle();
      expect(transcript.activeConversationId, 'main');
    });

    test('leaving an unsent thread drops its draft', () async {
      backend.conversations = [summary('main')];
      backend.histories['main'] = [const HistoryEntry(id: 'm1', fromUser: false, content: 'a')];
      final transcript = make(last: 'main');
      await settle();

      transcript.openThread('m1');
      transcript.closeThread();
      await settle();
      transcript.openThread('m1');
      expect(transcript.activeConversationId, 'new-1', reason: 'a new draft, not the abandoned one');
    });

    test('starts on the first conversation that is not a thread', () async {
      backend.conversations = [threadOf('t1', 'main', 'm1', 1), summary('main')];
      final transcript = make();
      await settle();
      expect(transcript.activeConversationId, 'main');
    });

    test('the answer that just arrived gets its id from the history', () async {
      backend.conversations = [summary('main')];
      final transcript = make(last: 'main');
      await settle();

      transcript.send('q');
      backend.histories['main'] = [
        const HistoryEntry(id: 'm1', fromUser: true, content: 'q'),
        const HistoryEntry(id: 'm2', fromUser: false, content: 'a'),
      ];
      replies.add(ChatResponseMessage('a', const Usage(promptTokens: 1, completionTokens: 1, totalTokens: 2), conversationId: 'main'));
      await settle();

      expect(transcript.entries.map((e) => e.id), ['m1', 'm2']);
    });

    test('ids go to the entries that lack one, skipping errors, and wait when the counts differ', () {
      const history = [HistoryEntry(id: 'a', fromUser: true, content: 'x'), HistoryEntry(id: 'b', fromUser: false, content: 'y')];
      final shown = [
        const ChatEntry(EntryRole.user, 'x', id: 'a'),
        const ChatEntry(EntryRole.error, 'boom'),
        const ChatEntry(EntryRole.assistant, 'y'),
      ];
      expect(withMessageIds(shown, history).map((e) => e.id), ['a', null, 'b']);
      final longer = [...shown, const ChatEntry(EntryRole.user, 'z')];
      expect(identical(withMessageIds(longer, history), longer), isTrue);
    });
  });

  test('titleFrom matches the hub: collapsed whitespace, 40 characters and an ellipsis', () {
    expect(titleFrom('  a   b\nc '), 'a b c');
    expect(titleFrom('x' * 45), '${'x' * 40}…');
  });
}
