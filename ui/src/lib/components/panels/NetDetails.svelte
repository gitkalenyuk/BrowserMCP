<script lang="ts">
  import { t } from '../../i18n/index.svelte';
  import { parseNetGet, parseWsFrames, type NetBody, type NetDetail, type NetRow, type WsFrame } from '../../net/parse';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  let { tab, row, onclose }: { tab: number; row: NetRow; onclose: () => void } = $props();

  const api = useChrome().api;
  const BODY_MAX = 64 * 1024;

  type Pane = 'headers' | 'request' | 'response' | 'frames';
  // The parent remounts this per request ({#key}), so the first pane is fixed here.
  // svelte-ignore state_referenced_locally
  let pane = $state<Pane>(row.type === 'websocket' ? 'frames' : 'headers');
  let detail = $state<NetDetail | null>(null);
  let reqBody = $state<NetBody | null>(null);
  let resBody = $state<NetBody | null>(null);
  let frames = $state<WsFrame[] | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);

  const isWs = $derived(row.type === 'websocket');
  // Reload when the request finished meanwhile (or got more frames).
  const key = $derived(`${tab}:${row.id}:${row.pending}:${row.status}:${row.frames}`);

  $effect(() => {
    void key;
    void load(pane);
  });

  async function load(p: Pane) {
    const id = row.id;
    loading = true;
    error = null;
    try {
      if (p === 'headers') {
        const r = await api.call('net.get', { tab, id, parts: ['headers'] });
        if (id === row.id) detail = parseNetGet(r.text);
      } else if (p === 'request') {
        const r = await api.call('net.get', { tab, id, parts: ['req_body'], max_bytes: BODY_MAX });
        if (id === row.id) reqBody = parseNetGet(r.text).reqBody;
      } else if (p === 'response') {
        const r = await api.call('net.get', { tab, id, parts: ['res_body'], max_bytes: BODY_MAX });
        if (id === row.id) resBody = parseNetGet(r.text).resBody;
      } else {
        const r = await api.call('net.ws_frames', { tab, id, limit: 500, max_bytes: 2000 });
        if (id === row.id) frames = parseWsFrames(r.text);
      }
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  /** Pretty-print JSON bodies; anything else as is. */
  function pretty(text: string): string {
    const s = text.trim();
    if (!(s.startsWith('{') || s.startsWith('['))) return text;
    try {
      return JSON.stringify(JSON.parse(s), null, 2);
    } catch {
      return text;
    }
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Clipboard may be unavailable in the embedded view.
    }
  }
</script>

<section class="details" aria-label={t('net.detail.label', { id: row.id })}>
  <div class="head">
    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={pane === 'headers'} onclick={() => (pane = 'headers')}>{t('net.detail.headers')}</button>
      {#if isWs}
        <button role="tab" aria-selected={pane === 'frames'} onclick={() => (pane = 'frames')}>{t('net.detail.frames')}{row.frames ? ` (${row.frames})` : ''}</button>
      {:else}
        <button role="tab" aria-selected={pane === 'request'} onclick={() => (pane = 'request')}>{t('net.stage.request')}</button>
        <button role="tab" aria-selected={pane === 'response'} onclick={() => (pane = 'response')}>{t('net.stage.response')}</button>
      {/if}
    </div>
    <span class="id">#{row.id}</span>
    <button class="icon-btn" aria-label={t('net.detail.close')} title={t('common.close')} onclick={onclose}><Icon name="x" size={13} /></button>
  </div>
  <div class="content">
    {#if error}
      <p class="err">{error}</p>
    {:else if pane === 'headers'}
      {#if detail}
        <div class="url">{detail.title.replace(/^#\d+ /, '')}</div>
        <div class="facts">{detail.facts}</div>
        {#each detail.extra as line (line)}<div class="facts">{line}</div>{/each}
        {#each [['req', detail.reqHeaders], ['res', detail.resHeaders]] as const as [side, headers] (side)}
          <h4>{t(`net.detail.headers.${side}`)} <span>{headers?.length ?? 0}</span></h4>
          {#if headers?.length}
            <dl>
              {#each headers as [n, v], i (i)}
                <dt>{n}</dt>
                <dd>{v}</dd>
              {/each}
            </dl>
          {:else}
            <p class="muted">{t('net.detail.none')}</p>
          {/if}
        {/each}
      {:else if loading}
        <p class="muted">{t('common.loading')}</p>
      {/if}
    {:else if pane === 'request' || pane === 'response'}
      {@const body = pane === 'request' ? reqBody : resBody}
      {#if body}
        {#if body.info === null}
          <p class="muted">{body.text === 'none' ? t('net.detail.noBody') : body.text}</p>
        {:else}
          <div class="body-head">
            <span>{body.info}</span>
            <button class="btn" onclick={() => copy(body.text)}>{t('common.copy')}</button>
          </div>
          <pre>{pretty(body.text)}</pre>
        {/if}
      {:else if loading}
        <p class="muted">{t('common.loading')}</p>
      {/if}
    {:else if frames}
      {#if frames.length === 0}
        <p class="muted">{t('net.detail.noFrames')}</p>
      {:else}
        <ol class="frames">
          {#each frames as f (f.n)}
            <li class:sent={f.sent}>
              <span class="dir" title={f.sent ? t('net.detail.sent') : t('net.detail.received')}>{f.sent ? '↑' : '↓'}</span>
              <span class="kind">{f.kind}</span>
              <span class="data">{f.data || '·'}</span>
              <span class="meta">{f.size} · +{f.at.toFixed(2)}s</span>
            </li>
          {/each}
        </ol>
      {/if}
    {:else if loading}
      <p class="muted">{t('common.loading')}</p>
    {/if}
  </div>
</section>

<style>
  .details {
    display: flex;
    flex-direction: column;
    flex: 1 1 55%;
    min-height: 120px;
    border-top: 1px solid var(--border-strong);
    background: var(--toolbar);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 4px 0 6px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .tabs {
    display: flex;
    flex: 1;
    gap: 2px;
  }
  .tabs button {
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .tabs button[aria-selected='true'] {
    border-bottom-color: var(--accent);
    color: var(--fg);
  }
  .id {
    color: var(--fg-subtle);
    font: var(--fs-xs) var(--font-mono);
  }
  .head .icon-btn {
    width: 24px;
    height: 24px;
  }
  .content {
    flex: 1;
    min-height: 0;
    padding: 8px 10px 12px;
    overflow: auto;
    user-select: text;
    font-size: var(--fs-xs);
  }
  .url {
    font: var(--fs-xs) / 1.4 var(--font-mono);
    overflow-wrap: anywhere;
    color: var(--fg);
  }
  .facts {
    margin-top: 3px;
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
  h4 {
    margin: 12px 0 4px;
    font-size: var(--fs-xs);
    font-weight: 650;
  }
  h4 span {
    color: var(--fg-subtle);
    font-weight: 500;
  }
  dl {
    display: grid;
    grid-template-columns: minmax(80px, max-content) minmax(0, 1fr);
    gap: 2px 10px;
    margin: 0;
    font: var(--fs-xs) / 1.45 var(--font-mono);
  }
  dt {
    color: var(--fg-muted);
    white-space: nowrap;
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .body-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 6px;
    color: var(--fg-subtle);
  }
  .body-head .btn {
    height: 22px;
    padding: 0 8px;
    font-size: var(--fs-xs);
  }
  pre {
    margin: 0;
    padding: 8px;
    border-radius: var(--r-sm);
    background: var(--surface);
    border: 1px solid var(--border);
    font: var(--fs-xs) / 1.45 var(--font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .frames {
    margin: 0;
    padding: 0;
    list-style: none;
    font: var(--fs-xs) / 1.45 var(--font-mono);
  }
  .frames li {
    display: grid;
    grid-template-columns: 14px 44px minmax(0, 1fr) auto;
    gap: 6px;
    padding: 2px 0;
    border-bottom: 1px solid var(--border);
  }
  .dir {
    color: var(--g-red);
  }
  .sent .dir {
    color: var(--g-green);
  }
  .kind,
  .meta {
    color: var(--fg-subtle);
  }
  .data {
    overflow-wrap: anywhere;
  }
  .muted {
    margin: 4px 0;
    color: var(--fg-subtle);
  }
  .err {
    margin: 4px 0;
    color: var(--danger);
  }
</style>
