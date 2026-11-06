// The AI heal flow of a paused run (crates/heal): proposals from `run.heal_proposed` /
// `run.heal_proposals`, the outcome from `run.heal_applied` / `run.heal_failed`, and a unified
// diff parsed for display. Pure, unit-tested; RunsStore keeps the state.

import type {
  HealAppliedEvent,
  HealFailedEvent,
  HealProposal,
  HealProposedEvent,
} from '../api/types-settings';

// ---- diff rendering --------------------------------------------------------------------------

export type DiffLineKind = 'add' | 'del' | 'ctx' | 'hunk' | 'meta';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  /** Line number in the old / new script (null for added / removed / header lines). */
  old: number | null;
  new: number | null;
}

/**
 * A unified diff (`--- a`, `+++ b`, `@@ -12,7 +12,7 @@`, ` `, `-`, `+`) as numbered lines. Lines
 * before the first hunk and `\ No newline` markers are `meta`. Without any hunk header (a model
 * sent loose lines) the +/- prefixes are still honoured, without numbers.
 */
export function parseDiff(diff: string): DiffLine[] {
  const out: DiffLine[] = [];
  let oldNo: number | null = null;
  let newNo: number | null = null;
  const lines = diff.replace(/\r\n/g, '\n').split('\n');
  if (lines.length && lines[lines.length - 1] === '') lines.pop();
  for (const line of lines) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(line);
    if (hunk) {
      oldNo = Number(hunk[1]);
      newNo = Number(hunk[2]);
      out.push({ kind: 'hunk', text: line, old: null, new: null });
    } else if (line.startsWith('@@')) {
      out.push({ kind: 'hunk', text: line, old: null, new: null });
    } else if (line.startsWith('---') || line.startsWith('+++') || line.startsWith('\\') || line.startsWith('diff ') || line.startsWith('index ')) {
      out.push({ kind: 'meta', text: line, old: null, new: null });
    } else if (line.startsWith('+')) {
      out.push({ kind: 'add', text: line.slice(1), old: null, new: newNo });
      if (newNo !== null) newNo++;
    } else if (line.startsWith('-')) {
      out.push({ kind: 'del', text: line.slice(1), old: oldNo, new: null });
      if (oldNo !== null) oldNo++;
    } else {
      out.push({ kind: 'ctx', text: line.startsWith(' ') ? line.slice(1) : line, old: oldNo, new: newNo });
      if (oldNo !== null) oldNo++;
      if (newNo !== null) newNo++;
    }
  }
  return out;
}

export function diffStats(lines: DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const l of lines) {
    if (l.kind === 'add') added++;
    else if (l.kind === 'del') removed++;
  }
  return { added, removed };
}

// ---- confidence ------------------------------------------------------------------------------

/** "auto" mode applies a patch from 0.8 on (crates/heal); below 0.5 it is a guess. */
export function confidenceLevel(c: number): 'high' | 'medium' | 'low' {
  if (c >= 0.8) return 'high';
  if (c >= 0.5) return 'medium';
  return 'low';
}

export function confidencePct(c: number): string {
  return `${Math.round(Math.max(0, Math.min(1, c)) * 100)}%`;
}

// ---- state per run ---------------------------------------------------------------------------

export interface HealState {
  /** Newest first. */
  proposals: HealProposal[];
  applied: HealAppliedEvent | null;
  failed: HealFailedEvent | null;
  /** `note` of the latest proposal ("auto" mode did not apply it). */
  note: string | null;
}

export const emptyHeal = (): HealState => ({ proposals: [], applied: null, failed: null, note: null });

/** A `run.heal_proposed` event as a pending proposal. */
export function proposalFromEvent(e: HealProposedEvent, now = Date.now()): HealProposal {
  return {
    proposal_id: e.proposal_id,
    id: e.id,
    row: e.row,
    script: e.script,
    diff: e.diff,
    patched_code: e.patched_code,
    explanation: e.explanation,
    confidence: e.confidence,
    attempt: e.attempt,
    max_attempts: e.max_attempts,
    checks: e.checks ?? [],
    status: 'pending',
    provider: '',
    model: null,
    created_ms: now,
  };
}

/** Add or replace a proposal; a new pending one supersedes the older pending ones. */
export function upsertProposal(s: HealState, p: HealProposal): HealState {
  const others = s.proposals
    .filter((x) => x.proposal_id !== p.proposal_id)
    .map((x) => (p.status === 'pending' && x.status === 'pending' ? { ...x, status: 'superseded' as const } : x));
  const known = s.proposals.find((x) => x.proposal_id === p.proposal_id);
  // Keep what only the list call knows (provider, model, created) when an event repeats it.
  const merged = known
    ? { ...known, ...p, provider: p.provider || known.provider, model: p.model ?? known.model, patched_code: p.patched_code || known.patched_code, created_ms: known.created_ms }
    : p;
  return {
    ...s,
    proposals: [merged, ...others].sort((a, b) => b.created_ms - a.created_ms || b.attempt - a.attempt),
    // A new proposal is a new attempt: an older failure no longer counts.
    failed: p.status === 'pending' ? null : s.failed,
    applied: p.status === 'pending' ? null : s.applied,
  };
}

export function applyProposed(s: HealState, e: HealProposedEvent, now = Date.now()): HealState {
  return { ...upsertProposal(s, proposalFromEvent(e, now)), note: e.note ?? null };
}

export function applyApplied(s: HealState, e: HealAppliedEvent): HealState {
  return {
    ...s,
    applied: e,
    failed: null,
    proposals: s.proposals.map((p) =>
      p.proposal_id === e.proposal_id ? { ...p, status: 'accepted' as const } : p.status === 'pending' ? { ...p, status: 'superseded' as const } : p,
    ),
  };
}

export function applyFailed(s: HealState, e: HealFailedEvent): HealState {
  return {
    ...s,
    failed: e,
    proposals: s.proposals.map((p) => (p.status === 'pending' ? { ...p, status: 'rejected' as const } : p)),
  };
}

/** Replace the list with what `run.heal_proposals` returned (newest first). */
export function withProposals(s: HealState, list: HealProposal[]): HealState {
  return { ...s, proposals: [...list].sort((a, b) => b.created_ms - a.created_ms || b.attempt - a.attempt) };
}

/** The proposal waiting for a decision, if any. */
export function pendingProposal(s: HealState | undefined): HealProposal | null {
  return s?.proposals.find((p) => p.status === 'pending') ?? null;
}
