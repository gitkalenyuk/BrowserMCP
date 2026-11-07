<script lang="ts">
  import { tick } from 'svelte';
  import type { AiPermissionDecision, AiPermissionMode, AiSession, AiCodexSandbox } from '../../api';
  import { closeNeedsConfirm, finishedIds } from '../../ai/sessions';
  import { argsPreview, doneLabel, failureReason, toolLabel } from '../../ai/thread';
  import { t, tp } from '../../i18n/index.svelte';
  import { PROVIDERS, providerHint } from '../../providers';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import AiPermissionCard from './AiPermissionCard.svelte';
  import AiSettings from './AiSettings.svelte';

  const chrome = useChrome();
  const ai = chrome.ai;

  const MODES: AiPermissionMode[] = ['ask', 'accept_edits', 'plan', 'full_access', 'browser_only'];
  const SANDBOXES: AiCodexSandbox[] = ['read_only', 'workspace_write', 'full_access'];

  let view = $state<'chat' | 'settings'>('chat');
  // The draft lives in the store: the page context menu ("Ask AI about this") fills it.
  const draft = $derived(ai.draft);
  let sending = $state(false);
  let error = $state<string | null>(null);
  let info = $state<string | null>(null);
  /** A chat whose × was clicked while it still works: asks before closing. */
  let confirmClose = $state<AiSession | null>(null);
  let log = $state<HTMLDivElement>();
  let stick = true;

  const providerList = $derived(
    ai.providers.length
      ? ai.providers.map((p) => ({ id: p.id, name: p.name, configured: p.configured, protocol: p.protocol }))
      : PROVIDERS.map((p) => ({ ...p, configured: true, protocol: p.id })),
  );
  const session = $derived(ai.active);
  const items = $derived(session ? (ai.threads[session.id] ?? []) : []);
  const sessionName = $derived(session ? label(session) : null);
  const current = $derived(providerList.find((p) => p.id === ai.provider) ?? providerList[0]);
  const ended = $derived(session?.status === 'stopped' || session?.status === 'failed');
  const running = $derived(session?.status === 'running');
  const canSend = $derived(draft.trim() !== '' && !sending && (session ? true : current?.configured !== false));
  const finished = $derived(finishedIds(ai.sessions, ai.pending));
  /** The new chat's provider is a CLI agent: permissions and folder can be picked. */
  const cliKind = $derived(current?.protocol === 'claude-code' || current?.protocol === 'codex' ? current.protocol : null);
  const defaults = $derived(current ? ai.access[current.id] : undefined);
  const chosenMode = $derived(ai.newChat.permission_mode ?? defaults?.permission_mode ?? 'ask');

  const suggestions = $derived([t('aiChat.suggest.summarize'), t('aiChat.suggest.pricing'), t('aiChat.suggest.compare')]);

  $effect(() => {
    void ai.refreshProviders();
    void ai.refreshSessions();
  });

  // The defaults shown on the new-chat page.
  $effect(() => {
    if (cliKind && current && !ai.access[current.id]) void ai.loadAccess(current.id);
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
      ai.draft = '';
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
    confirmClose = null;
  }

  /** × / middle click: close at once, or ask first while the chat still works. */
  function requestClose(s: AiSession) {
    if (closeNeedsConfirm(s, ai.pending)) confirmClose = s;
    else void close(s);
  }

  async function close(s: AiSession) {
    confirmClose = null;
    try {
      await ai.closeSession(s.id);
      if (ai.activeId) void ai.open(ai.activeId);
    } catch (err) {
      error = (err as Error).message;
    }
  }

  async function closeFinished() {
    try {
      const n = await ai.closeFinished();
      info = tp('aiChat.closedCount', n);
      if (ai.activeId) void ai.open(ai.activeId);
    } catch (err) {
      error = (err as Error).message;
    }
  }

  async function answer(id: string, decision: AiPermissionDecision, reason?: string) {
    await ai.answerPermission(id, decision, reason);
  }

  function label(s: AiSession): string {
    const n = s.id.split('-').pop();
    const name = ai.providerName(s.provider);
    return n && /^\d+$/.test(n) ? t('aiChat.tabLabel', { provider: name, n }) : name;
  }

  function statusLabel(s: AiSession): string {
    return t(`aiChat.status.${s.status}`);
  }

  function tabTitle(s: AiSession): string {
    const parts = [s.id, statusLabel(s)];
    if (s.tabs.length) parts.push(t('aiChat.tabTabs', { tabs: s.tabs.join(', ') }));
    if (s.access?.cwd) parts.push(t('aiChat.access.folder', { cwd: s.access.cwd }));
    return parts.join(' · ');
  }

  /** The access in effect for a CLI chat, as a short chip label. */
  function accessLabel(s: AiSession): string | null {
    if (!s.access) return null;
    if (s.provider === 'codex' || ai.providers.find((p) => p.id === s.provider)?.protocol === 'codex') {
      return t(`aiChat.sandbox.${s.access.sandbox}`);
    }
    return t(`aiChat.mode.${s.access.permission_mode}`);
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
        <span class="title">{sessionName}</span>
        <span class="status {session.status}">{statusLabel(session)}</span>
        {#if accessLabel(session)}
          <span class="mode-chip" class:warn={session.access?.permission_mode === 'full_access'} title={tabTitle(session)}>
            <Icon name={session.access?.permission_mode === 'full_access' ? 'lock-open' : 'lock'} size={11} />
            {accessLabel(session)}
          </span>
        {/if}
        {#if session.model}<span class="via">{session.model}</span>{/if}
      {:else}
        <label class="select">
          <span class="sr-only">{t('aiChat.provider')}</span>
          <select value={ai.provider} onchange={(e) => ai.setProvider(e.currentTarget.value)}>
            {#each providerList as p (p.id)}
              <option value={p.id}>{p.configured ? p.name : t('aiChat.providerNotSetUp', { name: p.name })}</option>
            {/each}
          </select>
          <Icon name="chevron-down" size={12} stroke={2.25} />
        </label>
        <span class="via">{providerHint(ai.provider)}</span>
      {/if}
      <span class="spacer"></span>
      <button class="icon-btn" title={t('aiChat.settingsTitle')} aria-label={t('aiChat.settings')} onclick={() => (view = 'settings')}>
        <Icon name="settings" size={15} />
      </button>
    </div>

    {#if ai.sessions.length > 0}
      <div class="sessions" role="tablist" aria-label={t('aiChat.sessions')}>
        <button class="sess new" role="tab" aria-selected={!session} onclick={newChat} title={t('aiChat.newTitle')}>
          <Icon name="plus" size={13} stroke={2} />
          {t('aiChat.new')}
        </button>
        {#each ai.sessions as s (s.id)}
          {@const waiting = ai.pendingFor(s.id).length}
          <div class="sess" class:selected={session?.id === s.id}>
            <button
              class="pick"
              role="tab"
              aria-selected={session?.id === s.id}
              title={tabTitle(s)}
              onclick={() => ai.open(s.id)}
              onauxclick={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  requestClose(s);
                }
              }}
            >
              <span class="dot" class:pulse={s.status === 'running'} style:background={s.color}></span>
              {label(s)}
              {#if waiting > 0}<span class="badge" title={tp('aiChat.waitingCount', waiting)}>{waiting}</span>{/if}
            </button>
            <button class="x" aria-label={t('aiChat.closeNamed', { name: label(s) })} title={t('aiChat.closeTitle')} onclick={() => requestClose(s)}>
              <Icon name="x" size={11} stroke={2.25} />
            </button>
          </div>
        {/each}
        {#if finished.length > 1}
          <button class="sess sweep" onclick={closeFinished} title={t('aiChat.closeFinishedTitle')}>
            <Icon name="trash" size={12} />
            {t('aiChat.closeFinished')}
          </button>
        {/if}
      </div>
    {/if}

    {#if confirmClose}
      {@const target = confirmClose}
      <div class="confirm" role="alertdialog" aria-label={t('aiChat.closeConfirm', { name: label(target) })}>
        <span>{t('aiChat.closeConfirm', { name: label(target) })}</span>
        <button class="btn danger" onclick={() => close(target)}>{t('aiChat.closeConfirmYes')}</button>
        <button class="btn" onclick={() => (confirmClose = null)}>{t('common.cancel')}</button>
      </div>
    {/if}

    <div class="log" bind:this={log} onscroll={onScroll} aria-live="polite">
      {#if !session}
        <div class="intro">
          <span class="glyph"><Icon name="sparkles" size={20} /></span>
          <h3>{t('aiChat.intro.title', { name: current?.name ?? t('aiChat.intro.anAgent') })}</h3>
          <p>{t('aiChat.intro.body')}</p>
          {#if current && !current.configured}
            <button class="btn" onclick={() => (view = 'settings')}>{t('aiChat.intro.setUp', { name: current.name })}</button>
          {:else}
            {#if cliKind}
              <div class="access" aria-label={t('aiChat.newChat.title')}>
                {#if cliKind === 'claude-code'}
                  <label>
                    <span>{t('aiChat.newChat.permissions')}</span>
                    <select
                      value={ai.newChat.permission_mode ?? ''}
                      onchange={(e) => (ai.newChat.permission_mode = (e.currentTarget.value || null) as AiPermissionMode | null)}
                    >
                      <option value="">{t('aiChat.newChat.default', { value: t(`aiChat.mode.${defaults?.permission_mode ?? 'ask'}`) })}</option>
                      {#each MODES as m (m)}<option value={m}>{t(`aiChat.mode.${m}`)}</option>{/each}
                    </select>
                  </label>
                  <p class="hint" class:warn={chosenMode === 'full_access'}>{t(`aiChat.modeHint.${chosenMode}`)}</p>
                {:else}
                  <label>
                    <span>{t('aiChat.newChat.sandbox')}</span>
                    <select
                      value={ai.newChat.sandbox ?? ''}
                      onchange={(e) => (ai.newChat.sandbox = (e.currentTarget.value || null) as AiCodexSandbox | null)}
                    >
                      <option value="">{t('aiChat.newChat.default', { value: t(`aiChat.sandbox.${defaults?.sandbox ?? 'workspace_write'}`) })}</option>
                      {#each SANDBOXES as m (m)}<option value={m}>{t(`aiChat.sandbox.${m}`)}</option>{/each}
                    </select>
                  </label>
                {/if}
                <label>
                  <span>{t('aiChat.newChat.folder')}</span>
                  <input
                    spellcheck="false"
                    bind:value={ai.newChat.cwd}
                    placeholder={defaults ? (defaults.cwd ?? defaults.default_cwd) : t('aiChat.newChat.folderDefault')}
                  />
                </label>
              </div>
            {/if}
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
          {:else if item.kind === 'permission'}
            <AiPermissionCard request={item.request} agent={sessionName ?? ''} onanswer={(d, r) => answer(item.request.id, d, r)} />
          {:else if item.kind === 'tool'}
            {@const why = failureReason(item)}
            <details class="tool" class:failed={item.ok === false}>
              <summary>
                <span
                  class="state"
                  aria-label={item.pending
                    ? t('aiChat.tool.running')
                    : item.ok
                      ? t('aiChat.tool.ok')
                      : item.ok === false
                        ? t('aiChat.tool.failed')
                        : t('aiChat.tool.noResult')}
                >
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
                  <div class="k">{t('aiChat.tool.args')}</div>
                  <pre>{pretty(item.args)}</pre>
                {/if}
                {#if item.summary !== null}
                  <div class="k">{item.ok ? t('aiChat.tool.result') : t('aiChat.tool.error')}</div>
                  <pre>{item.summary}</pre>
                {:else if item.pending}
                  <div class="k">{t('aiChat.tool.waiting')}</div>
                {/if}
              </div>
            </details>
            {#if why}<div class="why" title={why}>{why}</div>{/if}
          {:else if item.kind === 'done'}
            {@const text = doneLabel(item.reason)}
            {#if text}<div class="done" class:err={item.reason.startsWith('error')}>{text}</div>{/if}
          {/if}
        {/each}
        {#if running && items[items.length - 1]?.kind !== 'assistant' && ai.pendingFor(session.id).length === 0}
          <div class="thinking" aria-label={t('aiChat.working')}><span></span><span></span><span></span></div>
        {/if}
        {#if ended}
          <div class="ended">{t(session.status === 'failed' ? 'aiChat.ended.failed' : 'aiChat.ended.stopped')}</div>
        {/if}
      {/if}
    </div>

    {#if error}
      <div class="error" role="alert">
        <Icon name="alert" size={14} />
        <span>{error}</span>
        <button class="icon-btn" aria-label={t('aiChat.dismiss')} onclick={() => (error = null)}><Icon name="x" size={13} /></button>
      </div>
    {:else if info}
      <div class="error ok" role="status">
        <Icon name="check" size={14} />
        <span>{info}</span>
        <button class="icon-btn" aria-label={t('aiChat.dismiss')} onclick={() => (info = null)}><Icon name="x" size={13} /></button>
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
        bind:value={ai.draft}
        rows="2"
        placeholder={ended
          ? t('aiChat.placeholder.ended')
          : session
            ? t('aiChat.placeholder.reply', { name: sessionName ?? '' })
            : t('aiChat.placeholder.new', { name: current?.name ?? t('aiChat.placeholder.theAgent') })}
        aria-label={t('aiChat.message')}
        onkeydown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            void send();
          }
        }}
      ></textarea>
      {#if running}
        <button class="icon-btn stop" type="button" aria-label={t('aiChat.stop')} title={t('aiChat.stopTitle')} onclick={stop}>
          <Icon name="stop" size={14} stroke={2.25} />
        </button>
      {:else}
        <button class="icon-btn send" type="submit" aria-label={t('aiChat.send')} disabled={!canSend}>
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
  .sess[aria-selected='true'],
  .sess.selected {
    border-color: var(--accent-line);
    background: var(--accent-soft);
    color: var(--fg);
  }
  .sess.new,
  .sess.sweep {
    gap: 3px;
  }
  .sess.sweep {
    border-style: dashed;
  }
  /* A chat tab: the label picks it, the × closes it. */
  div.sess {
    gap: 0;
    padding: 0 3px 0 0;
  }
  .pick {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 100%;
    padding: 0 4px 0 9px;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
  }
  .x {
    display: grid;
    place-items: center;
    width: 17px;
    height: 17px;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--fg-subtle);
  }
  .x:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .badge {
    min-width: 15px;
    padding: 0 4px;
    border-radius: 8px;
    background: var(--accent);
    color: var(--on-accent);
    font-size: 10px;
    line-height: 15px;
    text-align: center;
  }
  .mode-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 7px;
    border-radius: 9px;
    background: var(--sunken);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
  }
  .mode-chip.warn {
    background: color-mix(in oklab, var(--danger) 12%, transparent);
    color: var(--danger);
  }
  .confirm {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 7px 10px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--danger) 8%, transparent);
    font-size: var(--fs-xs);
  }
  .confirm span {
    flex: 1 1 160px;
  }
  .btn.danger {
    border-color: color-mix(in oklab, var(--danger) 40%, transparent);
    color: var(--danger);
  }
  .access {
    display: flex;
    flex-direction: column;
    gap: 7px;
    width: 100%;
    margin-top: 4px;
    padding: 9px 10px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    text-align: left;
  }
  .access label {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .access label > span {
    flex: none;
    width: 82px;
  }
  .access select,
  .access input {
    flex: 1;
    min-width: 0;
    max-width: none;
    height: 26px;
    padding: 0 8px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
    font-size: var(--fs-xs);
    font-weight: 400;
    user-select: text;
  }
  .access select {
    appearance: auto;
  }
  .hint {
    max-width: none;
    margin: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    line-height: 1.4;
  }
  .hint.warn {
    color: var(--danger);
  }
  .why {
    margin: -5px 0 0 26px;
    color: var(--danger);
    font-size: var(--fs-xs);
    line-height: 1.4;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .ended {
    align-self: center;
    max-width: 36ch;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    text-align: center;
  }
  .error.ok {
    background: color-mix(in oklab, var(--ok) 12%, transparent);
    color: var(--ok);
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
