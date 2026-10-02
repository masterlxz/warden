import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/screens/skill_diff.dart';

String _show(String a, String b, {int context = 1}) => withContext(diffLines(a, b), context: context).join('|');

void main() {
  test('identical text has no changes and folds into one gap', () {
    expect(hasChanges(diffLines('a\nb', 'a\nb')), isFalse);
    expect(_show('a\nb\nc', 'a\nb\nc'), 'gap:… 3 identical lines');
  });

  test('a replaced line shows as del + add with context around it', () {
    expect(
      _show('a\nb\nc\nd\ne\nf', 'a\nb\nX\nd\ne\nf'),
      'gap:… 1 identical line|same:b|del:c|add:X|same:d|gap:… 2 identical lines',
    );
  });

  test('pure additions and removals', () {
    expect(_show('a\nb', 'a\nb\nc'), 'gap:… 1 identical line|same:b|add:c');
    expect(_show('a\nb\nc', 'a\nb'), 'gap:… 1 identical line|same:b|del:c');
  });

  test('empty before is a single blank-line removal plus the new text', () {
    expect(_show('', 'x'), 'del:|add:x');
  });
}
