<script lang="ts">
  import { load, save } from '../../storage';
  import { PROVIDERS } from '../../providers';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  const chrome = useChrome();
  let provider = $state(load('ai.provider', PROVIDERS[0].id));
  let draft = $state('');
  let messages = $state<{ role: 'user' | 'note'; text: string }[]>([]);
  const current = $derived(PROVIDERS.find((p) => p.id === provider) ?? PROVIDERS[0]);
  const suggestions = [
    'Summarize this page',
    'Find the pricing table and extract it as CSV',
    'Fill this form with my test data',
  ];

  async function send(text = draft) {
    const t = text.trim();
    if (!t) return;
    messages = [...messages, { role: 'user', text: t }];
    draft = '';
    try {
      await chrome.api.call('ai.prompt', { text: t, provider });
      messages = [...messages, { role: 'note', text: `${current.name} is not connected yet — chat arrives in phase 5.` }];
    } catch (err) {
      messages = [...messages, { role: 'note', text: `Could not reach ${current.name}: ${(err as Error).message}` }];
    }
  }
</script>

<div class="ai">
  <div class="bar">
    <label class="select">
      <span class="sr-only">Provider</span>
      <select bind:value={provider} onchange={() => save('ai.provider', provider)}>
        {#each PROVIDERS as p (p.id)}
          <option value={p.id}>{p.name}</option>
        {/each}
      </select>
      <Icon name="chevron-down" size={12} stroke={2.25} />
    </label>
    <span class="via">{current.via}</span>
  </div>

  <div class="log" aria-live="polite">
    {#if messages.length === 0}
      <div class="intro">
        <span class="glyph"><Icon name="sparkles" size={20} /></span>
        <h3>Ask {current.name} about this tab</h3>
        <p>It sees the page as an accessibility snapshot and acts through the same API you do. Tabs it drives join its colored group.</p>
        <div class="chips">
          {#each suggestions as s (s)}
            <button class="chip" onclick={() => send(s)}>{s}</button>
          {/each}
        </div>
      </div>
    {:else}
      {#each messages as m, i (i)}
        <div class="msg {m.role}">{m.text}</div>
      {/each}
    {/if}
  </div>

  <form
    class="composer"
    onsubmit={(e) => {
      e.preventDefault();
      void send();
    }}
  >
    <textarea
      bind:value={draft}
      rows="2"
      placeholder="Message {current.name}…"
      aria-label="Message"
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault();
          void send();
        }
      }}
    ></textarea>
    <button class="icon-btn send" type="submit" aria-label="Send" disabled={!draft.trim()}>
      <Icon name="send" size={16} stroke={2} />
    </button>
  </form>
</div>

<style>
  .ai {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border);
  }
  .select {
    position: relative;
    display: inline-flex;
    align-items: center;
    color: var(--fg-muted);
  }
  .select :global(svg) {
    position: absolute;
    right: 8px;
    pointer-events: none;
  }
  select {
    appearance: none;
    height: 28px;
    padding: 0 26px 0 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .via {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .log {
    display: flex;
    flex-direction: column;
    gap: 8px;
    flex: 1;
    min-height: 0;
    padding: 12px;
    overflow-y: auto;
    user-select: text;
  }
  .intro {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    margin: auto 0;
    padding: 12px 8px;
    text-align: center;
  }
  .glyph {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 12px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  h3 {
    margin: 4px 0 0;
    font-size: var(--fs);
    font-weight: 650;
  }
  p {
    max-width: 36ch;
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    line-height: 1.5;
  }
  .chips {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 5px;
    width: 100%;
    margin-top: 10px;
  }
  .chip {
    padding: 7px 10px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
    font-size: var(--fs-sm);
    text-align: left;
  }
  .chip:hover {
    border-color: var(--border-strong);
    background: var(--hover);
  }
  .msg {
    max-width: 90%;
    padding: 7px 10px;
    border-radius: var(--r-lg);
    font-size: var(--fs-sm);
    line-height: 1.45;
    white-space: pre-wrap;
  }
  .msg.user {
    align-self: flex-end;
    background: var(--accent);
    color: var(--on-accent);
  }
  .msg.note {
    align-self: flex-start;
    background: var(--sunken);
    color: var(--fg-muted);
  }
  .composer {
    position: relative;
    margin: 0 10px 10px;
  }
  textarea {
    display: block;
    width: 100%;
    min-height: 60px;
    max-height: 200px;
    padding: 9px 44px 9px 11px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    background: var(--surface);
    resize: vertical;
    font-size: var(--fs);
    user-select: text;
  }
  textarea:focus {
    outline: none;
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .send {
    position: absolute;
    right: 6px;
    bottom: 6px;
    background: var(--accent);
    color: var(--on-accent);
  }
  .send:hover:not(:disabled) {
    background: color-mix(in oklab, var(--accent) 88%, var(--fg));
    color: var(--on-accent);
  }
  .send:disabled {
    background: var(--sunken);
    color: var(--fg-subtle);
    opacity: 1;
  }
</style>
