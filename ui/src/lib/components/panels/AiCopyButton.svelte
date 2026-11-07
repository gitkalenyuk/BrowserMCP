<script lang="ts">
  // A small icon button of the AI panel that copies text and shows a check for a moment.
  import { copyText } from '../../ai/clipboard';
  import { t } from '../../i18n/index.svelte';
  import Icon from '../Icon.svelte';

  let {
    text,
    label,
    withLabel = false,
    onfail,
  }: {
    /** The text, or a function that makes it when clicked. */
    text: string | (() => string | Promise<string>);
    label: string;
    /** Show the label next to the icon. */
    withLabel?: boolean;
    onfail?: (message: string) => void;
  } = $props();

  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy(e: MouseEvent) {
    e.stopPropagation();
    const value = typeof text === 'function' ? await text() : text;
    if (await copyText(value)) {
      copied = true;
      clearTimeout(timer);
      timer = setTimeout(() => (copied = false), 1400);
    } else onfail?.(t('aiChat.msg.copyFailed'));
  }
</script>

<button class="copy" class:labelled={withLabel} type="button" title={label} aria-label={withLabel ? undefined : label} onclick={copy}>
  <Icon name={copied ? 'check' : 'copy'} size={12} />
  {#if withLabel}<span>{copied ? t('aiChat.msg.copied') : label}</span>{/if}
  <span class="sr-only" aria-live="polite">{copied ? t('aiChat.msg.copied') : ''}</span>
</button>

<style>
  .copy {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    min-width: 22px;
    height: 22px;
    padding: 0 4px;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .copy:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .copy.labelled {
    padding: 0 6px;
  }
</style>
