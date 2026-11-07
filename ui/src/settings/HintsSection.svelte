<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { HintsConfig } from '../lib/api/types-settings';
  import Icon from '../lib/components/Icon.svelte';
  import { t } from '../lib/i18n/index.svelte';
  import { HINT_MODES, addSite, hintKeyLabel, hintsErrors, hintsForm, hintsPatch, normalizeHost, type HintsForm } from '../lib/settings/forms';
  import { isEmptyPatch } from '../lib/settings/notify';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let config = $state<HintsConfig | null>(null);
  let form = $state<HintsForm | null>(null);
  let loadError = $state<string | null>(null);
  let saving = $state(false);
  let siteInput = $state('');
  let siteError = $state<string | null>(null);

  async function load() {
    try {
      const r = await api.call('hints.config', {});
      config = r.config;
      form = hintsForm(r.config);
      loadError = null;
    } catch (err) {
      loadError = (err as Error).message;
    }
  }
  $effect(() => {
    void load();
  });

  const errors = $derived(form ? hintsErrors(form) : {});
  const patch = $derived(config && form ? hintsPatch(config, form) : {});
  const dirty = $derived(!isEmptyPatch(patch));
  const invalid = $derived(Object.keys(errors).length > 0);

  async function save() {
    if (!dirty || invalid) return;
    saving = true;
    try {
      const r = await api.call('hints.config', patch);
      config = r.config;
      form = hintsForm(r.config);
      notify(t('settings.hints.saved'));
    } catch (err) {
      fail(err);
    } finally {
      saving = false;
    }
  }

  // Sites apply at once (one command each), independent of the form above.
  async function disableOn() {
    if (!config) return;
    const { error } = addSite(config.disabled_sites, siteInput);
    siteError = error;
    if (error) return;
    const host = normalizeHost(siteInput)!;
    try {
      config = (await api.call('hints.config', { disable_site: host })).config;
      siteInput = '';
    } catch (err) {
      fail(err);
    }
  }
  async function enableOn(host: string) {
    try {
      config = (await api.call('hints.config', { enable_site: host })).config;
    } catch (err) {
      fail(err);
    }
  }
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.hints.title')}</h2>
    <p>
      {t('settings.hints.intro')}
      {t('settings.hints.introAgents')} (<span class="set-mono">hints.show</span>).
    </p>
  </header>

  {#if loadError}
    <p class="lib-error" role="alert">{t('settings.hints.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.hints.retry')}</button>
  {:else if !form || !config}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="h-keys">
      <h3 id="h-keys">{t('settings.hints.keys')}</h3>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.hints.inPages')}</span><span class="help">{t('settings.hints.inPagesHelp')}</span></div>
        <div class="ctl"><Toggle label={t('settings.hints.inPages')} checked={form.enabled} onchange={(on) => (form!.enabled = on)} /></div>
      </div>
      {#each HINT_MODES as m (m.mode)}
        <div class="set-row">
          <div class="text">
            <span class="label">{m.label}</span>
            {#if errors[`key.${m.mode}`]}<span class="set-err">{errors[`key.${m.mode}`]}</span>
            {:else}<span class="help">{form.keys[m.mode] ? t('settings.hints.press', { key: hintKeyLabel(form.keys[m.mode]) }) : t('settings.hints.noKey')}</span>{/if}
          </div>
          <div class="ctl">
            <input
              class="lib-input key"
              maxlength="2"
              bind:value={form.keys[m.mode]}
              disabled={!form.enabled}
              placeholder="—"
              spellcheck="false"
              aria-label={t('settings.hints.keyFor', { mode: m.label })}
              aria-invalid={!!errors[`key.${m.mode}`]}
            />
            {#if form.keys[m.mode]}
              <button class="icon-btn" aria-label={t('settings.hints.clearKey', { mode: m.label })} disabled={!form.enabled} onclick={() => (form!.keys[m.mode] = '')}><Icon name="x" size={13} /></button>
            {/if}
          </div>
        </div>
      {/each}
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.hints.letters')}</span>
          {#if errors.alphabet}<span class="set-err">{errors.alphabet}</span>{:else}<span class="help">{t('settings.hints.lettersHelp')}</span>{/if}
        </div>
        <div class="ctl">
          <input class="lib-input set-mono alpha" bind:value={form.alphabet} spellcheck="false" aria-label={t('settings.hints.letters')} aria-invalid={!!errors.alphabet} />
        </div>
      </div>
    </section>

    <section class="set-card" aria-labelledby="h-sites">
      <h3 id="h-sites">{t('settings.hints.sites')}</h3>
      <p class="lead">{t('settings.hints.sitesLead')}</p>
      <div class="set-row stack">
        <div class="set-chips">
          {#each config.disabled_sites as host (host)}
            <span class="set-chip">
              {host}
              <button class="icon-btn" aria-label={t('settings.hints.enableOn', { host })} onclick={() => enableOn(host)}><Icon name="x" size={12} /></button>
            </span>
          {:else}
            <span class="set-muted">{t('settings.hints.noSites')}</span>
          {/each}
        </div>
        <form class="add" onsubmit={(e) => (e.preventDefault(), void disableOn())}>
          <input class="lib-input wide" bind:value={siteInput} placeholder="mail.google.com" spellcheck="false" aria-label={t('settings.hints.siteInput')} aria-invalid={!!siteError} />
          <button class="btn" type="submit" disabled={!siteInput.trim()}>{t('settings.hints.disableOn')}</button>
        </form>
        {#if siteError}<p class="set-err">{siteError}</p>{/if}
      </div>
    </section>

    {#if dirty}
      <div class="set-savebar" role="region" aria-label={t('settings.general.unsaved')}>
        <span>{invalid ? t('settings.general.fixToSave') : t('settings.general.unsaved')}</span>
        <button class="btn" disabled={saving} onclick={() => config && (form = hintsForm(config))}>{t('settings.general.discard')}</button>
        <button class="btn primary" disabled={saving || invalid} onclick={save}>{saving ? t('settings.general.saving') : t('common.save')}</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .key {
    width: 48px;
    font-family: var(--font-mono);
    text-align: center;
  }
  .alpha {
    width: 160px;
  }
  .add {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
</style>
