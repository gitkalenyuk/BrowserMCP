<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { ConnectionStatus } from '../lib/api/types';
  import { locale, t } from '../lib/i18n/index.svelte';
  import { diagnostics, fileUrl, uptimeParts } from '../lib/settings/e1-data';
  import { copyText } from '../lib/settings/e1-open';
  import type { About } from '../lib/settings/e1-types';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let about = $state<About | null>(null);
  let loadError = $state<string | null>(null);
  let status = $state<ConnectionStatus>('connecting');

  async function load() {
    try {
      about = await api.call('system.about');
      loadError = null;
    } catch (err) {
      loadError = err instanceof Error ? err.message : String(err);
    }
  }

  $effect(() => {
    void load();
    // onStatus only reports changes, so start from the client's current state:
    // the settings page is usually already connected when About opens.
    status = api.status;
    return api.onStatus((s) => (status = s));
  });

  const uptime = $derived.by(() => {
    if (!about) return '';
    const { d, h, m } = uptimeParts(about.uptime_secs);
    if (d) return t('settings.about.uptimeDays', { d, h });
    if (h) return t('settings.about.uptimeHours', { h, m });
    return t('settings.about.uptimeMinutes', { m });
  });

  async function copy(text: string, what: string) {
    if (await copyText(text)) notify(t('settings.about.copied', { what }));
    else notify(t('settings.about.copyFailed'), true);
  }

  async function openFolder(path: string) {
    try {
      await api.call('system.open_path', { path });
    } catch (err) {
      fail(err);
    }
  }

  async function reveal(path: string) {
    try {
      await api.call('system.reveal_path', { path });
    } catch (err) {
      fail(err);
    }
  }

  async function openLicense(path: string) {
    try {
      await api.call('tabs.open', { url: fileUrl(path) });
    } catch (err) {
      fail(err);
    }
  }

  function copyDiagnostics() {
    if (!about) return;
    const text = diagnostics(about, {
      connected: api.kind === 'mock' ? undefined : status === 'open',
      uiLocale: locale(),
      userAgent: typeof navigator === 'undefined' ? undefined : navigator.userAgent,
    });
    void copy(text, t('settings.about.diagnosticsWhat'));
  }

  const connLabel = $derived(
    api.kind === 'mock' ? t('settings.about.apiMock') : status === 'open' ? t('settings.about.apiOpen') : status === 'connecting' ? t('settings.about.apiConnecting') : t('settings.about.apiClosed'),
  );
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.about.title')}</h2>
    <p>{t('settings.about.intro')}</p>
  </header>

  {#if loadError && !about}
    <p class="lib-error" role="alert">{t('settings.about.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.about.retry')}</button>
  {:else if !about}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="ab-ver">
      <h3 id="ab-ver"><span class="grow">{about.name} {about.version}</span></h3>
      <dl class="facts">
        <dt>{t('settings.about.chromium')}</dt>
        <dd class="set-mono">{about.chromium_version || t('settings.about.unknown')}</dd>
        <dt>{t('settings.about.cef')}</dt>
        <dd class="set-mono">{about.cef_version || t('settings.about.unknown')}</dd>
        <dt>{t('settings.about.os')}</dt>
        <dd>{about.os}</dd>
        <dt>{t('settings.about.process')}</dt>
        <dd>{t('settings.about.processValue', { pid: about.pid, uptime })}</dd>
      </dl>
      <div class="set-actions">
        <button class="btn primary" onclick={copyDiagnostics}>{t('settings.about.copyDiagnostics')}</button>
        <span class="set-muted">{t('settings.about.copyDiagnosticsHelp')}</span>
      </div>
    </section>

    <section class="set-card" aria-labelledby="ab-api">
      <h3 id="ab-api"><span class="grow">{t('settings.about.api')}</span><span class="lib-chip" class:ok={status === 'open' && api.kind !== 'mock'}>{connLabel}</span></h3>
      <p class="lead">{t('settings.about.apiLead', { port: about.api.port })}</p>
      {#each [{ id: 'mcp', url: about.api.mcp }, { id: 'rest', url: about.api.rest }, { id: 'ws', url: about.api.ws }] as e (e.id)}
        <div class="set-row">
          <div class="text"><span class="label">{t(`settings.about.endpoint.${e.id}`)}</span><span class="set-mono path">{e.url || t('settings.about.unknown')}</span></div>
          <div class="ctl">
            <button class="btn small" disabled={!e.url} onclick={() => copy(e.url, t(`settings.about.endpoint.${e.id}`))}>{t('common.copy')}</button>
          </div>
        </div>
      {/each}
    </section>

    <section class="set-card" aria-labelledby="ab-dirs">
      <h3 id="ab-dirs">{t('settings.about.folders')}</h3>
      {#each [{ id: 'data', path: about.data_dir }, { id: 'logs', path: about.logs_dir }, { id: 'config', path: about.config_dir }, { id: 'exe', path: about.exe_dir }] as f (f.id)}
        <div class="set-row">
          <div class="text">
            <span class="label">{t(`settings.about.folder.${f.id}`)}</span>
            <span class="help">{t(`settings.about.folder.${f.id}.help`)}</span>
            <span class="set-mono path">{f.path}</span>
          </div>
          <div class="ctl">
            <button class="btn small" disabled={!f.path} onclick={() => openFolder(f.path)}>{t('settings.about.open')}</button>
            <button class="btn small" disabled={!f.path} onclick={() => copy(f.path, t(`settings.about.folder.${f.id}`))}>{t('common.copy')}</button>
          </div>
        </div>
      {/each}
    </section>

    <section class="set-card" aria-labelledby="ab-lic">
      <h3 id="ab-lic">{t('settings.about.licenses')}</h3>
      <p class="lead">{t('settings.about.licensesLead')}</p>
      {#each about.licenses as l (l.path)}
        <div class="set-row">
          <div class="text"><span class="label">{l.name}</span><span class="set-mono path">{l.path}</span></div>
          <div class="ctl">
            <button class="btn small" onclick={() => openLicense(l.path)}>{t('settings.about.open')}</button>
            <button class="btn small" onclick={() => reveal(l.path)}>{t('settings.about.showInFolder')}</button>
          </div>
        </div>
      {:else}
        <p class="set-muted pad">{t('settings.about.noLicenses')}</p>
      {/each}
    </section>
  {/if}
</div>

<style>
  .pad {
    padding: 0 16px 12px;
  }
  .facts {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 6px 18px;
    margin: 0;
    padding: 4px 16px 8px;
    font-size: var(--fs-sm);
  }
  .facts dt {
    color: var(--fg-subtle);
  }
  .facts dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .path {
    overflow-wrap: anywhere;
    color: var(--fg-muted);
  }
  .lib-chip.ok {
    color: var(--ok);
  }
  .btn.small {
    height: 26px;
    padding: 0 10px;
    font-size: var(--fs-sm);
  }
</style>
