<script lang="ts">
  // Read-only TypeScript view with line numbers; Prism is loaded on first use.
  let { code, label = 'Code', maxHeight = 320 }: { code: string; label?: string; maxHeight?: number } = $props();

  let html = $state<string | null>(null);
  const lines = $derived(code.split('\n').length - (code.endsWith('\n') ? 1 : 0));

  $effect(() => {
    const src = code;
    let live = true;
    html = null;
    void import('../../code/highlight')
      .then((m) => {
        if (live) html = m.highlightTs(src);
      })
      .catch(() => {
        // Plain text is fine.
      });
    return () => (live = false);
  });

  let copied = $state(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
      copied = true;
      setTimeout(() => (copied = false), 1200);
    } catch {
      // clipboard blocked: the text is selectable
    }
  }
</script>

<div class="code-view" style:max-height="{maxHeight}px">
  <button class="copy" onclick={copy} title="Copy to the clipboard">{copied ? 'Copied' : 'Copy'}</button>
  <div class="scroll" role="region" aria-label={label} tabindex="-1">
    <div class="gutter" aria-hidden="true">
      {#each { length: lines } as _, i (i)}<span>{i + 1}</span>{/each}
    </div>
    <!-- Prism escapes the source; the HTML only adds <span class="token …">. -->
    <pre class="src"><code>{#if html !== null}{@html html}{:else}{code}{/if}</code></pre>
  </div>
</div>

<style>
  .code-view {
    position: relative;
    display: flex;
    min-height: 60px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--sunken);
    overflow: hidden;
  }
  .scroll {
    display: flex;
    align-items: flex-start;
    flex: 1;
    min-width: 0;
    overflow: auto;
    outline: none;
  }
  .gutter {
    position: sticky;
    left: 0;
    display: flex;
    flex-direction: column;
    flex: none;
    padding: 8px 6px 8px 8px;
    background: var(--sunken);
    border-right: 1px solid var(--border);
    color: var(--fg-subtle);
    font: 11px / 17px var(--font-mono);
    text-align: right;
    user-select: none;
    opacity: 0.8;
  }
  .src {
    flex: 1;
    margin: 0;
    padding: 8px 10px;
    color: var(--fg);
    font: 11px / 17px var(--font-mono);
    tab-size: 2;
    user-select: text;
  }
  .copy {
    position: absolute;
    top: 6px;
    right: 8px;
    z-index: 1;
    height: 22px;
    padding: 0 8px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-sm);
    background: var(--surface);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    opacity: 0;
    transition: opacity 120ms;
  }
  .code-view:hover .copy,
  .copy:focus-visible {
    opacity: 1;
  }
  /* Prism tokens, on the app's palette (light + dark through the tokens). */
  .src :global(.token.comment) {
    color: var(--fg-subtle);
    font-style: italic;
  }
  .src :global(.token.string),
  .src :global(.token.template-string) {
    color: var(--g-green);
  }
  .src :global(.token.keyword),
  .src :global(.token.builtin) {
    color: var(--g-purple);
  }
  .src :global(.token.function) {
    color: var(--g-blue);
  }
  .src :global(.token.number),
  .src :global(.token.boolean) {
    color: var(--g-orange);
  }
  .src :global(.token.class-name) {
    color: var(--g-cyan);
  }
  .src :global(.token.regex) {
    color: var(--g-red);
  }
  .src :global(.token.operator),
  .src :global(.token.punctuation) {
    color: var(--fg-muted);
  }
</style>
