<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { PluginInfo, PluginLogLine, UserscriptInfo } from '../lib/api/types-settings';
  import Icon from '../lib/components/Icon.svelte';
  import { locale, t, tp } from '../lib/i18n/index.svelte';
  import Dialog from '../library/Dialog.svelte';
  import { richParts } from '../lib/settings/rich';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  const NEW_SCRIPT = `// @name My userscript
// @match https://example.com/*
// @run-at document-end

// Runs once per matching page load with inputs.tab and inputs.url.
await browser.userscripts.inject({
  tab: inputs.tab,
  js: "document.body.style.outline = '3px solid hotpink'",
});
`;

  let plugins = $state<PluginInfo[]>([]);
  let scripts = $state<UserscriptInfo[]>([]);
  let loadError = $state<string | null>(null);
  let loaded = $state(false);
  let busy = $state<string | null>(null);
  let logsFor = $state<string | null>(null);
  let logs = $state<PluginLogLine[]>([]);
  let installPath = $state('');

  // userscript editor
  let editing = $state<{ name: string; code: string; isNew: boolean } | null>(null);
  let editError = $state<string | null>(null);
  let toDelete = $state<UserscriptInfo | null>(null);
  let deleteOpen = $state(false);

  async function load() {
    try {
      const [p, u] = await Promise.all([api.call('plugins.list'), api.call('userscripts.list')]);
      plugins = p.plugins;
      scripts = u.userscripts;
      loadError = null;
    } catch (err) {
      loadError = (err as Error).message;
    } finally {
      loaded = true;
    }
  }
  $effect(() => {
    void load();
  });

  // Logs of the open plugin, every 2 s.
  $effect(() => {
    const name = logsFor;
    if (!name) return;
    const read = async () => {
      try {
        logs = (await api.call('plugins.logs', { name, limit: 200 })).lines;
      } catch (err) {
        logs = [{ ts_ms: Date.now(), stream: 'system', line: (err as Error).message }];
      }
    };
    void read();
    const timer = setInterval(read, 2000);
    return () => clearInterval(timer);
  });

  async function act<T>(key: string, f: () => Promise<T>): Promise<T | undefined> {
    busy = key;
    try {
      return await f();
    } catch (err) {
      fail(err);
      return undefined;
    } finally {
      busy = null;
    }
  }

  const setPlugin = (p: PluginInfo, on: boolean) =>
    act(`p:${p.name}`, async () => {
      const next = await api.call(on ? 'plugins.enable' : 'plugins.disable', { name: p.name });
      plugins = plugins.map((x) => (x.name === p.name ? next : x));
      void load(); // its userscripts and palette entries come and go with it
    });

  const restart = (name?: string) =>
    act(name ? `p:${name}` : 'reload', async () => {
      const r = await api.call('plugins.reload', name ? { name } : {});
      plugins = r.plugins;
      notify(name ? t('settings.plugins.restarted', { name }) : t('settings.plugins.rescanned'));
    });

  const install = () =>
    act('install', async () => {
      const p = await api.call('plugins.install', { path: installPath.trim() });
      installPath = '';
      notify(t('settings.plugins.installed', { name: p.name }));
      await load();
    });

  const setScript = (u: UserscriptInfo, on: boolean) =>
    act(`u:${u.name}`, async () => {
      await api.call('userscripts.enable', { name: u.name, enabled: on });
      scripts = scripts.map((x) => (x.name === u.name ? { ...x, enabled: on } : x));
    });

  async function edit(u: UserscriptInfo) {
    await act(`u:${u.name}`, async () => {
      const r = await api.call('userscripts.get', { name: u.name });
      editError = null;
      editing = { name: r.name, code: r.code, isNew: false };
    });
  }

  function create() {
    editError = null;
    editing = { name: '', code: NEW_SCRIPT, isNew: true };
  }

  async function saveScript() {
    if (!editing) return;
    const name = editing.name.trim();
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(name)) {
      editError = t('settings.plugins.err.name');
      return;
    }
    if (editing.isNew && scripts.some((s) => s.source === 'user' && s.name === name)) {
      editError = t('settings.plugins.err.exists', { name });
      return;
    }
    busy = 'save';
    try {
      const saved = await api.call('userscripts.save', { name, code: editing.code });
      scripts = [...scripts.filter((s) => !(s.source === 'user' && s.name === name)), saved].sort((a, b) => a.name.localeCompare(b.name));
      notify(t('settings.plugins.scriptSaved', { name }));
      editing = null;
      void load();
    } catch (err) {
      editError = (err as Error).message;
    } finally {
      busy = null;
    }
  }

  async function remove() {
    const u = toDelete;
    deleteOpen = false;
    if (!u) return;
    await act(`u:${u.name}`, async () => {
      await api.call('userscripts.delete', { name: u.name });
      scripts = scripts.filter((s) => s !== u);
      notify(t('settings.plugins.deleted', { name: u.name }));
    });
  }

  const STATUS_CLASS: Record<PluginInfo['status'], string> = {
    running: 'ok',
    disabled: '',
    waiting: 'warn',
    restarting: 'warn',
    exited: '',
    crashed: 'bad',
    error: 'bad',
  };
  const timeFmt = $derived(new Intl.DateTimeFormat(locale(), { hour: '2-digit', minute: '2-digit', second: '2-digit' }));
  const ago = (ms: number) => {
    const s = Math.round((Date.now() - ms) / 1000);
    if (s < 60) return t('settings.plugins.agoSecs', { n: s });
    return s < 3600 ? t('settings.plugins.agoMin', { n: Math.round(s / 60) }) : t('settings.plugins.agoH', { n: Math.round(s / 3600) });
  };
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.plugins.title')}</h2>
    <p>
      {#each richParts(t('settings.plugins.intro')) as p, i (i)}{#if p.slot === 'dir'}<span class="set-mono">data/plugins</span>{:else if p.slot === 'manifest'}<span class="set-mono">plugin.json</span>{:else}{p.text}{/if}{/each}
    </p>
  </header>

  {#if loadError}
    <p class="lib-error" role="alert">{t('settings.plugins.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.hints.retry')}</button>
  {:else if !loaded}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="pl-h">
      <h3 id="pl-h">
        <span class="grow">{t('settings.plugins.plugins')}</span>
        <button class="btn" disabled={busy !== null} onclick={() => restart()} title={t('settings.plugins.rescanHint')}><Icon name="reload" size={13} /> {t('settings.plugins.rescan')}</button>
      </h3>
      {#each plugins as p (p.name)}
        <div class="set-row plug">
          <div class="text">
            <span class="label">
              {p.manifest?.name ?? p.name}
              {#if p.manifest?.version}<span class="set-muted ver">v{p.manifest.version}</span>{/if}
              <span class="lib-chip {STATUS_CLASS[p.status]}">{t(`settings.plugins.status.${p.status}`)}{p.restarts ? ` · ${tp('settings.plugins.restarts', p.restarts)}` : ''}</span>
            </span>
            {#if p.manifest?.description}<span class="help">{p.manifest.description}</span>{/if}
            {#if p.error}<span class="set-err">{p.error}</span>{/if}
            {#each p.warnings ?? [] as w (w)}<span class="warnline"><Icon name="alert" size={12} /> {w}</span>{/each}
            {#if p.manifest}
              <span class="meta">
                {#if p.manifest.permissions.length}<span>{t('settings.plugins.canCall')} <span class="set-mono">{p.manifest.permissions.join(', ')}</span></span>{/if}
                {#if p.manifest.events.length}<span>{t('settings.plugins.listensTo')} <span class="set-mono">{p.manifest.events.join(', ')}</span></span>{/if}
                {#if p.manifest.ui.palette.length}<span>Ctrl+K: {p.manifest.ui.palette.map((e) => e.title).join(', ')}</span>{/if}
              </span>
            {/if}
          </div>
          <div class="ctl">
            <button class="btn" aria-expanded={logsFor === p.name} onclick={() => (logsFor = logsFor === p.name ? null : p.name)}>{t('settings.plugins.logs')}</button>
            {#if p.enabled}<button class="btn" disabled={busy !== null} onclick={() => restart(p.name)}>{t('settings.plugins.restart')}</button>{/if}
            <Toggle label={t('settings.plugins.pluginToggle', { name: p.name })} checked={p.enabled} disabled={busy !== null} onchange={(on) => setPlugin(p, on)} />
          </div>
        </div>
        {#if logsFor === p.name}
          <div class="logs" role="log" aria-label={t('settings.plugins.logsOf', { name: p.name })}>
            {#each logs as l, i (i)}
              <div class="ll {l.stream}"><span class="ts">{timeFmt.format(l.ts_ms)}</span><span class="st">{l.stream}</span><span class="tx">{l.line}</span></div>
            {:else}
              <div class="ll system"><span class="tx">{t('settings.plugins.noOutput')}</span></div>
            {/each}
          </div>
        {/if}
      {:else}
        <p class="lead">{t('settings.plugins.none1')} <span class="set-mono">data/plugins</span> {t('settings.plugins.none2')}</p>
      {/each}
      <form class="set-actions" onsubmit={(e) => (e.preventDefault(), void install())}>
        <input class="lib-input wide mono" bind:value={installPath} placeholder="C:\path\to\plugin-folder" spellcheck="false" aria-label={t('settings.plugins.installLabel')} />
        <button class="btn" type="submit" disabled={busy !== null || !installPath.trim()}>{t('settings.plugins.install')}</button>
      </form>
    </section>

    <section class="set-card" aria-labelledby="us-h">
      <h3 id="us-h">
        <span class="grow">{t('settings.plugins.userscripts')}</span>
        <button class="btn" onclick={create}><Icon name="plus" size={13} /> {t('settings.plugins.newScript')}</button>
      </h3>
      {#each scripts as u (u.source + u.name)}
        <div class="set-row">
          <div class="text">
            <span class="label">
              {u.header?.name ?? u.name}
              {#if u.source !== 'user'}<span class="lib-chip">{t('settings.plugins.fromPlugin', { name: u.source.replace('plugin:', '') })}</span>{/if}
              {#if u.last_run}
                <span class="lib-chip" class:ok={u.last_run.ok} class:bad={!u.last_run.ok} title={u.last_run.url}>
                  {u.last_run.ok ? t('settings.plugins.ran') : t('settings.plugins.failed')} {ago(u.last_run.at_ms)}
                </span>
              {/if}
            </span>
            {#if u.header}
              <span class="help set-mono">{[...u.header.matches, ...u.header.includes].join('  ')}{u.header.exclude_matches.length || u.header.excludes.length ? `  (${t('settings.plugins.not')} ${[...u.header.exclude_matches, ...u.header.excludes].join(', ')})` : ''} · {u.header.run_at}</span>
            {/if}
            {#if u.error}<span class="set-err">{u.error}</span>{/if}
            {#if u.last_run?.error}<span class="set-err">{u.last_run.error.code}: {u.last_run.error.message}{u.last_run.error.line ? ` (${t('settings.plugins.line', { n: u.last_run.error.line })})` : ''}</span>{/if}
          </div>
          <div class="ctl">
            {#if u.source === 'user'}
              <button class="icon-btn" aria-label={t('settings.plugins.editScript', { name: u.name })} title={t('common.edit')} disabled={busy !== null} onclick={() => edit(u)}><Icon name="pencil" size={14} /></button>
              <button class="icon-btn" aria-label={t('settings.plugins.deleteScript', { name: u.name })} title={t('common.delete')} disabled={busy !== null} onclick={() => ((toDelete = u), (deleteOpen = true))}><Icon name="trash" size={14} /></button>
              <Toggle label={t('settings.plugins.scriptToggle', { name: u.name })} checked={u.enabled} disabled={busy !== null} onchange={(on) => setScript(u, on)} />
            {:else}
              <span class="set-muted">{t('settings.plugins.partOfPlugin')}</span>
            {/if}
          </div>
        </div>
      {:else}
        <p class="lead">{t('settings.plugins.noScripts')} <span class="set-mono">data/userscripts/*.ts</span>.</p>
      {/each}

      {#if editing}
        <div class="editor" role="region" aria-label={t('settings.plugins.editor')}>
          <label class="lib-field">
            {t('settings.plugins.name')}
            <input class="lib-input mono" bind:value={editing.name} readonly={!editing.isNew} placeholder="my-userscript" spellcheck="false" />
          </label>
          <label class="lib-field">
            {t('settings.plugins.code')} <span class="set-muted">{t('settings.plugins.headerLines')} <span class="set-mono">// @match</span>, <span class="set-mono">// @exclude-match</span>, <span class="set-mono">// @run-at document-start | document-end | document-idle</span></span>
            <textarea class="set-code" bind:value={editing.code} spellcheck="false" wrap="off"></textarea>
          </label>
          {#if editError}<p class="set-err" role="alert">{editError}</p>{/if}
          <div class="row">
            <button class="btn primary" disabled={busy !== null} onclick={saveScript}>{busy === 'save' ? t('settings.general.saving') : t('common.save')}</button>
            <button class="btn" disabled={busy !== null} onclick={() => (editing = null)}>{t('common.cancel')}</button>
          </div>
        </div>
      {/if}
    </section>
  {/if}
</div>

<Dialog bind:open={deleteOpen} title={t('settings.plugins.deleteTitle')} onsubmit={remove}>
  <p>{t('settings.plugins.deleteFile1')} <strong>{toDelete?.name}.ts</strong> {t('settings.plugins.deleteFile2')}</p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (deleteOpen = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn danger primary">{t('common.delete')}</button>
  {/snippet}
</Dialog>

<style>
  .ver {
    margin-left: 4px;
    font-weight: 400;
  }
  .label .lib-chip {
    margin-left: 6px;
    vertical-align: 1px;
  }
  .plug {
    align-items: flex-start;
  }
  .warnline {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--warn);
    font-size: var(--fs-sm);
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .logs {
    max-height: 240px;
    margin: 0 16px 10px;
    padding: 6px 0;
    overflow: auto;
    border-radius: var(--r);
    background: var(--sunken);
    font: 11.5px / 1.5 var(--font-mono);
  }
  .ll {
    display: flex;
    gap: 10px;
    padding: 0 10px;
  }
  .ll .ts {
    color: var(--fg-subtle);
  }
  .ll .st {
    width: 6ch;
    color: var(--fg-subtle);
  }
  .ll .tx {
    flex: 1;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .ll.stderr .tx {
    color: var(--danger);
  }
  .ll.system .tx {
    color: var(--fg-muted);
    font-style: italic;
  }
  .editor {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 16px 14px;
    border-top: 1px solid var(--border);
    background: var(--toolbar);
  }
  .editor .row {
    display: flex;
    gap: 8px;
  }
</style>
