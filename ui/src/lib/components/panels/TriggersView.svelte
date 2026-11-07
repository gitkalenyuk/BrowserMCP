<script lang="ts">
  import { t } from '../../i18n/index.svelte';
  import { cronPreview, formatFire, parseCron, describeCron } from '../../runs/cron';
  import { runsStore } from '../../runs/store.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  const store = runsStore(useChrome().api);

  // Chip labels: `runs.trg.example.<n>`.
  const EXAMPLES = ['0 9 * * Mon-Fri', '*/15 * * * *', '0 2 * * *', '0 8 1 * *'];

  let adding = $state(false);
  let spec = $state('0 9 * * Mon-Fri');
  let script = $state('');
  let inputsText = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  let now = $state(new Date());

  const preview = $derived(cronPreview(spec, now, 5));

  $effect(() => {
    void store.refreshTriggers();
    void store.refreshScripts();
    const timer = setInterval(() => (now = new Date()), 30_000);
    return () => clearInterval(timer);
  });
  $effect(() => {
    if (!script && store.scripts.length) script = store.scripts[0].name;
  });

  /** `key=value` per line → inputs. */
  function parseInputs(text: string): Record<string, string> {
    const out: Record<string, string> = {};
    for (const [i, raw] of text.split('\n').entries()) {
      const line = raw.trim();
      if (!line) continue;
      const eq = line.indexOf('=');
      if (eq <= 0) throw new Error(t('runs.trg.badInput', { n: i + 1 }));
      out[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
    }
    return out;
  }

  async function add(e: SubmitEvent) {
    e.preventDefault();
    error = null;
    if (!preview.ok) {
      error = preview.error;
      return;
    }
    if (!script) {
      error = t('runs.trg.needScript');
      return;
    }
    busy = true;
    try {
      const inputs = parseInputs(inputsText);
      await store.api.call('trigger.set', { kind: 'cron', spec: spec.trim(), script, inputs });
      adding = false;
      inputsText = '';
      await store.refreshTriggers();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  async function remove(id: string) {
    error = null;
    try {
      await store.api.call('trigger.remove', { id });
      await store.refreshTriggers();
    } catch (err) {
      error = (err as Error).message;
    }
  }

  function describe(s: string): string {
    try {
      return describeCron(parseCron(s));
    } catch {
      return t('runs.trg.invalid');
    }
  }
</script>

<div class="rr-bar">
  <button class="btn" class:primary={!adding} aria-expanded={adding} onclick={() => (adding = !adding)}>
    <Icon name={adding ? 'x' : 'plus'} size={13} />
    {adding ? t('common.close') : t('runs.trg.new')}
  </button>
  <span class="rr-spacer"></span>
  <button class="icon-btn" aria-label={t('runs.trg.refresh')} title={t('record.refreshShort')} onclick={() => store.refreshTriggers()}><Icon name="reload" size={14} /></button>
</div>

{#if error}
  <div class="rr-note err" role="alert">
    <span>{error}</span>
    <button class="icon-btn" aria-label={t('chrome.dismiss')} onclick={() => (error = null)}><Icon name="x" size={12} /></button>
  </div>
{/if}

<div class="rr-scroll">
  {#if adding}
    <form class="rr-section rr-form add" onsubmit={add}>
      <label class="rr-field">
        {t('record.detail.script')}
        <select bind:value={script} disabled={store.scripts.length === 0}>
          {#if store.scripts.length === 0}<option value="">{t('runs.form.noScripts')}</option>{/if}
          {#each store.scripts as s (s.name)}<option value={s.name}>{s.name}</option>{/each}
        </select>
      </label>
      <label class="rr-field">
        <span>{t('runs.trg.schedule')} <span class="opt">{t('runs.trg.scheduleHint')}</span></span>
        <input class="mono" bind:value={spec} spellcheck="false" aria-invalid={!preview.ok} aria-describedby="cron-preview" />
      </label>
      <div class="rr-chips">
        {#each EXAMPLES as ex, i (ex)}
          <button type="button" class="rr-chip" aria-pressed={spec.trim() === ex} onclick={() => (spec = ex)}>{t(`runs.trg.example.${i + 1}`)}</button>
        {/each}
      </div>
      <div class="preview" id="cron-preview" aria-live="polite">
        {#if preview.ok}
          <p class="human">{preview.text}</p>
          {#if preview.warning}<p class="warn">{preview.warning}</p>{/if}
          {#if preview.next.length}
            <ol class="fires">
              {#each preview.next as d (d.getTime())}<li>{formatFire(d, now)}</li>{/each}
            </ol>
          {/if}
        {:else}
          <p class="bad">{preview.error}</p>
          {#if preview.hint}<p class="rr-hint">{preview.hint}</p>{/if}
        {/if}
      </div>
      <label class="rr-field">
        <span>{t('runs.col.inputs')} <span class="opt">{t('runs.trg.inputsHint')}</span></span>
        <textarea bind:value={inputsText} rows="3" spellcheck="false" placeholder="url=https://shop.example.test/p/1"></textarea>
      </label>
      <div class="rr-row">
        <button class="btn primary" type="submit" disabled={busy || !preview.ok}>{t('runs.trg.add')}</button>
        <span class="rr-hint">{t('runs.trg.missed')}</span>
      </div>
    </form>
  {/if}

  {#if store.triggers.length === 0 && !adding}
    <div class="rr-empty">
      <Icon name="runs" size={20} />
      <p>{t('runs.trg.empty')}</p>
      <button class="btn primary" onclick={() => (adding = true)}>{t('runs.trg.new')}</button>
    </div>
  {:else}
    <ul class="list">
      {#each store.triggers as trg (trg.id)}
        <li class="trg">
          <div class="main">
            <span class="top">
              <span class="script rr-mono">{trg.script}</span>
              <code class="spec">{trg.spec}</code>
            </span>
            <span class="desc">{describe(trg.spec)}</span>
            <span class="meta">
              {#if trg.next_fire_ms}{t('runs.trg.next', { when: formatFire(new Date(trg.next_fire_ms), now) })}{:else}{t('runs.trg.never')}{/if}
              {#if trg.last_fired_ms}
                · {t('runs.trg.last', { when: formatFire(new Date(trg.last_fired_ms), now) })}
                <span class="rr-pill {trg.last_status === 'ok' ? 'ok' : 'failed'}">{trg.last_status ?? '?'}</span>
              {/if}
            </span>
            {#if Object.keys(trg.inputs).length}<span class="meta rr-mono">{Object.entries(trg.inputs).map(([k, v]) => `${k}=${v}`).join('  ')}</span>{/if}
          </div>
          <button class="icon-btn" aria-label={t('runs.trg.remove', { id: trg.id })} title={t('runs.trg.removeShort')} onclick={() => remove(trg.id)}><Icon name="trash" size={14} /></button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .add {
    background: var(--surface);
  }
  .preview {
    padding: 8px 10px;
    border-radius: var(--r);
    background: var(--sunken);
    font-size: var(--fs-xs);
  }
  .preview p {
    margin: 0;
    line-height: 1.45;
  }
  .human {
    color: var(--fg);
    font-weight: 600;
  }
  .warn {
    margin-top: 4px !important;
    color: var(--warn);
  }
  .bad {
    color: var(--danger);
  }
  .fires {
    margin: 6px 0 0;
    padding-left: 18px;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
    line-height: 1.6;
  }
  .list {
    margin: 0;
    padding: 4px 0;
    list-style: none;
  }
  .trg {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 8px 6px 8px 10px;
    border-bottom: 1px solid color-mix(in oklab, var(--border) 60%, transparent);
  }
  .main {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .top {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .script {
    font-weight: 600;
  }
  .spec {
    padding: 0 5px;
    border-radius: 4px;
    background: var(--sunken);
    color: var(--fg-muted);
    font: 11px var(--font-mono);
  }
  .desc {
    color: var(--fg);
    font-size: var(--fs-xs);
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    overflow-wrap: anywhere;
  }
</style>
