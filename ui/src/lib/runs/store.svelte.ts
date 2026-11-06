import type { BrowserApi } from '../api/client';
import type { RunInfo, ScriptEntry, TriggerInfo } from '../api/types-record-runs';
import { applyFinished, applyHeal, applyProgress, upsertRun } from './logic';

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
      api.on('run.finished', (f) => {
        const run = this.runs.find((r) => r.id === f.id);
        if (run) this.runs = upsertRun(this.runs, applyFinished(run, f));
        void this.refreshRun(f.id);
        this.tick++;
      }),
      api.on('trigger.fired', (t) => {
        this.notice = t.ok
          ? `Trigger ${t.id} ran ${t.script} (${Math.round(t.duration_ms / 100) / 10} s)`
          : `Trigger ${t.id} failed: ${t.error?.code ?? 'error'} ${t.error?.message ?? ''}`.trim();
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
