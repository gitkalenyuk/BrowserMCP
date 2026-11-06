import { describe, expect, it } from 'vitest';
import type { RunInfo } from '../api/types-record-runs';
import {
  applyFinished,
  applyProgress,
  dataFromPath,
  filteredTotal,
  firstPage,
  formatDuration,
  healOf,
  nextPage,
  pageCount,
  pageCursor,
  pageRange,
  parseRows,
  prevPage,
  progressSegments,
  upsertRun,
} from './logic';

function run(over: Partial<RunInfo> = {}): RunInfo {
  return {
    id: 'run-1',
    script: 'login',
    status: 'running',
    total: 10,
    done: 0,
    created_ms: 1000,
    counts: { ok: 0, failed: 0, needs_heal: 0, cancelled: 0, running: 0, pending: 10 },
    options: { heal: true, parallel: 2, parallel_requested: 4, retries: 1, stop_on_error: false, timeout_secs: 300 },
    ...over,
  };
}

describe('results pager', () => {
  it('walks forward with cursors and back without refetching cursors', () => {
    let p = firstPage();
    expect(pageCursor(p)).toBeNull();
    p = nextPage(p, 100)!;
    expect(pageCursor(p)).toBe(100);
    p = nextPage(p, 200)!;
    expect(p).toEqual({ cursors: [null, 100, 200], index: 2 });
    expect(nextPage(p, null)).toBeNull();
    expect(nextPage(p, undefined)).toBeNull();
    p = prevPage(p)!;
    expect(pageCursor(p)).toBe(100);
    // Going forward again from page 1 replaces what came after it.
    p = nextPage(p, 150)!;
    expect(p).toEqual({ cursors: [null, 100, 150], index: 2 });
    expect(prevPage(firstPage())).toBeNull();
  });

  it('counts rows and pages for a status filter', () => {
    const r = run({ total: 230, counts: { ok: 200, failed: 30, needs_heal: 0, cancelled: 0, running: 0, pending: 0 } });
    expect(filteredTotal(r, 'all')).toBe(230);
    expect(filteredTotal(r, 'failed')).toBe(30);
    expect(pageCount(230, 50)).toBe(5);
    expect(pageCount(0, 50)).toBe(1);
    expect(pageRange({ cursors: [null, 50], index: 1 }, 50, 50, 230)).toBe('51–100 of 230');
    expect(pageRange(firstPage(), 50, 0, 0)).toBe('0 of 0');
  });
});

describe('data sources', () => {
  it('picks the data key by extension', () => {
    expect(dataFromPath('C:\\data\\leads.csv')).toEqual({ csv_path: 'C:\\data\\leads.csv' });
    expect(dataFromPath(' "rows.JSON" ')).toEqual({ json_path: 'rows.JSON' });
    expect(dataFromPath('book.xlsx')).toEqual({ xlsx_path: 'book.xlsx' });
    expect(() => dataFromPath('notes.txt')).toThrow(/\.csv/);
    expect(() => dataFromPath('  ')).toThrow(/path/);
  });

  it('parses pasted JSON, JSON lines and CSV', () => {
    expect(parseRows('[{"q":"a"},{"q":"b"}]')).toEqual([{ q: 'a' }, { q: 'b' }]);
    expect(parseRows('{"q":1}\n{"q":2}\n')).toEqual([{ q: 1 }, { q: 2 }]);
    expect(parseRows('email,name\nann@x.test,"Ann, A."\nbob@x.test,"Bob ""B"""')).toEqual([
      { email: 'ann@x.test', name: 'Ann, A.' },
      { email: 'bob@x.test', name: 'Bob "B"' },
    ]);
    expect(parseRows('a;b\n1;2')).toEqual([{ a: '1', b: '2' }]);
    expect(parseRows('a\tb\n1\t')).toEqual([{ a: '1', b: '' }]);
  });

  it('rejects rows it cannot use', () => {
    expect(() => parseRows('')).toThrow(/Paste rows/);
    expect(() => parseRows('[1,2]')).toThrow(/array of objects/);
    expect(() => parseRows('[{')).toThrow(/Not valid JSON/);
    expect(() => parseRows('only,header')).toThrow(/header row/);
    expect(() => parseRows('{"a":1}\n[2]')).toThrow(/Line 2/);
  });
});

describe('progress', () => {
  it('folds progress and finish events', () => {
    let r = applyProgress(run(), { id: 'run-1', done: 4, ok: 3, failed: 1, needs_heal: 0, cancelled: 0, running: 2, pending: 4, total: 10 });
    expect(r.done).toBe(4);
    expect(r.counts.running).toBe(2);
    expect(progressSegments(r)).toEqual([
      { kind: 'ok', pct: 30 },
      { kind: 'failed', pct: 10 },
      { kind: 'running', pct: 20 },
    ]);
    r = applyFinished(r, { id: 'run-1', status: 'finished', total: 10, ok: 9, failed: 1, cancelled: 0, needs_heal: 0, duration_ms: 5000 });
    expect(r.status).toBe('finished');
    expect(r.done).toBe(10);
    expect(r.finished_ms).toBe(6000);
  });

  it('reads the heal request and orders runs newest first', () => {
    expect(healOf(run())).toBeNull();
    expect(healOf(run({ heal: { id: 'run-1', row: 3, script: 'x', error: null, step: null, line: null } }))?.row).toBe(3);
    const list = upsertRun(upsertRun([], run({ id: 'a', created_ms: 1 })), run({ id: 'b', created_ms: 2 }));
    expect(list.map((r) => r.id)).toEqual(['b', 'a']);
    expect(upsertRun(list, run({ id: 'a', created_ms: 1, status: 'finished' })).find((r) => r.id === 'a')?.status).toBe('finished');
  });

  it('formats durations', () => {
    expect(formatDuration(850)).toBe('850 ms');
    expect(formatDuration(42_000)).toBe('42 s');
    expect(formatDuration(125_000)).toBe('2 min 5 s');
    expect(formatDuration(3_720_000)).toBe('1 h 2 min');
  });
});
