<script lang="ts">
  // Settings → Connections & API: how AI agents and automation tools reach this browser.
  // api.info / api.config (crates/api/src/info.rs), agents.list, tabs.list (locks).
  import type { BrowserApi } from '../lib/api';
  import type { Agent, TabsSnapshot } from '../lib/api/types';
  import type { ApiInfo } from '../lib/api/types-e2-settings';
  import Icon from '../lib/components/Icon.svelte';
  import { t } from '../lib/i18n/index.svelte';
  import {
    cdpPortError,
    claudeHttpCommand,
    claudeStdioCommand,
    codexHttpToml,
    codexStdioCommand,
    codexStdioToml,
    defaultToolsets,
    mcpJson,
    mcpUrl,
    playwrightSnippet,
    puppeteerSnippet,
    sortToolsets,
  } from '../lib/settings/e2-connections';
  import Dialog from '../library/Dialog.svelte';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let info = $state<ApiInfo | null>(null);
  let loadError = $state<string | null>(null);
  let agents = $state<Agent[]>([]);
  let tabs = $state<TabsSnapshot['tabs']>([]);
  /** Toolsets ticked in the picker (starts with the server's defaults). */
  let picked = $state<string[] | null>(null);
  let cdpPort = $state('');
  let confirmCdp = $state(false);
  let saving = $state(false);
  let copied = $state<string | null>(null);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  async function refresh() {
    try {
      const next = await api.call('api.info');
      if (!info || cdpPort === String(info.cdp.port)) cdpPort = String(next.cdp.port);
      info = next;
      picked ??= defaultToolsets(next.toolsets);
      loadError = null;
    } catch (err) {
      loadError = (err as Error).message;
    }
    // Agents and locks are extras: the section still works without them.
    try {
      agents = (await api.call('agents.list')).agents;
      tabs = (await api.call('tabs.list')).tabs;
    } catch {
      /* older core */
    }
  }

  $effect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 4000);
    return () => clearInterval(timer);
  });

  const toolsets = $derived(info ? sortToolsets(info.toolsets) : []);
  const defaults = $derived(info ? defaultToolsets(info.toolsets) : []);
  const url = $derived(
    info?.urls
      ? mcpUrl(
          info.urls.mcp,
          picked ?? defaults,
          defaults,
          toolsets.map((ts) => ts.name),
        )
      : '',
  );
  const exe = $derived(info?.exe ?? 'browsermcp.exe');
  const lockedTabs = $derived(tabs.filter((tab) => tab.lock));
  const portError = $derived(info ? cdpPortError(cdpPort, info.port) : '');
  const stealth = $derived(info?.cdp.stealth_profiles ?? []);
  const toolCount = $derived(toolsets.filter((ts) => (picked ?? defaults).includes(ts.name)).reduce((n, ts) => n + ts.commands, 0));

  function toggleToolset(name: string, on: boolean) {
    const now = picked ?? defaults;
    picked = on ? [...new Set([...now, name])] : now.filter((n) => n !== name);
  }

  async function copy(id: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = id;
      clearTimeout(copiedTimer);
      copiedTimer = setTimeout(() => (copied = null), 1500);
    } catch {
      notify(t('settings.connections.copyFailed'), true);
    }
  }

  async function saveCdp(patch: { cdp_enabled?: boolean; cdp_port?: number }) {
    saving = true;
    try {
      const r = await api.call('api.config', patch);
      if (info) info = { ...info, cdp: r.cdp };
      cdpPort = String(r.config.cdp_port);
      notify(r.cdp.restart_needed ? t('settings.connections.cdp.savedRestart') : t('settings.connections.cdp.saved'));
    } catch (err) {
      fail(err);
    } finally {
      saving = false;
    }
  }

  function onCdpToggle(on: boolean) {
    // Opening the port is the risky direction: ask first.
    if (on) confirmCdp = true;
    else void saveCdp({ cdp_enabled: false });
  }

  const lockLabel = (state: string) =>
    state === 'paused' ? t('settings.connections.lock.paused') : state === 'handoff' ? t('settings.connections.lock.handoff') : t('settings.connections.lock.active');
  const statusLabel = (s: Agent['status']) => t(`settings.connections.agent.${s}`);
</script>

{#snippet codeRow(id: string, label: string, help: string, text: string)}
  <div class="set-row stack">
    <div class="text"><span class="label">{label}</span><span class="help">{help}</span></div>
    <div class="code">
      <pre class="set-mono" aria-label={label}>{text}</pre>
      <button class="btn mini" onclick={() => copy(id, text)} aria-label={t('settings.connections.copyLabel', { what: label })}>
        <Icon name={copied === id ? 'check' : 'copy'} size={13} />
        {copied === id ? t('common.copied') : t('common.copy')}
      </button>
    </div>
  </div>
{/snippet}

<div class="set-page">
  <header>
    <h2>{t('settings.connections.title')}</h2>
    <p>{t('settings.connections.intro')}</p>
  </header>

  {#if loadError && !info}
    <p class="lib-error" role="alert">{t('settings.connections.loadError', { error: loadError })}</p>
    <button class="btn" onclick={refresh}>{t('settings.connections.retry')}</button>
  {:else if !info}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="c-api">
      <h3 id="c-api">{t('settings.connections.api.title')}</h3>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.connections.api.status')}</span>
          <span class="help">{t('settings.connections.api.statusHelp')}</span>
        </div>
        <div class="ctl">
          {#if info.listening && info.port}
            <span class="lib-chip ok">{t('settings.connections.api.listening', { port: info.port })}</span>
          {:else}
            <span class="lib-chip warn">{t('settings.connections.api.starting')}</span>
          {/if}
          <span class="set-muted">v{info.version}</span>
        </div>
      </div>
      {#if info.endpoint_file}
        <div class="set-row">
          <div class="text">
            <span class="label">{t('settings.connections.api.endpoint')}</span>
            <span class="help">{t('settings.connections.api.endpointHelp')}</span>
          </div>
          <div class="ctl">
            <span class="set-mono path" title={info.endpoint_file}>{info.endpoint_file}</span>
            {#if !info.endpoint_present}<span class="lib-chip warn">{t('settings.connections.api.endpointMissing')}</span>{/if}
          </div>
        </div>
      {/if}
      {#if info.urls}
        {@render codeRow('mcp', t('settings.connections.url.mcp'), t('settings.connections.url.mcpHelp'), info.urls.mcp)}
        {@render codeRow('rest', t('settings.connections.url.rest'), t('settings.connections.url.restHelp'), info.urls.rest)}
        {@render codeRow('ws', t('settings.connections.url.ws'), t('settings.connections.url.wsHelp'), info.urls.ws)}
        {@render codeRow('openapi', t('settings.connections.url.openapi'), t('settings.connections.url.openapiHelp'), info.urls.openapi)}
      {/if}
    </section>

    <section class="set-card" aria-labelledby="c-agents">
      <h3 id="c-agents">{t('settings.connections.connect.title')}</h3>
      <p class="lead">{t('settings.connections.connect.lead')}</p>
      <div class="set-row stack">
        <div class="text">
          <span class="label">{t('settings.connections.toolsets.label')}</span>
          <span class="help">{t('settings.connections.toolsets.help', { n: toolCount })}</span>
        </div>
        <div class="toolsets">
          {#each toolsets as ts (ts.name)}
            <label class="ts" title={t(`settings.connections.toolset.${ts.name}`)}>
              <input type="checkbox" checked={(picked ?? defaults).includes(ts.name)} onchange={(e) => toggleToolset(ts.name, e.currentTarget.checked)} />
              <span class="ts-name">{ts.name}</span>
              <span class="ts-count">{ts.commands}</span>
              <span class="ts-help">{t(`settings.connections.toolset.${ts.name}`)}</span>
            </label>
          {/each}
        </div>
        <div class="set-actions flush">
          <button class="btn mini" onclick={() => (picked = [...defaults])}>{t('settings.connections.toolsets.defaults')}</button>
          <button class="btn mini" onclick={() => (picked = toolsets.filter((ts) => ts.name !== 'hidden').map((ts) => ts.name))}>{t('settings.connections.toolsets.all')}</button>
        </div>
      </div>
      {#if url}
        {@render codeRow('url', t('settings.connections.cmd.url'), t('settings.connections.cmd.urlHelp'), url)}
        {@render codeRow('claude-http', t('settings.connections.cmd.claudeHttp'), t('settings.connections.cmd.claudeHttpHelp'), claudeHttpCommand(url))}
      {/if}
      {@render codeRow('claude-stdio', t('settings.connections.cmd.claudeStdio'), t('settings.connections.cmd.claudeStdioHelp'), claudeStdioCommand(exe))}
      {@render codeRow('codex', t('settings.connections.cmd.codex'), t('settings.connections.cmd.codexHelp'), codexStdioCommand(exe))}
      {@render codeRow('codex-toml', t('settings.connections.cmd.codexToml'), t('settings.connections.cmd.codexTomlHelp'), codexStdioToml(exe))}
      {#if url}
        {@render codeRow('codex-http', t('settings.connections.cmd.codexHttp'), t('settings.connections.cmd.codexHttpHelp'), codexHttpToml(url))}
        {@render codeRow('json', t('settings.connections.cmd.json'), t('settings.connections.cmd.jsonHelp'), mcpJson(url))}
      {/if}
    </section>

    <section class="set-card" aria-labelledby="c-cdp">
      <h3 id="c-cdp">{t('settings.connections.cdp.title')}</h3>
      <p class="lead">{t('settings.connections.cdp.lead')}</p>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.connections.cdp.enable')}</span>
          <span class="help">{t('settings.connections.cdp.enableHelp')}</span>
          {#if stealth.length}
            <span class="help warn">{t('settings.connections.cdp.stealthBlocks', { profiles: stealth.join(', ') })}</span>
          {/if}
        </div>
        <div class="ctl">
          <Toggle
            label={t('settings.connections.cdp.enable')}
            checked={info.cdp.enabled}
            disabled={saving || (stealth.length > 0 && !info.cdp.enabled)}
            onchange={onCdpToggle}
          />
        </div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.connections.cdp.port')}</span>
          <span class="help">{t('settings.connections.cdp.portHelp')}</span>
        </div>
        <div class="ctl">
          <input
            class="lib-input narrow"
            inputmode="numeric"
            bind:value={cdpPort}
            aria-label={t('settings.connections.cdp.port')}
            aria-invalid={!!portError}
          />
          <button class="btn" disabled={saving || !!portError || cdpPort === String(info.cdp.port)} onclick={() => saveCdp({ cdp_port: Number(cdpPort) })}>
            {t('common.save')}
          </button>
        </div>
      </div>
      {#if portError}<p class="set-err pad">{t(portError)}</p>{/if}
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.connections.cdp.now')}</span>
          <span class="help">
            {#if info.cdp.active_port}
              {t('settings.connections.cdp.open', { port: info.cdp.active_port })}
            {:else if info.cdp.refused}
              {t('settings.connections.cdp.refused')}
            {:else}
              {t('settings.connections.cdp.closed')}
            {/if}
          </span>
        </div>
        <div class="ctl">
          {#if info.cdp.restart_needed}<span class="lib-chip warn">{t('settings.connections.cdp.restart')}</span>{/if}
          <span class="lib-chip" class:ok={!!info.cdp.active_port}>{info.cdp.active_port ? t('common.on') : t('common.off')}</span>
        </div>
      </div>
      {#if info.cdp.active_port || info.cdp.enabled}
        {@const port = info.cdp.active_port ?? info.cdp.port}
        {@render codeRow('playwright', t('settings.connections.cdp.playwright'), t('settings.connections.cdp.playwrightHelp'), playwrightSnippet(port))}
        {@render codeRow('puppeteer', t('settings.connections.cdp.puppeteer'), t('settings.connections.cdp.puppeteerHelp'), puppeteerSnippet(port))}
      {/if}
    </section>

    <section class="set-card" aria-labelledby="c-live">
      <h3 id="c-live"><span class="grow">{t('settings.connections.live.title')}</span><span class="set-muted small" aria-hidden="true">{t('settings.connections.live.every')}</span></h3>
      <p class="lead">{t('settings.connections.live.lead')}</p>
      <div class="set-table-wrap">
        <table class="set-table">
          <thead>
            <tr>
              <th>{t('settings.connections.live.agent')}</th>
              <th>{t('settings.connections.live.status')}</th>
              <th>{t('settings.connections.live.task')}</th>
              <th class="r">{t('settings.connections.live.tabs')}</th>
            </tr>
          </thead>
          <tbody>
            {#each agents as a (a.id)}
              <tr>
                <td><span class="dot" style:--c="var(--g-{a.color}, var(--accent))"></span>{a.name}</td>
                <td><span class="lib-chip" class:ok={a.status === 'running'} class:warn={a.status === 'waiting'} class:bad={a.status === 'error'}>{statusLabel(a.status)}</span></td>
                <td class="set-muted">{a.task ?? '—'}</td>
                <td class="r">{a.tabs?.length ?? 0}</td>
              </tr>
            {:else}
              <tr><td colspan="4" class="set-muted">{t('settings.connections.live.none')}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if lockedTabs.length}
        <div class="locks">
          <h4>{t('settings.connections.lock.title')}</h4>
          <ul>
            {#each lockedTabs as tab (tab.id)}
              <li>
                <span class="lib-chip">{lockLabel(tab.lock!.state)}</span>
                <span class="t" title={tab.url}>{tab.title || tab.url}</span>
                <span class="set-muted">{t('settings.connections.lock.owner', { agent: tab.lock!.owner })}</span>
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </section>

    <section class="set-card" aria-labelledby="c-sec">
      <h3 id="c-sec">{t('settings.connections.security.title')}</h3>
      <ul class="facts">
        <li><Icon name="lock" size={14} />{t('settings.connections.security.local')}</li>
        <li><Icon name="globe" size={14} />{t('settings.connections.security.host')}</li>
        <li><Icon name="info" size={14} />{t('settings.connections.security.origin')}</li>
        <li><Icon name="key" size={14} />{t('settings.connections.security.tokens')}</li>
        <li><Icon name="layers" size={14} />{t('settings.connections.security.plugins')}</li>
      </ul>
    </section>
  {/if}
</div>

<Dialog bind:open={confirmCdp} title={t('settings.connections.cdp.confirmTitle')} onsubmit={() => ((confirmCdp = false), void saveCdp({ cdp_enabled: true }))}>
  <p>{t('settings.connections.cdp.confirmBody')}</p>
  <p class="set-muted">{t('settings.connections.cdp.confirmStealth')}</p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (confirmCdp = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn primary">{t('settings.connections.cdp.confirmOk')}</button>
  {/snippet}
</Dialog>

<style>
  .pad {
    padding: 0 16px 10px;
  }
  .small {
    font-size: var(--fs-xs);
    font-weight: 400;
  }
  .path {
    max-width: 420px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .help.warn {
    color: var(--warn);
  }
  .code {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }
  .code pre {
    flex: 1;
    min-width: 0;
    margin: 0;
    padding: 7px 10px;
    overflow-x: auto;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--sunken);
    white-space: pre;
    user-select: all;
  }
  .mini {
    height: 28px;
    padding: 0 10px;
    font-size: var(--fs-sm);
  }
  .code .btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .toolsets {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 6px;
  }
  .ts {
    display: grid;
    grid-template-columns: auto auto 1fr;
    grid-template-rows: auto auto;
    column-gap: 8px;
    align-items: center;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    cursor: pointer;
  }
  .ts:has(input:checked) {
    border-color: var(--accent-line);
    background: var(--accent-soft);
  }
  .ts-name {
    font-family: var(--font-mono);
    font-weight: 600;
  }
  .ts-count {
    justify-self: end;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .ts-help {
    grid-column: 2 / 4;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    line-height: 1.4;
  }
  .flush {
    padding: 0;
  }
  .dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    margin-right: 8px;
    border-radius: 50%;
    background: var(--c);
  }
  .locks {
    padding: 4px 16px 12px;
    border-top: 1px solid var(--border);
  }
  .locks h4 {
    margin: 8px 0 6px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 650;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }
  .locks ul,
  .facts {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: var(--fs-sm);
  }
  .locks li {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .locks .t {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .facts {
    padding: 4px 16px 14px;
    color: var(--fg-muted);
    line-height: 1.45;
  }
  .facts li {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }
  .facts :global(svg) {
    flex: none;
    margin-top: 2px;
  }
</style>
