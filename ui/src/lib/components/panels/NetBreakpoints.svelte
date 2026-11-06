<script lang="ts">
  import type { NetStage } from '../../api';
  import { buildMatch, buildResume, draftFromPaused, type ResumeDraft } from '../../net/logic';
  import { parseRuleLines, type RuleRow } from '../../net/parse';
  import type { PausedItem } from '../../net/store.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: number | null } = $props();

  const chrome = useChrome();
  const net = chrome.net;
  const METHODS = ['', 'GET', 'POST', 'PUT', 'PATCH', 'DELETE'];

  let breakpoints = $state<RuleRow[]>([]);
  let pattern = $state('');
  let method = $state('');
  let stage = $state<NetStage>('request');
  let error = $state<string | null>(null);
  let info = $state<string | null>(null);
  let adding = $state(false);
  let now = $state(Date.now());
  /** Edits per pause id (kept while other pauses come and go). */
  let drafts = $state<Record<string, ResumeDraft>>({});
  let openId = $state<string | null>(null);
  let sending = $state<string | null>(null);

  async function refresh() {
    try {
      breakpoints = parseRuleLines((await chrome.api.call('net.rule_list', {})).text).filter((r) => r.breakpoint);
    } catch (err) {
      error = (err as Error).message;
    }
  }

  $effect(() => {
    void refresh();
    void net.refreshPaused().catch(() => {});
    const t = setInterval(() => {
      now = Date.now();
    }, 1000);
    const r = setInterval(refresh, 3000);
    return () => {
      clearInterval(t);
      clearInterval(r);
    };
  });

  // Open the newest pause automatically.
  $effect(() => {
    const last = net.paused[net.paused.length - 1];
    if (last && !net.paused.some((p) => p.info.pause_id === openId)) openId = last.info.pause_id;
  });

  // An editable draft per held request.
  $effect(() => {
    for (const p of net.paused) if (!drafts[p.info.pause_id]) drafts[p.info.pause_id] = draftFromPaused(p.info);
  });

  async function add() {
    if (tab === null) return;
    adding = true;
    error = null;
    try {
      await chrome.api.call('net.breakpoint', { tab, match: buildMatch(pattern, false, method), stage });
      pattern = '';
      await refresh();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      adding = false;
    }
  }

  async function remove(id: string) {
    try {
      const r = await chrome.api.call('net.rule_remove', { id });
      info = r.text;
    } catch (err) {
      error = (err as Error).message;
    }
    await refresh();
    await net.refreshPaused().catch(() => {});
  }

  async function resume(p: PausedItem, action: 'continue' | 'drop') {
    sending = p.info.pause_id;
    error = null;
    try {
      info = await net.resume(buildResume(p.info, drafts[p.info.pause_id] ?? draftFromPaused(p.info), action));
      delete drafts[p.info.pause_id];
    } catch (err) {
      error = (err as Error).message;
    } finally {
      sending = null;
    }
  }

  function left(p: PausedItem): number {
    return Math.max(0, Math.round((p.deadline - now) / 1000));
  }
</script>

<div class="bps">
  <form
    class="add"
    onsubmit={(e) => {
      e.preventDefault();
      void add();
    }}
  >
    <input bind:value={pattern} spellcheck="false" placeholder="URL pattern, empty = every request" aria-label="Breakpoint URL pattern" />
    <select bind:value={method} aria-label="Method">
      {#each METHODS as m (m)}<option value={m}>{m || 'Any'}</option>{/each}
    </select>
    <select bind:value={stage} aria-label="Stage">
      <option value="request">Request</option>
      <option value="response">Response</option>
    </select>
    <button class="btn primary" type="submit" disabled={tab === null || adding} title="Pause matching requests of tab {tab}">
      <Icon name="breakpoint" size={12} stroke={2} /> Pause
    </button>
  </form>

  {#if error}<p class="msg err" role="alert">{error}</p>{:else if info}<p class="msg">{info}</p>{/if}

  {#if breakpoints.length}
    <ul class="bp-list" aria-label="Breakpoints">
      {#each breakpoints as b (b.id)}
        <li>
          <Icon name="breakpoint" size={12} stroke={2} />
          <span class="id">{b.id}</span>
          <span class="what">{b.stage} · {b.match} · tab {b.tab}</span>
          <span class="hits" title="Requests paused so far">{b.hits}</span>
          <button class="icon-btn" aria-label="Remove breakpoint {b.id}" title="Remove (continues what it holds)" onclick={() => remove(b.id)}>
            <Icon name="x" size={12} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  <h4>Paused <span>{net.paused.length}</span></h4>
  {#if net.paused.length === 0}
    <p class="none">
      {breakpoints.length ? 'Waiting for a matching request… use the page.' : 'Add a breakpoint to hold requests before they are sent (or before the page sees the answer) and edit them.'}
    </p>
  {/if}
  <ul class="paused">
    {#each net.paused as p (p.info.pause_id)}
      {@const isOpen = openId === p.info.pause_id}
      <li class:open={isOpen}>
        <button class="head" aria-expanded={isOpen} onclick={() => (openId = isOpen ? null : p.info.pause_id)}>
          <span class="stage {p.info.stage}">{p.info.stage === 'request' ? 'REQ' : 'RES'}</span>
          <span class="m">{p.info.method}{p.info.status ? ` ${p.info.status}` : ''}</span>
          <span class="u" title={p.info.url}>{p.info.url.replace(/^[a-z]+:\/\//i, '')}</span>
          <span class="left" class:soon={left(p) <= 10}>{left(p)}s</span>
        </button>
        {#if isOpen && drafts[p.info.pause_id]}
          {@const id = p.info.pause_id}
          <div class="editor">
            {#if p.partial}
              <p class="hint">Paused before this panel opened: headers and body are not known here; edits replace or add fields.</p>
            {/if}
            {#if p.info.stage === 'request'}
              <div class="line">
                <select bind:value={drafts[id].method} aria-label="Method">
                  {#each ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'] as m (m)}<option value={m}>{m}</option>{/each}
                </select>
                <input bind:value={drafts[id].url} spellcheck="false" aria-label="URL" />
              </div>
            {:else}
              <label class="line"><span>Status</span><input class="num" bind:value={drafts[id].status} inputmode="numeric" /></label>
            {/if}
            <label>
              <span>{p.info.stage === 'request' ? 'Request' : 'Response'} headers <em>Name: value per line; delete a line to remove</em></span>
              <textarea bind:value={drafts[id].headers} rows="5" spellcheck="false"></textarea>
            </label>
            {#if p.info.stage === 'request'}
              <label>
                <span>Body{p.info.post_data && p.info.post_data.length >= 2048 ? ' (first 2 kB shown)' : ''}</span>
                <textarea bind:value={drafts[id].body} rows="3" spellcheck="false" placeholder="No body"></textarea>
              </label>
            {:else}
              <label class="check"><input type="checkbox" bind:checked={drafts[id].replaceBody} /> Replace the response body</label>
              {#if drafts[id].replaceBody}
                <textarea bind:value={drafts[id].responseBody} rows="4" spellcheck="false" aria-label="New response body"></textarea>
              {/if}
            {/if}
            <div class="actions">
              <button class="btn" disabled={sending !== null} onclick={() => resume(p, 'drop')}>Drop</button>
              <button class="btn primary" disabled={sending !== null} onclick={() => resume(p, 'continue')}>
                <Icon name="play" size={11} /> Continue
              </button>
            </div>
          </div>
        {/if}
      </li>
    {/each}
  </ul>
</div>

<style>
  .bps {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    font-size: var(--fs-xs);
  }
  .add {
    display: flex;
    gap: 5px;
    padding: 10px;
  }
  .add input {
    flex: 1;
  }
  input:not([type='checkbox']),
  select,
  textarea {
    min-width: 0;
    height: 26px;
    padding: 0 7px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--field-focus);
    color: var(--fg);
    font: var(--fs-xs) var(--font-mono);
    user-select: text;
  }
  select {
    font-family: var(--font);
    padding: 0 4px;
  }
  textarea {
    width: 100%;
    height: auto;
    padding: 6px 7px;
    resize: vertical;
    line-height: 1.45;
  }
  .add .btn {
    gap: 5px;
    padding: 0 9px;
  }
  .msg {
    margin: 0 10px 8px;
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
  .msg.err {
    color: var(--danger);
  }
  .bp-list,
  .paused {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .bp-list {
    border-top: 1px solid var(--border);
  }
  .bp-list li {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 3px 6px 3px 10px;
    border-bottom: 1px solid var(--border);
    color: var(--danger);
  }
  .id {
    color: var(--fg-subtle);
    font-family: var(--font-mono);
  }
  .what {
    flex: 1;
    min-width: 0;
    color: var(--fg);
    font-family: var(--font-mono);
    overflow-wrap: anywhere;
  }
  .hits {
    padding: 0 6px;
    border-radius: 9px;
    background: var(--sunken);
    color: var(--fg-muted);
  }
  .bp-list .icon-btn {
    width: 24px;
    height: 24px;
  }
  h4 {
    margin: 12px 10px 6px;
    font-size: var(--fs-xs);
    font-weight: 650;
  }
  h4 span {
    color: var(--fg-subtle);
    font-weight: 500;
  }
  .none {
    margin: 0 10px;
    color: var(--fg-subtle);
    line-height: 1.45;
  }
  .paused li {
    border-top: 1px solid var(--border);
  }
  .paused li:last-child {
    border-bottom: 1px solid var(--border);
  }
  .paused li.open {
    background: var(--surface);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    height: 30px;
    padding: 0 10px;
    border: 0;
    background: transparent;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    text-align: left;
  }
  .head:hover {
    background: var(--hover);
  }
  .stage {
    padding: 0 5px;
    border-radius: 3px;
    background: color-mix(in oklab, var(--warn) 20%, transparent);
    color: var(--warn);
    font-family: var(--font);
    font-weight: 700;
    font-size: 10px;
  }
  .stage.response {
    background: color-mix(in oklab, var(--g-purple) 18%, transparent);
    color: var(--g-purple);
  }
  .m {
    font-weight: 600;
  }
  .u {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .left {
    color: var(--fg-subtle);
  }
  .left.soon {
    color: var(--danger);
    font-weight: 700;
  }
  .editor {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 4px 10px 10px;
  }
  .hint {
    margin: 0;
    color: var(--fg-subtle);
  }
  .line {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .line input {
    flex: 1;
  }
  .num {
    flex: 0 0 64px !important;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--fg-muted);
    font-weight: 600;
  }
  label.line {
    flex-direction: row;
  }
  em {
    color: var(--fg-subtle);
    font-style: normal;
    font-weight: 400;
  }
  .check {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    font-weight: 500;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }
  .actions .btn {
    gap: 5px;
  }
</style>
