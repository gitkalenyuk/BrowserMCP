<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import Icon from '../lib/components/Icon.svelte';
  import { applyTheme, settings, type WidgetId } from '../lib/settings.svelte';
  import AgentsWidget from './widgets/AgentsWidget.svelte';
  import PromptWidget from './widgets/PromptWidget.svelte';
  import ScriptsWidget from './widgets/ScriptsWidget.svelte';
  import SitesWidget from './widgets/SitesWidget.svelte';

  let { api, embedded }: { api: BrowserApi; embedded: boolean } = $props();

  $effect(() => applyTheme(document.documentElement, settings.value));

  const WIDGETS: { id: WidgetId; label: string; hint: string }[] = [
    { id: 'prompt', label: 'Ask AI', hint: '“What should I do?” prompt' },
    { id: 'agents', label: 'Agents & runs', hint: 'Live agents, runs, patches to approve' },
    { id: 'scripts', label: 'Quick launch', hint: 'Your scripts, one click away' },
    { id: 'sites', label: 'Bookmarks & top sites', hint: 'Where you go most' },
  ];
  const w = $derived(settings.value.widgets);
  const none = $derived(!Object.values(w).some(Boolean));
  let customizing = $state(false);
  let root: HTMLDivElement;

  function toggle(id: WidgetId) {
    settings.update({ widgets: { ...settings.value.widgets, [id]: !settings.value.widgets[id] } });
  }

  /** Links: navigate this tab. Inside the dev chrome's iframe, ask the chrome instead. */
  function go(e: MouseEvent, url: string) {
    if (!embedded || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey) return;
    e.preventDefault();
    window.parent.postMessage({ type: 'bmcp:navigate', url }, location.origin);
  }
</script>

<svelte:window
  onpointerdown={(e) => customizing && !root.contains(e.target as Node) && (customizing = false)}
  onkeydown={(e) => customizing && e.key === 'Escape' && (customizing = false)}
/>

<div class="page">
  <div class="top" bind:this={root}>
    <button class="btn ghost" aria-expanded={customizing} aria-haspopup="dialog" onclick={() => (customizing = !customizing)}>
      <Icon name="settings" size={14} />
      Customize
    </button>
    {#if customizing}
      <div class="pop" role="dialog" aria-label="Widgets">
        <h3>Widgets</h3>
        {#each WIDGETS as item (item.id)}
          <label class="toggle">
            <input type="checkbox" checked={w[item.id]} onchange={() => toggle(item.id)} />
            <span class="switch" aria-hidden="true"></span>
            <span class="lbl">
              <span>{item.label}</span>
              <span class="hint">{item.hint}</span>
            </span>
          </label>
        {/each}
      </div>
    {/if}
  </div>

  <main>
    {#if w.prompt}
      <PromptWidget {api} />
    {/if}

    {#if w.agents || w.scripts}
      <div class="row" class:single={!(w.agents && w.scripts)}>
        {#if w.agents}<AgentsWidget {api} />{/if}
        {#if w.scripts}<ScriptsWidget {api} />{/if}
      </div>
    {/if}

    {#if w.sites}
      <SitesWidget {api} {go} />
    {/if}

    {#if none}
      <div class="blank">
        <p>All widgets are hidden.</p>
        <button class="btn" onclick={() => (customizing = true)}>Choose widgets</button>
      </div>
    {/if}
  </main>
</div>

<style>
  :global(body) {
    background: var(--toolbar);
    overflow: auto;
  }
  .page {
    min-height: 100vh;
    padding: 0 24px 56px;
  }
  .top {
    position: relative;
    display: flex;
    justify-content: flex-end;
    padding: 12px 0;
  }
  .ghost {
    border-color: transparent;
    background: transparent;
    color: var(--fg-muted);
  }
  .ghost:hover {
    color: var(--fg);
  }
  .pop {
    position: absolute;
    z-index: 10;
    top: calc(100% - 4px);
    right: 0;
    width: 300px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--shadow-pop);
  }
  .pop h3 {
    margin: 0 0 6px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 4px;
    border-radius: var(--r);
  }
  .toggle:hover {
    background: var(--hover);
  }
  .toggle input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .switch {
    position: relative;
    width: 28px;
    height: 16px;
    border-radius: 8px;
    background: var(--border-strong);
    flex: none;
    transition: background-color 140ms;
  }
  .switch::after {
    content: '';
    position: absolute;
    top: 2px;
    left: 2px;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.25);
    transition: transform 160ms var(--ease-out);
  }
  .toggle input:checked + .switch {
    background: var(--accent);
  }
  .toggle input:checked + .switch::after {
    transform: translateX(12px);
  }
  .toggle input:focus-visible + .switch {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .lbl {
    display: grid;
    font-size: var(--fs-sm);
    font-weight: 550;
  }
  .hint {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 400;
  }
  main {
    display: flex;
    flex-direction: column;
    gap: 36px;
    max-width: 920px;
    margin: 6vh auto 0;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1.5fr) minmax(0, 1fr);
    gap: 36px;
  }
  .row.single {
    grid-template-columns: minmax(0, 1fr);
  }
  @media (max-width: 760px) {
    .page {
      padding: 0 16px 40px;
    }
    .row {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .blank {
    display: grid;
    justify-items: center;
    gap: 10px;
    margin-top: 10vh;
    color: var(--fg-muted);
  }
  .blank p {
    margin: 0;
  }
</style>
