<script lang="ts">
  // Guide text with its inline marks (`code`, **bold**, [[key]], [[cmd:<id>]]) and search matches
  // marked. Everything is rendered as text nodes: content never becomes HTML.
  import { t } from '../../lib/i18n/index.svelte';
  import { commandKey, highlight, parseInline, type Mark } from './inline';
  import { useKeymap } from './keymap-context';

  let { text, words = [] }: { text: string; words?: string[] } = $props();

  const keymap = useKeymap();

  /** A `cmd` mark shows the command's current key (or says it has none). */
  function shown(mark: Mark, value: string): { mark: Mark | 'unbound'; text: string } {
    if (mark !== 'cmd') return { mark, text: value };
    const key = commandKey(value, keymap());
    return key ? { mark: 'kbd', text: key } : { mark: 'unbound', text: t('guide.keys.unbound') };
  }

  const spans = $derived(
    parseInline(text).map((s) => {
      const v = shown(s.mark, s.text);
      return { ...v, pieces: highlight(v.text, words) };
    }),
  );
</script>

{#snippet pieces(list: { text: string; hit: boolean }[])}{#each list as piece, i (i)}{#if piece.hit}<mark>{piece.text}</mark>{:else}{piece.text}{/if}{/each}{/snippet}

{#each spans as s, i (i)}{#if s.mark === 'code'}<code>{@render pieces(s.pieces)}</code>{:else if s.mark === 'strong'}<strong>{@render pieces(s.pieces)}</strong>{:else if s.mark === 'kbd'}<kbd>{@render pieces(s.pieces)}</kbd>{:else if s.mark === 'unbound'}<em>{@render pieces(s.pieces)}</em>{:else}{@render pieces(s.pieces)}{/if}{/each}
