// Pure helpers of the Runs panel: results paging, data sources, progress folding.

import type {
  HealRequest,
  RowStatus,
  RunCounts,
  RunInfo,
  RunProgress,
  RunStartArgs,
  RecordRunsEvents,
} from '../api/types-record-runs';

// ---- results paging (run.results is cursor based) ----------------------------------------

/** `cursors[i]` fetches page i (null = first page); `index` is the page shown. */
export interface Pager {
  cursors: (number | null)[];
  index: number;
}

export const firstPage = (): Pager => ({ cursors: [null], index: 0 });

export function pageCursor(p: Pager): number | null {
  return p.cursors[p.index] ?? null;
}

/** After loading the current page: can we go further, and with which cursor. */
export function nextPage(p: Pager, nextCursor: number | null | undefined): Pager | null {
  if (nextCursor === null || nextCursor === undefined) return null;
  const cursors = p.cursors.slice(0, p.index + 1);
  cursors.push(nextCursor);
  return { cursors, index: p.index + 1 };
}

export function prevPage(p: Pager): Pager | null {
  return p.index > 0 ? { cursors: p.cursors, index: p.index - 1 } : null;
}

export type StatusFilter = RowStatus | 'all';

/** Rows the filter selects, from the run's counts. */
export function filteredTotal(run: Pick<RunInfo, 'counts' | 'total'>, status: StatusFilter): number {
  return status === 'all' ? run.total : run.counts[status];
}

export function pageCount(rows: number, limit: number): number {
  return Math.max(1, Math.ceil(rows / limit));
}

/** "51–100 of 230" (rows shown on the current page). */
export function pageRange(p: Pager, limit: number, onPage: number, total: number): string {
  if (onPage === 0) return `0 of ${total}`;
  const from = p.index * limit + 1;
  return `${from}–${from + onPage - 1} of ${Math.max(total, from + onPage - 1)}`;
}

// ---- data sources ---------------------------------------------------------------------------

export type RunData = RunStartArgs['data'];

/** `{csv_path}` / `{json_path}` / `{xlsx_path}` by file extension. */
export function dataFromPath(path: string): RunData {
  const p = path.trim().replace(/^"(.*)"$/, '$1');
  if (!p) throw new Error('Enter the path of a CSV, JSON or Excel file');
  const ext = /\.([a-z0-9]+)$/i.exec(p)?.[1]?.toLowerCase();
  switch (ext) {
    case 'csv':
      return { csv_path: p };
    case 'json':
      return { json_path: p };
    case 'xlsx':
    case 'xls':
    case 'ods':
      return { xlsx_path: p };
    default:
      throw new Error('The file must end in .csv, .json, .xlsx, .xls or .ods');
  }
}

/** Split one CSV line (quotes, doubled quotes) on `sep`. */
function splitCsv(line: string, sep: string): string[] {
  const out: string[] = [];
  let cur = '';
  let quoted = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quoted) {
      if (c === '"' && line[i + 1] === '"') {
        cur += '"';
        i++;
      } else if (c === '"') quoted = false;
      else cur += c;
    } else if (c === '"' && cur.trim() === '') {
      quoted = true;
      cur = '';
    } else if (c === sep) {
      out.push(cur.trim());
      cur = '';
    } else cur += c;
  }
  out.push(quoted ? cur : cur.trim());
  return out;
}

/**
 * Rows pasted by hand: a JSON array of objects, JSON lines, or CSV/TSV with a header row
 * (separator guessed from the header: tab, semicolon or comma).
 */
export function parseRows(text: string): Record<string, unknown>[] {
  const t = text.trim();
  if (!t) throw new Error('Paste rows: a JSON array, JSON lines or CSV with a header row');
  if (t.startsWith('[')) {
    let v: unknown;
    try {
      v = JSON.parse(t);
    } catch (err) {
      throw new Error(`Not valid JSON: ${(err as Error).message}`);
    }
    if (!Array.isArray(v) || v.some((r) => typeof r !== 'object' || r === null || Array.isArray(r))) {
      throw new Error('The JSON must be an array of objects ({"column": value})');
    }
    return v as Record<string, unknown>[];
  }
  const lines = t.split(/\r?\n/).filter((l) => l.trim() !== '');
  if (t.startsWith('{')) {
    return lines.map((l, i) => {
      try {
        const v = JSON.parse(l);
        if (typeof v !== 'object' || v === null || Array.isArray(v)) throw new Error('not an object');
        return v as Record<string, unknown>;
      } catch (err) {
        throw new Error(`Line ${i + 1}: ${(err as Error).message}`);
      }
    });
  }
  const head = lines[0];
  const sep = head.includes('\t') ? '\t' : head.includes(';') && !head.includes(',') ? ';' : ',';
  const cols = splitCsv(head, sep);
  if (cols.some((c) => !c)) throw new Error('The header row has an empty column name');
  if (lines.length < 2) throw new Error('Only a header row: add at least one data row');
  return lines.slice(1).map((l) => {
    const cells = splitCsv(l, sep);
    return Object.fromEntries(cols.map((c, i) => [c, cells[i] ?? '']));
  });
}

// ---- progress ---------------------------------------------------------------------------

/** Fold a `run.progress` event into the run. */
export function applyProgress(run: RunInfo, p: RunProgress): RunInfo {
  const counts: RunCounts = {
    ok: p.ok,
    failed: p.failed,
    needs_heal: p.needs_heal,
    cancelled: p.cancelled,
    running: p.running,
    pending: p.pending,
  };
  return { ...run, counts, done: p.done, total: p.total };
}

export function applyFinished(run: RunInfo, f: RecordRunsEvents['run.finished']): RunInfo {
  return {
    ...run,
    status: f.status,
    total: f.total,
    counts: { ...run.counts, ok: f.ok, failed: f.failed, cancelled: f.cancelled, needs_heal: f.needs_heal, running: 0, pending: 0 },
    done: f.ok + f.failed + f.cancelled,
    finished_ms: run.finished_ms ?? run.created_ms + f.duration_ms,
  };
}

export function applyHeal(run: RunInfo, h: HealRequest): RunInfo {
  return { ...run, status: 'paused', heal: h };
}

/** The open heal request of a run, if its `heal` field looks like one. */
export function healOf(run: Pick<RunInfo, 'heal'>): HealRequest | null {
  const h = run.heal as Partial<HealRequest> | null | undefined;
  return h && typeof h === 'object' && typeof h.row === 'number' ? (h as HealRequest) : null;
}

export interface Segment {
  kind: 'ok' | 'failed' | 'needs_heal' | 'cancelled' | 'running';
  pct: number;
}

/** Widths of the progress bar segments, in percent of all rows. */
export function progressSegments(run: Pick<RunInfo, 'counts' | 'total'>): Segment[] {
  const total = Math.max(1, run.total);
  const kinds: Segment['kind'][] = ['ok', 'failed', 'needs_heal', 'cancelled', 'running'];
  return kinds
    .map((kind) => ({ kind, pct: (run.counts[kind] / total) * 100 }))
    .filter((s) => s.pct > 0);
}

export const isActive = (run: Pick<RunInfo, 'status'>) => run.status === 'running' || run.status === 'paused';

/** Insert or replace by id, newest (created_ms) first. */
export function upsertRun(runs: RunInfo[], run: RunInfo): RunInfo[] {
  const rest = runs.filter((r) => r.id !== run.id);
  rest.push(run);
  return rest.sort((a, b) => b.created_ms - a.created_ms);
}

/** "1 h 2 min", "42 s", "850 ms". */
export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return '';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ${s % 60 ? `${s % 60} s` : ''}`.trim();
  return `${Math.floor(m / 60)} h ${m % 60} min`;
}

/** Compact one-line JSON of a value for table cells. */
export function compact(value: unknown, max = 120): string {
  if (value === null || value === undefined) return '';
  const text = typeof value === 'string' ? value : JSON.stringify(value);
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}
