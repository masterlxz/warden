// P115: line diff between an active skill and the revision the assistant suggested for it.

export type DiffOp = "same" | "add" | "del" | "gap";

export interface DiffLine {
  op: DiffOp;
  text: string;
}

/** Line-by-line diff (longest common subsequence). */
export function diffLines(before: string, after: string): DiffLine[] {
  const a = before.split("\n");
  const b = after.split("\n");
  // lcs[i][j] = length of the common subsequence of a[i..] and b[j..]
  const lcs: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ op: "same", text: a[i] });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.push({ op: "del", text: a[i++] });
    } else {
      out.push({ op: "add", text: b[j++] });
    }
  }
  while (i < a.length) out.push({ op: "del", text: a[i++] });
  while (j < b.length) out.push({ op: "add", text: b[j++] });
  return out;
}

/** Keeps `context` unchanged lines around each change and folds the rest into a single `gap` line. */
export function withContext(lines: DiffLine[], context = 2): DiffLine[] {
  const keep = lines.map((l) => l.op !== "same");
  const near = keep.map((_, i) => {
    for (let k = Math.max(0, i - context); k <= Math.min(lines.length - 1, i + context); k++) {
      if (keep[k]) return true;
    }
    return false;
  });
  const out: DiffLine[] = [];
  let skipped = 0;
  lines.forEach((line, i) => {
    if (near[i]) {
      if (skipped > 0) out.push({ op: "gap", text: `… ${skipped} ${skipped === 1 ? "linha igual" : "linhas iguais"}` });
      skipped = 0;
      out.push(line);
    } else {
      skipped++;
    }
  });
  if (skipped > 0) out.push({ op: "gap", text: `… ${skipped} ${skipped === 1 ? "linha igual" : "linhas iguais"}` });
  return out;
}

export function hasChanges(lines: DiffLine[]): boolean {
  return lines.some((l) => l.op === "add" || l.op === "del");
}
