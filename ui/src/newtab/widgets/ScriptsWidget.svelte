<script lang="ts">
  import { t } from '../../lib/i18n/index.svelte';
  import { relativeTime } from '../../lib/i18n/intl';
  import { scriptRunTimeoutMs, type BrowserApi, type ScriptEntry } from '../../lib/api';
  import Icon from '../../lib/components/Icon.svelte';
  import { outcomeOfError, type ScriptOutcome } from '../agents';

  let { api }: { api: BrowserApi } = $props();

  /** Scripts shown as tiles: the most recently changed ones. */
  const MAX = 6;

  let scripts = $state<ScriptEntry[] | null>(null);
  /** Last outcome per script name (`script.run` waits until the script ends). */
  let outcome = $state<Record<string, ScriptOutcome>>({});

  /** How long the core lets a script run (script.settings); unknown: the highest allowed. */
  async function scriptTimeoutSecs(): Promise<number | null> {
    try {
      return (await api.call('script.settings', {})).settings.script_timeout_secs;
    } catch {
      return null;
    }
  }

  $effect(() => {
    api
      .call('script.list')
      .then((r) => (scripts = [...r.scripts].sort((a, b) => b.modified_ms - a.modified_ms).slice(0, MAX)))
      .catch(() => (scripts = []));
  });

  async function run(s: ScriptEntry) {
    if (outcome[s.name]?.state === 'running') return;
    outcome = { ...outcome, [s.name]: { state: 'running' } };
    try {
      // The call waits as long as the script may run (the 15 s default would report a longer
      // script as failed and let a second click start it again).
      const timeout = scriptRunTimeoutMs(await scriptTimeoutSecs());
      const r = await api.call('script.run', { name: s.name }, { timeout });
      outcome = {
        ...outcome,
        [s.name]: r.ok ? { state: 'ok' } : { state: 'failed', error: r.error?.message ?? t('newtab.scripts.exitCode', { code: String(r.exit_code ?? '—') }) },
      };
    } catch (err) {
      outcome = { ...outcome, [s.name]: outcomeOfError(err) };
    }
  }

  function meta(s: ScriptEntry): string {
    const o = outcome[s.name];
    if (o?.state === 'running') return t('newtab.scripts.running');
    if (o?.state === 'ok') return t('newtab.scripts.ok');
    if (o?.state === 'failed') return t('newtab.scripts.failed', { error: o.error });
    if (o?.state === 'unknown') return t('newtab.scripts.noAnswer');
    return t('newtab.scripts.changed', { when: relativeTime(s.modified_ms) });
  }
</script>

<section aria-labelledby="scripts-title">
  <h2 id="scripts-title">{t('newtab.widget.scripts')}</h2>
  {#if scripts === null}
    <p class="muted">{t('common.loading')}</p>
  {:else if scripts.length === 0}
    <p class="muted">{t('newtab.scripts.none')}</p>
  {:else}
    <div class="tiles">
      {#each scripts as s (s.name)}
        {@const o = outcome[s.name]}
        <button class="tile" onclick={() => run(s)} title={t('newtab.scripts.runHint', { name: s.name })} aria-busy={o?.state === 'running'}>
          <span class="play" class:ok={o?.state === 'ok'} class:bad={o?.state === 'failed'}>
            <Icon name={o?.state === 'ok' ? 'check' : o?.state === 'failed' ? 'x' : 'play'} size={13} stroke={2.25} />
          </span>
          <span class="text">
            <span class="name">{s.name}</span>
            <span class="meta">{meta(s)}</span>
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
  .play.bad,
  .tile:hover .play.bad {
    background: color-mix(in oklab, var(--danger) 16%, transparent);
    color: var(--danger);
  }
  .meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
</style>
