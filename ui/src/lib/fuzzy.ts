// Small fuzzy matcher for the command palette: subsequence match with bonuses for
// word starts, consecutive runs and prefix hits. Returns matched indices for highlighting.

export interface FuzzyMatch {
  score: number;
  /** Indices into the target string that matched the query characters. */
  indices: number[];
}

const WORD_SEP = /[\s\-_./:·>]/;

function isWordStart(text: string, i: number): boolean {
  if (i === 0) return true;
  const prev = text[i - 1];
  if (WORD_SEP.test(prev)) return true;
  // camelCase boundary
  return prev === prev.toLowerCase() && text[i] !== text[i].toLowerCase();
}

/**
 * Greedy-with-lookahead subsequence scoring. Case-insensitive.
 * Returns null when the query is not a subsequence of the text.
 */
export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  const q = query.trim().toLowerCase();
  if (!q) return { score: 0, indices: [] };
  const t = text.toLowerCase();

  // Exact substring gets a big boost and contiguous indices.
  const sub = t.indexOf(q);
  if (sub >= 0) {
    const indices = Array.from({ length: q.length }, (_, k) => sub + k);
    const score = 100 + q.length * 10 + (sub === 0 ? 50 : isWordStart(text, sub) ? 25 : 0) - text.length * 0.1;
    return { score, indices };
  }

  const indices: number[] = [];
  let score = 0;
  let ti = 0;
  let prev = -2;
  for (let qi = 0; qi < q.length; qi++) {
    const ch = q[qi];
    if (ch === ' ') continue;
    // Prefer the next occurrence that starts a word, if any, before falling back to the first.
    let found = -1;
    let firstAny = -1;
    for (let k = ti; k < t.length; k++) {
      if (t[k] !== ch) continue;
      if (firstAny < 0) firstAny = k;
      if (k === prev + 1) {
        found = k;
        break;
      }
      if (isWordStart(text, k)) {
        found = k;
        break;
      }
    }
    if (found < 0) found = firstAny;
    if (found < 0) return null;
    score += 1;
    if (found === prev + 1) score += 6;
    if (isWordStart(text, found)) score += 8;
    if (found === 0) score += 4;
    score -= Math.min(found - ti, 6) * 0.5; // gap penalty
    indices.push(found);
    prev = found;
    ti = found + 1;
  }
  score -= text.length * 0.1;
  return { score, indices };
}

export interface Ranked<T> {
  item: T;
  match: FuzzyMatch;
}

/** Filters and sorts items by fuzzy score; stable for equal scores. */
export function fuzzyFilter<T>(query: string, items: readonly T[], text: (item: T) => string): Ranked<T>[] {
  const out: (Ranked<T> & { order: number })[] = [];
  items.forEach((item, order) => {
    const match = fuzzyMatch(query, text(item));
    if (match) out.push({ item, match, order });
  });
  out.sort((a, b) => b.match.score - a.match.score || a.order - b.order);
  return out.map(({ item, match }) => ({ item, match }));
}

/** Splits text into highlighted / plain segments for rendering. */
export function highlight(text: string, indices: readonly number[]): { text: string; hit: boolean }[] {
  const set = new Set(indices);
  const parts: { text: string; hit: boolean }[] = [];
  for (let i = 0; i < text.length; i++) {
    const hit = set.has(i);
    const last = parts[parts.length - 1];
    if (last && last.hit === hit) last.text += text[i];
    else parts.push({ text: text[i], hit });
  }
  return parts;
}
