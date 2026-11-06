<script lang="ts">
  import type { BrowserApi, Script } from '../../lib/api';
  import Icon from '../../lib/components/Icon.svelte';

  let { api }: { api: BrowserApi } = $props();
  let scripts = $state<Script[] | null>(null);
  let started = $state<string | null>(null);

  $effect(() => {
    api.call('scripts.list').then((s) => (scripts = s)).catch(() => (scripts = []));
  });

  async function run(s: Script) {
    await api.call('scripts.run', { id: s.id });
    started = s.id;
    scripts = await api.call('scripts.list');
    setTimeout(() => started === s.id && (started = null), 2000);
  }
</script>

<section aria-labelledby="scripts-title">
  <h2 id="scripts-title">Quick launch</h2>
  {#if scripts === null}
    <p class="muted">Loading…</p>
  {:else if scripts.length === 0}
    <p class="muted">No scripts yet. Record one from the Recording panel.</p>
  {:else}
    <div class="tiles">
      {#each scripts as s (s.id)}
        <button class="tile" onclick={() => run(s)} title={s.description}>
          <span class="play" class:ok={started === s.id}>
            <Icon name={started === s.id ? 'check' : 'play'} size={13} stroke={2.25} />
          </span>
          <span class="text">
            <span class="name">{s.name}</span>
            <span class="meta">{started === s.id ? 'Started' : s.lastRun ? `Ran ${s.lastRun}` : 'Never run'}</span>
          </span>
        </button>
      {/each}
    </div>
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
    margin: 0;
  }
  .tiles {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .tile {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px 8px 8px;
    border: 1px solid transparent;
    border-radius: var(--r-lg);
    background: transparent;
    text-align: left;
    transition: background-color 120ms, border-color 120ms;
  }
  .tile:hover {
    border-color: var(--border);
    background: var(--surface);
  }
  .play {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: var(--sunken);
    color: var(--fg-muted);
    flex: none;
    transition: background-color 120ms, color 120ms;
  }
  .tile:hover .play {
    background: var(--accent);
    color: var(--on-accent);
  }
  .play.ok,
  .tile:hover .play.ok {
    background: color-mix(in oklab, var(--ok) 18%, transparent);
    color: var(--ok);
  }
  .text {
    display: grid;
    min-width: 0;
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
</style>
