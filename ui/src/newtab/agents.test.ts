import { describe, expect, it } from 'vitest';
import { RpcError, type HealProposal, type RunInfo } from '../lib/api';
import { openProposals, outcomeOfError, stillRewriting } from './agents';

const proposal = (proposal_id: string, id: string, extra: Partial<HealProposal> = {}): HealProposal =>
  ({ proposal_id, id, row: 1, script: 'login', status: 'pending', ...extra }) as HealProposal;
const run = (id: string, status: string, heal: unknown = null): RunInfo => ({ id, status, heal }) as unknown as RunInfo;

describe('heal proposals on the new tab', () => {
  it('shows a pending proposal only while its run waits for that fix', () => {
    const proposals = [
      proposal('waiting', 'r1'),
      proposal('cancelled', 'r2'),
      proposal('fixed-by-hand', 'r3'),
      proposal('older-break', 'r4', { row: 0 }),
      proposal('unknown-run', 'r9'),
      proposal('rejected', 'r1', { status: 'rejected' }),
    ];
    const runs = [
      run('r1', 'paused', { row: 1, script: 'login' }),
      run('r2', 'cancelled'),
      // Fixed by hand: running again with login.v2, no heal request.
      run('r3', 'running'),
      // Broke again at another row.
      run('r4', 'paused', { row: 5, script: 'login.v2' }),
    ];
    expect(openProposals(proposals, runs).map((p) => p.proposal_id)).toEqual(['waiting', 'unknown-run']);
  });

  it('keeps waiting for the next patch when the reject call only timed out', () => {
    expect(stillRewriting(new RpcError('TIMEOUT', 'run.heal_reject timed out after 180000 ms'))).toBe(true);
    expect(stillRewriting(new RpcError('INVALID_PARAMS', 'no usable patch after 3 attempts'))).toBe(false);
    expect(stillRewriting(new Error('boom'))).toBe(false);
  });
});

describe('quick launch outcome', () => {
  it('does not call a script failed when the answer only timed out', () => {
    expect(outcomeOfError(new RpcError('TIMEOUT', 'script.run timed out'))).toEqual({ state: 'unknown' });
    expect(outcomeOfError(new RpcError('NOT_FOUND', "no script 'x'"))).toEqual({ state: 'failed', error: "no script 'x'" });
  });
});
