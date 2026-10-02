import 'dart:math' as math;

/// P115 — line diff between an active skill and the revision the assistant suggested for it.
enum DiffOp { same, add, del, gap }

class DiffLine {
  const DiffLine(this.op, this.text);

  final DiffOp op;
  final String text;

  @override
  bool operator ==(Object other) => other is DiffLine && other.op == op && other.text == text;

  @override
  int get hashCode => Object.hash(op, text);

  @override
  String toString() => '${op.name}:$text';
}

/// Line-by-line diff (longest common subsequence).
List<DiffLine> diffLines(String before, String after) {
  final a = before.split('\n');
  final b = after.split('\n');
  // lcs[i][j] = length of the common subsequence of a[i..] and b[j..]
  final lcs = List.generate(a.length + 1, (_) => List.filled(b.length + 1, 0));
  for (var i = a.length - 1; i >= 0; i--) {
    for (var j = b.length - 1; j >= 0; j--) {
      lcs[i][j] = a[i] == b[j] ? lcs[i + 1][j + 1] + 1 : math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  final out = <DiffLine>[];
  var i = 0;
  var j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] == b[j]) {
      out.add(DiffLine(DiffOp.same, a[i]));
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.add(DiffLine(DiffOp.del, a[i++]));
    } else {
      out.add(DiffLine(DiffOp.add, b[j++]));
    }
  }
  while (i < a.length) {
    out.add(DiffLine(DiffOp.del, a[i++]));
  }
  while (j < b.length) {
    out.add(DiffLine(DiffOp.add, b[j++]));
  }
  return out;
}

/// Keeps [context] unchanged lines around each change and folds the rest into a single gap line.
List<DiffLine> withContext(List<DiffLine> lines, {int context = 2}) {
  final changed = [for (final l in lines) l.op != DiffOp.same];
  bool near(int i) {
    for (var k = math.max(0, i - context); k <= math.min(lines.length - 1, i + context); k++) {
      if (changed[k]) return true;
    }
    return false;
  }

  DiffLine gap(int n) => DiffLine(DiffOp.gap, '… $n ${n == 1 ? 'identical line' : 'identical lines'}');

  final out = <DiffLine>[];
  var skipped = 0;
  for (var i = 0; i < lines.length; i++) {
    if (near(i)) {
      if (skipped > 0) out.add(gap(skipped));
      skipped = 0;
      out.add(lines[i]);
    } else {
      skipped++;
    }
  }
  if (skipped > 0) out.add(gap(skipped));
  return out;
}

bool hasChanges(List<DiffLine> lines) => lines.any((l) => l.op == DiffOp.add || l.op == DiffOp.del);
