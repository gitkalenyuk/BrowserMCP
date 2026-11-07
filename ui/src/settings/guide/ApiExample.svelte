<script lang="ts">
  // One command shown as an MCP tool call, a PowerShell / curl request and an SDK call.
  import { t } from '../../lib/i18n/index.svelte';
  import CopyButton from './CopyButton.svelte';
  import InlineText from './InlineText.svelte';
  import { highlight } from './inline';
  import { apiPref, setApiFlavor } from './prefs.svelte';
  import { API_FLAVORS, snippet, type ApiFlavor } from './snippets';

  let {
    command,
    args,
    text,
    words = [],
    onfail,
  }: {
    command: string;
    args: Record<string, unknown>;
    text?: string;
    words?: string[];
    onfail: (message: string) => void;
  } = $props();

  const LABEL_KEYS: Record<ApiFlavor, string> = {
    mcp: 'guide.api.mcp',
    powershell: 'guide.api.powershell',
    curl: 'guide.api.curl',
    sdk: 'guide.api.sdk',
  };
  const uid = $props.id();
  const code = $derived(snippet(apiPref.flavor, command, args));
  const pieces = $derived(highlight(code, words));

  function onTabKey(e: KeyboardEvent) {
    if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
    e.preventDefault();
    const i = API_FLAVORS.indexOf(apiPref.flavor);
    const next = API_FLAVORS[(i + (e.key === 'ArrowRight' ? 1 : API_FLAVORS.length - 1)) % API_FLAVORS.length];
    setApiFlavor(next);
    document.getElementById(`${uid}-${next}`)?.focus();
  }
</script>

<figure class="api">
  {#if text}<p class="note"><InlineText {text} {words} /></p>{/if}
  <div class="bar">
    <div class="tabs" role="tablist" aria-label={t('guide.api.label')} tabindex="-1" onkeydown={onTabKey}>
      {#each API_FLAVORS as f (f)}
        <button
          id="{uid}-{f}"
          role="tab"
          aria-selected={apiPref.flavor === f}
          aria-controls="{uid}-code"
          tabindex={apiPref.flavor === f ? 0 : -1}
          onclick={() => setApiFlavor(f)}>{t(LABEL_KEYS[f])}</button
        >
      {/each}
    </div>
    <CopyButton text={code} {onfail} />
  </div>
  <div id="{uid}-code" role="tabpanel" aria-labelledby="{uid}-{apiPref.flavor}">
    <pre><code>{#each pieces as piece, i (i)}{#if piece.hit}<mark>{piece.text}</mark>{:else}{piece.text}{/if}{/each}</code></pre>
  </div>
</figure>

<style>
  .api {
    margin: 10px 0;
    border: 1px solid var(--accent-line);
    border-radius: var(--r);
    background: var(--sunken);
    overflow: hidden;
  }
  .note {
    margin: 0;
    padding: 8px 12px 0;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
  }
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 6px;
    border-bottom: 1px solid var(--border);
  }
  .tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
  }
  .tabs button {
    height: 24px;
    padding: 0 9px;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .tabs button:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .tabs button[aria-selected='true'] {
    background: var(--accent-soft);
    color: var(--accent);
  }
  pre {
    margin: 0;
    padding: 10px 12px;
    overflow-x: auto;
    font: 12px / 1.55 var(--font-mono);
    white-space: pre;
    user-select: text;
  }
</style>
