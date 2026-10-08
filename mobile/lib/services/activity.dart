import '../protocol/messages.dart';

// P121 — the feed of activity: who did what among the agents, newest first. Pure; the screen only draws what comes out of here.
// Mirrors `web/src/hub/activity.ts`, in English.

/// What the screen calls the assistant that answers with no agent picked (an event with no `actor`).
const noAgentName = 'The assistant';

String _who(String name) => name.trim().isEmpty ? noAgentName : name;

/// The runs of scheduled tasks and webhooks: their target is the id of the task or the webhook, not an agent.
const _runKinds = {'scheduled_ran', 'scheduled_failed', 'webhook_ran', 'webhook_failed'};

/// The agent the event is done to, when its target is an agent.
String? _targetAgent(ActivityEvent event) => _runKinds.contains(event.kind) ? null : event.target;

/// The sentence of an event, without its text (the text is the objective, the answer or the message).
String activityHeadline(ActivityEvent event) {
  final actor = _who(event.actor);
  final target = event.target == null ? '' : _who(event.target!);
  return switch (event.kind) {
    'created_agent' => '$actor created the agent $target',
    'removed_agent' => '$actor removed the agent $target',
    'scheduled_ran' => '$actor ran the scheduled task $target',
    'scheduled_failed' => 'The scheduled task $target failed',
    'webhook_ran' => '$actor answered the webhook $target',
    'webhook_failed' => 'The webhook $target failed',
    'delegated' => '$actor delegated a task to $target',
    'started' => '$actor started the task',
    'done' => '$actor finished the task',
    'failed' => '$actor could not finish the task',
    'cancelled' => "$actor's task was cancelled",
    'note' => '$actor left a note for $target',
    'reply' => '$actor answered $target',
    'messaged_user' => '$actor wrote to you',
    _ => '$actor: ${event.kind}',
  };
}

/// A short mark per kind, to scan the list by eye.
String activityMark(String kind) => switch (kind) {
      'delegated' => '→',
      'started' => '▶',
      'done' => '✓',
      'failed' || 'scheduled_failed' || 'webhook_failed' => '✕',
      'cancelled' => '■',
      'created_agent' => '+',
      'removed_agent' => '−',
      'scheduled_ran' => '⏱',
      'webhook_ran' => '⚡',
      'note' || 'reply' => '✉',
      'messaged_user' => '●',
      _ => '·',
    };

/// The events an agent appears in, doing or receiving.
List<ActivityEvent> activityInvolving(List<ActivityEvent> events, String agent) =>
    [for (final e in events) if (e.actor == agent || _targetAgent(e) == agent) e];

/// The agents that appear in the events, alphabetical, without the nameless assistant (nor the id of a scheduled task or a webhook).
List<String> activityAgents(List<ActivityEvent> events) {
  final names = <String>{};
  for (final e in events) {
    if (e.actor.trim().isNotEmpty) names.add(e.actor);
    final target = _targetAgent(e);
    if (target != null && target.trim().isNotEmpty) names.add(target);
  }
  return names.toList()..sort((a, b) => a.compareTo(b));
}

enum ActivityTarget { tasks, conversation, channel }

/// What a click on an event opens: the agent's tasks, the conversation between two agents, or the agent's channel.
class ActivityDestination {
  const ActivityDestination(this.target, {this.agent, this.conversationId});

  final ActivityTarget target;
  final String? agent;
  final String? conversationId;

  @override
  bool operator ==(Object other) =>
      other is ActivityDestination && other.target == target && other.agent == agent && other.conversationId == conversationId;

  @override
  int get hashCode => Object.hash(target, agent, conversationId);
}

ActivityDestination? activityDestination(ActivityEvent event) {
  if (event.kind == 'messaged_user') return ActivityDestination(ActivityTarget.channel, agent: event.actor);
  final conversation = event.conversationId;
  if (conversation != null) return ActivityDestination(ActivityTarget.conversation, conversationId: conversation);
  if (event.taskId != null) {
    final agent = event.kind == 'delegated' ? event.target : event.actor;
    return agent == null || agent.isEmpty ? null : ActivityDestination(ActivityTarget.tasks, agent: agent);
  }
  return null;
}

/// The events of one day, with its label ("Today", "Yesterday" or the date).
class ActivityDay {
  const ActivityDay(this.label, this.events);

  final String label;
  final List<ActivityEvent> events;
}

const _months = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];

int _dayKey(DateTime d) => d.year * 10000 + d.month * 100 + d.day;

/// The events (already newest first) split by day, each day with its label.
List<ActivityDay> activityByDay(List<ActivityEvent> events, DateTime now) {
  final today = _dayKey(now);
  final yesterday = _dayKey(now.subtract(const Duration(days: 1)));
  final days = <ActivityDay>[];
  for (final event in events) {
    final at = DateTime.fromMillisecondsSinceEpoch(event.atMs);
    final key = _dayKey(at);
    final label = key == today
        ? 'Today'
        : key == yesterday
            ? 'Yesterday'
            : '${at.day} ${_months[at.month - 1]} ${at.year}';
    if (days.isNotEmpty && days.last.label == label) {
      days.last.events.add(event);
    } else {
      days.add(ActivityDay(label, [event]));
    }
  }
  return days;
}
