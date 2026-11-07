// Pure helpers of the Recording panel: the live step list and the inputs of a generated script.

import type { CodegenInput, RecordStep } from '../api/types-record-runs';
import { t } from '../i18n/index.svelte';

// ---- steps ------------------------------------------------------------------------------

/** Add a step or replace the one with the same seq (API steps are re-sent once resolved). */
export function upsertStep(steps: RecordStep[], step: RecordStep): RecordStep[] {
  const i = steps.findIndex((s) => s.seq === step.seq);
  if (i >= 0) {
    const next = steps.slice();
    next[i] = step;
    return next;
  }
  if (steps.length === 0 || steps[steps.length - 1].seq < step.seq) return [...steps, step];
  return [...steps, step].sort((a, b) => a.seq - b.seq);
}

/** `record.get {format: "jsonl"}` text → steps (bad lines are skipped). */
export function parseStepsJsonl(text: string): RecordStep[] {
  const out: RecordStep[] = [];
  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    try {
      const s = JSON.parse(line) as RecordStep;
      if (typeof s.seq === 'number' && typeof s.action === 'string') out.push(s);
    } catch {
      // a half-written last line while recording
    }
  }
  return out;
}

/** Steps worth showing: no human copies of API actions. */
export function visibleSteps(steps: RecordStep[]): RecordStep[] {
  return steps.filter((s) => s.dup_of === undefined || s.dup_of === null);
}

export interface StepView {
  kind: 'nav' | 'action' | 'command' | 'wait' | 'system';
  verb: string;
  /** `button "Sign in"`, a URL, a command name. */
  target: string;
  /** Typed value, keys, option (secrets as ***). */
  value: string | null;
  failed: boolean;
}

/** Actions with a translated verb (`record.verb.<action>`); others show the raw action name. */
const VERBS = new Set([
  'start',
  'goto',
  'navigated',
  'back',
  'forward',
  'reload',
  'open_tab',
  'wait',
  'click',
  'dblclick',
  'fill',
  'type',
  'select',
  'check',
  'uncheck',
  'hover',
  'press',
  'scroll',
  'focus',
]);

function verb(action: string): string {
  return VERBS.has(action) ? t(`record.verb.${action}`) : action;
}

function quote(s: string, max = 48): string {
  const text = s.replace(/\s+/g, ' ').trim();
  return `"${text.length > max ? `${text.slice(0, max - 1)}…` : text}"`;
}

/** `button "Sign in"`, `text "Accept"`, the CSS path or the tag. */
export function targetLabel(t: RecordStep['target']): string {
  if (!t) return '';
  const role = t.role ?? t.tag ?? '';
  if (t.name) return `${role} ${quote(t.name)}${t.nth ? ` #${t.nth}` : ''}`.trim();
  if (t.text) return `${role} ${quote(t.text)}`.trim();
  if (t.css) return t.css;
  return role || (t.ref ?? '');
}

export function describeStep(s: RecordStep): StepView {
  const failed = s.command?.ok === false;
  const value = s.secret ? '***' : (s.value ?? null);
  switch (s.action) {
    case 'start':
    case 'goto':
    case 'navigated':
    case 'open_tab':
    case 'back':
    case 'forward':
    case 'reload':
      return { kind: s.action === 'start' ? 'system' : 'nav', verb: verb(s.action), target: s.url ?? '', value: null, failed };
    case 'wait':
      return { kind: 'wait', verb: verb('wait'), target: s.wait_for ? t('record.waitFor', { what: s.wait_for }) : '', value, failed };
    case 'command':
      return { kind: 'command', verb: s.command?.name ?? 'command', target: '', value: s.command?.error ?? null, failed };
    default:
      return { kind: 'action', verb: verb(s.action), target: targetLabel(s.target), value, failed };
  }
}

// ---- inputs of a generated script -------------------------------------------------------------

export interface ScriptInput {
  name: string;
  /** Recorded value (`inputs.x ?? "value"`); null when the script has no default. */
  default: string | null;
  /** No default in the script: the run fails without it. */
  required: boolean;
  /** A password (`need("password")` in codegen output): never prefilled, never stored. */
  secret: boolean;
  label: string;
}

/** Drop comments (whole-line `//` and block comments) so commented-out code does not count. */
function stripComments(code: string): string {
  return code.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/^\s*\/\/.*$/gm, '');
}

const IDENT = '[A-Za-z_$][\\w$]*';
const STR = `"(?:[^"\\\\\\n]|\\\\.)*"|'(?:[^'\\\\\\n]|\\\\.)*'`;
const USAGE = new RegExp(
  '(?:' +
  [
    `\\binputs\\s*\\.\\s*(${IDENT})`, // inputs.email
    `\\binputs\\s*\\[\\s*(${STR})\\s*\\]`, // inputs["first name"]
    `\\bneed\\(\\s*(${STR})\\s*\\)`, // need("password") — codegen's required input
  ]
    .map((p) => `(?:${p})`)
    .join('|') +
    `)(\\s*\\?\\?\\s*(${STR}))?`,
  'g',
);

function unquote(lit: string): string {
  if (lit.startsWith('"')) {
    try {
      return JSON.parse(lit) as string;
    } catch {
      return lit.slice(1, -1);
    }
  }
  return lit.slice(1, -1).replace(/\\'/g, "'").replace(/\\\\/g, '\\');
}

/**
 * The inputs a script reads, in the order they first appear: `inputs.<name>`, `inputs["name"]`
 * and codegen's `need("name")`, with `?? "default"` when present. `specs` (record.codegen's
 * `inputs`) add labels and the secret flag.
 */
export function inferInputs(code: string, specs: readonly CodegenInput[] = []): ScriptInput[] {
  const byName = new Map<string, ScriptInput>();
  const spec = new Map(specs.map((s) => [s.name, s]));
  for (const m of stripComments(code).matchAll(USAGE)) {
    const name = m[1] ?? (m[2] ? unquote(m[2]) : m[3] ? unquote(m[3]) : '');
    if (!name) continue;
    const viaNeed = m[3] !== undefined;
    const dflt = m[5] !== undefined ? unquote(m[5]) : null;
    const s = spec.get(name);
    const prev = byName.get(name);
    if (prev) {
      if (prev.default === null && dflt !== null && !prev.secret) {
        prev.default = dflt;
        prev.required = false;
      }
      continue;
    }
    const secret = s?.secret ?? false;
    byName.set(name, {
      name,
      default: secret ? null : (dflt ?? s?.default ?? null),
      required: viaNeed || secret || dflt === null,
      secret,
      label: s?.label || name,
    });
  }
  // Inputs codegen reported that the code no longer mentions are left out on purpose.
  return [...byName.values()];
}

/** The form's values → script.run `inputs`; empty fields fall back to the script's default. */
export function buildInputs(
  defs: readonly ScriptInput[],
  values: Readonly<Record<string, string>>,
): { inputs: Record<string, string>; missing: string[] } {
  const inputs: Record<string, string> = {};
  const missing: string[] = [];
  for (const d of defs) {
    const v = values[d.name] ?? '';
    if (v !== '') inputs[d.name] = v;
    else if (d.required) missing.push(d.name);
  }
  return { inputs, missing };
}

/** A script name script.save accepts: 1-64 of A-Z a-z 0-9 _ -. */
export function scriptName(raw: string): string {
  return raw
    .trim()
    .replace(/\.ts$/i, '')
    .replace(/[^A-Za-z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64);
}

/** `C:\x\data\recordings\r1\video.webm` → `file:///C:/x/data/recordings/r1/` (folder) or the file. */
export function fileUrl(path: string, folder = false): string {
  let p = path.replace(/\\/g, '/');
  if (folder) p = p.replace(/\/[^/]*$/, '/');
  const encoded = p
    .split('/')
    .map((seg, i) => (i === 0 && /^[A-Za-z]:$/.test(seg) ? seg : encodeURIComponent(seg)))
    .join('/');
  return encoded.startsWith('/') ? `file://${encoded}` : `file:///${encoded}`;
}
