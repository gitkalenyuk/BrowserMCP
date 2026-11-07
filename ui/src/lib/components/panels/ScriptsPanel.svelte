<script lang="ts">
  // Scripts panel: library (script.list/save/delete), Monaco editor with IntelliSense from the
  // generated browser.d.ts, Run with an inputs form (script.run), and the step debugger
  // (script.debug_start/debug_cmd + script.debug_* events). State lives in scripts/store.
  import { t, tp } from '../../i18n/index.svelte';
  import { formatDate, relativeTime } from '../../i18n/intl';
  import { settings } from '../../settings.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import { freeName } from '../../scripts/logic';
  import type { EditorKey, ScriptEditor } from '../../scripts/monaco';
  import { scriptsStore } from '../../scripts/store.svelte';
  import Icon from '../Icon.svelte';
  import DebugVariables from './DebugVariables.svelte';
  import MonacoEditor from './MonacoEditor.svelte';
  import ScriptConsole from './ScriptConsole.svelte';

  const store = scriptsStore(useChrome().api);
  let editor: ScriptEditor | null = null;

  /** Library row being renamed, or "" for a new script. */
  let naming = $state<{ from: string | null; value: string } | null>(null);
  let confirmDelete = $state<string | null>(null);
  let bottomTab = $state<'console' | 'variables'>('console');
  let showInputs = $state(true);
  let libraryOpen = $state(true);

  $effect(() => {
    store.start();
    if (!store.loaded) void store.refresh();
  });

  const debugging = $derived(store.debug !== null);
  const paused = $derived(store.debug?.status === 'paused' ? store.debug.paused : null);
  // Paused: show the variables; finished: back to the console.
  $effect(() => {
    if (paused) bottomTab = 'variables';
  });
  $effect(() => {
    if (!debugging) bottomTab = 'console';
  });

  const markedLine = $derived(paused?.line ?? store.errorLine);
  const theme = $derived(settings.effectiveTheme === 'dark' ? 'dark' : 'light');

  function jump(line: number) {
    editor?.revealLine(line);
    editor?.focus();
  }

  function onKey(key: EditorKey) {
    switch (key) {
      case 'save':
        return void store.save();
      case 'run':
        return void store.run();
      case 'continue':
        return void store.continueOrStart();
      case 'stop':
        return void store.debugCmd('stop');
      case 'stepOver':
        return void (paused && store.debugCmd('step_over'));
      case 'stepInto':
        return void (paused && store.debugCmd('step_into'));
      case 'stepOut':
        return void (paused && store.debugCmd('step_out'));
      case 'toggleBreakpoint':
        return;
    }
  }

  // The same keys when focus is elsewhere in the panel (Monaco handles them itself inside).
  function onPanelKey(e: KeyboardEvent) {
    if (e.defaultPrevented || e.isComposing) return;
    const key: EditorKey | null =
      e.ctrlKey && !e.shiftKey && e.code === 'KeyS'
        ? 'save'
        : e.key === 'F5'
          ? e.shiftKey
            ? 'stop'
            : 'continue'
          : e.key === 'F10'
            ? 'stepOver'
            : e.key === 'F11'
              ? e.shiftKey
                ? 'stepOut'
                : 'stepInto'
              : null;
    if (!key) return;
    // Text fields (inputs form, rename) keep Ctrl+S etc. out of the library actions.
    e.preventDefault();
    onKey(key);
  }

  function startNew() {
    confirmDelete = null;
    naming = { from: null, value: freeName('script', store.scripts.map((s) => s.name)) };
  }

  async function commitName() {
    if (!naming) return;
    const { from, value } = naming;
    const name = value.trim();
    if (!name) {
      naming = null;
      return;
    }
    const ok = from === null ? await store.create(name) : await store.rename(from, name);
    if (ok) naming = null;
  }

  function nameKey(e: KeyboardEvent) {
    if (e.key === 'Enter') void commitName();
    else if (e.key === 'Escape') naming = null;
  }

  async function remove(name: string) {
    if (confirmDelete !== name) {
      confirmDelete = name;
      return;
    }
    confirmDelete = null;
    await store.remove(name);
  }

  /** "5 min ago" within a day, else the date (in the UI language). */
  function ago(ms: number): string {
    const now = Date.now();
    if (now - ms < 86_400_000) return relativeTime(Math.min(ms, now), now);
    return formatDate(ms, { day: 'numeric', month: 'short', year: 'numeric' });
  }

  function focusOnMount(el: HTMLInputElement) {
    el.focus();
    el.select();
  }
</script>

<!-- data-own-keys: the chrome leaves F5/F9/F10/F11/Ctrl+S to this panel (see scripts/logic.ts). -->
<div class="scripts" data-own-keys role="presentation" onkeydown={onPanelKey}>
  <section class="library" class:open={libraryOpen}>
    <header>
      <button class="lib-toggle" onclick={() => (libraryOpen = !libraryOpen)} aria-expanded={libraryOpen}>
        <Icon name={libraryOpen ? 'chevron-down' : 'chevron-right'} size={13} stroke={2.25} />
        {t('scripts.library')} <span class="count">{store.scripts.length}</span>
      </button>
      <button class="icon-btn small" title={t('scripts.new')} aria-label={t('scripts.new')} onclick={startNew}>
        <Icon name="plus" size={14} stroke={2.25} />
      </button>
    </header>
    {#if libraryOpen}
      <ul class="list" aria-label={t('scripts.saved')}>
        {#if naming && naming.from === null}
          <li class="row naming">
            <input
              class="name-input"
              aria-label={t('scripts.newName')}
              bind:value={naming.value}
              onkeydown={nameKey}
              onblur={() => void commitName()}
              use:focusOnMount
            />
          </li>
        {/if}
        {#each store.scripts as s (s.name)}
          <li class="row" class:active={s.name === store.current}>
            {#if naming && naming.from === s.name}
              <input
                class="name-input"
                aria-label={t('scripts.renameLabel', { name: s.name })}
                bind:value={naming.value}
                onkeydown={nameKey}
                onblur={() => void commitName()}
                use:focusOnMount
              />
            {:else}
              <button class="pick" onclick={() => void store.open(s.name)} title="{s.name}.ts · {tp('scripts.bytes', s.size)}">
                <Icon name="code" size={13} stroke={2} />
                <span class="name">{s.name}</span>
                {#if store.isDirty(s.name)}<span class="dot" title={t('scripts.unsavedChanges')}></span>{/if}
                <span class="when">{ago(s.modified_ms)}</span>
              </button>
              <button class="icon-btn small act" title={t('scripts.rename')} aria-label={t('scripts.renameLabel', { name: s.name })} onclick={() => (naming = { from: s.name, value: s.name })}>
                <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16zM14 6l4 4" /></svg>
              </button>
              <button
                class="icon-btn small act"
                class:danger={confirmDelete === s.name}
                title={confirmDelete === s.name ? t('scripts.confirmDelete') : t('common.delete')}
                aria-label={t('scripts.deleteLabel', { name: s.name })}
                onclick={() => void remove(s.name)}
                onblur={() => confirmDelete === s.name && (confirmDelete = null)}
              >
                <Icon name="trash" size={13} stroke={2} />
              </button>
            {/if}
          </li>
        {:else}
          {#if store.loaded && !naming}
            <li class="empty-lib">{t('scripts.none')} <button class="link" onclick={startNew}>{t('scripts.createOne')}</button></li>
          {/if}
        {/each}
      </ul>
    {/if}
  </section>

  {#if store.error}
    <div class="error-bar" role="alert">
      <Icon name="alert" size={13} stroke={2} />
      <span>{store.error}</span>
      <button class="icon-btn small" aria-label={t('chrome.dismiss')} onclick={() => (store.error = null)}><Icon name="x" size={12} /></button>
    </div>
  {/if}

  {#if store.current !== null}
    <div class="toolbar" role="toolbar" aria-label={t('scripts.actions')}>
      <span class="title" title="{store.current}.ts">{store.current}<span class="ext">.ts</span>{#if store.dirty}<span class="dot" title={t('scripts.unsaved')}></span>{/if}</span>
      <button class="icon-btn small" title="{t('common.save')} (Ctrl+S)" aria-label={t('common.save')} disabled={!store.dirty} onclick={() => void store.save()}>
        <Icon name="download" size={14} stroke={2} />
      </button>
      {#if !debugging}
        <button class="btn primary run" disabled={store.running} onclick={() => void store.run()} title={t('scripts.runHint')}>
          {#if store.running}<span class="spinner" aria-hidden="true"></span>{:else}<Icon name="play" size={12} stroke={2.25} />{/if}
          {t('scripts.run')}
        </button>
        <button class="btn" disabled={store.running} onclick={() => {
          // Keys (F10/F11/F5) go to the editor right away.
          editor?.focus();
          void store.debugStart();
        }} title="{t('scripts.debugHint')} (F5)">
          <Icon name="breakpoint" size={12} stroke={2.25} /> {t('scripts.debug')}
        </button>
      {:else}
        <div class="dbg" role="group" aria-label={t('scripts.op.debugger')}>
          <span class="state" class:paused={!!paused}>{store.debug ? t(`scripts.debug.${store.debug.status}`) : ''}</span>
          <button class="icon-btn small" title="{t('scripts.continue')} (F5)" aria-label={t('scripts.continue')} disabled={!paused} onclick={() => void store.debugCmd('continue')}>
            <Icon name="play" size={13} stroke={2.25} />
          </button>
          <button class="icon-btn small" title={t('runs.detail.pause')} aria-label={t('runs.detail.pause')} disabled={!!paused || store.debug?.id === null} onclick={() => void store.debugCmd('pause')}>
            <Icon name="pause" size={13} stroke={2.5} />
          </button>
          <button class="icon-btn small" title="{t('scripts.stepOver')} (F10)" aria-label={t('scripts.stepOver')} disabled={!paused} onclick={() => void store.debugCmd('step_over')}>
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 13a8 8 0 0 1 15-3.5M19 4v5.5h-5.5" /><circle cx="12" cy="19" r="2" /></svg>
          </button>
          <button class="icon-btn small" title="{t('scripts.stepInto')} (F11)" aria-label={t('scripts.stepInto')} disabled={!paused} onclick={() => void store.debugCmd('step_into')}>
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3v11M7.5 9.5 12 14l4.5-4.5" /><circle cx="12" cy="19.5" r="2" /></svg>
          </button>
          <button class="icon-btn small" title="{t('scripts.stepOut')} (Shift+F11)" aria-label={t('scripts.stepOut')} disabled={!paused} onclick={() => void store.debugCmd('step_out')}>
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 15V4M7.5 8.5 12 4l4.5 4.5" /><circle cx="12" cy="19.5" r="2" /></svg>
          </button>
          <button class="icon-btn small stop" title="{t('record.stop')} (Shift+F5)" aria-label={t('record.stop')} disabled={store.debug?.id === null} onclick={() => void store.debugCmd('stop')}>
            <Icon name="stop" size={13} stroke={2.25} />
          </button>
        </div>
      {/if}
    </div>

    {#if store.inputNames.length > 0}
      <section class="inputs">
        <button class="inputs-toggle" onclick={() => (showInputs = !showInputs)} aria-expanded={showInputs}>
          <Icon name={showInputs ? 'chevron-down' : 'chevron-right'} size={12} stroke={2.25} />
          {t('scripts.inputs')} <span class="count">{store.inputNames.length}</span>
        </button>
        {#if showInputs}
          <div class="grid">
            {#each store.inputNames as name (name)}
              <label for="input-{name}">{name}</label>
              <input
                id="input-{name}"
                placeholder="inputs.{name}"
                value={store.inputValues[name] ?? ''}
                oninput={(e) => (store.inputValues = { ...store.inputValues, [name]: e.currentTarget.value })}
                type={/pass|secret|token/i.test(name) ? 'password' : 'text'}
                autocomplete="off"
                spellcheck="false"
              />
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    <div class="editor">
      <MonacoEditor
        path={store.current}
        value={store.code}
        {theme}
        breakpoints={store.currentBreakpoints}
        verified={store.verified}
        {markedLine}
        markKind={paused ? 'current' : 'error'}
        onchange={(v) => (store.code = v)}
        onbreakpoints={(lines) => void store.setBreakpoints(lines)}
        onkey={onKey}
        oneditor={(e) => (editor = e)}
      />
    </div>

    <section class="bottom">
      <div class="tabs" role="tablist">
        <button role="tab" aria-selected={bottomTab === 'console'} class:active={bottomTab === 'console'} onclick={() => (bottomTab = 'console')}>{t('scripts.console')}</button>
        <button role="tab" aria-selected={bottomTab === 'variables'} class:active={bottomTab === 'variables'} onclick={() => (bottomTab = 'variables')}>
          {t('scripts.variables')}{#if paused}<span class="live"></span>{/if}
        </button>
        <span class="spacer"></span>
        {#if bottomTab === 'console'}
          <button class="icon-btn small" title={t('scripts.clear')} aria-label={t('scripts.clearConsole')} onclick={() => store.clearConsole()}><Icon name="x" size={12} /></button>
        {/if}
      </div>
      {#if bottomTab === 'console'}
        <ScriptConsole entries={store.console} onjump={jump} />
      {:else}
        <DebugVariables {paused} onjump={jump} />
      {/if}
    </section>
  {:else if store.loaded}
    <div class="empty">
      <Icon name="code" size={20} />
      <h3>{t('scripts.noneOpen')}</h3>
      <p>{t('scripts.intro')}</p>
      <button class="btn primary" onclick={startNew}><Icon name="plus" size={12} stroke={2.5} /> {t('scripts.new')}</button>
    </div>
  {/if}
</div>

<style>
  .scripts {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    font-size: var(--fs);
  }
  .icon-btn.small {
    width: 26px;
    height: 26px;
  }
  .count {
    color: var(--fg-subtle);
    font-weight: 500;
    font-size: var(--fs-xs);
  }
  .dot {
    width: 7px;
    height: 7px;
    margin-left: 5px;
    border-radius: 50%;
    background: var(--accent);
    flex: none;
  }

  /* ---- library ---- */
  .library {
    flex: none;
    border-bottom: 1px solid var(--border);
  }
  .library header {
    display: flex;
    align-items: center;
    height: 32px;
    padding: 0 6px 0 4px;
  }
  .lib-toggle,
  .inputs-toggle {
    display: flex;
    align-items: center;
    gap: 5px;
    flex: 1;
    height: 26px;
    padding: 0 6px;
    border: 0;
    border-radius: var(--r-sm);
    background: none;
    color: var(--fg-muted);
    font: 650 var(--fs-xs) / 1 var(--font);
    letter-spacing: 0.04em;
    text-transform: uppercase;
    text-align: left;
  }
  .lib-toggle:hover,
  .inputs-toggle:hover {
    color: var(--fg);
  }
  .list {
    max-height: 26vh;
    margin: 0;
    padding: 0 6px 6px;
    overflow: auto;
    list-style: none;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 2px;
    border-radius: var(--r-sm);
  }
  .row:hover {
    background: var(--hover);
  }
  .row.active {
    background: var(--accent-soft);
  }
  .pick {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border: 0;
    background: none;
    color: var(--fg);
    text-align: left;
  }
  .row.active .pick {
    color: var(--accent);
    font-weight: 600;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .when {
    margin-left: auto;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 400;
    white-space: nowrap;
  }
  .act {
    opacity: 0;
  }
  .row:hover .act,
  .row.active .act,
  .act:focus-visible,
  .act.danger {
    opacity: 1;
  }
  .act.danger {
    color: var(--danger);
    background: color-mix(in oklab, var(--danger) 12%, transparent);
  }
  .naming {
    padding: 2px 0;
  }
  .name-input {
    flex: 1;
    height: 26px;
    margin: 1px 0;
    padding: 0 8px;
    border: 1px solid var(--accent-line);
    border-radius: var(--r-sm);
    background: var(--field-focus);
    color: var(--fg);
    font: inherit;
  }
  .empty-lib {
    padding: 6px 10px 8px;
    color: var(--fg-subtle);
  }
  .link {
    border: 0;
    padding: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }

  .error-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px 4px 12px;
    color: var(--danger);
    background: color-mix(in oklab, var(--danger) 10%, transparent);
    border-bottom: 1px solid var(--border);
    font-size: var(--fs-sm);
  }
  .error-bar span {
    flex: 1;
  }

  /* ---- toolbar ---- */
  .toolbar {
    display: flex;
    align-items: center;
    gap: 4px;
    min-height: 40px;
    padding: 4px 8px 4px 12px;
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .title {
    display: flex;
    align-items: center;
    flex: 1;
    min-width: 60px;
    overflow: hidden;
    font-weight: 650;
    white-space: nowrap;
  }
  .ext {
    color: var(--fg-subtle);
    font-weight: 400;
  }
  .btn.run,
  .toolbar .btn {
    height: 26px;
    padding: 0 9px;
  }
  .spinner {
    width: 10px;
    height: 10px;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .dbg {
    display: flex;
    align-items: center;
    gap: 1px;
    padding: 1px 2px 1px 8px;
    border-radius: var(--r);
    background: var(--sunken);
  }
  .state {
    margin-right: 4px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .state.paused {
    color: var(--warn);
  }
  .stop:not(:disabled) {
    color: var(--danger);
  }

  /* ---- inputs ---- */
  .inputs {
    flex: none;
    padding: 4px 8px 6px 4px;
    border-bottom: 1px solid var(--border);
  }
  .grid {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px 10px;
    align-items: center;
    padding: 4px 0 0 10px;
  }
  .grid label {
    color: var(--fg-muted);
    font: 12px var(--font-mono);
  }
  .grid input {
    height: 26px;
    min-width: 0;
    padding: 0 8px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--field);
    color: var(--fg);
    font: 12px var(--font-mono);
  }
  .grid input:focus {
    outline: none;
    border-color: var(--accent-line);
    background: var(--field-focus);
  }

  /* ---- editor + bottom ---- */
  .editor {
    display: flex;
    flex: 1;
    min-height: 120px;
    background: var(--surface);
  }
  .bottom {
    display: flex;
    flex-direction: column;
    flex: none;
    height: 34%;
    min-height: 120px;
    border-top: 1px solid var(--border);
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 30px;
    padding: 0 6px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .tabs [role='tab'] {
    position: relative;
    height: 26px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--r-sm);
    background: none;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .tabs [role='tab']:hover {
    color: var(--fg);
  }
  .tabs [role='tab'].active {
    color: var(--fg);
    background: var(--hover);
  }
  .live {
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-left: 6px;
    border-radius: 50%;
    background: var(--warn);
    vertical-align: 2px;
  }
  .spacer {
    flex: 1;
  }

  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    gap: 8px;
    padding: 24px;
    color: var(--fg-muted);
    text-align: center;
  }
  .empty h3 {
    margin: 4px 0 0;
    color: var(--fg);
    font-size: var(--fs-lg);
  }
  .empty p {
    max-width: 280px;
    margin: 0 0 6px;
    line-height: 1.5;
  }
</style>
