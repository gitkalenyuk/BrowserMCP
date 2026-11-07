<script lang="ts">
  import { VOICE_TRANSCRIBE_TIMEOUT_MS, type BrowserApi } from '../lib/api';
  import type {
    VoiceCloudEngine,
    VoiceEngineId,
    VoiceEngines,
    VoiceLocalStatus,
    VoiceSettings,
    VoiceSettingsPatch,
    VoiceTranscript,
  } from '../lib/api/types-voice';
  import Icon from '../lib/components/Icon.svelte';
  import { locale, t } from '../lib/i18n/index.svelte';
  import { intlLocale } from '../lib/i18n/intl';
  import { bytesLabel } from '../lib/settings/e1-data';
  import { MicCaptureError, recordFor } from '../lib/voice/capture';
  import { hotkeyFromEvent } from '../lib/voice/keys';
  import { VOICE_LANGUAGES, languageName } from '../lib/voice/languages';
  import { bytesToBase64, encodeWav } from '../lib/voice/wav';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let s = $state<VoiceSettings | null>(null);
  let engines = $state<VoiceEngines | null>(null);
  let local = $state<VoiceLocalStatus | null>(null);
  let loadError = $state<string | null>(null);
  /** What the key / model / address fields hold before Save. */
  let draft = $state<{ key: string; model: string; base_url: string }>({ key: '', model: '', base_url: '' });
  let capturing = $state(false);
  let testing = $state<'idle' | 'recording' | 'transcribing'>('idle');
  let testLevel = $state(0);
  let testResult = $state<{ ok: true; r: VoiceTranscript } | { ok: false; error: string } | null>(null);
  let poll: ReturnType<typeof setInterval> | undefined;

  const loc = $derived(intlLocale(locale()));
  const size = (n: number | null | undefined) => (n == null ? '?' : bytesLabel(n, loc));

  async function refresh() {
    try {
      [s, engines, local] = await Promise.all([api.call('voice.settings_get'), api.call('voice.engines'), api.call('voice.local_status')]);
      loadError = null;
      resetDraft();
      watchDownload();
    } catch (err) {
      loadError = (err as Error).message;
    }
  }
  $effect(() => {
    void refresh();
    return () => clearInterval(poll);
  });

  const cloud = $derived(s && s.engine !== 'local' ? (s.engine as VoiceCloudEngine) : null);
  const engineInfo = $derived(engines?.engines.find((e) => e.id === s?.engine) ?? null);
  const downloading = $derived(!!local?.download && ['running', 'verifying', 'installing'].includes(local.download.phase));

  function resetDraft() {
    if (!s || !cloud) return;
    draft = { key: '', model: s[cloud].model ?? '', base_url: s[cloud].base_url ?? '' };
  }

  async function set(patch: VoiceSettingsPatch, message = t('settings.voice.saved')) {
    try {
      s = await api.call('voice.settings_set', patch);
      engines = await api.call('voice.engines');
      notify(message);
    } catch (err) {
      fail(err);
    }
  }

  async function chooseEngine(id: VoiceEngineId) {
    await set({ engine: id });
    resetDraft();
    testResult = null;
  }

  async function saveCloud() {
    if (!s || !cloud) return;
    const patch: { api_key?: string; model?: string; base_url?: string } = { model: draft.model.trim() };
    if (draft.key.trim()) patch.api_key = draft.key.trim();
    if (cloud === 'custom' || draft.base_url.trim() !== (s[cloud].base_url ?? '')) patch.base_url = draft.base_url.trim();
    await set({ [cloud]: patch } as VoiceSettingsPatch);
    resetDraft();
  }

  async function removeKey() {
    if (!cloud) return;
    await set({ [cloud]: { api_key: '' } } as VoiceSettingsPatch, t('settings.voice.keyRemoved'));
  }

  // ---- Local Whisper downloads ----

  function watchDownload() {
    clearInterval(poll);
    if (!downloading) return;
    poll = setInterval(async () => {
      try {
        local = await api.call('voice.local_status');
        if (!downloading) {
          clearInterval(poll);
          engines = await api.call('voice.engines');
          const d = local.download;
          if (d?.phase === 'done') notify(t('settings.voice.local.downloaded', { what: whatName(d.what) }));
          else if (d?.phase === 'failed') notify(t('settings.voice.local.failed', { what: whatName(d.what), error: d.error ?? '' }), true);
        }
      } catch {
        clearInterval(poll);
      }
    }, 500);
  }

  function whatName(what: string): string {
    if (what === 'engine') return t('settings.voice.local.engineName');
    return local?.models.find((m) => m.id === what)?.name ?? what;
  }

  async function download(what: string) {
    try {
      local = await api.call('voice.local_download', { what });
      watchDownload();
    } catch (err) {
      fail(err);
    }
  }
  async function cancelDownload() {
    try {
      local = await api.call('voice.local_cancel');
    } catch (err) {
      fail(err);
    }
  }
  /** Delete asks twice: the first click arms the button (no JS dialogs in our pages). */
  let armed = $state<string | null>(null);
  async function remove(what: string) {
    if (armed !== what) {
      armed = what;
      return;
    }
    armed = null;
    try {
      local = await api.call('voice.local_delete', { what });
      engines = await api.call('voice.engines');
      notify(t('settings.voice.local.deleted', { what: whatName(what) }));
    } catch (err) {
      fail(err);
    }
  }

  // ---- hotkey capture ----

  function captureKey(e: KeyboardEvent) {
    if (e.key === 'Tab') return;
    e.preventDefault();
    if (e.key === 'Escape') {
      (e.currentTarget as HTMLElement).blur();
      return;
    }
    const combo = hotkeyFromEvent(e);
    if (!combo) return;
    capturing = false;
    (e.currentTarget as HTMLElement).blur();
    void set({ hotkey: combo });
  }

  // ---- test ----

  async function test() {
    if (!s) return;
    testResult = null;
    testing = 'recording';
    try {
      const samples = await recordFor(3000, (l) => (testLevel = l));
      testLevel = 0;
      testing = 'transcribing';
      const language = s.language === '' ? locale() : s.language;
      // Transcribing takes longer than the default 15 s call limit (a local engine starts first).
      const r = await api.call('voice.transcribe', { audio_base64: bytesToBase64(encodeWav(samples)), mime: 'audio/wav', language }, { timeout: VOICE_TRANSCRIBE_TIMEOUT_MS });
      testResult = { ok: true, r };
    } catch (err) {
      const error =
        err instanceof MicCaptureError
          ? err.reason === 'denied'
            ? t('voice.err.denied')
            : err.reason === 'no-device'
              ? t('voice.err.noDevice')
              : err.reason === 'unsupported'
                ? t('voice.err.unsupported')
                : err.message
          : (err as Error).message;
      testResult = { ok: false, error };
    } finally {
      testing = 'idle';
      testLevel = 0;
    }
  }

  const ENGINE_IDS: VoiceEngineId[] = ['local', 'openai', 'groq', 'elevenlabs', 'deepgram', 'custom'];
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.voice.title')}</h2>
    <p>{t('settings.voice.intro')}</p>
  </header>

  {#if loadError}
    <p class="lib-error" role="alert">{t('settings.voice.loadError', { error: loadError })}</p>
    <button class="btn" onclick={refresh}>{t('settings.voice.retry')}</button>
  {:else if !s || !engines || !local}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="v-dictation">
      <h3 id="v-dictation">{t('settings.voice.dictation')}</h3>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.voice.enabled')}</span><span class="help">{t('settings.voice.enabledHelp')}</span></div>
        <div class="ctl"><Toggle label={t('settings.voice.enabled')} checked={s.enabled} onchange={(on) => set({ enabled: on })} /></div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.voice.mode')}</span>
          <span class="help">{s.mode === 'hold' ? t('settings.voice.modeHoldHelp') : t('settings.voice.modeTapHelp')}</span>
        </div>
        <div class="ctl">
          <div class="set-seg" role="radiogroup" aria-label={t('settings.voice.mode')}>
            <button role="radio" aria-checked={s.mode === 'hold'} onclick={() => set({ mode: 'hold' })}>{t('settings.voice.modeHold')}</button>
            <button role="radio" aria-checked={s.mode === 'tap'} onclick={() => set({ mode: 'tap' })}>{t('settings.voice.modeTap')}</button>
          </div>
        </div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.voice.hotkey')}</span>
          <span class="help">{s.hotkey === 'Space' ? t('settings.voice.hotkeySpaceHelp') : t('settings.voice.hotkeyComboHelp')}</span>
        </div>
        <div class="ctl">
          <button
            type="button"
            class="lib-input hotkey"
            class:capturing
            aria-label={t('settings.voice.hotkeyChange')}
            onfocus={() => (capturing = true)}
            onblur={() => (capturing = false)}
            onkeydown={captureKey}
          >
            {capturing ? t('settings.voice.hotkeyPress') : s.hotkey === 'Space' ? t('voice.key.space') : s.hotkey}
          </button>
          {#if s.hotkey !== 'Space'}
            <button class="btn" onclick={() => set({ hotkey: 'Space' })}>{t('settings.voice.hotkeyReset')}</button>
          {/if}
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.voice.autoHold')}</span><span class="help">{t('settings.voice.autoHelp')}</span></div>
        <div class="ctl"><Toggle label={t('settings.voice.autoHold')} checked={s.auto_submit_hold} onchange={(on) => set({ auto_submit_hold: on })} /></div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.voice.autoTap')}</span><span class="help">{t('settings.voice.autoHelp')}</span></div>
        <div class="ctl"><Toggle label={t('settings.voice.autoTap')} checked={s.auto_submit_tap} onchange={(on) => set({ auto_submit_tap: on })} /></div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.voice.language')}</span><span class="help">{t('settings.voice.languageHelp')}</span></div>
        <div class="ctl">
          <select class="lib-input" value={s.language} aria-label={t('settings.voice.language')} onchange={(e) => set({ language: e.currentTarget.value })}>
            <option value="">{t('settings.voice.languageUi', { name: languageName(locale(), loc) })}</option>
            <option value="auto">{t('settings.voice.languageAuto')}</option>
            {#each VOICE_LANGUAGES as code (code)}
              <option value={code}>{languageName(code, loc)}</option>
            {/each}
          </select>
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.voice.pageHint')}</span><span class="help">{t('settings.voice.pageHintHelp')}</span></div>
        <div class="ctl"><Toggle label={t('settings.voice.pageHint')} checked={s.page_hint} onchange={(on) => set({ page_hint: on })} /></div>
      </div>
    </section>

    <section class="set-card" aria-labelledby="v-engine">
      <h3 id="v-engine">{t('settings.voice.engine')}</h3>
      <p class="lead">{t('settings.voice.engineLead')}</p>
      <div class="engines" role="radiogroup" aria-labelledby="v-engine">
        {#each ENGINE_IDS as id (id)}
          {@const e = engines.engines.find((x) => x.id === id)}
          <button type="button" class="engine" role="radio" aria-checked={s.engine === id} onclick={() => chooseEngine(id)}>
            <span class="radio" aria-hidden="true"></span>
            <span class="body">
              <span class="name">
                {t(`settings.voice.engineName.${id}`)}
                {#if e?.offline}<span class="lib-chip ok">{t('settings.voice.offline')}</span>{/if}
                {#if e?.interim}<span class="lib-chip">{t('settings.voice.live')}</span>{/if}
                {#if id !== 'local' && id !== 'custom'}<span class="lib-chip">{t('settings.voice.paid')}</span>{/if}
              </span>
              <span class="desc">{t(`settings.voice.engine.${id}`)}</span>
            </span>
            <span class="state">
              {#if e?.configured}
                <span class="lib-chip ok"><Icon name="check" size={11} stroke={2.5} />&nbsp;{t('settings.voice.ready')}</span>
              {:else}
                <span class="lib-chip warn">{t('settings.voice.notReady')}</span>
              {/if}
            </span>
          </button>
        {/each}
      </div>
    </section>

    {#if s.engine === 'local'}
      <section class="set-card" aria-labelledby="v-local">
        <h3 id="v-local">{t('settings.voice.local.title')}</h3>
        <p class="lead">{t('settings.voice.local.lead')}</p>
        {#if !local.supported}
          <div class="set-row"><p class="set-err">{t('settings.voice.local.unsupported')}</p></div>
        {/if}
        <div class="set-row">
          <div class="text">
            <span class="label">{t('settings.voice.local.engineName')}</span>
            <span class="help">
              {#if local.engine.installed}
                {t('settings.voice.local.engineInstalled', { version: local.engine.version ?? '?', size: size(local.engine.size) })}
              {:else}
                {t('settings.voice.local.engineMissing')}
              {/if}
            </span>
          </div>
          <div class="ctl">
            {#if local.engine.installed}
              <button class="btn" disabled={downloading} onclick={() => download('engine')}>{t('settings.voice.local.update')}</button>
              <button class="btn danger" class:primary={armed === 'engine'} disabled={downloading} onclick={() => remove('engine')}>
                {armed === 'engine' ? t('settings.voice.local.deleteConfirm') : t('settings.voice.local.delete')}
              </button>
            {:else}
              <button class="btn primary" disabled={downloading || !local.supported} onclick={() => download('engine')}>{t('settings.voice.local.download')}</button>
            {/if}
          </div>
        </div>
        {#if local.download && local.download.phase !== 'done'}
          {@const d = local.download}
          <div class="set-row stack" aria-live="polite">
            {#if downloading}
              <div class="progress-line">
                <span>
                  {d.phase === 'running'
                    ? t('settings.voice.local.progress', { what: whatName(d.what), got: size(d.received), total: size(d.total) })
                    : d.phase === 'verifying'
                      ? t('settings.voice.local.verifying', { what: whatName(d.what) })
                      : t('settings.voice.local.installing', { what: whatName(d.what) })}
                </span>
                <button class="btn" onclick={cancelDownload}>{t('settings.voice.local.cancel')}</button>
              </div>
              <progress max={d.total ?? 1} value={d.total ? d.received : undefined}></progress>
            {:else if d.phase === 'failed'}
              <p class="set-err">{t('settings.voice.local.failed', { what: whatName(d.what), error: d.error ?? '' })}</p>
            {:else if d.phase === 'cancelled'}
              <p class="set-muted">{t('settings.voice.local.cancelled', { what: whatName(d.what) })}</p>
            {/if}
          </div>
        {/if}
        <div class="set-row stack">
          <span class="label">{t('settings.voice.local.models')}</span>
          <div class="set-table-wrap">
            <table class="set-table">
              <thead>
                <tr>
                  <th>{t('settings.voice.local.model')}</th>
                  <th class="r">{t('settings.voice.local.size')}</th>
                  <th>{t('settings.voice.local.quality')}</th>
                  <th>{t('settings.voice.local.speed')}</th>
                  <th class="r"><span class="sr-only">{t('settings.voice.local.actions')}</span></th>
                </tr>
              </thead>
              <tbody>
                {#each local.models as m (m.id)}
                  <tr>
                    <td>
                      <label class="model-pick">
                        <input
                          type="radio"
                          name="voice-model"
                          checked={local.active_model === m.id}
                          disabled={!m.installed}
                          onchange={() => set({ local: { model: m.id } }).then(refresh)}
                        />
                        <span>{m.name}</span>
                        {#if m.recommended}<span class="lib-chip ok">{t('settings.voice.local.recommended')}</span>{/if}
                      </label>
                    </td>
                    <td class="r">{size(m.size)}</td>
                    <td><span class="dots" aria-label={t('settings.voice.local.outOf5', { n: m.quality })}>{'●'.repeat(m.quality)}<span class="off">{'●'.repeat(5 - m.quality)}</span></span></td>
                    <td><span class="dots" aria-label={t('settings.voice.local.outOf5', { n: m.speed })}>{'●'.repeat(m.speed)}<span class="off">{'●'.repeat(5 - m.speed)}</span></span></td>
                    <td class="r">
                      {#if m.installed}
                        {#if armed === m.id}
                          <button class="btn danger primary" onclick={() => remove(m.id)}>{t('settings.voice.local.deleteConfirm')}</button>
                        {:else}
                          <button class="icon-btn" title={t('settings.voice.local.delete')} aria-label={t('settings.voice.local.deleteModel', { name: m.name })} onclick={() => remove(m.id)}><Icon name="x" size={13} /></button>
                        {/if}
                      {:else}
                        <button class="btn" disabled={downloading || !local.supported} onclick={() => download(m.id)}>{t('settings.voice.local.download')}</button>
                      {/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          <p class="set-muted">{t('settings.voice.local.modelsHelp')}</p>
        </div>
        <div class="set-row">
          <div class="text"><span class="label">{t('settings.voice.local.idle')}</span><span class="help">{t('settings.voice.local.idleHelp')}</span></div>
          <div class="ctl">
            <input
              class="lib-input narrow"
              type="number"
              min="1"
              max="120"
              value={s.local.idle_minutes}
              aria-label={t('settings.voice.local.idle')}
              onchange={(e) => set({ local: { idle_minutes: Number(e.currentTarget.value) } })}
            />
          </div>
        </div>
        <div class="set-row">
          <div class="text"><span class="label">{t('settings.voice.local.threads')}</span><span class="help">{t('settings.voice.local.threadsHelp')}</span></div>
          <div class="ctl">
            <input
              class="lib-input narrow"
              type="number"
              min="0"
              max="64"
              value={s.local.threads ?? 0}
              aria-label={t('settings.voice.local.threads')}
              onchange={(e) => set({ local: { threads: Number(e.currentTarget.value) } })}
            />
          </div>
        </div>
        {#if local.server}
          <div class="set-row"><p class="set-muted">{t('settings.voice.local.running', { port: local.server.port, model: local.server.model })}</p></div>
        {/if}
      </section>
    {:else if cloud}
      {@const c = s[cloud]}
      <section class="set-card" aria-labelledby="v-cloud">
        <h3 id="v-cloud">{t(`settings.voice.engineName.${cloud}`)}</h3>
        <p class="lead">{cloud === 'deepgram' ? t('settings.voice.cloud.liveLead') : t('settings.voice.cloud.batchLead')}</p>
        {#if cloud === 'custom'}
          <div class="set-row">
            <div class="text"><span class="label">{t('settings.voice.cloud.baseUrl')}</span><span class="help">{t('settings.voice.cloud.baseUrlHelp')}</span></div>
            <div class="ctl"><input class="lib-input wide set-mono" bind:value={draft.base_url} placeholder="http://localhost:8000/v1" spellcheck="false" aria-label={t('settings.voice.cloud.baseUrl')} /></div>
          </div>
        {/if}
        <div class="set-row">
          <div class="text">
            <span class="label">{t('settings.voice.cloud.key')}</span>
            <span class="help">
              {#if c.api_key_set}
                {t('settings.voice.cloud.keySet', { hint: c.api_key_hint ?? '' })}
              {:else if c.ai_provider_key}
                {t('settings.voice.cloud.keyFromAi')}
              {:else if cloud === 'custom'}
                {t('settings.voice.cloud.keyOptional')}
              {:else}
                {t('settings.voice.cloud.keyMissing')}
              {/if}
            </span>
          </div>
          <div class="ctl">
            <input
              class="lib-input wide set-mono"
              type="password"
              autocomplete="off"
              bind:value={draft.key}
              placeholder={c.api_key_set ? (c.api_key_hint ?? '••••') : t('settings.voice.cloud.keyPlaceholder')}
              spellcheck="false"
              aria-label={t('settings.voice.cloud.key')}
            />
            {#if c.api_key_set}
              <button class="btn" onclick={removeKey}>{t('settings.voice.cloud.removeKey')}</button>
            {/if}
          </div>
        </div>
        <div class="set-row">
          <div class="text"><span class="label">{t('settings.voice.cloud.model')}</span><span class="help">{t('settings.voice.cloud.modelHelp')}</span></div>
          <div class="ctl">
            {#if engineInfo && engineInfo.models.length > 0}
              <select class="lib-input" bind:value={draft.model} aria-label={t('settings.voice.cloud.model')}>
                <option value="">{t('settings.voice.cloud.modelDefault', { model: engineInfo.models[0] })}</option>
                {#each engineInfo.models as m (m)}
                  <option value={m}>{m}</option>
                {/each}
              </select>
            {:else}
              <input class="lib-input set-mono" bind:value={draft.model} placeholder="whisper-1" spellcheck="false" aria-label={t('settings.voice.cloud.model')} />
            {/if}
          </div>
        </div>
        <div class="set-row">
          <div class="text"><span class="help">{t('settings.voice.cloud.storage')}</span></div>
          <div class="ctl"><button class="btn primary" onclick={saveCloud}>{t('common.save')}</button></div>
        </div>
      </section>
    {/if}

    <section class="set-card" aria-labelledby="v-test">
      <h3 id="v-test">{t('settings.voice.test')}</h3>
      <p class="lead">{t('settings.voice.testLead', { engine: t(`settings.voice.engineName.${s.engine}`) })}</p>
      <div class="set-row">
        <div class="text">
          {#if testing === 'recording'}
            <span class="label rec"><span class="dot" aria-hidden="true"></span>{t('settings.voice.testRecording')}</span>
            <span class="meter" aria-hidden="true" style:--lvl={testLevel.toFixed(3)}><span class="fill"></span></span>
          {:else if testing === 'transcribing'}
            <span class="label">{t('voice.status.transcribing')}</span>
          {:else if testResult?.ok}
            <span class="label">{testResult.r.text ? `“${testResult.r.text}”` : t('voice.status.nothing')}</span>
            <span class="help">{t('settings.voice.testDone', { engine: t(`settings.voice.engineName.${testResult.r.engine}`), ms: testResult.r.elapsed_ms, language: testResult.r.language ?? '—' })}</span>
          {:else if testResult}
            <span class="set-err" role="alert">{testResult.error}</span>
          {:else}
            <span class="help" title={engineInfo?.reason ?? undefined}>{engineInfo?.configured ? t('settings.voice.testIdle') : t('settings.voice.testNotReady')}</span>
          {/if}
        </div>
        <div class="ctl">
          <button class="btn primary" disabled={testing !== 'idle' || !engineInfo?.configured} onclick={test}>
            <Icon name="mic" size={14} />
            {t('settings.voice.testButton')}
          </button>
        </div>
      </div>
    </section>
  {/if}
</div>

<style>
  .hotkey {
    min-width: 140px;
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    text-align: center;
    cursor: pointer;
  }
  .hotkey.capturing {
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
    color: var(--accent);
  }
  .engines {
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
  }
  .engine {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border: 0;
    border-top: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    text-align: left;
  }
  .engine:first-child {
    border-top: 0;
  }
  .engine:hover {
    background: var(--hover);
  }
  .engine[aria-checked='true'] {
    background: var(--accent-soft);
  }
  .radio {
    width: 16px;
    height: 16px;
    flex: none;
    border: 2px solid var(--border-strong);
    border-radius: 50%;
  }
  [aria-checked='true'] .radio {
    border: 5px solid var(--accent);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }
  .name {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-weight: 600;
  }
  .desc {
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }
  .state {
    flex: none;
  }
  .progress-line {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    font-size: var(--fs-sm);
  }
  progress {
    width: 100%;
    height: 6px;
    accent-color: var(--accent);
  }
  .model-pick {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .dots {
    color: var(--accent);
    font-size: 9px;
    letter-spacing: 2px;
  }
  .dots .off {
    color: var(--border-strong);
  }
  .rec {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
  }
  .meter {
    position: relative;
    display: block;
    width: 160px;
    height: 5px;
    margin-top: 4px;
    overflow: hidden;
    border-radius: 3px;
    background: var(--sunken);
  }
  .meter .fill {
    position: absolute;
    inset: 0;
    background: var(--danger);
    transform-origin: left;
    transform: scaleX(var(--lvl));
    transition: transform 90ms linear;
  }
  @media (prefers-reduced-motion: reduce) {
    .meter .fill {
      transition: none;
    }
  }
</style>
