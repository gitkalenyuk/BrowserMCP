<script lang="ts">
  import { t, tp } from '../../i18n/index.svelte';
  import type { HealMode } from '../../api/types-settings';
  import { dataFromPath, parseRows, upsertRun, type RunData } from '../../runs/logic';
  import { runsStore } from '../../runs/store.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  let { onstarted }: { onstarted: (id: string) => void } = $props();

  const store = runsStore(useChrome().api);

  let script = $state('');
  let source = $state<'file' | 'paste'>('file');
  let path = $state('');
  let pasted = $state('');
  let parallel = $state(4);
  let retries = $state(1);
  let timeout = $state(300);
  let heal = $state<HealMode>('ask');
  let stopOnError = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    void store.refreshScripts();
  });
  $effect(() => {
    if (!script && store.scripts.length) script = store.scripts[0].name;
  });

  /** What the data section would send, or why it cannot. */
  const data = $derived.by((): { ok: true; data: RunData; summary: string } | { ok: false; error: string } | null => {
    try {
      if (source === 'file') {
        if (!path.trim()) return null;
        const d = dataFromPath(path);
        const kind = d.csv_path ? 'CSV' : d.json_path ? 'JSON' : t('runs.form.excel');
        return { ok: true, data: d, summary: t('runs.form.readByCore', { kind }) };
      }
      if (!pasted.trim()) return null;
      const rows = parseRows(pasted);
      const cols = [...new Set(rows.flatMap((r) => Object.keys(r)))];
      return { ok: true, data: { rows }, summary: `${tp('runs.form.rows', rows.length)} · ${t('runs.form.columns', { cols: cols.join(', ') })}` };
    } catch (err) {
      return { ok: false, error: (err as Error).message };
    }
  });

  async function start(e: SubmitEvent) {
    e.preventDefault();
    if (!data || !data.ok) {
      error = data && !data.ok ? data.error : source === 'file' ? t('runs.form.needFile') : t('runs.form.needRows');
      return;
    }
    if (!script) {
      error = t('runs.form.needScript');
      return;
    }
    busy = true;
    error = null;
    try {
      const run = await store.api.call('run.start', {
        script,
        data: data.data,
        parallel,
        retries,
        timeout_secs: timeout,
        heal,
        stop_on_error: stopOnError,
      });
      store.runs = upsertRun(store.runs, run);
      onstarted(run.id);
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }
</script>

<form class="rr-section rr-form new" onsubmit={start}>
  <label class="rr-field">
    {t('record.detail.script')}
    <select bind:value={script} disabled={store.scripts.length === 0}>
      {#if store.scripts.length === 0}<option value="">{t('runs.form.noScripts')}</option>{/if}
      {#each store.scripts as s (s.name)}<option value={s.name}>{s.name}</option>{/each}
    </select>
  </label>

  <div class="rr-field">
    {t('runs.form.data')}
    <div class="rr-chips" role="group" aria-label={t('runs.form.source')}>
      <button type="button" class="rr-chip" aria-pressed={source === 'file'} onclick={() => (source = 'file')}>{t('runs.form.file')}</button>
      <button type="button" class="rr-chip" aria-pressed={source === 'paste'} onclick={() => (source = 'paste')}>{t('runs.form.paste')}</button>
    </div>
  </div>
  {#if source === 'file'}
    <label class="rr-field">
      <span class="sr-only">{t('runs.form.filePath')}</span>
      <input class="mono" bind:value={path} placeholder="C:\data\leads.csv (.csv, .json, .xlsx)" spellcheck="false" aria-invalid={data?.ok === false} />
    </label>
  {:else}
    <label class="rr-field">
      <span class="sr-only">{t('runs.form.rowsLabel')}</span>
      <textarea bind:value={pasted} rows="5" spellcheck="false" placeholder={t('runs.form.pastePlaceholder')}></textarea>
    </label>
  {/if}
  {#if data}
    <p class="rr-hint" class:bad={!data.ok}>{data.ok ? data.summary : data.error}</p>
  {:else}
    <p class="rr-hint">{t('runs.form.columnsHintBefore')} <span class="rr-mono">inputs.&lt;column&gt;</span> {t('runs.form.columnsHintAfter')}</p>
  {/if}

  <div class="rr-row nums">
    <label class="rr-field">{t('runs.form.parallel')} <input type="number" min="1" max="32" bind:value={parallel} /></label>
    <label class="rr-field">{t('runs.form.retries')} <input type="number" min="0" max="10" bind:value={retries} /></label>
    <label class="rr-field">{t('runs.form.timeout')} <input type="number" min="1" max="86400" bind:value={timeout} /></label>
  </div>
  <div class="rr-row">
    <label class="rr-field heal-mode" title={t('runs.form.healHint')}>
      {t('runs.form.heal')}
      <select bind:value={heal}>
        <option value="ask">{t('runs.form.heal.ask')}</option>
        <option value="auto">{t('runs.form.heal.auto')}</option>
        <option value="off">{t('runs.form.heal.off')}</option>
      </select>
    </label>
    <label class="rr-check"><input type="checkbox" bind:checked={stopOnError} /> {t('runs.form.stopOnError')}</label>
  </div>
  {#if error}<p class="rr-hint bad" role="alert">{error}</p>{/if}
  <div class="rr-row">
    <button class="btn primary" type="submit" disabled={busy}><Icon name="play" size={12} /> {busy ? t('runs.form.starting') : t('runs.form.start')}</button>
    <span class="rr-hint">{t('runs.form.cap')}</span>
  </div>
</form>

<style>
  .new {
    background: var(--surface);
  }
  .nums .rr-field {
    flex: 1;
    min-width: 70px;
  }
  .nums input {
    width: 100%;
  }
  .bad {
    color: var(--danger);
  }
</style>
