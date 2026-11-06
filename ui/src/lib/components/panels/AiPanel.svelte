<script lang="ts">
  import { tick } from 'svelte';
  import type { AiSession } from '../../api';
  import { argsPreview, doneLabel, toolLabel } from '../../ai/thread';
  import { PROVIDERS, providerHint } from '../../providers';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import AiSettings from './AiSettings.svelte';

  const chrome = useChrome();
  const ai = chrome.ai;

  let view = $state<'chat' | 'settings'>('chat');
  let draft = $state('');
  let sending = $state(false);
  let error = $state<string | null>(null);
  let log = $state<HTMLDivElement>();
  let stick = true;

  const providerList = $derived(
    ai.providers.length ? ai.providers.map((p) => ({ id: p.id, name: p.name, configured: p.configured })) : PROVIDERS.map((p) => ({ ...p, configured: true })),
  );
  const session = $derived(ai.active);
  const items = $derived(session ? (ai.threads[session.id] ?? []) : []);
  const sessionProvider = $derived(session ? ai.providerName(session.provider) : null);
  const current = $derived(providerList.find((p) => p.id === ai.provider) ?? providerList[0]);
  const ended = $derived(session?.status === 'stopped' || session?.status === 'failed');
  const running = $derived(session?.status === 'running');
  const canSend = $derived(draft.trim() !== '' && !sending && (session && !ended ? true : current?.configured !== false));

  const suggestions = ['Summarize this page', 'Find the pricing table and extract it as CSV', 'Compare prices for this product in 3 stores'];

  $effect(() => {
    void ai.refreshProviders();
    void ai.refreshSessions();
  });

  // Keep the newest message in view while the user is at the bottom.
  $effect(() => {
    void items.length;
    void items[items.length - 1];
    if (!log || !stick) return;
    void tick().then(() => log && (log.scrollTop = log.scrollHeight));
  });

  function onScroll() {
    if (!log) return;
    stick = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  }

  async function send(text = draft) {
    if (!text.trim() || sending) return;
    sending = true;
    error = null;
    try {
      await ai.send(text);
      draft = '';
      stick = true;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      sending = false;
    }
  }

  async function stop() {
    if (!session) return;
    try {
      await ai.stopSession(session.id);
    } catch (err) {
      error = (err as Error).message;
    }
  }

  function newChat() {
    void ai.open(null);
    error = null;
  }

  function label(s: AiSession): string {
    const n = s.id.split('-').pop();
    return `${ai.providerName(s.provider)} ${n && /^\d+$/.test(n) ? `#${n}` : ''}`.trim();
  }

  function pretty(v: unknown): string {
    try {
      return JSON.stringify(v, null, 2);
    } catch {
      return String(v);
    }
  }
</script>

{#if view === 'settings'}
  <AiSettings onback={() => (view = 'chat')} />
{:else}
  <div class="ai">
    <div class="bar">
      {#if session}
        <span class="dot" style:background={session.color} aria-hidden="true"></span>
        <span class="title">{sessionProvider}</span>
        <span class="status {session.status}">{session.status}</span>
        {#if session.model}<span class="via">{session.model}</span>{/if}
      {:else}
        <label class="select">
          <span class="sr-only">Provider</span>
          <select value={ai.provider} onchange={(e) => ai.setProvider(e.currentTarget.value)}>
            {#each providerList as p (p.id)}
              <option value={p.id}>{p.name}{p.configured ? '' : ' — not set up'}</option>
            {/each}
          </select>
          <Icon name="chevron-down" size={12} stroke={2.25} />
        </label>
        <span class="via">{providerHint(ai.provider)}</span>
      {/if}
      <span class="spacer"></span>
      <button class="icon-btn" title="Providers and keys" aria-label="AI provider settings" onclick={() => (view = 'settings')}>
        <Icon name="settings" size={15} />
      </button>
    </div>

    {#if ai.sessions.length > 0}
      <div class="sessions" role="tablist" aria-label="AI sessions">
        <button class="sess new" role="tab" aria-selected={!session} onclick={newChat} title="New chat">
          <Icon name="plus" size={13} stroke={2} /> New
        </button>
        {#each ai.sessions as s (s.id)}
          <button
            class="sess"
            role="tab"
            aria-selected={session?.id === s.id}
            title="{s.id} · {s.status}{s.tabs.length ? ` · tabs ${s.tabs.join(', ')}` : ''}"
            onclick={() => ai.open(s.id)}
          >
            <span class="dot" class:pulse={s.status === 'running'} style:background={s.color}></span>
            {label(s)}
          </button>
        {/each}
      </div>
    {/if}

    <div class="log" bind:this={log} onscroll={onScroll} aria-live="polite">
      {#if !session}
        <div class="intro">
          <span class="glyph"><Icon name="sparkles" size={20} /></span>
          <h3>Ask {current?.name ?? 'an agent'} to do something</h3>
          <p>It drives the browser through the same API you do. Tabs it opens join its own colored group, locked until you take over.</p>
          {#if current && !current.configured}
            <button class="btn" onclick={() => (view = 'settings')}>Set up {current.name}</button>
          {:else}
            <div class="chips">
              {#each suggestions as s (s)}
                <button class="chip" onclick={() => send(s)}>{s}</button>
              {/each}
            </div>
          {/if}
        </div>
      {:else}
        {#each items as item, i (i)}
          {#if item.kind === 'user'}
            <div class="msg user">{item.text}</div>
          {:else if item.kind === 'assistant'}
            <div class="msg assistant" class:streaming={item.streaming}>{item.text}</div>
          {:else if item.kind === 'system'}
            <div class="notice"><Icon name="info" size={13} />{item.text}</div>
          {:else if item.kind === 'tool'}
            <details class="tool" class:failed={item.ok === false}>
              <summary>
                <span class="state" aria-label={item.pending ? 'running' : item.ok ? 'ok' : item.ok === false ? 'failed' : 'no result'}>
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
                  <div class="k">Arguments</div>
                  <pre>{pretty(item.args)}</pre>
                {/if}
                {#if item.summary !== null}
                  <div class="k">{item.ok ? 'Result' : 'Error'}</div>
                  <pre>{item.summary}</pre>
                {:else if item.pending}
                  <div class="k">Waiting for the result…</div>
                {/if}
              </div>
            </details>
          {:else if item.kind === 'done'}
            {@const text = doneLabel(item.reason)}
            {#if text}<div class="done" class:err={item.reason.startsWith('error')}>{text}</div>{/if}
          {/if}
        {/each}
        {#if running && items[items.length - 1]?.kind !== 'assistant'}
          <div class="thinking" aria-label="Working"><span></span><span></span><span></span></div>
        {/if}
      {/if}
    </div>

    {#if error}
      <div class="error" role="alert">
        <Icon name="alert" size={14} />
        <span>{error}</span>
        <button class="icon-btn" aria-label="Dismiss" onclick={() => (error = null)}><Icon name="x" size={13} /></button>
      </div>
    {/if}

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
        placeholder={ended
          ? 'This session has ended. Write to start a new chat…'
          : session
            ? `Reply to ${sessionProvider}…`
            : `Message ${current?.name ?? 'the agent'}…`}
        aria-label="Message"
        onkeydown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            if (ended) newChat();
            void send();
          }
        }}
      ></textarea>
      {#if running}
        <button class="icon-btn stop" type="button" aria-label="Stop" title="Stop the agent" onclick={stop}>
          <Icon name="stop" size={14} stroke={2.25} />
        </button>
      {:else}
        <button
          class="icon-btn send"
          type="submit"
          aria-label="Send"
          disabled={!canSend}
          onclick={() => {
            if (ended) newChat();
          }}
        >
          <Icon name="send" size={16} stroke={2} />
        </button>
      {/if}
    </form>
  </div>
{/if}

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
    gap: 8px;
    min-height: 44px;
    padding: 6px 6px 6px 10px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    font-weight: 650;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .dot.pulse {
    animation: pulse 1.4s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .status {
    padding: 1px 7px;
    border-radius: 9px;
    background: var(--sunken);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .status.running {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .status.failed {
    color: var(--danger);
  }
  .select {
    position: relative;
    display: inline-flex;
    align-items: center;
    min-width: 0;
    color: var(--fg-muted);
  }
  .select :global(svg) {
    position: absolute;
    right: 8px;
    pointer-events: none;
  }
  select {
    appearance: none;
    max-width: 210px;
    height: 28px;
    padding: 0 26px 0 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-overflow: ellipsis;
  }
  .via {
    min-width: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sessions {
    display: flex;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
    scrollbar-width: thin;
    flex: none;
  }
  .sess {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 24px;
    padding: 0 9px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
    flex: none;
  }
  .sess:hover {
    border-color: var(--border-strong);
    color: var(--fg);
  }
  .sess[aria-selected='true'] {
    border-color: var(--accent-line);
    background: var(--accent-soft);
    color: var(--fg);
  }
  .sess.new {
    gap: 3px;
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
    margin: 0 0 4px;
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
    margin-top: 6px;
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
    max-width: 92%;
    font-size: var(--fs-sm);
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .msg.user {
    align-self: flex-end;
    padding: 7px 10px;
    border-radius: var(--r-lg) var(--r-lg) 3px var(--r-lg);
    background: var(--accent);
    color: var(--on-accent);
  }
  .msg.assistant {
    align-self: flex-start;
    max-width: 100%;
    padding: 1px 2px;
    color: var(--fg);
  }
  .msg.streaming::after {
    content: '';
    display: inline-block;
    width: 7px;
    height: 13px;
    margin-left: 2px;
    vertical-align: -2px;
    background: var(--accent);
    animation: blink 1s steps(1) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .notice {
    display: flex;
    gap: 6px;
    align-items: flex-start;
    padding: 6px 9px;
    border-radius: var(--r);
    background: var(--sunken);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .notice :global(svg) {
    flex: none;
    margin-top: 1px;
  }
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
  }
  .k {
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
  .done {
    align-self: center;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .done.err {
    color: var(--danger);
  }
  .thinking {
    display: flex;
    gap: 4px;
    padding: 4px 2px;
  }
  .thinking span {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--fg-subtle);
    animation: pulse 1.2s ease-in-out infinite;
  }
  .thinking span:nth-child(2) {
    animation-delay: 0.2s;
  }
  .thinking span:nth-child(3) {
    animation-delay: 0.4s;
  }
  .error {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 10px 8px;
    padding: 4px 4px 4px 9px;
    border-radius: var(--r);
    background: color-mix(in oklab, var(--danger) 12%, transparent);
    color: var(--danger);
    font-size: var(--fs-xs);
  }
  .error span {
    flex: 1;
  }
  .error .icon-btn {
    width: 22px;
    height: 22px;
    color: inherit;
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
  .send,
  .stop {
    position: absolute;
    right: 6px;
    bottom: 6px;
    background: var(--accent);
    color: var(--on-accent);
  }
  .stop {
    background: var(--fg);
    color: var(--surface);
  }
  .send:hover:not(:disabled) {
    background: color-mix(in oklab, var(--accent) 88%, var(--fg));
    color: var(--on-accent);
  }
  .stop:hover {
    background: color-mix(in oklab, var(--fg) 85%, var(--surface));
    color: var(--surface);
  }
  .send:disabled {
    background: var(--sunken);
    color: var(--fg-subtle);
    opacity: 1;
  }
</style>
