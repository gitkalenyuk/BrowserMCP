<script lang="ts">
  // Guide text with its inline marks (`code`, **bold**, [[key]]) and search matches marked.
  // Everything is rendered as text nodes: content never becomes HTML.
  import { highlight, parseInline } from './inline';

  let { text, words = [] }: { text: string; words?: string[] } = $props();

  const spans = $derived(parseInline(text).map((s) => ({ ...s, pieces: highlight(s.text, words) })));
</script>

{#snippet pieces(list: { text: string; hit: boolean }[])}{#each list as piece, i (i)}{#if piece.hit}<mark>{piece.text}</mark>{:else}{piece.text}{/if}{/each}{/snippet}

{#each spans as s, i (i)}{#if s.mark === 'code'}<code>{@render pieces(s.pieces)}</code>{:else if s.mark === 'strong'}<strong>{@render pieces(s.pieces)}</strong>{:else if s.mark === 'kbd'}<kbd>{@render pieces(s.pieces)}</kbd>{:else}{@render pieces(s.pieces)}{/if}{/each}
