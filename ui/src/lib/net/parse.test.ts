import { describe, expect, it } from 'vitest';
import {
  mergeRows,
  nextPollCursor,
  parseNetGet,
  parseNetLine,
  parseNetList,
  parsePausedLines,
  parseRuleLines,
  parseWsFrames,
  type NetRow,
} from './parse';

describe('parseNetLine', () => {
  it('reads a finished request', () => {
    expect(parseNetLine('#12 GET 200 xhr 3.4kB 120ms https://api.test/items?page=2')).toEqual({
      id: 12,
      method: 'GET',
      status: 200,
      failed: false,
      pending: false,
      type: 'xhr',
      size: '3.4kB',
      time: '120ms',
      url: 'https://api.test/items?page=2',
      error: null,
      frames: null,
      notes: null,
    });
  });

  it('reads failed, pending, websocket and noted requests', () => {
    expect(parseNetLine('#3 GET ERR image - - https://a.test/x.png !net::ERR_BLOCKED_BY_CLIENT [r1 block]')).toMatchObject({
      status: null,
      failed: true,
      error: 'net::ERR_BLOCKED_BY_CLIENT',
      url: 'https://a.test/x.png',
      notes: 'r1 block',
    });
    expect(parseNetLine('#4 POST … fetch - - https://a.test/api')).toMatchObject({ pending: true, status: null });
    expect(parseNetLine('#5 GET 101 websocket - open wss://a.test/ws (7 frames)')).toMatchObject({
      frames: 7,
      time: 'open',
      url: 'wss://a.test/ws',
    });
  });

  it('keeps spaces inside data: URLs', () => {
    expect(parseNetLine('#6 GET 200 image cache 2ms data:image/svg+xml,<svg a b>')?.url).toBe('data:image/svg+xml,<svg a b>');
  });

  it('ignores other lines', () => {
    expect(parseNetLine('… 3 more: net_list cursor=9')).toBeNull();
  });
});

describe('parseNetList', () => {
  it('reads the header and rows', () => {
    const { header, rows } = parseNetList(
      'tab 1: 5 captured (capture off), 2 oldest dropped, 2 match\n#3 GET 200 document 1.2kB 80ms https://a.test/\n#4 GET 404 script - 9ms https://a.test/x.js\n… 1 more: net_list cursor=5',
    );
    expect(header).toEqual({ captured: 5, capturing: false, dropped: 2 });
    expect(rows.map((r) => r.id)).toEqual([3, 4]);
    expect(parseNetList('tab 2: 0 captured, 0 match').header).toEqual({ captured: 0, capturing: true, dropped: 0 });
  });
});

const row = (id: number, extra: Partial<NetRow> = {}): NetRow => ({
  id,
  method: 'GET',
  status: 200,
  failed: false,
  pending: false,
  type: 'xhr',
  size: '-',
  time: '-',
  url: `https://a.test/${id}`,
  error: null,
  frames: null,
  notes: null,
  ...extra,
});

describe('mergeRows / nextPollCursor', () => {
  it('replaces rows by id, keeps order and the cap', () => {
    const merged = mergeRows([row(1), row(2, { pending: true, status: null })], [row(2), row(3)], 2);
    expect(merged.map((r) => [r.id, r.pending])).toEqual([
      [2, false],
      [3, false],
    ]);
  });

  it('polls again from the oldest pending non-websocket row', () => {
    const rows = [row(1, { type: 'websocket', pending: true }), row(2), row(3, { pending: true }), row(4)];
    expect(nextPollCursor(rows, 5)).toBe(3);
    expect(nextPollCursor([row(1), row(2)], 3)).toBe(3);
  });
});

describe('parseNetGet', () => {
  it('reads summary and headers', () => {
    const d = parseNetGet(
      [
        '#7 POST https://api.test/login',
        'status 200 OK · fetch · application/json · 120B · 50ms · h2',
        'initiator: script https://a.test/app.js:10',
        'request headers (2):',
        '  :authority: api.test',
        '  content-type: application/json',
        'response headers (1):',
        '  set-cookie: a=b; Path=/',
      ].join('\n'),
    );
    expect(d.title).toBe('#7 POST https://api.test/login');
    expect(d.extra).toEqual(['initiator: script https://a.test/app.js:10']);
    expect(d.reqHeaders).toEqual([
      [':authority', 'api.test'],
      ['content-type', 'application/json'],
    ]);
    expect(d.resHeaders).toEqual([['set-cookie', 'a=b; Path=/']]);
    expect(d.reqBody).toBeNull();
  });

  it('reads bodies, including multi-line ones and missing ones', () => {
    const d = parseNetGet('#7 POST u\nfacts\ninitiator: other\nrequest body (12B):\n{"a":\n 1}\nresponse body (5.0kB, cut to 4096B):\n<html>\n</html>');
    expect(d.reqBody).toEqual({ info: '12B', text: '{"a":\n 1}', cut: false });
    expect(d.resBody).toEqual({ info: '5.0kB, cut to 4096B', text: '<html>\n</html>', cut: true });
    const none = parseNetGet('#7 GET u\nfacts\ninitiator: other\nrequest body: none');
    expect(none.reqBody).toEqual({ info: null, text: 'none', cut: false });
    const loading = parseNetGet('#7 GET u\nfacts\ninitiator: other\nresponse body: still loading');
    expect(loading.resBody).toEqual({ info: null, text: 'still loading', cut: false });
  });
});

describe('parseWsFrames', () => {
  it('reads frame lines after the header', () => {
    const frames = parseWsFrames('#5 websocket wss://a.test/ws — 2 frames\n0 → text 12B +0.010s {"op":"hi"}\n1 ← binary 4B +1.500s AAAA…');
    expect(frames).toEqual([
      { n: 0, sent: true, kind: 'text', size: '12B', at: 0.01, data: '{"op":"hi"}' },
      { n: 1, sent: false, kind: 'binary', size: '4B', at: 1.5, data: 'AAAA…' },
    ]);
  });
});

describe('rule and pause lines', () => {
  it('parseRuleLines', () => {
    expect(
      parseRuleLines('r1 tab 3 request GET glob "*/api/*" → block (2 hits)\nr2 tab 3 request any url → redirect → https://b.test/ (0 hits)\nb3 tab 1 response any url → pause (1 hits)'),
    ).toEqual([
      { id: 'r1', tab: 3, stage: 'request', match: 'GET glob "*/api/*"', action: 'block', hits: 2, breakpoint: false },
      { id: 'r2', tab: 3, stage: 'request', match: 'any url', action: 'redirect → https://b.test/', hits: 0, breakpoint: false },
      { id: 'b3', tab: 1, stage: 'response', match: 'any url', action: 'pause', hits: 1, breakpoint: true },
    ]);
    expect(parseRuleLines('no rules or breakpoints')).toEqual([]);
  });

  it('parsePausedLines', () => {
    expect(parsePausedLines('p3 b2 tab 1 request POST https://a.test/api (42s left)\np4 b2 tab 1 response GET 404 https://a.test/x (3s left)')).toEqual([
      { pause_id: 'p3', rule: 'b2', tab: 1, stage: 'request', method: 'POST', status: null, url: 'https://a.test/api', secondsLeft: 42 },
      { pause_id: 'p4', rule: 'b2', tab: 1, stage: 'response', method: 'GET', status: 404, url: 'https://a.test/x', secondsLeft: 3 },
    ]);
    expect(parsePausedLines('nothing is paused')).toEqual([]);
  });
});
