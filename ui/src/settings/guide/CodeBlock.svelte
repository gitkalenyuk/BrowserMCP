<script lang="ts">
  import { t } from '../../lib/i18n/index.svelte';
  import CopyButton from './CopyButton.svelte';
  import { highlight } from './inline';

  let {
    code,
    lang = 'text',
    words = [],
    onfail,
  }: { code: string; lang?: string; words?: string[]; onfail: (message: string) => void } = $props();

  const pieces = $derived(highlight(code, words));
</script>

<figure class="code">
  <figcaption>
    <span class="lang">{lang === 'text' ? t('guide.code.label') : lang}</span>
    <CopyButton text={code} {onfail} />
  </figcaption>
  <pre tabindex="-1"><code>{#each pieces as piece, i (i)}{#if piece.hit}<mark>{piece.text}</mark>{:else}{piece.text}{/if}{/each}</code></pre>
</figure>

<style>
  .code {
    margin: 10px 0;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--sunken);
    overflow: hidden;
  }
  figcaption {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 6px 4px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
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
