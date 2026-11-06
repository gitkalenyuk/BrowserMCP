import { describe, expect, it } from 'vitest';
import type { HealProposedEvent } from '../api/types-settings';
import { mockPatch } from '../api/mock-settings';
import {
  applyApplied,
  applyFailed,
  applyProposed,
  confidenceLevel,
  confidencePct,
  diffStats,
  emptyHeal,
  parseDiff,
  pendingProposal,
  upsertProposal,
  withProposals,
} from './heal';

const DIFF = `--- a/login.ts
+++ b/login.ts
@@ -12,4 +12,5 @@ async function main
 await browser.nav.goto({ url: inputs.url });
-await browser.page.act({ role: "button", name: "Sign in" }, "click");
+await browser.page.act({ role: "button", name: /sign in|log in/i }, "click");
+await browser.nav.wait({ for: "load" });
 const ok = true;
\\ No newline at end of file
`;

function event(id: string, attempt: number, extra: Partial<HealProposedEvent> = {}): HealProposedEvent {
  return {
    id: 'r1',
    proposal_id: id,
    row: 7,
    script: 'login',
    diff: DIFF,
    patched_code: '// patched',
    explanation: 'Renamed button',
    confidence: 0.82,
    attempt,
    max_attempts: 3,
    checks: ['applies', 'transpiles'],
    mode: 'ask',
    ...extra,
  };
}

describe('diff rendering', () => {
  it('numbers old and new lines per hunk', () => {
    const lines = parseDiff(DIFF);
    expect(lines.map((l) => l.kind)).toEqual(['meta', 'meta', 'hunk', 'ctx', 'del', 'add', 'add', 'ctx', 'meta']);
    expect(lines[3]).toEqual({ kind: 'ctx', text: 'await browser.nav.goto({ url: inputs.url });', old: 12, new: 12 });
    expect(lines[4]).toMatchObject({ kind: 'del', old: 13, new: null });
    expect(lines[5]).toMatchObject({ kind: 'add', old: null, new: 13 });
    expect(lines[6]).toMatchObject({ kind: 'add', new: 14 });
    expect(lines[7]).toMatchObject({ kind: 'ctx', old: 14, new: 15 });
    expect(diffStats(lines)).toEqual({ added: 2, removed: 1 });
  });

  it('copes with CRLF and loose +/- lines without a hunk header', () => {
    const lines = parseDiff('-a\r\n+b\r\n c');
    expect(lines).toEqual([
      { kind: 'del', text: 'a', old: null, new: null },
      { kind: 'add', text: 'b', old: null, new: null },
      { kind: 'ctx', text: 'c', old: null, new: null },
    ]);
    expect(parseDiff('')).toEqual([]);
  });

  it('labels confidence like the auto threshold', () => {
    expect(confidenceLevel(0.8)).toBe('high');
    expect(confidenceLevel(0.6)).toBe('medium');
    expect(confidenceLevel(0.2)).toBe('low');
    expect(confidencePct(0.826)).toBe('83%');
    expect(confidencePct(1.4)).toBe('100%');
  });

  it('renders the mock patch as a valid one-line diff', () => {
    const code = 'a\nb\nconst x = await browser.page.find({ role: "button", name: "Sign in" });\nc\n';
    const p = mockPatch(code, 3, 'login');
    const lines = parseDiff(p.diff);
    expect(diffStats(lines)).toEqual({ added: 1, removed: 1 });
    expect(lines.find((l) => l.kind === 'del')).toMatchObject({ old: 3 });
    expect(p.patched).toContain('name: "Log in"');
    // A blank failing line: the nearest locator within 6 lines is patched.
    const q = mockPatch('x\n\nawait act("button", "Sign in", "click");\n', 2, 'login');
    expect(parseDiff(q.diff).find((l) => l.kind === 'add')).toMatchObject({ new: 3, text: 'await act("button", "Log in", "click");' });
  });
});

describe('heal state per run', () => {
  it('folds proposals: a newer one supersedes the pending one', () => {
    let s = applyProposed(emptyHeal(), event('hp1', 1), 1000);
    expect(pendingProposal(s)?.proposal_id).toBe('hp1');
    s = applyProposed(s, event('hp2', 2, { note: 'confidence 0.5 is below 0.8' }), 2000);
    expect(s.proposals.map((p) => [p.proposal_id, p.status])).toEqual([
      ['hp2', 'pending'],
      ['hp1', 'superseded'],
    ]);
    expect(s.note).toMatch(/below/);
  });

  it('keeps what the list call knows when the event repeats a proposal', () => {
    let s = withProposals(emptyHeal(), [{ ...applyProposed(emptyHeal(), event('hp1', 1), 500).proposals[0], provider: 'claude-code', model: 'sonnet' }]);
    s = applyProposed(s, event('hp1', 1), 9000);
    expect(s.proposals).toHaveLength(1);
    expect(s.proposals[0]).toMatchObject({ provider: 'claude-code', model: 'sonnet', created_ms: 500, status: 'pending' });
    // A command answer without the code keeps the code from the event.
    s = upsertProposal(s, { ...s.proposals[0], patched_code: '' });
    expect(s.proposals[0].patched_code).toBe('// patched');
  });

  it('marks the accepted proposal and clears the failure', () => {
    let s = applyFailed(emptyHeal(), { id: 'r1', row: 7, attempts: 0, error: 'no AI provider is configured' });
    expect(s.failed?.error).toMatch(/provider/);
    s = applyProposed(s, event('hp1', 1));
    expect(s.failed).toBeNull();
    s = applyApplied(s, { id: 'r1', proposal_id: 'hp1', script: 'login.v2', requeued: 1, auto: true });
    expect(s.proposals[0].status).toBe('accepted');
    expect(s.applied?.script).toBe('login.v2');
    expect(pendingProposal(s)).toBeNull();
  });

  it('rejects what is pending when the AI gives up', () => {
    let s = applyProposed(emptyHeal(), event('hp3', 3));
    s = applyFailed(s, { id: 'r1', row: 7, attempts: 3, error: 'no usable patch' });
    expect(s.proposals[0].status).toBe('rejected');
    expect(pendingProposal(s)).toBeNull();
    expect(pendingProposal(undefined)).toBeNull();
  });
});
