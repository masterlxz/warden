import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/screens/member_org_tab.dart';
import 'package:mobile/services/agent_work.dart';
import 'package:mobile/services/server_connection.dart' show ConversationException, HubRequestException;

/// A hub in memory for one member: it holds the owner's tree and the access the owner gave, and answers like the real one
/// (the tree with only the id, the role and the superior; a refusal when the access is none or too low to edit).
class _FakeHub implements MemberOrgBackend {
  _FakeHub(this.access, {List<AgentInfo> agents = const []}) : agents = List.of(agents);

  OrgAccess access;
  List<AgentInfo> agents;
  final edits = <OrgEdit>[];

  /// The hub can't be reached: neither a refusal nor an answer.
  bool unreachable = false;

  @override
  Future<MemberOrg> listAgentOrg() async {
    if (unreachable) throw ConversationException('the hub did not answer');
    if (access == OrgAccess.none) throw const HubRequestException("the workspace's owner hasn't given you access to the organization of the agents");
    return MemberOrg(List.of(agents), access.name);
  }

  @override
  Future<MemberOrg> editAgentOrgAsMember(OrgEdit edit) async {
    if (access != OrgAccess.edit) throw const HubRequestException("the workspace's owner hasn't let you change the organization of the agents");
    edits.add(edit);
    switch (edit) {
      case SetPositionEdit(:final id, :final role, :final reportsTo):
        agents = [
          for (final a in agents)
            if (a.id == id) AgentInfo(id: a.id, role: role, reportsTo: (reportsTo ?? '').isEmpty ? null : reportsTo) else a,
        ];
      case AddReportEdit(:final id, :final reportsTo):
        agents = [...agents, AgentInfo(id: id, reportsTo: reportsTo)];
      case RemoveAgentEdit(:final id):
        agents = [for (final a in agents) if (a.id != id) a];
      default:
        throw const HubRequestException('only the workspace\'s owner changes that');
    }
    return MemberOrg(List.of(agents), access.name);
  }
}

const _team = [
  AgentInfo(id: 'chief', role: 'CTO'),
  AgentInfo(id: 'dev', reportsTo: 'chief'),
];

Future<void> _pump(WidgetTester tester, _FakeHub hub, {OrgAccess initial = OrgAccess.none}) async {
  await tester.pumpWidget(MaterialApp(home: Scaffold(body: MemberOrganizationTab(backend: hub, initialAccess: initial))));
  await tester.pumpAndSettle();
}

void main() {
  group('the organization tab of a member', () {
    testWidgets('none: a note, no tree', (tester) async {
      await _pump(tester, _FakeHub(OrgAccess.none, agents: _team));
      expect(find.byKey(const Key('member-org-none')), findsOneWidget);
      expect(find.text('chief'), findsNothing);
    });

    testWidgets('view: the tree with its roles, and no way to change it', (tester) async {
      await _pump(tester, _FakeHub(OrgAccess.view, agents: _team), initial: OrgAccess.view);
      expect(find.text('chief'), findsOneWidget);
      expect(find.text('CTO'), findsOneWidget);
      expect(find.text('1 report'), findsOneWidget);
      expect(find.textContaining('You can look at the tree'), findsOneWidget);
      expect(find.byKey(const Key('member-menu-chief')), findsNothing);
      expect(find.byKey(const Key('member-add-top')), findsNothing);
    });

    testWidgets('edit: gives an agent a role and a superior, with no pairing key asked', (tester) async {
      final hub = _FakeHub(OrgAccess.edit, agents: [..._team, const AgentInfo(id: 'qa')]);
      await _pump(tester, hub, initial: OrgAccess.edit);
      await tester.tap(find.byKey(const Key('member-menu-qa')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Edit position').last);
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('role-field')), 'Tester');
      await tester.tap(find.text('Save').last);
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('pairing-key-field')), findsNothing, reason: 'the member\'s session is the authorization');
      expect(hub.edits, hasLength(1));
      expect((hub.edits.single as SetPositionEdit).role, 'Tester');
      expect(find.text('Tester'), findsOneWidget);
    });

    testWidgets('edit: a new agent at the top, and a removal after a yes', (tester) async {
      final hub = _FakeHub(OrgAccess.edit, agents: _team);
      await _pump(tester, hub, initial: OrgAccess.edit);
      await tester.tap(find.byKey(const Key('member-add-top')));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('new-name-field')), 'reviewer');
      await tester.enterText(find.byKey(const Key('new-persona-field')), 'reads the diffs');
      await tester.pump();
      await tester.tap(find.byKey(const Key('new-agent-confirm')));
      await tester.pumpAndSettle();
      expect(hub.edits.single, isA<AddReportEdit>());
      expect(find.text('reviewer'), findsOneWidget);

      await tester.tap(find.byKey(const Key('member-menu-dev')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Remove').last);
      await tester.pumpAndSettle();
      expect(hub.edits, hasLength(1), reason: 'nothing is removed before the yes');
      await tester.tap(find.widgetWithText(FilledButton, 'Remove'));
      await tester.pumpAndSettle();
      expect(hub.edits.last, isA<RemoveAgentEdit>());
      expect(find.text('dev'), findsNothing);
    });

    testWidgets('signed in with edit, and the owner took it back in the meantime: the tab follows the hub', (tester) async {
      await _pump(tester, _FakeHub(OrgAccess.none, agents: _team), initial: OrgAccess.edit);
      expect(find.byKey(const Key('member-org-none')), findsOneWidget);
      expect(find.byKey(const Key('member-add-top')), findsNothing);
    });

    testWidgets('signed in with none, and the owner gave view in the meantime: the tab follows the hub', (tester) async {
      await _pump(tester, _FakeHub(OrgAccess.view, agents: _team));
      expect(find.text('chief'), findsOneWidget);
      expect(find.byKey(const Key('member-org-none')), findsNothing);
    });

    testWidgets('an edit the hub refuses shows the hub\'s reason and looks at the access again', (tester) async {
      final hub = _FakeHub(OrgAccess.edit, agents: _team);
      await _pump(tester, hub, initial: OrgAccess.edit);
      hub.access = OrgAccess.view; // taken back while the screen was open
      await tester.tap(find.byKey(const Key('member-menu-dev')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Remove').last);
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FilledButton, 'Remove'));
      await tester.pumpAndSettle();
      expect(find.textContaining("hasn't let you change the organization"), findsOneWidget);
      expect(hub.edits, isEmpty);
      expect(find.byKey(const Key('member-menu-chief')), findsNothing, reason: 'it is view now, so no menu');
    });

    testWidgets('a hub that cannot be reached says nothing about the access: the tab keeps what it showed', (tester) async {
      final hub = _FakeHub(OrgAccess.view, agents: _team);
      await _pump(tester, hub, initial: OrgAccess.view);
      hub.unreachable = true;
      await tester.fling(find.byType(ListView), const Offset(0, 300), 1000);
      await tester.pumpAndSettle();
      expect(find.text('chief'), findsOneWidget);
      expect(find.byKey(const Key('member-org-none')), findsNothing);
      expect(find.textContaining('the hub did not answer'), findsOneWidget);
    });
  });
}
