// Per-call options of BrowserApi.call (a longer timeout for slow commands) in WsApi and the mock,
// and the mock's heal proposals that go stale with their run (like crates/heal).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { isTimeout, RpcError, timeoutOf } from './client';
import { MockApi } from './mock';
import { scriptRunTimeoutMs } from './timeouts';
import { WsApi } from './ws';

/** A WebSocket the test answers by hand. */
class FakeSocket {
  static readonly OPEN = 1;
  static last: FakeSocket | null = null;
  readyState = 0;
  sent: Array<{ id?: number; method: string; params?: unknown }> = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(readonly url: string) {
    FakeSocket.last = this;
    setTimeout(() => {
      this.readyState = FakeSocket.OPEN;
      this.onopen?.();
    }, 0);
  }
  send(text: string): void {
    this.sent.push(JSON.parse(text));
  }
  close(): void {
    this.readyState = 3;
  }
  answer(id: number, result: unknown): void {
    this.onmessage?.({ data: JSON.stringify({ jsonrpc: '2.0', id, result }) });
  }
}

let mock: MockApi | null = null;
afterEach(() => {
  mock?.close();
  mock = null;
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('timeout options', () => {
  it('accepts only positive finite numbers', () => {
    expect(timeoutOf({ timeout: 5000 }, 15)).toBe(5000);
    expect(timeoutOf({}, 15)).toBe(15);
    expect(timeoutOf(undefined, 15)).toBe(15);
    expect(timeoutOf({ timeout: 0 }, 15)).toBe(15);
    expect(timeoutOf({ timeout: Number.NaN }, 15)).toBe(15);
    expect(timeoutOf({ timeout: 1e12 }, 15)).toBe(2_147_483_647);
  });

  it('waits a script out: its own limit plus a margin, the highest limit when unknown', () => {
    expect(scriptRunTimeoutMs(300)).toBe(330_000);
    expect(scriptRunTimeoutMs(null)).toBe(86_400_000 + 30_000);
    expect(scriptRunTimeoutMs(0)).toBe(86_400_000 + 30_000);
  });
});

describe('WsApi per-call timeout', () => {
  it('waits longer for a call that asks for it, keeps the default for the others', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', FakeSocket);
    const api = new WsApi({ url: 'ws://test', timeout: 1000, topics: [] });
    const connected = api.connect();
    await vi.advanceTimersByTimeAsync(1);
    await connected;

    const slow = api.call('script.run', { name: 'long' }, { timeout: 60_000 });
    const quick = api.call('tabs.list');
    const quickFailed = quick.catch((e: unknown) => e);
    const ws = FakeSocket.last!;
    expect(ws.sent.map((m) => m.method)).toEqual(['script.run', 'tabs.list']);
    expect(ws.sent[0].params).toEqual({ name: 'long' });
    expect('params' in ws.sent[1]).toBe(false);

    // 15 s in: the default limit (1 s here) is long gone, the long call still waits.
    await vi.advanceTimersByTimeAsync(15_000);
    const err = await quickFailed;
    expect(isTimeout(err)).toBe(true);
    ws.answer(ws.sent[0].id!, { ok: true, exit_code: 0 });
    await expect(slow).resolves.toEqual({ ok: true, exit_code: 0 });
    api.close();
  });

  it('times out at the per-call limit', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', FakeSocket);
    const api = new WsApi({ url: 'ws://test', timeout: 100_000, topics: [] });
    const connected = api.connect();
    await vi.advanceTimersByTimeAsync(1);
    await connected;
    const call = api.call('run.heal_reject', { proposal_id: 'hp1' }, { timeout: 500 }).catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(501);
    const err = (await call) as RpcError;
    expect(err).toBeInstanceOf(RpcError);
    expect(err.code).toBe('TIMEOUT');
    expect(err.message).toContain('500 ms');
    api.close();
  });
});

describe('MockApi per-call timeout', () => {
  it('rejects a slow answer like WsApi, and has no limit by default', async () => {
    mock = new MockApi({ simulate: false });
    // script.run takes ~1.2 s in the mock.
    const err = await mock.call('script.run', { code: 'result {}' }, { timeout: 20 }).catch((e: unknown) => e);
    expect(isTimeout(err)).toBe(true);
    const r = await mock.call('script.run', { code: 'result {}' });
    expect(r.ok).toBe(true);
  });
});

describe('mock heal proposals follow their run', () => {
  /** The proposal the mock writes by itself for its seeded paused run. */
  async function pausedRunWithProposal(api: MockApi) {
    const p = await new Promise<{ id: string; proposal_id: string }>((resolve) => api.on('run.heal_proposed', resolve));
    const run = await api.call('run.status', { id: p.id });
    expect(run.status).toBe('paused');
    return { run, p };
  }

  it('a cancelled run leaves no pending proposal to approve', async () => {
    mock = new MockApi({ simulate: false });
    const { run, p } = await pausedRunWithProposal(mock);
    await mock.call('run.cancel', { id: run.id });
    const { proposals } = await mock.call('run.heal_proposals', { id: run.id });
    expect(proposals.find((x) => x.proposal_id === p.proposal_id)?.status).toBe('superseded');
    await expect(mock.call('run.heal_accept', { proposal_id: p.proposal_id })).rejects.toThrow(/not pending/);
  });

  it('a run fixed by hand refuses the outdated proposal', async () => {
    mock = new MockApi({ simulate: false });
    const { run, p } = await pausedRunWithProposal(mock);
    await mock.call('run.heal_apply', { id: run.id, patched_code: '// fixed by hand\n' });
    await expect(mock.call('run.heal_accept', { proposal_id: p.proposal_id })).rejects.toThrow(/no longer waiting/);
    // And the run takes no second patch for the same break.
    await expect(mock.call('run.heal_apply', { id: run.id, patched_code: '// again\n' })).rejects.toThrow(/not waiting for a fix/);
  });
});
