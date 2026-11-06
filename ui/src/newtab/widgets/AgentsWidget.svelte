<script lang="ts">
  import type { Agent, BrowserApi, Patch, Run } from '../../lib/api';
  import Icon from '../../lib/components/Icon.svelte';

  let { api }: { api: BrowserApi } = $props();

  let agents = $state<Agent[]>([]);
  let runs = $state<Run[]>([]);
  let patches = $state<Patch[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    const offs = [
      api.on('agents.updated', (p) => (agents = p.agents)),
      api.on('runs.updated', (p) => {
        runs = p.runs;
        patches = p.patches;
      }),
    ];
    Promise.all([api.call('agents.list'), api.call('runs.list')])
      .then(([a, r]) => {
        agents = a;
        runs = r.runs;
        patches = r.patches;
        loaded = true;
      })
      .catch((err: Error) => (error = err.message));
    return () => offs.forEach((off) => off());
  });

  const runName = (id: string) => runs.find((r) => r.id === id)?.name ?? id;
  const STATUS: Record<Run['status'], string> = {
    queued: 'Queued',
    running: 'Running',
    paused: 'Needs approval',
    done: 'Done',
    failed: 'Failed',
  };
  const AGENT_STATUS: Record<Agent['status'], string> = {
    idle: 'Idle',
    running: 'Working',
    waiting: 'Waiting for you',
    error: 'Error',
  };
</script>

<section aria-labelledby="agents-title">
  <h2 id="agents-title">Agents & runs</h2>

  {#if error}
    <p class="muted">Could not load agents: {error}</p>
  {:else if !loaded}
    <p class="muted">Loading…</p>
  {:else}
    {#each patches as p (p.id)}
      <div class="patch" role="group" aria-label="Patch awaiting approval">
        <div class="patch-head">
          <Icon name="patch" size={15} />
          <span class="patch-title">Patch for <strong>{p.script}</strong></span>
          <span class="muted small">{runName(p.runId)}</span>
        </div>
        <p class="patch-summary">{p.summary}</p>
        <div class="patch-actions">
          <button class="btn primary" onclick={() => api.call('runs.patch.approve', { id: p.id })}>
            <Icon name="check" size={14} stroke={2.25} /> Approve and resume
          </button>
          <button class="btn" onclick={() => api.call('runs.patch.reject', { id: p.id })}>Reject</button>
        </div>
      </div>
    {/each}

    <ul class="list">
      {#each agents as a (a.id)}
        <li class="agent" style:--c="var(--g-{a.color})">
          <span class="badge"><Icon name="bot" size={14} stroke={2} /></span>
          <span class="main">
            <span class="name">{a.name}</span>
            <span class="muted small ellipsis">{a.task ?? 'No task'}</span>
          </span>
          <span class="state" class:live={a.status === 'running'}>{AGENT_STATUS[a.status]}</span>
        </li>
      {:else}
        <li class="muted small">No agents connected. Add BrowserMCP to Claude Code or Codex to see them here.</li>
      {/each}
    </ul>

    {#if runs.length}
      <ul class="list runs">
        {#each runs as r (r.id)}
          {@const pct = r.total ? Math.round((r.done / r.total) * 100) : 0}
          <li class="run">
            <div class="run-head">
              <span class="name ellipsis">{r.name}</span>
              <span class="pill {r.status}">{STATUS[r.status]}</span>
            </div>
            <div
              class="bar {r.status}"
              role="progressbar"
              aria-valuenow={pct}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-label="{r.name} progress"
            >
              <span style:transform="scaleX({pct / 100})"></span>
            </div>
            <div class="muted small nums">
              <span>{r.done} / {r.total}</span>
              {#if r.failed}<span class="fail">{r.failed} failed</span>{/if}
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
  .pill.done {
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
  .bar.done span {
    background: var(--ok);
  }
  .bar.failed span {
    background: var(--danger);
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
