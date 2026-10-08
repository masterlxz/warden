import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';

// P120, P123 — the wire shape of the agent tasks and of the organization edits, locked the same way
// `crates/warden-server-protocol/src/protocol.rs` locks it on the Rust side.
void main() {
  group('client messages', () {
    test('listing and controlling the delegated tasks', () {
      expect(const ListAgentTasksMessage(4).toJson(), {'type': 'listAgentTasks', 'requestId': 4});
      expect(
        const ControlAgentTaskMessage(5, 'k', 'at-1', 'pause').toJson(),
        {'type': 'controlAgentTask', 'requestId': 5, 'pairingKey': 'k', 'taskId': 'at-1', 'action': 'pause'},
      );
    });

    test('asking for the feed of activity', () {
      expect(const ListActivityMessage(9).toJson(), {'type': 'listActivity', 'requestId': 9});
    });

    test('an organization edit leaves out a blank role and a missing superior', () {
      expect(
        jsonDecode(const EditAgentOrgMessage(6, 'k', SetPositionEdit('dev', role: 'Backend', reportsTo: 'lead')).encode()),
        {
          'type': 'editAgentOrg',
          'requestId': 6,
          'pairingKey': 'k',
          'edit': {'kind': 'setPosition', 'id': 'dev', 'role': 'Backend', 'reportsTo': 'lead'},
        },
      );
      expect(const SetPositionEdit('dev', role: '  ', reportsTo: '').toJson(), {'kind': 'setPosition', 'id': 'dev'});
      expect(
        const AddReportEdit(' qa ', ' tests ', role: 'QA').toJson(),
        {'kind': 'addReport', 'id': 'qa', 'persona': 'tests', 'role': 'QA'},
      );
      expect(const RemoveAgentEdit('dev').toJson(), {'kind': 'remove', 'id': 'dev'});
    });

    test('the model limit and the policies go as the hub reads them, with a description always present', () {
      expect(const SetDelegationModelsEdit('chief', ['fast', 'main']).toJson(), {'kind': 'setDelegationModels', 'id': 'chief', 'models': ['fast', 'main']});
      expect(const SetDelegationModelsEdit('chief', []).toJson(), {'kind': 'setDelegationModels', 'id': 'chief', 'models': []});
      expect(
        const SetModelPoliciesEdit([ModelPolicy(id: 'fast', model: 'main', description: 'quick'), ModelPolicy(id: 'deep', model: 'spare')]).toJson(),
        {
          'kind': 'setModelPolicies',
          'policies': [
            {'id': 'fast', 'model': 'main', 'description': 'quick'},
            {'id': 'deep', 'model': 'spare', 'description': ''},
          ],
        },
      );
    });
  });

  group('server messages', () {
    test('settings carry the organization and the model policies', () {
      final msg = ServerMessage.decode(jsonEncode({
        'type': 'settings',
        'requestId': 3,
        'version': 'v',
        'secretsWritable': false,
        'settings': {
          'agents': [
            {
              'id': 'chief',
              'role': 'CTO',
              'canDelegateToAgents': true,
              'canManageAgents': true,
              'autonomy': 3,
              'approvalRequired': ['spend_money'],
              'delegationModels': ['fast'],
            },
            {'id': 'dev', 'reportsTo': 'chief'},
          ],
          'modelPolicies': [
            {'id': 'fast', 'model': 'gpt-mini', 'description': 'quick'},
          ],
        },
      })) as SettingsMessage;
      expect(msg.agentIds, ['chief', 'dev']);
      expect(msg.agents[0].role, 'CTO');
      expect(msg.agents[0].canDelegateToAgents, isTrue);
      expect(msg.agents[0].autonomy, 3);
      expect(msg.agents[0].approvalRequired, ['spend_money']);
      expect(msg.agents[0].delegationModels, ['fast']);
      expect(msg.agents[1].reportsTo, 'chief');
      expect(msg.modelPolicies.single.model, 'gpt-mini');
    });

    test('a hub that leaves fields out gets the defaults', () {
      final msg = ServerMessage.decode('{"type":"settings","requestId":1,"settings":{"agents":[{"id":"solo"}]}}') as SettingsMessage;
      expect(msg.modelPolicies, isEmpty);
      expect(msg.agents.single.autonomy, 4);
      expect(msg.agents.single.delegationModels, isEmpty);
      expect(msg.agents.single.reportsTo, isNull);
    });

    test('settings carry the ids of the providers and combos, apart from the policies', () {
      final msg = ServerMessage.decode(jsonEncode({
        'type': 'settings',
        'requestId': 1,
        'settings': {
          'agents': <dynamic>[],
          'providers': [
            {'id': 'main'},
            {'id': 'spare'},
          ],
          'combos': [
            {'id': 'both'},
          ],
          'modelPolicies': [
            {'id': 'fast', 'model': 'main', 'description': ''},
          ],
        },
      })) as SettingsMessage;
      expect(msg.modelIds, ['main', 'spare', 'both']);
      expect(msg.modelPolicies.single.id, 'fast');
      final saved = ServerMessage.decode('{"type":"settingsSaved","requestId":2,"settings":{"agents":[]}}') as SettingsSavedMessage;
      expect(saved.modelIds, isEmpty);
    });

    test('an organization edit is answered with the agents as they are now', () {
      final msg = ServerMessage.decode(
          '{"type":"settingsSaved","requestId":9,"version":"v2","settings":{"agents":[{"id":"a","reportsTo":"b"},{"id":"b"}]}}');
      expect(msg, isA<SettingsSavedMessage>());
      expect((msg as SettingsSavedMessage).agents.map((a) => a.id), ['a', 'b']);
    });

    test('a refused key comes through as authRejected', () {
      final settings = ServerMessage.decode('{"type":"settingsError","requestId":2,"message":"wrong key","conflict":false,"authRejected":true}');
      expect((settings as SettingsErrorMessage).authRejected, isTrue);
      final plain = ServerMessage.decode('{"type":"settingsError","requestId":2,"message":"nope"}');
      expect((plain as SettingsErrorMessage).authRejected, isFalse);
      final task = ServerMessage.decode('{"type":"taskError","requestId":5,"message":"not running here","authRejected":false}');
      expect((task as TaskErrorMessage).authRejected, isFalse);
    });

    test('a task list keeps what the hub sent', () {
      final msg = ServerMessage.decode(jsonEncode({
        'type': 'agentTaskList',
        'requestId': 4,
        'tasks': [
          {
            'id': 'at-1',
            'group': 'g',
            'owner': 'chief',
            'assignee': 'dev',
            'parentId': 'at-0',
            'objective': 'x',
            'model': 'fast',
            'channel': 'cli',
            'state': 'running',
            'totalTokens': 120,
            'createdAtMs': 5,
            'startedAtMs': 6,
            'controllable': true,
            'pausable': true,
          },
        ],
      })) as AgentTaskListMessage;
      final task = msg.tasks.single;
      expect(task.assignee, 'dev');
      expect(task.owner, 'chief');
      expect(task.parentId, 'at-0');
      expect(task.state, 'running');
      expect(task.totalTokens, 120);
      expect(task.controllable, isTrue);
      expect(task.pausable, isTrue);
      expect(task.finishedAtMs, isNull);
    });

    test('the feed of activity keeps its events and leaves the optional fields null', () {
      final msg = ServerMessage.decode(jsonEncode({
        'type': 'activityList',
        'requestId': 9,
        'events': [
          {'id': 'at-1-delegated', 'atMs': 5, 'kind': 'delegated', 'actor': 'chief', 'target': 'dev', 'text': 'build it', 'taskId': 'at-1'},
          {'id': 'c-m1', 'atMs': 9, 'kind': 'messaged_user', 'actor': 'pirate', 'text': 'disk full', 'conversationId': 'channel-1'},
        ],
      })) as ActivityListMessage;
      expect(msg.requestId, 9);
      expect(msg.events.map((e) => e.kind), ['delegated', 'messaged_user']);
      final first = msg.events.first;
      expect((first.actor, first.target, first.taskId, first.conversationId), ('chief', 'dev', 'at-1', null));
      final second = msg.events.last;
      expect((second.target, second.taskId, second.conversationId), (null, null, 'channel-1'));
    });

    test('a feed with no events key is an empty one', () {
      final msg = ServerMessage.decode(jsonEncode({'type': 'activityList', 'requestId': 1})) as ActivityListMessage;
      expect(msg.events, isEmpty);
    });
  });
}
