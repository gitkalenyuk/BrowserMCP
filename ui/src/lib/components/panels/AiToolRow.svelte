<script lang="ts">
  // One tool call of the AI chat: a collapsible row (state, name, argument preview) whose
  // body shows the arguments and the result, each with a Copy button.
  import { argsPreview, failureReason, pretty, summaryText, toolLabel, type ChatItem } from '../../ai/thread';
  import { formatTime } from '../../i18n/intl';
  import { t } from '../../i18n/index.svelte';
  import Icon from '../Icon.svelte';
  import AiCopyButton from './AiCopyButton.svelte';

  let { item, oncopyfail }: { item: Extract<ChatItem, { kind: 'tool' }>; oncopyfail?: (message: string) => void } = $props();

  const why = $derived(failureReason(item));
  const stateLabel = $derived(
    item.pending ? t('aiChat.tool.running') : item.ok ? t('aiChat.tool.ok') : item.ok === false ? t('aiChat.tool.failed') : t('aiChat.tool.noResult'),
  );
</script>

<details class="tool" class:failed={item.ok === false}>
  <summary title={item.ts ? formatTime(item.ts) : undefined}>
    <span class="state" role="img" aria-label={stateLabel}>
      {#if item.pending}
        <span class="spinner"></span>
      {:else if item.ok}
        <Icon name="check" size={13} stroke={2.25} />
      {:else if item.ok === false}
        <Icon name="x" size={13} stroke={2.25} />
      {:else}
        <span class="dash">–</span>
      {/if}
    </span>
    <span class="name">{toolLabel(item.name)}</span>
    <span class="args">{argsPreview(item.args, 60)}</span>
    <Icon name="chevron-right" size={12} class="chev" />
  </summary>
  <div class="tool-body">
    {#if item.args !== null && argsPreview(item.args)}
      <div class="k">
        <span>{t('aiChat.tool.args')}</span>
        <AiCopyButton text={pretty(item.args)} label={t('aiChat.tool.copyArgs')} onfail={oncopyfail} />
      </div>
      <pre>{pretty(item.args)}</pre>
    {/if}
    {#if item.summary !== null}
      <div class="k">
        <span>{item.ok ? t('aiChat.tool.result') : t('aiChat.tool.error')}</span>
        <AiCopyButton text={summaryText(item.summary)} label={t('aiChat.tool.copyResult')} onfail={oncopyfail} />
      </div>
      <pre>{summaryText(item.summary)}</pre>
    {:else if item.pending}
      <div class="k">{t('aiChat.tool.waiting')}</div>
    {/if}
  </div>
</details>
{#if why}<div class="why" title={why}>{why}</div>{/if}

<style>
  .tool {
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    font-size: var(--fs-xs);
  }
  .tool summary {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 28px;
    padding: 0 8px;
    list-style: none;
    cursor: default;
  }
  .tool summary::-webkit-details-marker {
    display: none;
  }
  .tool summary:hover {
    background: var(--hover);
  }
  .tool :global(.chev) {
    flex: none;
    color: var(--fg-subtle);
    transition: transform 120ms;
  }
  .tool[open] :global(.chev) {
    transform: rotate(90deg);
  }
  .state {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    flex: none;
    color: var(--ok);
  }
  .tool.failed .state {
    color: var(--danger);
  }
  .dash {
    color: var(--fg-subtle);
  }
  .spinner {
    width: 11px;
    height: 11px;
    border: 2px solid var(--accent-soft);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .name {
    flex: none;
    font-family: var(--font-mono);
    font-weight: 600;
    color: var(--fg);
  }
  .args {
    flex: 1;
    min-width: 0;
    color: var(--fg-subtle);
    font-family: var(--font-mono);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tool-body {
    padding: 4px 9px 9px;
    border-top: 1px solid var(--border);
    user-select: text;
  }
  .k {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 6px 0 3px;
    color: var(--fg-subtle);
    font-weight: 600;
  }
  pre {
    margin: 0;
    max-height: 220px;
    padding: 6px 8px;
    overflow: auto;
    border-radius: var(--r-sm);
    background: var(--sunken);
    font: var(--fs-xs) / 1.45 var(--font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .why {
    margin: -5px 0 0 26px;
    color: var(--danger);
    font-size: var(--fs-xs);
    line-height: 1.4;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
</style>
