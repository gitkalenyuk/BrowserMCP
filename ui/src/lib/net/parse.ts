// Parsers for the compact text the net.* commands return (they are made for agents: one
// line per request, see crates/net/src/format.rs). Pure functions, tested in parse.test.ts.

export interface NetRow {
  id: number;
  method: string;
  /** HTTP status; null while pending or when the request failed. */
  status: number | null;
  failed: boolean;
  pending: boolean;
  /** CDP resource type, lowercase: xhr, fetch, document, script, stylesheet, image, websocket… */
  type: string;
  /** As printed: `3.4kB`, `512B`, `cache`, `-`. */
  size: string;
  /** As printed: `120ms`, `3.2s`, `open`, `-`. */
  time: string;
  /** May be shortened in the middle with `…` when longer than 120 characters. */
  url: string;
  error: string | null;
  frames: number | null;
  /** Rule notes, e.g. `r1 block`. */
  notes: string | null;
}

export interface NetListHeader {
  captured: number;
  capturing: boolean;
  dropped: number;
}

const LINE = /^#(\d+) (\S+) (\S+) (\S+) (\S+) (\S+) (.*)$/;

/** One `net.list` line: `#12 GET 200 xhr 3.4kB 120ms https://… [!err] [(N frames)] [[notes]]`. */
export function parseNetLine(line: string): NetRow | null {
  const m = LINE.exec(line);
  if (!m) return null;
  const [, id, method, statusText, type, size, time] = m;
  let rest = m[7];
  let notes: string | null = null;
  let frames: number | null = null;
  let error: string | null = null;
  const notesMatch = / \[([^\]]*)\]$/.exec(rest);
  if (notesMatch) {
    notes = notesMatch[1];
    rest = rest.slice(0, notesMatch.index);
  }
  const framesMatch = / \((\d+) frames\)$/.exec(rest);
  if (framesMatch && type === 'websocket') {
    frames = Number(framesMatch[1]);
    rest = rest.slice(0, framesMatch.index);
  }
  const failed = statusText === 'ERR';
  if (failed) {
    const errMatch = / !(\S+)$/.exec(rest);
    if (errMatch) {
      error = errMatch[1];
      rest = rest.slice(0, errMatch.index);
    }
  }
  const code = Number(statusText);
  return {
    id: Number(id),
    method,
    status: Number.isInteger(code) && !failed ? code : null,
    failed,
    pending: statusText === '…',
    type,
    size,
    time,
    url: rest,
    error,
    frames,
    notes,
  };
}

/** `tab 3: 12 captured (capture off), 3 oldest dropped, 12 match`. */
export function parseNetListHeader(line: string): NetListHeader | null {
  const m = /^tab \d+: (\d+) captured( \(capture off\))?(?:, (\d+) oldest dropped)?/.exec(line);
  if (!m) return null;
  return { captured: Number(m[1]), capturing: !m[2], dropped: Number(m[3] ?? 0) };
}

/** The whole `net.list` text: header + rows (the "… N more" footer is ignored). */
export function parseNetList(text: string): { header: NetListHeader | null; rows: NetRow[] } {
  const lines = text.split('\n');
  const header = parseNetListHeader(lines[0] ?? '');
  const rows: NetRow[] = [];
  for (const line of lines.slice(header ? 1 : 0)) {
    const row = parseNetLine(line);
    if (row) rows.push(row);
  }
  return { header, rows };
}

/**
 * Merge a page of rows into the known ones (same id = newer data wins), keep id order and
 * at most `max` rows (the core keeps the last 5000 too).
 */
export function mergeRows(known: NetRow[], page: NetRow[], max = 5000): NetRow[] {
  if (page.length === 0) return known;
  const byId = new Map<number, NetRow>();
  for (const r of known) byId.set(r.id, r);
  for (const r of page) byId.set(r.id, r);
  const merged = [...byId.values()].sort((a, b) => a.id - b.id);
  return merged.length > max ? merged.slice(merged.length - max) : merged;
}

/**
 * Where the next `net.list` poll should start: the oldest recent row that may still change
 * (pending, not a websocket — those stay open for minutes), else the server's next_cursor.
 */
export function nextPollCursor(rows: NetRow[], nextCursor: number, window = 200): number {
  const recent = rows.slice(-window);
  const pending = recent.find((r) => r.pending && r.type !== 'websocket');
  return pending ? Math.min(pending.id, nextCursor) : nextCursor;
}

// ---- net.get -------------------------------------------------------------------------------

export type Header = [name: string, value: string];

export interface NetBody {
  /** `3.4kB` or `3.4kB, cut to 4096B`; null when there is no body. */
  info: string | null;
  text: string;
  cut: boolean;
}

export interface NetDetail {
  /** `#12 GET https://…` */
  title: string;
  /** `status 200 OK · xhr · application/json · 3.4kB · 120ms · h2` */
  facts: string;
  /** initiator, notes, frames lines. */
  extra: string[];
  reqHeaders: Header[] | null;
  resHeaders: Header[] | null;
  reqBody: NetBody | null;
  resBody: NetBody | null;
}

function parseHeaderLine(line: string): Header {
  const s = line.replace(/^ {2}/, '');
  // HTTP/2 pseudo headers start with ':' (`:authority: example.com`).
  const at = s.indexOf(': ', s.startsWith(':') ? 1 : 0);
  if (at < 0) return [s.replace(/:$/, ''), ''];
  return [s.slice(0, at), s.slice(at + 2).replace(/…$/, '')];
}

function parseBody(head: string, rest: string): NetBody {
  // `request body (3.4kB, cut to 4096B):` / `request body: none` / `response body: still loading`
  const m = /^(?:request|response) body \((.*)\):$/.exec(head);
  if (m) return { info: m[1], text: rest, cut: m[1].includes('cut to') };
  const why = head.replace(/^(?:request|response) body:\s*/, '');
  return { info: null, text: why, cut: false };
}

/** The text of `net.get` (any combination of parts). */
export function parseNetGet(text: string): NetDetail {
  const lines = text.split('\n');
  const detail: NetDetail = {
    title: lines[0] ?? '',
    facts: lines[1] ?? '',
    extra: [],
    reqHeaders: null,
    resHeaders: null,
    reqBody: null,
    resBody: null,
  };
  let i = 2;
  let headers: Header[] | null = null;
  for (; i < lines.length; i++) {
    const line = lines[i];
    if (/^request headers \(\d+\):$/.test(line)) {
      headers = detail.reqHeaders = [];
    } else if (/^response headers \(\d+\):$/.test(line)) {
      headers = detail.resHeaders = [];
    } else if (/^(request|response) body/.test(line)) {
      break;
    } else if (headers && line.startsWith('  ')) {
      headers.push(parseHeaderLine(line));
    } else if (!headers) {
      detail.extra.push(line);
    }
  }
  if (i < lines.length) {
    // Bodies run to the end of the text, a request body up to the response body marker.
    const rest = lines.slice(i).join('\n');
    const reqAt = rest.startsWith('request body') ? 0 : -1;
    const resAt = rest.search(/(^|\n)response body[ :]/);
    if (reqAt === 0) {
      const end = resAt > 0 ? resAt : rest.length;
      const chunk = rest.slice(0, end);
      const nl = chunk.indexOf('\n');
      detail.reqBody = parseBody(nl < 0 ? chunk : chunk.slice(0, nl), nl < 0 ? '' : chunk.slice(nl + 1));
    }
    if (resAt >= 0) {
      const chunk = rest.slice(resAt).replace(/^\n/, '');
      const nl = chunk.indexOf('\n');
      detail.resBody = parseBody(nl < 0 ? chunk : chunk.slice(0, nl), nl < 0 ? '' : chunk.slice(nl + 1));
    }
  }
  return detail;
}

// ---- net.ws_frames ---------------------------------------------------------------------------

export interface WsFrame {
  n: number;
  sent: boolean;
  kind: string;
  size: string;
  /** Seconds since the socket opened. */
  at: number;
  data: string;
}

/** `3 → text 12B +0.123s {"op":"ping"}` per line after the header. */
export function parseWsFrames(text: string): WsFrame[] {
  const out: WsFrame[] = [];
  for (const line of text.split('\n').slice(1)) {
    const m = /^(\d+) (→|←) (\S+) (\S+) \+([\d.]+)s ?(.*)$/.exec(line);
    if (!m) continue;
    out.push({ n: Number(m[1]), sent: m[2] === '→', kind: m[3], size: m[4], at: Number(m[5]), data: m[6].replace(/\\n/g, '\n') });
  }
  return out;
}

// ---- rules and pauses ------------------------------------------------------------------------

export interface RuleRow {
  id: string;
  tab: number;
  stage: 'request' | 'response';
  /** `POST glob "*\/api/*"`, `regex "…"`, `any url`. */
  match: string;
  /** `block`, `redirect → url`, `mock 404`, `pause` (breakpoint)… */
  action: string;
  hits: number;
  breakpoint: boolean;
}

/** `net.rule_list` text: `r1 tab 3 request GET glob "*\/api/*" → block (2 hits)`. */
export function parseRuleLines(text: string): RuleRow[] {
  const out: RuleRow[] = [];
  for (const line of text.split('\n')) {
    const m = /^(\S+) tab (\d+) (request|response) (.+?) → (.+) \((\d+) hits\)$/.exec(line);
    if (!m) continue;
    out.push({
      id: m[1],
      tab: Number(m[2]),
      stage: m[3] as RuleRow['stage'],
      match: m[4],
      action: m[5],
      hits: Number(m[6]),
      breakpoint: m[5] === 'pause',
    });
  }
  return out;
}

export interface PausedRow {
  pause_id: string;
  rule: string;
  tab: number;
  stage: 'request' | 'response';
  method: string;
  status: number | null;
  url: string;
  secondsLeft: number;
}

/** `net.paused_list` text: `p3 b2 tab 1 request POST [200 ]https://… (42s left)`. */
export function parsePausedLines(text: string): PausedRow[] {
  const out: PausedRow[] = [];
  for (const line of text.split('\n')) {
    const m = /^(\S+) (\S+) tab (\d+) (request|response) (\S+)(?: (\d{3}))? (.+) \((\d+)s left\)$/.exec(line);
    if (!m) continue;
    out.push({
      pause_id: m[1],
      rule: m[2],
      tab: Number(m[3]),
      stage: m[4] as PausedRow['stage'],
      method: m[5],
      status: m[6] ? Number(m[6]) : null,
      url: m[7],
      secondsLeft: Number(m[8]),
    });
  }
  return out;
}
