import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/screens/skills_screen.dart';
import 'package:mobile/services/skills_repository.dart';

/// In-memory stand-in for the Rust bridge, enforcing the two rules the screen depends on: create
/// refuses a taken name, and an invalid skill surfaces as an exception carrying a message.
class _FakeRepository implements SkillsRepository {
  _FakeRepository(List<SkillDto> initial) : skills = List.of(initial);

  final List<SkillDto> skills;

  @override
  Future<List<SkillDto>> list() async => List.of(skills);

  @override
  Future<void> save(SkillDto skill, {required bool overwrite}) async {
    if (skill.name.isEmpty) throw 'skill name must be 1-64 characters';
    final index = skills.indexWhere((s) => s.name == skill.name);
    if (index >= 0 && !overwrite) throw "a skill named '${skill.name}' already exists";
    if (index >= 0) {
      skills[index] = skill;
    } else {
      skills.add(skill);
    }
  }

  @override
  Future<void> delete(String name) async => skills.removeWhere((s) => s.name == name);
}

const _review = SkillDto(name: 'review-pr', description: 'Reviews a PR', body: 'Read the diff.', proposed: false);
const _tip = SkillDto(name: 'tip', description: 'A tip', body: 'Do it.', proposed: true, source: 'a conversa');
const _change = SkillDto(name: 'review-pr-revision', description: 'Reviews a PR', body: 'Check tests first.', proposed: true, revises: 'review-pr');

Future<void> _pump(WidgetTester tester, _FakeRepository repo) async {
  await tester.pumpWidget(MaterialApp(home: SkillsScreen(repository: repo)));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('shows an empty state when there are no skills', (tester) async {
    await _pump(tester, _FakeRepository([]));
    expect(find.textContaining('No skills yet'), findsOneWidget);
  });

  testWidgets('lists the skills with their descriptions', (tester) async {
    await _pump(tester, _FakeRepository([_review]));
    expect(find.text('review-pr'), findsOneWidget);
    expect(find.text('Reviews a PR'), findsOneWidget);
  });

  testWidgets('creating a skill saves it and returns to the list', (tester) async {
    final repo = _FakeRepository([]);
    await _pump(tester, repo);

    await tester.tap(find.text('New skill'));
    await tester.pumpAndSettle();
    await tester.enterText(find.widgetWithText(TextField, 'Name'), 'summarize');
    await tester.enterText(find.widgetWithText(TextField, 'Description'), 'Summarizes text');
    await tester.enterText(find.widgetWithText(TextField, 'Instructions'), 'Be brief.');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();

    expect(repo.skills.single.name, 'summarize');
    expect(repo.skills.single.body, 'Be brief.');
    expect(find.text('summarize'), findsOneWidget);
  });

  testWidgets('a rejected save stays on the form and shows the error', (tester) async {
    await _pump(tester, _FakeRepository([_review]));

    await tester.tap(find.text('New skill'));
    await tester.pumpAndSettle();
    await tester.enterText(find.widgetWithText(TextField, 'Name'), 'review-pr');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();

    expect(find.textContaining('already exists'), findsOneWidget);
    expect(find.text('New skill'), findsWidgets);
  });

  testWidgets('editing locks the name and overwrites the skill', (tester) async {
    final repo = _FakeRepository([_review]);
    await _pump(tester, repo);

    await tester.tap(find.text('review-pr'));
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(find.widgetWithText(TextField, 'Name')).enabled, isFalse);

    await tester.enterText(find.widgetWithText(TextField, 'Description'), 'Reviews a pull request');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();

    expect(repo.skills.single.description, 'Reviews a pull request');
  });

  testWidgets('delete asks for confirmation and only removes on confirm', (tester) async {
    final repo = _FakeRepository([_review]);
    await _pump(tester, repo);

    await tester.tap(find.byTooltip('Delete review-pr'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(repo.skills, hasLength(1));

    await tester.tap(find.byTooltip('Delete review-pr'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(TextButton, 'Delete'));
    await tester.pumpAndSettle();
    expect(repo.skills, isEmpty);
    expect(find.textContaining('No skills yet'), findsOneWidget);
  });

  testWidgets('a suggestion gets its own section and stays pending when edited and saved', (tester) async {
    final repo = _FakeRepository([_review, _tip]);
    await _pump(tester, repo);
    expect(find.text('Suggested by the AI'), findsOneWidget);
    expect(find.text('Suggested'), findsOneWidget);

    await tester.tap(find.text('Edit'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();

    expect(repo.skills.firstWhere((s) => s.name == 'tip').proposed, isTrue);
  });

  testWidgets('accepting a suggestion saves it as active', (tester) async {
    final repo = _FakeRepository([_tip]);
    await _pump(tester, repo);

    await tester.tap(find.text('Accept'));
    await tester.pumpAndSettle();

    expect(repo.skills.single.proposed, isFalse);
    expect(find.text('Suggested by the AI'), findsNothing);
  });

  testWidgets('rejecting a suggestion deletes it after confirming', (tester) async {
    final repo = _FakeRepository([_tip]);
    await _pump(tester, repo);

    await tester.tap(find.text('Reject'));
    await tester.pumpAndSettle();
    await tester.tap(find.descendant(of: find.byType(AlertDialog), matching: find.text('Reject')));
    await tester.pumpAndSettle();

    expect(repo.skills, isEmpty);
  });

  testWidgets('a suggested change to a skill says which skill it changes', (tester) async {
    await _pump(tester, _FakeRepository([_review, _change]));
    expect(find.text('Change to review-pr'), findsOneWidget);
    expect(find.text('See what changes in review-pr'), findsOneWidget);
    await tester.tap(find.text('See what changes in review-pr'));
    await tester.pumpAndSettle();
    expect(find.text('- Read the diff.'), findsOneWidget);
    expect(find.text('+ Check tests first.'), findsOneWidget);
    expect(find.text('Suggested'), findsNothing);
  });
}
