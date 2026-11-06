import { describe, expect, it } from 'vitest';
import type { NetPaused } from '../api/types';
import {
  EMPTY_RULE,
  bodyOnly,
  buildResume,
  buildRule,
  chipOf,
  diffHeaders,
  draftFromPaused,
  filterRows,
  parseHeaderText,
  responseStageAllowed,
  toggleChip,
} from './logic';
import type { NetRow } from './parse';

const row = (id: number, extra: Partial<NetRow>): NetRow => ({
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

const ROWS = [
  row(1, { type: 'document', url: 'https://shop.test/' }),
  row(2, { type: 'xhr', method: 'POST', url: 'https://api.shop.test/cart', status: 201 }),
  row(3, { type: 'fetch', url: 'https://api.shop.test/items?page=2', status: 404 }),
  row(4, { type: 'image', url: 'https://cdn.test/logo.png', failed: true, status: null, error: 'net::ERR_BLOCKED_BY_CLIENT', notes: 'r1 block' }),
  row(5, { type: 'websocket', url: 'wss://api.shop.test/live', status: 101 }),
  row(6, { type: 'ping', url: 'https://t.test/beacon' }),
];
const ids = (rows: NetRow[]) => rows.map((r) => r.id);

describe('filterRows', () => {
  it('no filter returns the same array', () => {
    expect(filterRows(ROWS, { text: '', chips: ['all'] })).toBe(ROWS);
  });
  it('type chips group CDP types', () => {
    expect(ids(filterRows(ROWS, { text: '', chips: ['fetch'] }))).toEqual([2, 3]);
    expect(ids(filterRows(ROWS, { text: '', chips: ['img', 'ws'] }))).toEqual([4, 5]);
    expect(ids(filterRows(ROWS, { text: '', chips: ['other'] }))).toEqual([6]);
    expect(chipOf('Stylesheet')).toBe('css');
  });
  it('text matches url case-insensitively, all words must match', () => {
    expect(ids(filterRows(ROWS, { text: 'API.shop', chips: [] }))).toEqual([2, 3, 5]);
    expect(ids(filterRows(ROWS, { text: 'api items', chips: [] }))).toEqual([3]);
  });
  it('understands method:, status:, type: and -exclusions', () => {
    expect(ids(filterRows(ROWS, { text: 'method:post', chips: [] }))).toEqual([2]);
    expect(ids(filterRows(ROWS, { text: 'status:4xx', chips: [] }))).toEqual([3]);
    expect(ids(filterRows(ROWS, { text: 'status:failed', chips: [] }))).toEqual([4]);
    expect(ids(filterRows(ROWS, { text: 'type:websocket', chips: [] }))).toEqual([5]);
    expect(ids(filterRows(ROWS, { text: 'shop -api', chips: [] }))).toEqual([1]);
    expect(ids(filterRows(ROWS, { text: 'block', chips: [] }))).toEqual([4]);
  });
  it('a URL with a scheme is text, not a key:value', () => {
    expect(ids(filterRows(ROWS, { text: 'https://cdn.test', chips: [] }))).toEqual([4]);
  });
  it('combines chips and text', () => {
    expect(ids(filterRows(ROWS, { text: 'shop', chips: ['fetch'] }))).toEqual([2, 3]);
  });
});

describe('toggleChip', () => {
  it('single select toggles back to all', () => {
    expect(toggleChip(['all'], 'js', false)).toEqual(['js']);
    expect(toggleChip(['js'], 'js', false)).toEqual(['all']);
  });
  it('multi select adds and removes', () => {
    expect(toggleChip(['js'], 'css', true)).toEqual(['js', 'css']);
    expect(toggleChip(['js', 'css'], 'js', true)).toEqual(['css']);
    expect(toggleChip(['css'], 'css', true)).toEqual(['all']);
    expect(toggleChip(['css'], 'all', true)).toEqual(['all']);
  });
});

describe('buildRule', () => {
  it('blocks with a glob made from a fragment', () => {
    expect(buildRule({ ...EMPTY_RULE, pattern: '/ads/' })).toEqual({ match: { url_glob: '*/ads/*' }, action: 'block' });
    expect(buildRule({ ...EMPTY_RULE, pattern: '*.png', method: 'get' })).toEqual({
      match: { url_glob: '*.png', method: 'GET' },
      action: 'block',
    });
  });
  it('regex, redirect, mock and rewrite', () => {
    expect(buildRule({ ...EMPTY_RULE, pattern: '/items/\\d+', regex: true, kind: 'redirect', redirectUrl: ' https://b.test/ ' })).toEqual({
      match: { url_regex: '/items/\\d+' },
      action: { redirect: { url: 'https://b.test/' } },
    });
    expect(buildRule({ ...EMPTY_RULE, kind: 'mock', mockStatus: '404', mockBody: 'nope', mockContentType: 'text/plain' })).toEqual({
      match: {},
      action: { mock: { status: 404, body: 'nope', headers: { 'content-type': 'text/plain' } } },
    });
    expect(buildRule({ ...EMPTY_RULE, kind: 'rewrite_body', stage: 'response', find: 'a', replace: 'b' })).toEqual({
      match: { stage: 'response' },
      action: { rewrite_body: { find: 'a', replace: 'b' } },
    });
  });
  it('mocks and redirects always run at the request stage', () => {
    expect(responseStageAllowed('block')).toBe(true);
    expect(responseStageAllowed('rewrite_body')).toBe(true);
    expect(responseStageAllowed('mock')).toBe(false);
    expect(responseStageAllowed('redirect')).toBe(false);
    // A stage left over from another kind is not sent: net.rule_add refuses response-stage mocks.
    expect(buildRule({ ...EMPTY_RULE, kind: 'mock', stage: 'response' })).toEqual({ match: {}, action: { mock: { status: 200, body: '' } } });
    expect(buildRule({ ...EMPTY_RULE, kind: 'redirect', stage: 'response', redirectUrl: 'https://b.test/' })).toEqual({
      match: {},
      action: { redirect: { url: 'https://b.test/' } },
    });
    expect(buildRule({ ...EMPTY_RULE, kind: 'block', stage: 'response' })).toEqual({ match: { stage: 'response' }, action: 'block' });
  });
  it('reports form errors', () => {
    expect(buildRule({ ...EMPTY_RULE, kind: 'redirect' })).toEqual({ error: expect.any(String) });
    expect(buildRule({ ...EMPTY_RULE, kind: 'mock', mockStatus: '99' })).toEqual({ error: expect.any(String) });
    expect(buildRule({ ...EMPTY_RULE, kind: 'rewrite_body' })).toEqual({ error: expect.any(String) });
  });
});

describe('headers text', () => {
  it('parses Name: value lines', () => {
    expect(parseHeaderText('Accept: */*\n\n:authority: a.test\nX-Empty:\nbroken')).toEqual([
      ['Accept', '*/*'],
      [':authority', 'a.test'],
      ['X-Empty', ''],
      ['broken', ''],
    ]);
  });
  it('diffs case-insensitively, null removes', () => {
    expect(diffHeaders([['Accept', '*/*'], ['Cookie', 'a=1']], [['accept', '*/*'], ['X-New', '1']])).toEqual({ 'X-New': '1', Cookie: null });
    expect(diffHeaders([['A', '1']], [['a', '1']])).toBeUndefined();
  });
});

describe('buildResume', () => {
  const req: NetPaused = {
    pause_id: 'p1',
    tab: 1,
    rule: 'b1',
    stage: 'request',
    method: 'POST',
    url: 'https://api.test/login',
    resource_type: 'fetch',
    headers: { 'Content-Type': 'application/json', Cookie: 'sid=1' },
    post_data: '{"user":"a"}',
  };
  const res: NetPaused = {
    pause_id: 'p2',
    tab: 1,
    rule: 'b2',
    stage: 'response',
    method: 'GET',
    url: 'https://api.test/me',
    resource_type: 'xhr',
    headers: {},
    status: 200,
    response_headers: [{ name: 'content-type', value: 'application/json' }],
  };

  it('continue without changes sends no edits', () => {
    expect(buildResume(req, draftFromPaused(req), 'continue')).toEqual({ pause_id: 'p1', action: 'continue' });
    expect(buildResume(res, draftFromPaused(res), 'continue')).toEqual({ pause_id: 'p2', action: 'continue' });
  });

  it('drop ignores edits', () => {
    expect(buildResume(req, { ...draftFromPaused(req), url: 'x' }, 'drop')).toEqual({ pause_id: 'p1', action: 'drop' });
  });

  it('request stage: url, method, headers and body changes only', () => {
    const d = draftFromPaused(req);
    expect(
      buildResume(req, { ...d, url: 'https://api.test/login2', method: 'put', headers: 'Content-Type: text/plain', body: '{"user":"b"}' }, 'continue'),
    ).toEqual({
      pause_id: 'p1',
      action: 'continue',
      edits: {
        url: 'https://api.test/login2',
        method: 'PUT',
        headers: { 'Content-Type': 'text/plain', Cookie: null },
        body: '{"user":"b"}',
      },
    });
  });

  it('response stage of a server response: only the body, never status or headers', () => {
    const d = draftFromPaused(res);
    expect(d.status).toBe('200');
    expect(bodyOnly(res)).toBe(true);
    // CEF keeps the server's status and headers, so the core refuses them: they are not sent.
    expect(buildResume(res, { ...d, status: '500', headers: 'content-type: text/plain', replaceBody: true, responseBody: 'oops' }, 'continue')).toEqual({
      pause_id: 'p2',
      action: 'continue',
      edits: { response_body: 'oops' },
    });
    expect(buildResume(res, { ...d, status: '500', headers: '' }, 'continue')).toEqual({ pause_id: 'p2', action: 'continue' });
    // url/method/body are never sent at the response stage (the core rejects them).
    expect(buildResume(res, { ...d, url: 'https://x.test/', body: 'b' }, 'continue')).toEqual({ pause_id: 'p2', action: 'continue' });
  });

  it('response stage of a failed response: status, headers and body', () => {
    const failed: NetPaused = { ...res, pause_id: 'p3', status: undefined, response_headers: undefined, error: 'ConnectionReset' };
    expect(bodyOnly(failed)).toBe(false);
    const d = draftFromPaused(failed);
    expect(d.status).toBe('');
    expect(buildResume(failed, { ...d, status: '503', headers: 'retry-after: 5', replaceBody: true, responseBody: 'down' }, 'continue')).toEqual({
      pause_id: 'p3',
      action: 'continue',
      edits: { status: 503, headers: { 'retry-after': '5' }, response_body: 'down' },
    });
  });

  it('bodyOnly: request stage, and pauses known only from net.paused_list', () => {
    expect(bodyOnly(req)).toBe(false);
    // The list line shows the status of a server response and none for a failed one.
    expect(bodyOnly({ ...res, response_headers: undefined })).toBe(true);
    expect(bodyOnly({ ...res, status: undefined, response_headers: undefined })).toBe(false);
  });
});
