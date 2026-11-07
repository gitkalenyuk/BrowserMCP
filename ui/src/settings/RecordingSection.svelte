<script lang="ts">
  // Settings → Recording & scripts: video defaults and ffmpeg (record.settings), defaults of
  // script runs and mass runs and the script host (script.settings), the data folders.
  import type { BrowserApi } from '../lib/api';
  import type { RecordSettingsOut, ScriptSettingsOut } from '../lib/api/types-e2-settings';
  import Icon from '../lib/components/Icon.svelte';
  import { t } from '../lib/i18n/index.svelte';
  import {
    recordErrors,
    recordForm,
    recordPatch,
    sameSettings,
    scriptErrors,
    scriptForm,
    scriptPatch,
    type RecordForm,
    type ScriptForm,
  } from '../lib/settings/e2-recording';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let rec = $state<RecordSettingsOut | null>(null);
  let scr = $state<ScriptSettingsOut | null>(null);
  let rf = $state<RecordForm | null>(null);
  let sf = $state<ScriptForm | null>(null);
  let recError = $state<string | null>(null);
  let scrError = $state<string | null>(null);
  let busy = $state(false);
  let copied = $state(false);

  async function loadRecord() {
    try {
      rec = await api.call('record.settings', {});
      rf = recordForm(rec.settings);
      recError = null;
    } catch (err) {
      recError = (err as Error).message;
    }
  }

  async function loadScripts() {
    try {
      scr = await api.call('script.settings', {});
      sf = scriptForm(scr.settings);
      scrError = null;
    } catch (err) {
      scrError = (err as Error).message;
    }
  }

  $effect(() => {
    void loadRecord();
    void loadScripts();
  });

  const recErrs = $derived(rf ? recordErrors(rf) : {});
  const recDirty = $derived(!!rec && !!rf && Object.keys(recordPatch(rec.settings, rf)).length > 0);
  const scrErrs = $derived(sf && scr ? scriptErrors(sf, scr.max_parallel) : {});
  const scrDirty = $derived(!!scr && !!sf && Object.keys(scriptPatch(scr.settings, sf)).length > 0);

  async function saveRecord(reset = false) {
    if (!rec || !rf) return;
    busy = true;
    try {
      rec = await api.call('record.settings', reset ? { reset: true } : recordPatch(rec.settings, rf));
      rf = recordForm(rec.settings);
      notify(reset ? t('settings.recording.video.resetDone') : t('settings.recording.video.saved'));
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function saveScripts(reset = false) {
    if (!scr || !sf) return;
    busy = true;
    try {
      scr = await api.call('script.settings', reset ? { reset: true } : scriptPatch(scr.settings, sf));
      sf = scriptForm(scr.settings);
      notify(reset ? t('settings.recording.scripts.resetDone') : t('settings.recording.scripts.saved'));
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function openFolder(path: string) {
    try {
      await api.call('system.open_path', { path });
    } catch (err) {
      fail(err);
    }
  }

  async function copyInstall(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      notify(t('settings.recording.copyFailed'), true);
    }
  }

  const sourceLabel = (s: string | null | undefined) => (s ? t(`settings.recording.ffmpeg.source.${s}`) : '');
  const folders = $derived(
    [
      rec && { id: 'recordings', path: rec.recordings_dir },
      rec && { id: 'videos', path: rec.videos_dir },
      scr && { id: 'scripts', path: scr.scripts_dir },
      scr && { id: 'runs', path: scr.runs_dir },
    ].filter((f): f is { id: string; path: string } => !!f),
  );
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.recording.title')}</h2>
    <p>{t('settings.recording.intro')}</p>
  </header>

  <section class="set-card" aria-labelledby="r-video">
    <h3 id="r-video">{t('settings.recording.video.title')}</h3>
    {#if recError && !rec}
      <p class="lead lib-error" role="alert">{t('settings.recording.loadError', { error: recError })}</p>
    {:else if !rec || !rf}
      <p class="lead">{t('common.loading')}</p>
    {:else}
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.recording.ffmpeg.label')}</span>
          {#if rec.ffmpeg.found}
            <span class="help">{t('settings.recording.ffmpeg.foundHelp', { source: sourceLabel(rec.ffmpeg.source) })}</span>
            <span class="help set-mono" title={rec.ffmpeg.path ?? ''}>{rec.ffmpeg.path}</span>
            {#if rec.ffmpeg.version}<span class="help">{rec.ffmpeg.version}</span>{/if}
          {:else}
            <span class="help">{t('settings.recording.ffmpeg.missingHelp')}</span>
          {/if}
        </div>
        <div class="ctl">
          <span class="lib-chip" class:ok={rec.ffmpeg.found} class:bad={!rec.ffmpeg.found}>
            {rec.ffmpeg.found ? t('settings.recording.ffmpeg.found') : t('settings.recording.ffmpeg.missing')}
          </span>
          <button class="btn" disabled={busy} onclick={loadRecord}><Icon name="reload" size={13} /> {t('settings.recording.ffmpeg.recheck')}</button>
        </div>
      </div>
      {#if !rec.ffmpeg.found}
        <div class="set-row stack">
          <div class="text"><span class="label">{t('settings.recording.ffmpeg.howTitle')}</span></div>
          <ol class="how">
            <li>
              {t('settings.recording.ffmpeg.how1')}
              <span class="cmd"><code class="set-mono">{rec.ffmpeg.install}</code>
                <button class="btn mini" onclick={() => copyInstall(rec!.ffmpeg.install)}><Icon name={copied ? 'check' : 'copy'} size={12} /> {copied ? t('common.copied') : t('common.copy')}</button></span>
            </li>
            <li>{t('settings.recording.ffmpeg.how2', { dir: rec.ffmpeg.exe_dir ?? 'browsermcp.exe' })}</li>
            <li>{t('settings.recording.ffmpeg.how3')}</li>
            <li>{t('settings.recording.ffmpeg.how4')}</li>
          </ol>
          {#if rec.ffmpeg.env}<p class="set-err">{t('settings.recording.ffmpeg.envBad', { path: rec.ffmpeg.env })}</p>{/if}
        </div>
      {/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.recording.video.fps')}</span><span class="help">{t('settings.recording.video.fpsHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" inputmode="numeric" bind:value={rf.fps} aria-label={t('settings.recording.video.fps')} aria-invalid={!!recErrs.fps} />
        </div>
      </div>
      {#if recErrs.fps}<p class="set-err pad">{t(recErrs.fps)}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.recording.video.format')}</span><span class="help">{t('settings.recording.video.formatHelp')}</span></div>
        <div class="ctl">
          <div class="set-seg" role="radiogroup" aria-label={t('settings.recording.video.format')}>
            <button role="radio" aria-checked={rf.format === 'webm'} onclick={() => (rf!.format = 'webm')}>WebM (VP9)</button>
            <button role="radio" aria-checked={rf.format === 'mp4'} onclick={() => (rf!.format = 'mp4')}>MP4 (H.264)</button>
          </div>
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.recording.video.quality')}</span><span class="help">{t('settings.recording.video.qualityHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" inputmode="numeric" bind:value={rf.quality} aria-label={t('settings.recording.video.quality')} aria-invalid={!!recErrs.quality} />
        </div>
      </div>
      {#if recErrs.quality}<p class="set-err pad">{t(recErrs.quality)}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.recording.video.size')}</span><span class="help">{t('settings.recording.video.sizeHelp')}</span></div>
      </div>
      <div class="set-actions">
        {#if recDirty}
          <button class="btn primary" disabled={busy || Object.keys(recErrs).length > 0} onclick={() => saveRecord()}>{t('common.save')}</button>
          <button class="btn" disabled={busy} onclick={() => (rf = recordForm(rec!.settings))}>{t('settings.recording.discard')}</button>
        {/if}
        {#if !sameSettings(rec.settings, rec.defaults)}
          <button class="btn" disabled={busy} onclick={() => saveRecord(true)}>{t('settings.recording.reset')}</button>
        {/if}
      </div>
    {/if}
  </section>

  <section class="set-card" aria-labelledby="r-scripts">
    <h3 id="r-scripts">{t('settings.recording.scripts.title')}</h3>
    {#if scrError && !scr}
      <p class="lead lib-error" role="alert">{t('settings.recording.loadError', { error: scrError })}</p>
    {:else if !scr || !sf}
      <p class="lead">{t('common.loading')}</p>
    {:else}
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.recording.host.label')}</span>
          {#if scr.host.found}
            <span class="help">{t('settings.recording.host.foundHelp')}</span>
            <span class="help set-mono">{scr.host.path}</span>
          {:else}
            <span class="help">{t('settings.recording.host.missingHelp')}</span>
            <span class="help set-mono">{scr.host.error}</span>
          {/if}
        </div>
        <div class="ctl">
          <span class="lib-chip" class:ok={scr.host.found} class:bad={!scr.host.found}>
            {scr.host.found ? t('settings.recording.host.found') : t('settings.recording.host.missing')}
          </span>
          <button class="btn" disabled={busy} onclick={loadScripts}><Icon name="reload" size={13} /> {t('settings.recording.ffmpeg.recheck')}</button>
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.recording.scripts.timeout')}</span><span class="help">{t('settings.recording.scripts.timeoutHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" inputmode="numeric" bind:value={sf.scriptTimeout} aria-label={t('settings.recording.scripts.timeout')} aria-invalid={!!scrErrs.scriptTimeout} />
          {t('settings.recording.sec')}
        </div>
      </div>
      {#if scrErrs.scriptTimeout}<p class="set-err pad">{t(scrErrs.scriptTimeout)}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.recording.scripts.rowTimeout')}</span><span class="help">{t('settings.recording.scripts.rowTimeoutHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" inputmode="numeric" bind:value={sf.runTimeout} aria-label={t('settings.recording.scripts.rowTimeout')} aria-invalid={!!scrErrs.runTimeout} />
          {t('settings.recording.sec')}
        </div>
      </div>
      {#if scrErrs.runTimeout}<p class="set-err pad">{t(scrErrs.runTimeout)}</p>{/if}
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.recording.scripts.parallel')}</span>
          <span class="help">{t('settings.recording.scripts.parallelHelp', { recommended: scr.recommended_parallel, max: scr.max_parallel })}</span>
        </div>
        <div class="ctl">
          <input class="lib-input narrow" inputmode="numeric" bind:value={sf.parallel} aria-label={t('settings.recording.scripts.parallel')} aria-invalid={!!scrErrs.parallel} />
        </div>
      </div>
      {#if scrErrs.parallel}<p class="set-err pad">{t(scrErrs.parallel, { max: scr.max_parallel })}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.recording.scripts.retries')}</span><span class="help">{t('settings.recording.scripts.retriesHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" inputmode="numeric" bind:value={sf.retries} aria-label={t('settings.recording.scripts.retries')} aria-invalid={!!scrErrs.retries} />
        </div>
      </div>
      {#if scrErrs.retries}<p class="set-err pad">{t(scrErrs.retries)}</p>{/if}
      <div class="set-actions">
        {#if scrDirty}
          <button class="btn primary" disabled={busy || Object.keys(scrErrs).length > 0} onclick={() => saveScripts()}>{t('common.save')}</button>
          <button class="btn" disabled={busy} onclick={() => (sf = scriptForm(scr!.settings))}>{t('settings.recording.discard')}</button>
        {/if}
        {#if !sameSettings(scr.settings, scr.defaults)}
          <button class="btn" disabled={busy} onclick={() => saveScripts(true)}>{t('settings.recording.reset')}</button>
        {/if}
      </div>
    {/if}
  </section>

  {#if folders.length}
    <section class="set-card" aria-labelledby="r-folders">
      <h3 id="r-folders">{t('settings.recording.folders.title')}</h3>
      {#each folders as f (f.id)}
        <div class="set-row">
          <div class="text">
            <span class="label">{t(`settings.recording.folders.${f.id}`)}</span>
            <span class="help">{t(`settings.recording.folders.${f.id}Help`)}</span>
            <span class="help set-mono" title={f.path}>{f.path}</span>
          </div>
          <div class="ctl">
            <button class="btn" onclick={() => openFolder(f.path)}><Icon name="folder" size={13} /> {t('settings.recording.folders.open')}</button>
          </div>
        </div>
      {/each}
    </section>
  {/if}
</div>

<style>
  .pad {
    padding: 0 16px 10px;
  }
  .ctl .btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .mini {
    height: 26px;
    padding: 0 8px;
    font-size: var(--fs-xs);
  }
  .how {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding-left: 20px;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    line-height: 1.5;
  }
  .cmd {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: 4px;
  }
  .cmd code {
    padding: 2px 6px;
    border-radius: var(--r);
    background: var(--sunken);
    user-select: all;
  }
</style>
