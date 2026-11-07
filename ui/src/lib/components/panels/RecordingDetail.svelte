<script lang="ts">
  import { t } from '../../i18n/index.svelte';
  import { formatDateTime } from '../../i18n/intl';
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
      error = t('record.detail.needName');
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
  <button class="icon-btn" aria-label={t('record.detail.back')} title={t('common.back')} onclick={onclose}><Icon name="arrow-left" size={15} /></button>
  <h3 title={id}>{meta?.name ?? id}</h3>
  {#if detail?.active}<span class="rr-pill rec">{t('record.detail.recording')}</span>{/if}
</div>

{#if error}
  <div class="rr-note err" role="alert">
    <span>{error}</span>
    <button class="icon-btn" aria-label={t('chrome.dismiss')} onclick={() => (error = null)}><Icon name="x" size={12} /></button>
  </div>
{/if}

<div class="rr-scroll">
  {#if meta}
    <div class="rr-section facts">
      <dl>
        <dt>{t('record.detail.started')}</dt>
        <dd>{formatDateTime(meta.started_ms)}</dd>
        <dt>{t('record.detail.steps')}</dt>
        <dd>{duration !== null ? t('record.detail.stepsIn', { n: meta.steps, time: formatDuration(duration) }) : meta.steps}</dd>
        <dt>{t('record.detail.page')}</dt>
        <dd class="rr-mono url" title={meta.start_url}>{meta.start_url}</dd>
        <dt>{t('record.detail.capture')}</dt>
        <dd>{meta.human_capture}</dd>
      </dl>
      {#if meta.video}
        <div class="file">
          <span class="rr-h">{t('record.video')}</span>
          <code class="rr-path">{meta.video}</code>
          <div class="rr-row">
            <button class="btn sm" onclick={() => open(meta.video!, false)} title={t('record.detail.playHint')}><Icon name="play" size={12} /> {t('record.detail.openVideo')}</button>
            <button class="btn sm" onclick={() => open(meta.video!, true)} title={t('record.detail.folderHint')}>{t('record.detail.openFolder')}</button>
            <button class="btn sm" onclick={() => copy(meta.video!)}>{t('record.detail.copyPath')}</button>
          </div>
        </div>
      {/if}
      {#if meta.traffic}
        <div class="file">
          <span class="rr-h">{t('record.detail.trafficLog')}</span>
          <code class="rr-path">{meta.traffic}</code>
        </div>
      {/if}
    </div>

    <div class="rr-section">
      <button class="rr-h toggle" aria-expanded={showSteps} onclick={() => (showSteps = !showSteps)}>
        <Icon name={showSteps ? 'chevron-down' : 'chevron-right'} size={12} /> {t('record.detail.steps')}
      </button>
      {#if showSteps}
        <div class="steps-box"><StepList {steps} /></div>
      {/if}
    </div>

    <div class="rr-section">
      <h4>{t('record.detail.script')}</h4>
      <div class="rr-form">
        <div class="rr-row">
          <label class="rr-field grow">
            {t('record.detail.name')}
            <input class="mono" bind:value={name} spellcheck="false" placeholder="login" />
          </label>
        </div>
        <label class="rr-check" title={t('record.detail.parametrizeHint')}>
          <input type="checkbox" bind:checked={parametrize} /> {t('record.detail.parametrize')}
        </label>
        <div class="rr-row">
          <button class="btn primary" disabled={busy !== null || detail?.active} onclick={generate}>
            <Icon name="code" size={13} />
            {busy === 'codegen' ? t('record.detail.generating') : gen ? t('record.detail.generateAgain') : t('record.detail.generate')}
          </button>
          {#if detail?.active}<span class="rr-hint">{t('record.detail.stopFirst')}</span>{/if}
        </div>
      </div>
    </div>

    {#if gen}
      <div class="rr-section">
        <p class="rr-hint saved">{t('record.detail.savedTo')} <span class="rr-mono">{gen.path}</span></p>
        <CodeView code={gen.code} label={t('record.detail.generated')} />
      </div>

      <div class="rr-section">
        <h4>{t('record.detail.runOnce')}</h4>
        <div class="rr-form">
          {#if inputs.length === 0}
            <p class="rr-hint">{t('record.detail.noInputs')}</p>
          {/if}
          {#each inputs as inp (inp.name)}
            <label class="rr-field">
              <span>
                {inp.label}
                {#if inp.label !== inp.name}<span class="opt rr-mono">inputs.{inp.name}</span>{/if}
                {#if inp.required}<span class="opt">· {t('record.detail.required')}</span>{/if}
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
            <p class="rr-hint err">{t('record.detail.fillIn', { names: missing.join(', ') })}</p>
          {/if}
          <div class="rr-row">
            <button class="btn primary" disabled={busy !== null} onclick={saveAndRun}>
              <Icon name="play" size={12} />
              {busy === 'run' ? t('record.detail.running') : t('record.detail.saveRun')}
            </button>
            {#if savedAs}<span class="rr-hint">{t('record.detail.as')} <span class="rr-mono">{savedAs}</span></span>{/if}
          </div>
        </div>
        {#if result}
          <div class="result" class:bad={!result.ok}>
            <div class="rr-row">
              <span class="rr-pill {result.ok ? 'ok' : 'failed'}">{result.ok ? 'ok' : result.exit_code !== null && result.exit_code !== undefined ? t('record.detail.exit', { code: result.exit_code }) : t('record.detail.killed')}</span>
              <span class="rr-hint">{formatDuration(result.duration_ms)}</span>
            </div>
            {#if result.error}
              <p class="err-line">
                <b>{result.error.code}</b>
                {#if result.error.command}<span class="rr-mono">{result.error.command}</span>{/if}
                {result.error.message}
                {#if result.error.line}<span class="rr-hint">({t('scripts.console.line', { line: result.error.line })})</span>{/if}
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
    <div class="rr-empty">{t('common.loading')}</div>
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
