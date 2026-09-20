import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/services/chat_transcript.dart';

void main() {
  late StreamController<ServerMessage> replies;
  late List<String> sent;
  late ChatTranscript transcript;

  setUp(() {
    replies = StreamController<ServerMessage>.broadcast();
    sent = [];
    transcript = ChatTranscript(chatStream: replies.stream, sendChat: sent.add);
  });

  tearDown(() {
    transcript.dispose();
    replies.close();
  });

  test('send records the user turn, sends it, and waits for the reply', () {
    expect(transcript.send('  hello  '), isTrue);

    expect(sent, ['hello']);
    expect(transcript.waitingForReply, isTrue);
    expect(transcript.entries.single.role, EntryRole.user);
    expect(transcript.entries.single.text, 'hello');
  });

  test('blank text and a pending reply are both ignored', () {
    expect(transcript.send('   '), isFalse);
    transcript.send('first');
    expect(transcript.send('second'), isFalse);

    expect(sent, ['first']);
    expect(transcript.entries, hasLength(1));
  });

  test('a reply and an error are appended and clear the waiting state', () async {
    transcript.send('q');
    replies.add(const ChatResponseMessage('answer', null));
    await Future<void>.delayed(Duration.zero);

    expect(transcript.waitingForReply, isFalse);
    expect(transcript.entries.map((e) => e.role), [EntryRole.user, EntryRole.assistant]);
    expect(transcript.entries.last.text, 'answer');

    transcript.send('q2');
    replies.add(const ChatErrorMessage('boom'));
    await Future<void>.delayed(Duration.zero);

    expect(transcript.entries.last.role, EntryRole.error);
    expect(transcript.waitingForReply, isFalse);
  });

  test('a reply that arrives with no screen attached is still kept (P41)', () async {
    // No listener other than the transcript itself — this is the "chat screen was popped" case.
    transcript.send('q');
    replies.add(const ChatResponseMessage('late answer', null));
    await Future<void>.delayed(Duration.zero);

    expect(transcript.entries.last.text, 'late answer');
  });

  test('listeners are notified on send and on reply', () async {
    var notifications = 0;
    transcript.addListener(() => notifications++);

    transcript.send('q');
    replies.add(const ChatResponseMessage('a', null));
    await Future<void>.delayed(Duration.zero);

    expect(notifications, 2);
  });
}
