import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/services/activity.dart';

// P121 — the pure half of the feed of activity. Same cases as the web's, the desktop's and the extension's `activity.test.mjs`.

ActivityEvent _event(String kind, String actor, {String? target, String? taskId, String? conversationId, int atMs = 1}) =>
    ActivityEvent(id: '$kind-$actor', atMs: atMs, kind: kind, actor: actor, target: target, text: '', taskId: taskId, conversationId: conversationId);

void main() {
  group('the sentence of an event', () {
    test('says who did what to whom', () {
      expect(activityHeadline(_event('delegated', 'manager', target: 'backend')), 'manager delegated a task to backend');
      expect(activityHeadline(_event('note', 'ana', target: 'bia')), 'ana left a note for bia');
      expect(activityHeadline(_event('reply', 'bia', target: 'ana')), 'bia answered ana');
      expect(activityHeadline(_event('messaged_user', 'pirate')), 'pirate wrote to you');
      expect(activityHeadline(_event('done', 'backend')), 'backend finished the task');
      expect(activityHeadline(_event('cancelled', 'backend')), "backend's task was cancelled");
    });

    test('names the assistant nobody picked', () {
      expect(activityHeadline(_event('delegated', '', target: 'writer')), 'The assistant delegated a task to writer');
    });

    test('every kind has a mark, and an unknown one still reads', () {
      for (final kind in ['delegated', 'started', 'done', 'failed', 'cancelled', 'note', 'reply', 'messaged_user']) {
        expect(activityMark(kind), isNot('·'), reason: kind);
      }
      expect(activityMark('later'), '·');
      expect(activityHeadline(_event('later', 'x')), 'x: later');
    });
  });

  group('the agents', () {
    final events = [
      _event('delegated', 'manager', target: 'backend'),
      _event('done', 'backend'),
      _event('messaged_user', 'pirate'),
      _event('started', ''),
    ];

    test('the filter keeps the events an agent did or received', () {
      expect(activityInvolving(events, 'backend').map((e) => e.kind), ['delegated', 'done']);
      expect(activityInvolving(events, 'ghost'), isEmpty);
    });

    test('the list of agents is sorted and leaves out the nameless one', () {
      expect(activityAgents(events), ['backend', 'manager', 'pirate']);
    });
  });

  group('where a tap goes', () {
    test('a message the agent started opens its channel', () {
      expect(activityDestination(_event('messaged_user', 'pirate', conversationId: 'channel-1')), const ActivityDestination(ActivityTarget.channel, agent: 'pirate'));
    });

    test('a note or an answer opens the conversation between the two', () {
      expect(
        activityDestination(_event('note', 'ana', target: 'bia', conversationId: 'agents-1')),
        const ActivityDestination(ActivityTarget.conversation, conversationId: 'agents-1'),
      );
    });

    test('a task event opens the work of the agent it is about', () {
      expect(activityDestination(_event('delegated', 'manager', target: 'backend', taskId: 't1')), const ActivityDestination(ActivityTarget.tasks, agent: 'backend'));
      expect(activityDestination(_event('done', 'backend', taskId: 't1')), const ActivityDestination(ActivityTarget.tasks, agent: 'backend'));
      expect(activityDestination(_event('done', '', taskId: 't1')), isNull, reason: 'no agent to filter by');
      expect(activityDestination(_event('later', 'x')), isNull);
    });
  });

  group('the days', () {
    int at(int day, int hour) => DateTime(2026, 10, day, hour).millisecondsSinceEpoch;
    final now = DateTime(2026, 10, 7, 15);

    test('groups the newest-first list by day and names today and yesterday', () {
      final events = [
        _event('done', 'a', atMs: at(7, 14)),
        _event('done', 'b', atMs: at(7, 9)),
        _event('done', 'c', atMs: at(6, 22)),
        _event('done', 'd', atMs: at(1, 10)),
      ];
      final days = activityByDay(events, now);
      expect(days.map((d) => (d.label, d.events.length)).toList(), [('Today', 2), ('Yesterday', 1), ('1 October 2026', 1)]);
    });

    test('an empty list has no day', () {
      expect(activityByDay(const [], now), isEmpty);
    });
  });
}
