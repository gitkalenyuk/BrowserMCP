// Pure helpers of the Traffic panel: request list filtering, rule forms, breakpoint resume
// payloads. Tested in logic.test.ts.

import type { NetAction, NetEdits, NetMatch, NetPaused, NetResume, NetStage } from '../api/types';
import type { NetRow } from './parse';

// ---- request list filter ---------------------------------------------------------------------

export type TypeChip = 'all' | 'fetch' | 'doc' | 'js' | 'css' | 'img' | 'media' | 'font' | 'ws' | 'other';

export const TYPE_CHIPS: { id: TypeChip; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'fetch', label: 'Fetch/XHR' },
  { id: 'doc', label: 'Doc' },
  { id: 'js', label: 'JS' },
  { id: 'css', label: 'CSS' },
  { id: 'img', label: 'Img' },
  { id: 'media', label: 'Media' },
  { id: 'font', label: 'Font' },
  { id: 'ws', label: 'WS' },
  { id: 'other', label: 'Other' },
];

const CHIP_TYPES: Record<Exclude<TypeChip, 'all' | 'other'>, string[]> = {
  fetch: ['xhr', 'fetch', 'eventsource'],
  doc: ['document'],
  js: ['script'],
  css: ['stylesheet'],
  img: ['image'],
  media: ['media'],
  font: ['font'],
  ws: ['websocket'],
};

export function chipOf(type: string): Exclude<TypeChip, 'all'> {
  const t = type.toLowerCase();
  for (const [chip, types] of Object.entries(CHIP_TYPES)) if (types.includes(t)) return chip as Exclude<TypeChip, 'all'>;
  return 'other';
}

export interface RowFilter {
  /** Free text. Words must all match; `method:post`, `status:4xx` / `status:404` / `status:failed`,
   *  `type:xhr` and `-word` (exclude) are understood. */
  text: string;
  /** Empty or containing 'all' = every type. */
  chips: TypeChip[];
}

function statusMatches(row: NetRow, want: string): boolean {
  const w = want.toLowerCase();
  if (w === 'failed' || w === 'err' || w === 'error') return row.failed;
  if (w === 'pending') return row.pending;
  if (row.status === null) return false;
  if (/^\dxx$/.test(w)) return String(row.status)[0] === w[0];
  return String(row.status) === w;
}

function wordMatches(row: NetRow, word: string): boolean {
  const colon = word.indexOf(':');
  if (colon > 0 && !word.includes('//')) {
    const key = word.slice(0, colon).toLowerCase();
    const value = word.slice(colon + 1);
    if (key === 'method') return row.method.toLowerCase() === value.toLowerCase();
    if (key === 'status') return statusMatches(row, value);
    if (key === 'type') return row.type.toLowerCase() === value.toLowerCase();
  }
  const w = word.toLowerCase();
  return (
    row.url.toLowerCase().includes(w) ||
    row.method.toLowerCase() === w ||
    (row.status !== null && String(row.status) === w) ||
    (row.notes?.toLowerCase().includes(w) ?? false)
  );
}

export function rowMatches(row: NetRow, filter: RowFilter): boolean {
  const chips = filter.chips.filter((c) => c !== 'all');
  if (chips.length > 0 && !chips.includes(chipOf(row.type))) return false;
  for (const raw of filter.text.trim().split(/\s+/)) {
    if (!raw) continue;
    if (raw.startsWith('-') && raw.length > 1) {
      if (wordMatches(row, raw.slice(1))) return false;
    } else if (!wordMatches(row, raw)) {
      return false;
    }
  }
  return true;
}

export function filterRows(rows: NetRow[], filter: RowFilter): NetRow[] {
  if (!filter.text.trim() && filter.chips.every((c) => c === 'all')) return rows;
  return rows.filter((r) => rowMatches(r, filter));
}

/** Toggle a chip: 'all' clears the others; the last chip removed falls back to 'all'. */
export function toggleChip(chips: TypeChip[], chip: TypeChip, multi: boolean): TypeChip[] {
  if (chip === 'all') return ['all'];
  if (!multi) return chips.length === 1 && chips[0] === chip ? ['all'] : [chip];
  const without = chips.filter((c) => c !== 'all');
  const next = without.includes(chip) ? without.filter((c) => c !== chip) : [...without, chip];
  return next.length === 0 ? ['all'] : next;
}

// ---- rules -----------------------------------------------------------------------------------

export type RuleKindId = 'block' | 'redirect' | 'mock' | 'rewrite_body';

export interface RuleForm {
  pattern: string;
  regex: boolean;
  method: string;
  stage: NetStage;
  kind: RuleKindId;
  redirectUrl: string;
  mockStatus: string;
  mockContentType: string;
  mockBody: string;
  find: string;
  replace: string;
}

export const EMPTY_RULE: RuleForm = {
  pattern: '',
  regex: false,
  method: '',
  stage: 'request',
  kind: 'block',
  redirectUrl: '',
  mockStatus: '200',
  mockContentType: '',
  mockBody: '',
  find: '',
  replace: '',
};

/** A bare fragment (`/api/items`) becomes a glob that finds it anywhere (`*\/api/items*`). */
export function globFor(pattern: string): string | undefined {
  const p = pattern.trim();
  if (!p || p === '*') return undefined;
  return /[*?]/.test(p) ? p : `*${p}*`;
}

export function buildMatch(pattern: string, regex: boolean, method: string, stage?: NetStage): NetMatch {
  const match: NetMatch = {};
  const p = pattern.trim();
  if (regex && p) match.url_regex = p;
  else {
    const glob = globFor(p);
    if (glob) match.url_glob = glob;
  }
  if (method.trim()) match.method = method.trim().toUpperCase();
  if (stage) match.stage = stage;
  return match;
}

/** `net.rule_add` params from the form, or an error message for the user. */
export function buildRule(form: RuleForm): { match: NetMatch; action: NetAction } | { error: string } {
  // Redirects only exist at the request stage; mocks answer before sending anyway.
  const stage: NetStage = form.kind === 'redirect' || form.kind === 'mock' ? 'request' : form.stage;
  const match = buildMatch(form.pattern, form.regex, form.method, stage === 'request' ? undefined : stage);
  switch (form.kind) {
    case 'block':
      return { match, action: 'block' };
    case 'redirect': {
      const url = form.redirectUrl.trim();
      if (!url) return { error: 'Enter the URL to redirect to.' };
      return { match, action: { redirect: { url } } };
    }
    case 'mock': {
      const status = form.mockStatus.trim() ? Number(form.mockStatus) : 200;
      if (!Number.isInteger(status) || status < 100 || status > 599) return { error: 'Status must be 100–599.' };
      const mock: { status: number; body: string; headers?: Record<string, string> } = { status, body: form.mockBody };
      if (form.mockContentType.trim()) mock.headers = { 'content-type': form.mockContentType.trim() };
      return { match, action: { mock } };
    }
    case 'rewrite_body': {
      if (!form.find) return { error: 'Enter the text to find.' };
      return { match, action: { rewrite_body: { find: form.find, replace: form.replace } } };
    }
  }
}

// ---- breakpoints -----------------------------------------------------------------------------

export interface ResumeDraft {
  url: string;
  method: string;
  /** `Name: value` per line. */
  headers: string;
  body: string;
  status: string;
  /** Response stage: send `responseBody` instead of the real body. */
  replaceBody: boolean;
  responseBody: string;
}

type HeaderList = [string, string][];

export function headersToText(headers: HeaderList): string {
  return headers.map(([n, v]) => `${n}: ${v}`).join('\n');
}

/** `Name: value` lines -> pairs; blank lines and lines without a name are skipped. */
export function parseHeaderText(text: string): HeaderList {
  const out: HeaderList = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const at = line.indexOf(':', line.startsWith(':') ? 1 : 0);
    const name = (at < 0 ? line : line.slice(0, at)).trim();
    if (!name) continue;
    out.push([name, at < 0 ? '' : line.slice(at + 1).trim()]);
  }
  return out;
}

/** Changes from `before` to `after` for `edits.headers` (null removes); undefined = none. */
export function diffHeaders(before: HeaderList, after: HeaderList): Record<string, string | null> | undefined {
  const lower = (h: HeaderList) => new Map(h.map(([n, v]) => [n.toLowerCase(), { n, v }]));
  const a = lower(before);
  const b = lower(after);
  const out: Record<string, string | null> = {};
  for (const [key, { n, v }] of b) if (a.get(key)?.v !== v) out[n] = v;
  for (const [key, { n }] of a) if (!b.has(key)) out[n] = null;
  return Object.keys(out).length ? out : undefined;
}

function pausedHeaders(p: NetPaused): HeaderList {
  if (p.stage === 'response') return (p.response_headers ?? []).map((h) => [h.name, h.value]);
  return Object.entries(p.headers ?? {});
}

export function draftFromPaused(p: NetPaused): ResumeDraft {
  return {
    url: p.url,
    method: p.method,
    headers: headersToText(pausedHeaders(p)),
    body: p.post_data ?? '',
    status: p.status !== undefined && p.status !== null ? String(p.status) : '',
    replaceBody: false,
    responseBody: '',
  };
}

/** `net.resume` params: only what the user changed, `action: drop` drops. */
export function buildResume(p: NetPaused, draft: ResumeDraft, action: 'continue' | 'drop'): NetResume {
  if (action === 'drop') return { pause_id: p.pause_id, action: 'drop' };
  const edits: NetEdits = {};
  const headers = diffHeaders(pausedHeaders(p), parseHeaderText(draft.headers));
  if (headers) edits.headers = headers;
  if (p.stage === 'request') {
    const url = draft.url.trim();
    if (url && url !== p.url) edits.url = url;
    const method = draft.method.trim().toUpperCase();
    if (method && method !== p.method.toUpperCase()) edits.method = method;
    if (draft.body !== (p.post_data ?? '')) edits.body = draft.body;
  } else {
    const status = Number(draft.status);
    if (draft.status.trim() && Number.isInteger(status) && status !== p.status) edits.status = status;
    if (draft.replaceBody) edits.response_body = draft.responseBody;
  }
  const out: NetResume = { pause_id: p.pause_id, action: 'continue' };
  if (Object.keys(edits).length) out.edits = edits;
  return out;
}
