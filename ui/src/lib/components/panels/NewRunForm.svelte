<script lang="ts">
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
        const kind = d.csv_path ? 'CSV' : d.json_path ? 'JSON' : 'Excel (first sheet)';
        return { ok: true, data: d, summary: `${kind}, read by the core` };
      }
      if (!pasted.trim()) return null;
      const rows = parseRows(pasted);
      const cols = [...new Set(rows.flatMap((r) => Object.keys(r)))];
      return { ok: true, data: { rows }, summary: `${rows.length} rows · columns ${cols.join(', ')}` };
    } catch (err) {
      return { ok: false, error: (err as Error).message };
    }
  });

  async function start(e: SubmitEvent) {
    e.preventDefault();
    if (!data || !data.ok) {
      error = data && !data.ok ? data.error : source === 'file' ? 'Enter a data file path' : 'Paste some rows';
      return;
    }
    if (!script) {
      error = 'Pick a script (save one from the Recording panel first)';
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
    Script
    <select bind:value={script} disabled={store.scripts.length === 0}>
      {#if store.scripts.length === 0}<option value="">No saved scripts</option>{/if}
      {#each store.scripts as s (s.name)}<option value={s.name}>{s.name}</option>{/each}
    </select>
  </label>

  <div class="rr-field">
    Data
    <div class="rr-chips" role="group" aria-label="Data source">
      <button type="button" class="rr-chip" aria-pressed={source === 'file'} onclick={() => (source = 'file')}>File</button>
      <button type="button" class="rr-chip" aria-pressed={source === 'paste'} onclick={() => (source = 'paste')}>Paste rows</button>
    </div>
  </div>
  {#if source === 'file'}
    <label class="rr-field">
      <span class="sr-only">Data file path</span>
      <input class="mono" bind:value={path} placeholder="C:\data\leads.csv (.csv, .json, .xlsx)" spellcheck="false" aria-invalid={data?.ok === false} />
    </label>
  {:else}
    <label class="rr-field">
      <span class="sr-only">Rows</span>
      <textarea bind:value={pasted} rows="5" spellcheck="false" placeholder={'email,name\nann@example.test,Ann\n\nor [{"email": "ann@example.test"}]'}></textarea>
    </label>
  {/if}
  {#if data}
    <p class="rr-hint" class:bad={!data.ok}>{data.ok ? data.summary : data.error}</p>
  {:else}
    <p class="rr-hint">Each column becomes <span class="rr-mono">inputs.&lt;column&gt;</span> for one run of the script.</p>
  {/if}

  <div class="rr-row nums">
    <label class="rr-field">Parallel <input type="number" min="1" max="32" bind:value={parallel} /></label>
    <label class="rr-field">Retries <input type="number" min="0" max="10" bind:value={retries} /></label>
    <label class="rr-field">Timeout, s <input type="number" min="1" max="86400" bind:value={timeout} /></label>
  </div>
  <div class="rr-row">
    <label class="rr-field heal-mode" title="What happens when a row breaks on a selector error (element not found, not clickable, …)">
      On selector errors
      <select bind:value={heal}>
        <option value="ask">Pause, AI proposes a patch, I accept it</option>
        <option value="auto">AI patches by itself when confident</option>
        <option value="off">Just fail the row</option>
      </select>
    </label>
    <label class="rr-check"><input type="checkbox" bind:checked={stopOnError} /> Stop at the first failure</label>
  </div>
  {#if error}<p class="rr-hint bad" role="alert">{error}</p>{/if}
  <div class="rr-row">
    <button class="btn primary" type="submit" disabled={busy}><Icon name="play" size={12} /> {busy ? 'Starting…' : 'Start run'}</button>
    <span class="rr-hint">Parallel is capped by the core (CPUs / 2, free RAM / 400 MB).</span>
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
