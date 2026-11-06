// The Traffic panel's forms against the core's response-stage limits (CEF keeps a server
// response's status and headers), as the mock API in api/mock-panels.ts enforces them.
import { describe, expect, it } from 'vitest';
import { createPanelMocks, responseProblem } from '../api/mock-panels';
import type { NetPaused } from '../api/types';
import { EMPTY_RULE, buildResume, buildRule, draftFromPaused, type RuleKindId } from './logic';

const res: NetPaused = {
  pause_id: 'p1',
  tab: 1,
  rule: 'b1',
  stage: 'response',
  method: 'GET',
  url: 'https://api.test/me',
  resource_type: 'xhr',
  headers: {},
  status: 200,
  response_headers: [{ name: 'content-type', value: 'application/json' }],
};
const failed: NetPaused = { ...res, status: undefined, response_headers: undefined, error: 'ConnectionReset' };

describe('responseProblem (mock net.resume)', () => {
  it('refuses a new status or header change on a server response', () => {
    expect(responseProblem(res, { status: 500 })).toMatch(/status 200 → 500/);
    expect(responseProblem(res, { headers: { 'x-a': '1' } })).toMatch(/the headers/);
    expect(responseProblem(res, { headers: { 'content-type': null } })).toMatch(/the headers/);
  });
  it('accepts the body alone and no-op edits', () => {
    expect(responseProblem(res, { response_body: 'x' })).toBeNull();
    expect(responseProblem(res, { status: 200, headers: { 'Content-Type': 'application/json' } })).toBeNull();
    expect(responseProblem(res, {})).toBeNull();
  });
  it('a failed response takes status and headers together with a body', () => {
    expect(responseProblem(failed, { status: 503, headers: { 'retry-after': '5' }, response_body: 'down' })).toBeNull();
    expect(responseProblem(failed, { status: 503 })).toMatch(/no body to keep/);
  });
  it('whatever the user types, the breakpoint form sends nothing the core refuses', () => {
    const d = { ...draftFromPaused(res), status: '404', headers: 'x-new: 1', replaceBody: true, responseBody: 'b' };
    expect(responseProblem(res, buildResume(res, d, 'continue').edits ?? {})).toBeNull();
  });
});

describe('mock net.rule_add', () => {
  const mocks = createPanelMocks(
    () => {},
    () => 1,
  );
  it('refuses response-stage header rules and mocks, keeps request-stage ones', () => {
    const at = (stage: 'request' | 'response') => ({ tab: 1, match: { stage } });
    expect(() => mocks['net.rule_add']({ ...at('response'), action: { set_headers: { a: '1' } } })).toThrow(/set_headers/);
    expect(() => mocks['net.rule_add']({ ...at('response'), action: { remove_headers: ['a'] } })).toThrow(/remove_headers/);
    expect(() => mocks['net.rule_add']({ ...at('response'), action: { mock: { status: 404 } } })).toThrow(/request stage/);
    expect(mocks['net.rule_add']({ ...at('request'), action: { set_headers: { a: '1' } } })).toMatchObject({ id: expect.any(String) });
    expect(mocks['net.rule_add']({ ...at('response'), action: { rewrite_body: { find: 'a', replace: 'b' } } })).toMatchObject({ id: expect.any(String) });
  });
  it('accepts every rule the form builds, whatever stage was picked', () => {
    const kinds: RuleKindId[] = ['block', 'redirect', 'mock', 'rewrite_body'];
    for (const kind of kinds) {
      const built = buildRule({ ...EMPTY_RULE, kind, stage: 'response', redirectUrl: 'https://b.test/', find: 'a' });
      if ('error' in built) throw new Error(built.error);
      expect(() => mocks['net.rule_add']({ tab: 1, ...built })).not.toThrow();
    }
  });
});
