import type { BrowserApi } from '../api/client';
import type { RunInfo, ScriptEntry, TriggerInfo } from '../api/types-record-runs';
import type { HealProposal } from '../api/types-settings';
import { applyApplied, applyFailed, applyProposed, emptyHeal, upsertProposal, withProposals, type HealState } from './heal';
import { t as tr } from '../i18n/index.svelte';
import { applyFinished, applyHeal, applyProgress, formatDuration, upsertRun } from './logic';

/**
 * Runs, scripts and triggers, kept current from run.* / trigger.* events (also for runs
 * started by agents or triggers). One per API, created when the Runs or Recording panel opens.
 */
export class RunsStore {
  readonly api: BrowserApi;
  runs = $state<RunInfo[]>([]);
  scripts = $state<ScriptEntry[]>([]);
  triggers = $state<TriggerInfo[]>([]);
  error = $state<string | null>(null);
  /** Last trigger firing / row failure, for a one-line notice. */
  notice = $state<string | null>(null);
  /** Bumped on every progress/finish event: detail views reload their results page. */
  tick = $state(0);
  /** AI heal proposals and outcomes per run id (run.heal_* events, run.heal_proposals). */
  heal = $state<Record<string, HealState>>({});
  #unsubs: Array<() => void> = [];

  constructor(api: BrowserApi) {
    this.api = api;
    this.#unsubs.push(
      api.on('run.started', ({ id }) => void this.refreshRun(id)),
      api.on('run.progress', (p) => {
        const run = this.runs.find((r) => r.id === p.id);
        if (run) this.runs = upsertRun(this.runs, applyProgress(run, p));
        else void this.refreshRun(p.id);
        this.tick++;
      }),
      api.on('run.heal_needed', (h) => {
        const run = this.runs.find((r) => r.id === h.id);
        if (run) this.runs = upsertRun(this.runs, applyHeal(run, h));
        void this.refreshRun(h.id);
      }),
      api.on('run.heal_proposed', (e) => {
        this.#heal(e.id, (s) => applyProposed(s, e));
      }),
      api.on('run.heal_applied', (e) => {
        this.#heal(e.id, (s) => applyApplied(s, e));
        void this.refreshRun(e.id);
        void this.refreshScripts();
      }),
      api.on('run.heal_failed', (e) => {
        this.#heal(e.id, (s) => applyFailed(s, e));
      }),
      api.on('run.finished', (f) => {
        const run = this.runs.find((r) => r.id === f.id);
        if (run) this.runs = upsertRun(this.runs, applyFinished(run, f));
        void this.refreshRun(f.id);
        this.tick++;
      }),
      api.on('trigger.fired', (t) => {
        this.notice = t.ok
          ? tr('runs.trigger.ran', { id: t.id, script: t.script, time: formatDuration(t.duration_ms) })
          : tr('runs.trigger.failed', { id: t.id, error: `${t.error?.code ?? 'error'} ${t.error?.message ?? ''}`.trim() });
        void this.refreshTriggers();
      }),
      api.onStatus((s) => {
        if (s === 'open') void this.refresh();
      }),
    );
  }

  stop(): void {
    this.#unsubs.forEach((u) => u());
    this.#unsubs = [];
  }

  async #guard<T>(p: Promise<T>): Promise<T | undefined> {
    try {
      const v = await p;
      this.error = null;
      return v;
    } catch (err) {
      this.error = (err as Error).message;
      return undefined;
    }
  }

  async refresh(): Promise<void> {
    await Promise.all([this.refreshRuns(), this.refreshScripts(), this.refreshTriggers()]);
  }

  async refreshRuns(): Promise<void> {
    const r = await this.#guard(this.api.call('run.list', { limit: 100 }));
    if (r) this.runs = r.runs;
  }

  async refreshScripts(): Promise<void> {
    const r = await this.#guard(this.api.call('script.list'));
    if (r) this.scripts = r.scripts;
  }

  async refreshTriggers(): Promise<void> {
    const r = await this.#guard(this.api.call('trigger.list'));
    if (r) this.triggers = r.triggers;
  }

  async refreshRun(id: string): Promise<RunInfo | undefined> {
    try {
      const run = await this.api.call('run.status', { id });
      this.runs = upsertRun(this.runs, run);
      return run;
    } catch {
      return undefined;
    }
  }

  #heal(id: string, f: (s: HealState) => HealState): void {
    this.heal = { ...this.heal, [id]: f(this.heal[id] ?? emptyHeal()) };
  }

  /** Proposals the core still has for a run (they live in memory there). */
  async loadProposals(id: string): Promise<void> {
    try {
      const { proposals } = await this.api.call('run.heal_proposals', { id });
      this.#heal(id, (s) => withProposals(s, proposals));
    } catch {
      // No heal service in this core: the manual patch editor still works.
    }
  }

  /** Ask the AI for a patch now (also after a failed attempt). */
  async propose(id: string, feedback?: string): Promise<HealProposal> {
    const p: HealProposal = { patched_code: '', ...(await this.api.call('run.heal_propose', { id, feedback: feedback?.trim() || undefined })) };
    this.#heal(id, (s) => upsertProposal(s, p));
    return p;
  }

  async accept(p: HealProposal): Promise<void> {
    const r = await this.api.call('run.heal_accept', { proposal_id: p.proposal_id });
    this.#heal(p.id, (s) => applyApplied(s, { ...r, auto: false }));
    await this.refreshRun(p.id);
    void this.refreshScripts();
  }

  /** Reject with feedback; the answer is the next proposal (or an error after the last attempt). */
  async reject(p: HealProposal, feedback: string): Promise<HealProposal> {
    this.#heal(p.id, (s) => ({ ...s, proposals: s.proposals.map((x) => (x.proposal_id === p.proposal_id ? { ...x, status: 'rejected' } : x)) }));
    const next: HealProposal = {
      patched_code: '',
      ...(await this.api.call('run.heal_reject', { proposal_id: p.proposal_id, feedback: feedback.trim() || undefined })),
    };
    this.#heal(p.id, (s) => upsertProposal(s, next));
    return next;
  }

  /** pause / resume / cancel; the answer is the run's new state. */
  async control(id: string, action: 'pause' | 'resume' | 'cancel'): Promise<void> {
    const method = action === 'pause' ? 'run.pause' : action === 'resume' ? 'run.resume' : 'run.cancel';
    const run = await this.api.call(method, { id });
    this.runs = upsertRun(this.runs, run);
  }
}

const stores = new WeakMap<BrowserApi, RunsStore>();

export function runsStore(api: BrowserApi): RunsStore {
  let s = stores.get(api);
  if (!s) stores.set(api, (s = new RunsStore(api)));
  return s;
}
