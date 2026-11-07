<script lang="ts">
  import { t } from '../../lib/i18n/index.svelte';
  import { HEAL_AI_TIMEOUT_MS, type Agent, type BrowserApi, type HealProposal, type RunInfo } from '../../lib/api';
  import Icon from '../../lib/components/Icon.svelte';
  import { confidencePct } from '../../lib/runs/heal';
  import { openProposals, stillRewriting } from '../agents';

  let { api }: { api: BrowserApi } = $props();

  /** Runs shown: the active ones first, then the latest finished ones. */
  const MAX_RUNS = 4;

  let agents = $state<Agent[]>([]);
  let runs = $state<RunInfo[]>([]);
  let proposals = $state<HealProposal[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  /** Proposal ids being accepted / rejected (buttons disabled meanwhile). */
  let busy = $state<Record<string, boolean>>({});
  let actionError = $state<string | null>(null);
  /** Runs whose rejected patch the AI is rewriting: run id → script name. */
  let rewriting = $state<Record<string, string>>({});

  const ACTIVE = new Set(['running', 'paused']);
  const shown = $derived(
    [...runs]
      .sort((a, b) => Number(ACTIVE.has(b.status)) - Number(ACTIVE.has(a.status)) || b.created_ms - a.created_ms)
      .slice(0, MAX_RUNS),
  );
  // Only proposals whose run still waits for that fix (not cancelled, finished or fixed by hand).
  const pending = $derived(openProposals(proposals, runs));
  const rewritingRuns = $derived(Object.entries(rewriting).filter(([id]) => !pending.some((p) => p.id === id)));

  async function loadRuns(): Promise<void> {
    // Runs and heal proposals are optional: a core without the jobs / heal service still shows agents.
    const [r, h] = await Promise.allSettled([api.call('run.list', { limit: 20 }), api.call('run.heal_proposals', {})]);
    if (r.status === 'fulfilled') runs = r.value.runs;
    if (h.status === 'fulfilled') proposals = h.value.proposals;
  }

  let timer: ReturnType<typeof setTimeout> | null = null;
  /** Progress events come in bursts: re-read at most every 500 ms. */
  function refreshSoon(): void {
    if (timer) return;
    timer = setTimeout(() => {
      timer = null;
      void loadRuns();
    }, 500);
  }

  function doneRewriting(run: string): void {
    if (!(run in rewriting)) return;
    const rest = { ...rewriting };
    delete rest[run];
    rewriting = rest;
  }

  $effect(() => {
    const offs = [
      api.on('agents.updated', (p) => (agents = p.agents)),
      api.on('run.started', refreshSoon),
      api.on('run.progress', refreshSoon),
      api.on('run.finished', (e) => {
        doneRewriting(e.id);
        refreshSoon();
      }),
      api.on('run.heal_needed', refreshSoon),
      // The answer to a rejection that outlived its call: the next patch, or no patch at all.
      api.on('run.heal_proposed', (e) => {
        doneRewriting(e.id);
        refreshSoon();
      }),
      api.on('run.heal_applied', refreshSoon),
      api.on('run.heal_failed', (e) => {
        if (e.id in rewriting) actionError = e.error;
        doneRewriting(e.id);
        refreshSoon();
      }),
    ];
    Promise.all([api.call('agents.list'), loadRuns()])
      .then(([a]) => {
        agents = a.agents;
        loaded = true;
      })
      .catch((err: Error) => (error = err.message));
    return () => {
      offs.forEach((off) => off());
      if (timer) clearTimeout(timer);
    };
  });

  async function decide(p: HealProposal, accept: boolean): Promise<void> {
    busy = { ...busy, [p.proposal_id]: true };
    actionError = null;
    try {
      if (accept) {
        await api.call('run.heal_accept', { proposal_id: p.proposal_id });
      } else {
        // The core rejects at once, then the AI writes the next patch before it answers (maybe
        // minutes later): the card goes now, a note stands in until the next one arrives.
        rewriting = { ...rewriting, [p.id]: p.script };
        proposals = proposals.map((x) => (x.proposal_id === p.proposal_id ? { ...x, status: 'rejected' } : x));
        await api.call('run.heal_reject', { proposal_id: p.proposal_id }, { timeout: HEAL_AI_TIMEOUT_MS });
        doneRewriting(p.id);
      }
    } catch (err) {
      // A timeout only means the AI is still writing: run.heal_proposed / run.heal_failed follow.
      if (accept || !stillRewriting(err)) {
        doneRewriting(p.id);
        actionError = (err as Error).message;
      }
    } finally {
      const rest = { ...busy };
      delete rest[p.proposal_id];
      busy = rest;
      // Either way the card's status changed (or it turned out outdated): show what is true now.
      await loadRuns();
    }
  }
  // Status labels: `newtab.run.<status>` and `newtab.agent.<status>`.
</script>

<section aria-labelledby="agents-title">
  <h2 id="agents-title">{t('newtab.widget.agents')}</h2>

  {#if error}
    <p class="muted">{t('newtab.agents.loadFailed', { error })}</p>
  {:else if !loaded}
    <p class="muted">{t('common.loading')}</p>
  {:else}
    {#each pending as p (p.proposal_id)}
      <div class="patch" role="group" aria-label={t('newtab.patch.label')}>
        <div class="patch-head">
          <Icon name="patch" size={15} />
          <span class="patch-title">{t('newtab.patch.for')} <strong>{p.script}</strong></span>
          <span class="muted small">{t('newtab.patch.confidence', { pct: confidencePct(p.confidence) })}</span>
        </div>
        <p class="patch-summary">{p.explanation}</p>
        <div class="patch-actions">
          <button class="btn primary" disabled={busy[p.proposal_id]} onclick={() => void decide(p, true)}>
            <Icon name="check" size={14} stroke={2.25} /> {t('newtab.patch.approve')}
          </button>
          <button class="btn" disabled={busy[p.proposal_id]} onclick={() => void decide(p, false)}>{t('newtab.patch.reject')}</button>
        </div>
      </div>
    {/each}
    {#each rewritingRuns as [run, script] (run)}
      <p class="patch rewriting muted small" role="status">
        <Icon name="patch" size={15} />
        {t('newtab.patch.rewriting', { script })}
      </p>
    {/each}
    {#if actionError}<p class="fail small" role="alert">{actionError}</p>{/if}

    <ul class="list">
      {#each agents as a (a.id)}
        <li class="agent" style:--c="var(--g-{a.color})">
          <span class="badge"><Icon name="bot" size={14} stroke={2} /></span>
          <span class="main">
            <span class="name">{a.name}</span>
            <span class="muted small ellipsis">{a.task ?? t('newtab.agents.noTask')}</span>
          </span>
          <span class="state" class:live={a.status === 'running'}>{t(`newtab.agent.${a.status}`)}</span>
        </li>
      {:else}
        <li class="muted small">{t('newtab.agents.none')}</li>
      {/each}
    </ul>

    {#if shown.length}
      <ul class="list runs">
        {#each shown as r (r.id)}
          {@const pct = r.total ? Math.round((r.done / r.total) * 100) : 0}
          {@const healing = r.status === 'paused' && r.heal != null}
          <li class="run">
            <div class="run-head">
              <span class="name ellipsis">{r.script}</span>
              <span class="pill {r.status}" class:heal={healing}>{healing ? t('newtab.run.needsFix') : t(`newtab.run.${r.status}`)}</span>
            </div>
            <div
              class="bar {r.status}"
              role="progressbar"
              aria-valuenow={pct}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-label={t('newtab.run.progress', { name: r.script })}
            >
              <span style:transform="scaleX({pct / 100})"></span>
            </div>
            <div class="muted small nums">
              <span>{r.done} / {r.total}</span>
              {#if r.counts.failed}<span class="fail">{t('newtab.run.failedCount', { n: r.counts.failed })}</span>{/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

<style>
  h2 {
    margin: 0 0 12px;
    font-size: var(--fs-lg);
    font-weight: 650;
  }
  .muted {
    color: var(--fg-muted);
  }
  .small {
    font-size: var(--fs-sm);
  }
  .ellipsis {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .list {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .agent {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    border-bottom: 1px solid var(--border);
  }
  .badge {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: color-mix(in oklab, var(--c) 16%, transparent);
    color: var(--c);
    flex: none;
  }
  .main {
    display: grid;
    flex: 1;
    min-width: 0;
  }
  .name {
    font-weight: 600;
  }
  .state {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }
  .state.live::before {
    content: '';
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--c);
  }
  .runs {
    margin-top: 14px;
    gap: 12px;
  }
  .run-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .pill {
    padding: 1px 7px;
    border-radius: 9px;
    background: var(--sunken);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
  }
  .pill.running {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .pill.paused {
    background: color-mix(in oklab, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .pill.finished {
    color: var(--ok);
  }
  .pill.failed {
    color: var(--danger);
  }
  .bar {
    height: 4px;
    margin: 6px 0 4px;
    border-radius: 2px;
    background: var(--sunken);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    border-radius: 2px;
    background: var(--accent);
    transform-origin: left;
    transition: transform 600ms var(--ease-out);
  }
  .bar.paused span {
    background: var(--warn);
  }
  .bar.finished span {
    background: var(--ok);
  }
  .bar.failed span {
    background: var(--danger);
  }
  .bar.cancelled span {
    background: var(--fg-subtle);
  }
  .nums {
    display: flex;
    gap: 10px;
    font-variant-numeric: tabular-nums;
  }
  .fail {
    color: var(--danger);
  }
  .patch {
    margin-bottom: 14px;
    padding: 12px 14px;
    border: 1px solid color-mix(in oklab, var(--warn) 40%, var(--border));
    border-radius: var(--r-lg);
    background: color-mix(in oklab, var(--warn) 7%, var(--surface));
  }
  .patch.rewriting {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .patch-head {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--warn);
  }
  .patch-title {
    color: var(--fg);
  }
  .patch-head .small {
    margin-left: auto;
  }
  .patch-summary {
    margin: 6px 0 10px;
    font-size: var(--fs-sm);
    line-height: 1.5;
    color: var(--fg-muted);
    user-select: text;
  }
  .patch-actions {
    display: flex;
    gap: 6px;
  }
</style>
