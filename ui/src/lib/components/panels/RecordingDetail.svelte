<script lang="ts">
  import type { CodegenOutput, RecordStep, RecordingDetail, ScriptRunOutput } from '../../api/types-record-runs';
  import { buildInputs, fileUrl, inferInputs, parseStepsJsonl, scriptName, type ScriptInput } from '../../record/logic';
  import { formatDuration } from '../../runs/logic';
  import { runsStore } from '../../runs/store.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import CodeView from './CodeView.svelte';
  import StepList from './StepList.svelte';

  let { id, onclose }: { id: string; onclose: () => void } = $props();

  const chrome = useChrome();
  const api = chrome.api;

  let detail = $state<RecordingDetail | null>(null);
  let steps = $state.raw<RecordStep[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<'codegen' | 'run' | null>(null);
  let showSteps = $state(true);

  let name = $state('');
  let parametrize = $state(true);
  let gen = $state<CodegenOutput | null>(null);
  let inputs = $state.raw<ScriptInput[]>([]);
  let values = $state<Record<string, string>>({});
  let result = $state<ScriptRunOutput | null>(null);
  let savedAs = $state<string | null>(null);
  let missing = $state<string[]>([]);

  $effect(() => {
    void load(id);
  });

  async function load(rid: string) {
    try {
      const d = await api.call('record.get', { id: rid, format: 'jsonl' });
      detail = d;
      steps = parseStepsJsonl(d.text);
      if (!name) name = scriptName(d.meta.name) || 'recording';
    } catch (err) {
      error = (err as Error).message;
    }
  }

  async function generate() {
    if (!detail) return;
    busy = 'codegen';
    error = null;
    result = null;
    savedAs = null;
    try {
      const n = scriptName(name) || undefined;
      gen = await api.call('record.codegen', { id: detail.id, name: n, parametrize });
      inputs = inferInputs(gen.code, gen.inputs);
      values = Object.fromEntries(inputs.map((i) => [i.name, i.default ?? '']));
      missing = [];
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = null;
    }
  }

  async function saveAndRun() {
    if (!gen) return;
    const n = scriptName(name);
    if (!n) {
      error = 'Give the script a name (A-Z a-z 0-9 _ -)';
      return;
    }
    const built = buildInputs(inputs, values);
    missing = built.missing;
    if (missing.length) return;
    busy = 'run';
    error = null;
    result = null;
    try {
      await api.call('script.save', { name: n, code: gen.code });
      savedAs = n;
      void runsStore(api).refreshScripts();
      result = await api.call('script.run', { name: n, inputs: built.inputs });
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = null;
    }
  }

  async function open(path: string, folder: boolean) {
    error = null;
    try {
      // The folder opens in Explorer with the file selected; the file itself plays in a tab.
      if (folder) await api.call('system.reveal_path', { path });
      else await api.call('tabs.open', { url: fileUrl(path, false) });
    } catch (err) {
      error = (err as Error).message;
    }
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // the path is selectable
    }
  }

  const meta = $derived(detail?.meta);
  const duration = $derived(meta?.stopped_ms ? meta.stopped_ms - meta.started_ms : null);
</script>

<div class="rr-bar">
  <button class="icon-btn" aria-label="Back to recordings" title="Back" onclick={onclose}><Icon name="arrow-left" size={15} /></button>
  <h3 title={id}>{meta?.name ?? id}</h3>
  {#if detail?.active}<span class="rr-pill rec">recording</span>{/if}
</div>

{#if error}
  <div class="rr-note err" role="alert">
    <span>{error}</span>
    <button class="icon-btn" aria-label="Dismiss" onclick={() => (error = null)}><Icon name="x" size={12} /></button>
  </div>
{/if}

<div class="rr-scroll">
  {#if meta}
    <div class="rr-section facts">
      <dl>
        <dt>Started</dt>
        <dd>{new Date(meta.started_ms).toLocaleString()}</dd>
        <dt>Steps</dt>
        <dd>{meta.steps}{duration !== null ? ` in ${formatDuration(duration)}` : ''}</dd>
        <dt>Page</dt>
        <dd class="rr-mono url" title={meta.start_url}>{meta.start_url}</dd>
        <dt>Capture</dt>
        <dd>{meta.human_capture}</dd>
      </dl>
      {#if meta.video}
        <div class="file">
          <span class="rr-h">Video</span>
          <code class="rr-path">{meta.video}</code>
          <div class="rr-row">
            <button class="btn sm" onclick={() => open(meta.video!, false)} title="Play it in a new tab"><Icon name="play" size={12} /> Open video</button>
            <button class="btn sm" onclick={() => open(meta.video!, true)} title="Show the video in Explorer">Open folder</button>
            <button class="btn sm" onclick={() => copy(meta.video!)}>Copy path</button>
          </div>
        </div>
      {/if}
      {#if meta.traffic}
        <div class="file">
          <span class="rr-h">Traffic log</span>
          <code class="rr-path">{meta.traffic}</code>
        </div>
      {/if}
    </div>

    <div class="rr-section">
      <button class="rr-h toggle" aria-expanded={showSteps} onclick={() => (showSteps = !showSteps)}>
        <Icon name={showSteps ? 'chevron-down' : 'chevron-right'} size={12} /> Steps
      </button>
      {#if showSteps}
        <div class="steps-box"><StepList {steps} /></div>
      {/if}
    </div>

    <div class="rr-section">
      <h4>Script</h4>
      <div class="rr-form">
        <div class="rr-row">
          <label class="rr-field grow">
            Name
            <input class="mono" bind:value={name} spellcheck="false" placeholder="login" />
          </label>
        </div>
        <label class="rr-check" title="Typed and selected values become inputs.<name> with the recorded value as default. Passwords are always inputs.">
          <input type="checkbox" bind:checked={parametrize} /> Turn typed values into inputs
        </label>
        <div class="rr-row">
          <button class="btn primary" disabled={busy !== null || detail?.active} onclick={generate}>
            <Icon name="code" size={13} />
            {busy === 'codegen' ? 'Generating…' : gen ? 'Generate again' : 'Generate script'}
          </button>
          {#if detail?.active}<span class="rr-hint">Stop the recording first.</span>{/if}
        </div>
      </div>
    </div>

    {#if gen}
      <div class="rr-section">
        <p class="rr-hint saved">Saved to <span class="rr-mono">{gen.path}</span></p>
        <CodeView code={gen.code} label="Generated script" />
      </div>

      <div class="rr-section">
        <h4>Run it once</h4>
        <div class="rr-form">
          {#if inputs.length === 0}
            <p class="rr-hint">The script reads no inputs.</p>
          {/if}
          {#each inputs as inp (inp.name)}
            <label class="rr-field">
              <span>
                {inp.label}
                {#if inp.label !== inp.name}<span class="opt rr-mono">inputs.{inp.name}</span>{/if}
                {#if inp.required}<span class="opt">· required</span>{/if}
              </span>
              <input
                type={inp.secret ? 'password' : 'text'}
                bind:value={values[inp.name]}
                placeholder={inp.default ?? ''}
                autocomplete="off"
                spellcheck="false"
                aria-invalid={missing.includes(inp.name)}
              />
            </label>
          {/each}
          {#if missing.length}
            <p class="rr-hint err">Fill in: {missing.join(', ')}</p>
          {/if}
          <div class="rr-row">
            <button class="btn primary" disabled={busy !== null} onclick={saveAndRun}>
              <Icon name="play" size={12} />
              {busy === 'run' ? 'Running…' : 'Save & run'}
            </button>
            {#if savedAs}<span class="rr-hint">as <span class="rr-mono">{savedAs}</span></span>{/if}
          </div>
        </div>
        {#if result}
          <div class="result" class:bad={!result.ok}>
            <div class="rr-row">
              <span class="rr-pill {result.ok ? 'ok' : 'failed'}">{result.ok ? 'ok' : `exit ${result.exit_code ?? 'killed'}`}</span>
              <span class="rr-hint">{formatDuration(result.duration_ms)}</span>
            </div>
            {#if result.error}
              <p class="err-line">
                <b>{result.error.code}</b>
                {#if result.error.command}<span class="rr-mono">{result.error.command}</span>{/if}
                {result.error.message}
                {#if result.error.line}<span class="rr-hint">(line {result.error.line})</span>{/if}
              </p>
            {/if}
            {#if result.result !== null && result.result !== undefined}
              <pre class="out">{JSON.stringify(result.result, null, 2)}</pre>
            {/if}
            {#if result.stdout}<pre class="out muted">{result.stdout}</pre>{/if}
            {#if !result.ok && result.stderr}<pre class="out muted">{result.stderr}</pre>{/if}
          </div>
        {/if}
      </div>
    {/if}
  {:else if !error}
    <div class="rr-empty">Loading…</div>
  {/if}
</div>

<style>
  dl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 3px 10px;
    margin: 0;
    font-size: var(--fs-xs);
  }
  dt {
    color: var(--fg-subtle);
  }
  dd {
    margin: 0;
    color: var(--fg);
    overflow-wrap: anywhere;
  }
  .url {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .file {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin-top: 10px;
  }
  .file .rr-h {
    margin: 0;
  }
  .toggle {
    margin: 0;
    padding: 0;
    border: 0;
    background: transparent;
  }
  .steps-box {
    display: flex;
    max-height: 220px;
    margin-top: 6px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
  }
  .steps-box :global(.steps) {
    flex: 1;
  }
  .saved {
    margin-bottom: 6px;
  }
  .err {
    color: var(--danger);
  }
  .result {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 10px;
    padding: 8px;
    border: 1px solid color-mix(in oklab, var(--ok) 35%, var(--border));
    border-radius: var(--r);
    background: var(--surface);
  }
  .result.bad {
    border-color: color-mix(in oklab, var(--danger) 40%, var(--border));
  }
  .err-line {
    margin: 0;
    color: var(--danger);
    font-size: var(--fs-xs);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  .out {
    max-height: 160px;
    margin: 0;
    padding: 6px 8px;
    overflow: auto;
    border-radius: var(--r-sm);
    background: var(--sunken);
    font: 11px / 1.45 var(--font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .out.muted {
    color: var(--fg-muted);
  }
</style>
