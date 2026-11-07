<script lang="ts">
  import { t } from '../../i18n/index.svelte';
  import { load, save } from '../../storage';
  import { settings } from '../../settings.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import XTermView from './XTermView.svelte';

  const terms = useChrome().terms;
  // Shell names: `term.shell.<id>` (Windows calls cmd "Command Prompt" / "Командний рядок").
  const SHELLS = ['pwsh', 'cmd', 'bash'];
  const shellName = (id: string) => (SHELLS.includes(id) ? t(`term.shell.${id}`) : id);
  let shell = $state(load('term.shell', 'pwsh'));
  let starting = $state(false);
  let ready = $state(false);

  async function open(s = shell) {
    if (starting) return;
    starting = true;
    try {
      await terms.open(s);
    } finally {
      starting = false;
    }
  }

  // Adopt terminals opened while the panel was closed (or by REST / agents); start one if none.
  // Only once per chrome session: closing the last terminal leaves the empty state.
  $effect(() => {
    void terms.refresh().then(() => {
      ready = true;
      if (terms.tabs.length === 0 && !terms.autoOpened) {
        terms.autoOpened = true;
        void open();
      }
    });
  });
</script>

<div class="term" class:dark={settings.effectiveTheme === 'dark'}>
  <div class="tabs" role="tablist" aria-label={t('term.list')}>
    {#each terms.tabs as tab (tab.id)}
      <div class="tab" class:active={terms.activeId === tab.id} class:exited={tab.exited}>
        <button
          class="pick"
          role="tab"
          aria-selected={terms.activeId === tab.id}
          title="{tab.shell} · {tab.id}{tab.exited ? ` · ${tab.exitCode === null ? t('term.exited') : t('term.exitedCode', { code: tab.exitCode })}` : ''}"
          onclick={() => (terms.activeId = tab.id)}
        >
          <Icon name="terminal" size={12} stroke={2} />
          <span>{tab.label}</span>
        </button>
        <button class="x" aria-label={t('term.closeLabel', { name: tab.label })} title={tab.exited ? t('term.remove') : t('term.closeHint')} onclick={() => terms.close(tab.id)}>
          <Icon name="x" size={11} stroke={2.25} />
        </button>
      </div>
    {/each}
    <div class="new">
      <button class="icon-btn add" aria-label={t('term.new')} title={t('term.newHint', { shell: shellName(shell) })} disabled={starting} onclick={() => open()}>
        <Icon name="plus" size={14} stroke={2} />
      </button>
      <label class="select" title={t('term.shellHint')}>
        <span class="sr-only">{t('term.shell')}</span>
        <select bind:value={shell} onchange={() => save('term.shell', shell)}>
          {#each SHELLS as s (s)}
            <option value={s}>{shellName(s)}{terms.unavailable.includes(s) ? ` (${t('term.notFound')})` : ''}</option>
          {/each}
        </select>
        <Icon name="chevron-down" size={11} stroke={2.25} />
      </label>
    </div>
  </div>

  <div class="screens">
    {#each terms.tabs as tab (tab.id)}
      <XTermView id={tab.id} store={terms} active={terms.activeId === tab.id} />
    {/each}
    {#if ready && terms.tabs.length === 0}
      <div class="empty">
        <Icon name="terminal" size={20} />
        <p>{t('term.empty')}</p>
        <div class="choices">
          {#each SHELLS as s (s)}
            <button class="btn" disabled={starting} onclick={() => open(s)}>{shellName(s)}</button>
          {/each}
        </div>
      </div>
    {/if}
  </div>

  {#if terms.error}
    <div class="error" role="alert">
      <span>{terms.error}</span>
      <button class="icon-btn" aria-label={t('chrome.dismiss')} onclick={() => (terms.error = null)}><Icon name="x" size={12} /></button>
    </div>
  {/if}
</div>

<style>
  .term {
    --term-bg: #ffffff;
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .term.dark {
    --term-bg: #0d0f12;
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 34px;
    padding: 0 4px 0 6px;
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
    scrollbar-width: none;
    flex: none;
  }
  .tab {
    display: flex;
    align-items: center;
    height: 26px;
    border-radius: var(--r);
    color: var(--fg-muted);
    flex: none;
  }
  .tab:hover {
    background: var(--hover);
  }
  .tab.active {
    background: var(--surface);
    color: var(--fg);
    box-shadow: var(--shadow-tab);
  }
  .tab.exited span {
    text-decoration: line-through;
    opacity: 0.7;
  }
  .pick {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 100%;
    padding: 0 4px 0 8px;
    border: 0;
    background: transparent;
    color: inherit;
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .x {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    margin-right: 4px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--fg-subtle);
  }
  .x:hover {
    background: var(--press);
    color: var(--fg);
  }
  .new {
    display: flex;
    align-items: center;
    gap: 0;
    margin-left: 2px;
    flex: none;
  }
  .add {
    width: 26px;
    height: 26px;
  }
  .select {
    position: relative;
    display: inline-flex;
    align-items: center;
    color: var(--fg-subtle);
  }
  .select :global(svg) {
    position: absolute;
    right: 4px;
    pointer-events: none;
  }
  select {
    appearance: none;
    height: 26px;
    padding: 0 18px 0 6px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  select:hover {
    background: var(--hover);
  }
  option {
    color: var(--fg);
    background: var(--surface);
  }
  .screens {
    position: relative;
    flex: 1;
    min-height: 0;
    background: var(--term-bg);
  }
  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 24px;
    color: var(--fg-muted);
    background: var(--toolbar);
    text-align: center;
  }
  .empty p {
    max-width: 32ch;
    margin: 0;
    font-size: var(--fs-sm);
    line-height: 1.5;
  }
  .choices {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 6px;
  }
  .error {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 4px 4px 10px;
    border-top: 1px solid var(--border);
    color: var(--danger);
    font-size: var(--fs-xs);
  }
  .error span {
    flex: 1;
  }
  .error .icon-btn {
    width: 22px;
    height: 22px;
    color: inherit;
  }
</style>
