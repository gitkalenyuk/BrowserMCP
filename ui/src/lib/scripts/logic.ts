// Pure helpers of the Scripts panel (unit-tested in logic.test.ts).

import { chordFromEvent, chordId, parseChord, type KeyLike } from '../keymap';
import type { DebugEnded, ScriptFailure, ScriptRunOutcome } from '../api/types-scripts';

/**
 * Input names a script reads: `inputs.email`, `inputs["first name"]`, `const { a, b: x } = inputs`.
 * In first-seen order, without duplicates. Comments are not skipped (a commented-out input
 * still shows up in the form, which is harmless).
 */
export function inferInputs(code: string): string[] {
  const names: string[] = [];
  const add = (n: string) => {
    if (n && !names.includes(n)) names.push(n);
  };
  const access = /\binputs\s*(?:\?\.|\.)\s*([A-Za-z_$][\w$]*)|\binputs\s*(?:\?\.)?\[\s*(['"`])([^'"`\n]+)\2\s*\]/g;
  for (const m of code.matchAll(access)) add(m[1] ?? m[3]);
  const destructure = /\{([^{}]*)\}\s*=\s*inputs\b/g;
  for (const m of code.matchAll(destructure)) {
    for (const part of m[1].split(',')) {
      // `a`, `a: alias`, `a = "default"`, `...rest` (skipped)
      const name = part.trim().split(/[:=\s]/)[0];
      if (/^[A-Za-z_$][\w$]*$/.test(name)) add(name);
    }
  }
  return names;
}

/** One line of the output console. `line` makes it a "jump to line" link. */
export interface ConsoleEntry {
  kind: 'stdout' | 'stderr' | 'result' | 'error' | 'info';
  text: string;
  line?: number;
}

function lines(text: string): string[] {
  return text.replace(/\r\n/g, '\n').split('\n').filter((l, i, all) => l !== '' || i < all.length - 1);
}

/** Console entries for a finished `script.run`. */
export function outcomeEntries(outcome: ScriptRunOutcome): ConsoleEntry[] {
  const out: ConsoleEntry[] = [];
  for (const l of lines(outcome.stdout)) out.push({ kind: 'stdout', text: l });
  // The host repeats the uncaught error at the end of stderr; the error entry shows it once.
  const errorAt = outcome.error ? lastErrorLine(outcome.stderr) : -1;
  lines(outcome.stderr).forEach((l, i) => {
    if (errorAt < 0 || i < errorAt) out.push({ kind: 'stderr', text: l });
  });
  if (outcome.result !== null && outcome.result !== undefined) {
    out.push({ kind: 'result', text: JSON.stringify(outcome.result, null, 2) });
  }
  out.push(endEntry(outcome.ok, outcome.duration_ms, outcome.error ?? null, false));
  return out;
}

/** Index of the host's final `error: …` line in stderr, -1 if none. */
function lastErrorLine(stderr: string): number {
  const all = lines(stderr);
  for (let i = all.length - 1; i >= 0; i--) if (all[i].startsWith('error: ')) return i;
  return -1;
}

/** The last console entry: "done in 120 ms" or the error (with its line). */
export function endEntry(ok: boolean, ms: number, error: ScriptFailure | null, stopped: boolean): ConsoleEntry {
  if (stopped) return { kind: 'info', text: `stopped after ${formatMs(ms)}` };
  if (ok || !error) return { kind: 'info', text: `done in ${formatMs(ms)}` };
  const where = error.command ? ` (browser.${error.command})` : '';
  return { kind: 'error', text: `${error.code}${where}: ${error.message}`, line: error.line ?? undefined };
}

export function debugEndEntry(ended: DebugEnded): ConsoleEntry {
  return endEntry(ended.ok, ended.duration_ms, ended.error, ended.stopped);
}

export function formatMs(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
}

/** Toggle `line` in a sorted breakpoint list. */
export function toggleLine(lines: number[], line: number): number[] {
  return lines.includes(line) ? lines.filter((l) => l !== line) : [...lines, line].sort((a, b) => a - b);
}

/** A starter script for "New script". */
export function newScriptTemplate(name: string): string {
  return `// ${name}.ts: runs in the BrowserMCP script host.
// browser.<ns>.<method>(args) calls a browser command, inputs.<name> are the Run form values,
// browser.output({...}) reports a result.

const { tabs } = await browser.tabs.list();
console.log(\`\${tabs.length} tabs open\`);

const url = inputs.url ?? "https://example.com/";
browser.output({ url, tabs: tabs.length });
`;
}

/** A free name like "script-2" that is not in `taken`. */
export function freeName(base: string, taken: string[]): string {
  if (!taken.includes(base)) return base;
  for (let i = 2; ; i++) if (!taken.includes(`${base}-${i}`)) return `${base}-${i}`;
}

/** Same rule as crates/jobs/src/scripts.rs `validate_name` (without the Windows names). */
export function validName(name: string): boolean {
  return /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}(\.v\d+)?$/.test(name);
}

// ---- keys -------------------------------------------------------------------------------

/** Keys the script editor handles itself, even though the chrome binds some (F5 = reload). */
export const EDITOR_KEYS = ['F5', 'Shift+F5', 'F9', 'F10', 'F11', 'Shift+F11', 'Ctrl+S'];
const EDITOR_KEY_IDS = new Set(EDITOR_KEYS.map((k) => chordId(parseChord(k)!)));

/** True when `e` happened inside an element marked `data-own-keys` and is one of its keys. */
export function editorOwnsKey(e: KeyLike & { target: EventTarget | null }): boolean {
  const target = e.target as { closest?: (sel: string) => unknown } | null;
  if (!target?.closest?.('[data-own-keys]')) return false;
  const chord = chordFromEvent(e);
  return chord !== null && EDITOR_KEY_IDS.has(chordId(chord));
}
