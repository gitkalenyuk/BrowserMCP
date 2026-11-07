// The new tab's Agents & runs and Quick launch widgets: which heal proposals still wait for a
// person, and what a long call that got no answer in time means. Pure, unit-tested.

import { isTimeout, type HealProposal, type RunInfo } from '../lib/api';

/** The open heal request of a run (`RunInfo.heal` while it is paused for a fix). */
function openRequest(run: RunInfo): { row: number; script: string } | null {
  const heal = run.heal as { row?: unknown; script?: unknown } | null | undefined;
  if (run.status !== 'paused' || !heal || typeof heal !== 'object') return null;
  return { row: Number(heal.row), script: String(heal.script) };
}

/**
 * Pending proposals a person can still approve. One whose run is listed but no longer waits for
 * that fix (cancelled, finished, resumed, fixed by hand, or broken at another row since) is
 * hidden; a run outside the list (older than the runs read) keeps its proposals.
 */
export function openProposals(proposals: HealProposal[], runs: RunInfo[]): HealProposal[] {
  const byId = new Map(runs.map((r) => [r.id, r]));
  return proposals.filter((p) => {
    if (p.status !== 'pending') return false;
    const run = byId.get(p.id);
    if (!run) return true;
    const open = openRequest(run);
    return open !== null && open.row === p.row && open.script === p.script;
  });
}

/**
 * After `run.heal_reject` failed: `true` when the AI may still be writing the next patch (the
 * call only timed out; `run.heal_proposed` / `run.heal_failed` will tell), `false` for a real
 * error.
 */
export const stillRewriting = (err: unknown): boolean => isTimeout(err);

export type ScriptOutcome =
  | { state: 'running' }
  | { state: 'ok' }
  | { state: 'failed'; error: string }
  /** The call timed out after the script's own limit: the result is unknown. */
  | { state: 'unknown' };

/**
 * The outcome of a `script.run` call that threw. A timeout is not a failure: the call waits
 * longer than the core lets the script run, so no answer means it was lost, not that it failed.
 */
export function outcomeOfError(err: unknown): ScriptOutcome {
  return isTimeout(err) ? { state: 'unknown' } : { state: 'failed', error: (err as Error)?.message ?? String(err) };
}
