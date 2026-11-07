<script lang="ts">
  // An assistant answer as Markdown. The tree comes from lib/ai/markdown.ts and is rendered
  // with ordinary elements: no {@html}, so nothing the model writes can run. Links never
  // navigate the panel itself: a click hands the address to `onlink` (a new browser tab).
  import { parseMarkdown, type Block, type Inline } from '../../ai/markdown';
  import { t } from '../../i18n/index.svelte';
  import AiCopyButton from './AiCopyButton.svelte';

  let {
    text,
    onlink,
    oncopyfail,
  }: { text: string; onlink: (href: string) => void; oncopyfail?: (message: string) => void } = $props();

  const blocks = $derived(parseMarkdown(text));

  function open(e: MouseEvent, href: string) {
    e.preventDefault();
    if (e.type === 'auxclick' && e.button !== 1) return;
    onlink(href);
  }
</script>

{#snippet inline(nodes: Inline[])}
  {#each nodes as n, i (i)}
    {#if n.t === 'text'}{n.v}{:else if n.t === 'code'}<code>{n.v}</code>{:else if n.t === 'strong'}<strong
        >{@render inline(n.c)}</strong
      >{:else if n.t === 'em'}<em>{@render inline(n.c)}</em>{:else if n.t === 'del'}<del>{@render inline(n.c)}</del
      >{:else if n.t === 'link'}<a
        href={n.href}
        target="_blank"
        rel="noopener noreferrer"
        title={n.href}
        onclick={(e) => open(e, n.href)}
        onauxclick={(e) => open(e, n.href)}>{@render inline(n.c)}</a
      >{:else if n.t === 'br'}<br />{/if}
  {/each}
{/snippet}

{#snippet render(list: Block[])}
  {#each list as b, i (i)}
    {#if b.t === 'p'}
      <p>{@render inline(b.c)}</p>
    {:else if b.t === 'h'}
      <svelte:element this={`h${Math.min(b.level + 2, 6)}`} class="h h{b.level}">{@render inline(b.c)}</svelte:element>
    {:else if b.t === 'code'}
      <figure class="codeblock">
        <figcaption>
          <span class="lang">{b.lang || t('aiChat.code.label')}</span>
          <AiCopyButton text={b.text} label={t('aiChat.code.copy')} withLabel onfail={oncopyfail} />
        </figcaption>
        <pre><code>{b.text}</code></pre>
      </figure>
    {:else if b.t === 'list'}
      {#if b.ordered}
        <ol start={b.start}>
          {#each b.items as item, k (k)}<li>{@render render(item)}</li>{/each}
        </ol>
      {:else}
        <ul>
          {#each b.items as item, k (k)}<li>{@render render(item)}</li>{/each}
        </ul>
      {/if}
    {:else if b.t === 'quote'}
      <blockquote>{@render render(b.c)}</blockquote>
    {:else if b.t === 'hr'}
      <hr />
    {:else if b.t === 'table'}
      <div class="table">
        <table>
          <thead>
            <tr>
              {#each b.head as cell, k (k)}<th style:text-align={b.align[k]}>{@render inline(cell)}</th>{/each}
            </tr>
          </thead>
          <tbody>
            {#each b.rows as row, r (r)}
              <tr>
                {#each row as cell, k (k)}<td style:text-align={b.align[k]}>{@render inline(cell)}</td>{/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/each}
{/snippet}

<div class="md">{@render render(blocks)}</div>

<style>
  .md {
    font-size: var(--fs-sm);
    line-height: 1.55;
    overflow-wrap: anywhere;
  }
  .md > :global(:first-child) {
    margin-top: 0;
  }
  .md > :global(:last-child) {
    margin-bottom: 0;
  }
  p {
    margin: 0 0 8px;
  }
  .h {
    margin: 12px 0 6px;
    font-weight: 650;
    line-height: 1.3;
  }
  .h1 {
    font-size: var(--fs-lg);
  }
  .h2 {
    font-size: calc(var(--fs) + 1px);
  }
  .h3,
  .h4,
  .h5,
  .h6 {
    font-size: var(--fs);
  }
  ul,
  ol {
    margin: 0 0 8px;
    padding-left: 20px;
  }
  li > :global(p) {
    margin: 0 0 2px;
  }
  li > :global(ul),
  li > :global(ol) {
    margin-bottom: 2px;
  }
  blockquote {
    margin: 0 0 8px;
    padding: 2px 0 2px 10px;
    border-left: 3px solid var(--border-strong);
    color: var(--fg-muted);
  }
  hr {
    margin: 10px 0;
    border: 0;
    border-top: 1px solid var(--border);
  }
  a {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  code {
    padding: 0 4px;
    border-radius: var(--r-sm);
    background: var(--sunken);
    font: 0.92em / 1.4 var(--font-mono);
  }
  .codeblock {
    margin: 0 0 8px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--sunken);
    overflow: hidden;
  }
  figcaption {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    height: 26px;
    padding: 0 3px 0 9px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .lang {
    font-family: var(--font-mono);
  }
  pre {
    margin: 0;
    max-height: 360px;
    padding: 8px 10px;
    overflow: auto;
    font: var(--fs-xs) / 1.5 var(--font-mono);
    white-space: pre;
  }
  pre code {
    padding: 0;
    background: none;
    font: inherit;
  }
  .table {
    margin: 0 0 8px;
    overflow-x: auto;
    border: 1px solid var(--border);
    border-radius: var(--r);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--fs-xs);
  }
  th,
  td {
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    text-align: left;
    vertical-align: top;
  }
  th {
    background: var(--sunken);
    font-weight: 650;
  }
  tr:last-child td {
    border-bottom: 0;
  }
</style>
