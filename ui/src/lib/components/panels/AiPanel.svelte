<script lang="ts">
  import { tick } from 'svelte';
  import type { AiCodexApproval, AiCodexSandbox, AiPermissionDecision, AiPermissionMode, AiSession } from '../../api';
  import { copyText } from '../../ai/clipboard';
  import { closeNeedsConfirm, finishedIds } from '../../ai/sessions';
  import { doneLabel, groupTools, lastUserIndex, noticeText, queuedCount, userOrdinal } from '../../ai/thread';
  import { t, tp } from '../../i18n/index.svelte';
  import { PROVIDERS, providerHint } from '../../providers';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import AiComposer from './AiComposer.svelte';
  import AiHistory from './AiHistory.svelte';
  import AiMessage from './AiMessage.svelte';
  import AiModelPicker from './AiModelPicker.svelte';
  import AiPermissionCard from './AiPermissionCard.svelte';
  import AiSettings from './AiSettings.svelte';
  import AiToolRow from './AiToolRow.svelte';

  const chrome = useChrome();
  const ai = chrome.ai;

  const MODES: AiPermissionMode[] = ['ask', 'accept_edits', 'plan', 'full_access', 'browser_only'];
  const SANDBOXES: AiCodexSandbox[] = ['read_only', 'workspace_write', 'full_access'];
  const APPROVALS: AiCodexApproval[] = ['untrusted', 'on_request', 'never'];

  let view = $state<'chat' | 'settings'>('chat');
  let sending = $state(false);
  let error = $state<string | null>(null);
  let info = $state<string | null>(null);
  /** Close of a working chat, or Delete: asks first. */
  let confirm = $state<{ kind: 'close' | 'delete'; session: AiSession } | null>(null);
  let historyOpen = $state(false);
  /** The tab being renamed in place, and its text. */
  let renaming = $state<string | null>(null);
  let renameText = $state('');
  /** The tab context menu (right-click, the context-menu key, Shift+F10). */
  let menu = $state<{ session: AiSession; x: number; y: number } | null>(null);
  let menuEl = $state<HTMLDivElement>();
  /** Row index of the user message in the inline editor. */
  let editing = $state<number | null>(null);
  /** Tool groups the user expanded, by `<chat>:<first row>`. */
  let openGroups = $state<Record<string, boolean>>({});
  let root = $state<HTMLDivElement>();
  let log = $state<HTMLDivElement>();
  let stick = true;
  let atBottom = $state(true);

  const providerList = $derived(
    ai.providers.length
      ? ai.providers.map((p) => ({ id: p.id, name: p.name, configured: p.configured, protocol: p.protocol }))
      : PROVIDERS.map((p) => ({ ...p, configured: true, protocol: p.id })),
  );
  const session = $derived(ai.active);
  const items = $derived(session ? (ai.threads[session.id] ?? []) : []);
  const rows = $derived(groupTools(items));
  const sessionName = $derived(session ? ai.title(session) : null);
  const current = $derived(providerList.find((p) => p.id === ai.provider) ?? providerList[0]);
  const ended = $derived(session?.status === 'stopped' || session?.status === 'failed');
  const running = $derived(session?.status === 'running');
  const canSend = $derived(ai.draft.trim() !== '' && !sending && (session ? true : current?.configured !== false));
  const finished = $derived(finishedIds(ai.sessions, ai.pending));
  const waitingCount = $derived(session ? (session.queued ?? queuedCount(items)) : 0);
  const lastUser = $derived(lastUserIndex(items));
  /** The last row of the conversation itself (queued messages wait below it). */
  const lastRow = $derived(items[items.length - 1 - queuedCount(items)]);
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

  // Another chat shown: no editor, at the bottom.
  $effect(() => {
    void ai.activeId;
    editing = null;
    stick = true;
    atBottom = true;
  });

  // Esc while the shown chat works interrupts the answer (inner editors and menus stop it first).
  $effect(() => {
    const el = root;
    if (!el) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      if (menu) {
        menu = null;
        return;
      }
      if (renaming || confirm) return;
      if (running) {
        e.preventDefault();
        void interrupt();
      }
    };
    el.addEventListener('keydown', onKey);
    return () => el.removeEventListener('keydown', onKey);
  });

  // The tab menu: focus its first item; arrows move, Esc / a click elsewhere closes it.
  $effect(() => {
    if (menu && menuEl) menuEl.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
  });

  function onScroll() {
    if (!log) return;
    stick = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
    atBottom = stick;
  }

  function scrollDown() {
    stick = true;
    atBottom = true;
    log?.scrollTo({ top: log.scrollHeight, behavior: 'smooth' });
  }

  function fail(err: unknown) {
    error = (err as Error).message ?? String(err);
    info = null;
  }

  function note(text: string) {
    info = text;
    error = null;
  }

  /**
   * Send the box's text (or a suggestion). The box is emptied at once, so what the user types
   * while the call is on its way is kept; on failure the text comes back if the box is empty.
   */
  async function send(text = ai.draft) {
    if (!text.trim() || sending) return;
    const fromBox = text === ai.draft;
    if (fromBox) ai.draft = '';
    sending = true;
    error = null;
    stick = true;
    try {
      await ai.send(text);
    } catch (err) {
      if (fromBox && !ai.draft.trim()) ai.draft = text;
      fail(err);
    } finally {
      sending = false;
    }
  }

  async function interrupt() {
    if (!session) return;
    try {
      await ai.interrupt(session.id);
    } catch (err) {
      fail(err);
    }
  }

  async function stopAgent() {
    if (!session) return;
    try {
      await ai.stopSession(session.id);
    } catch (err) {
      fail(err);
    }
  }

  function newChat() {
    void ai.open(null);
    error = null;
    confirm = null;
  }

  /** × / middle click / menu: close (archive) at once, or ask first while the chat still works. */
  function requestClose(s: AiSession) {
    menu = null;
    if (closeNeedsConfirm(s, ai.pending)) confirm = { kind: 'close', session: s };
    else void close(s);
  }

  async function close(s: AiSession) {
    confirm = null;
    try {
      await ai.closeSession(s.id);
      if (ai.activeId) void ai.open(ai.activeId);
    } catch (err) {
      fail(err);
    }
  }

  async function remove(s: AiSession) {
    confirm = null;
    const name = ai.title(s);
    try {
      await ai.deleteSession(s.id);
      if (ai.activeId) void ai.open(ai.activeId);
      note(t('aiChat.deleted', { name }));
    } catch (err) {
      fail(err);
    }
  }

  async function closeFinished() {
    try {
      const n = await ai.closeFinished();
      note(tp('aiChat.closedCount', n));
      if (ai.activeId) void ai.open(ai.activeId);
    } catch (err) {
      fail(err);
    }
  }

  async function reopen(s: AiSession) {
    try {
      await ai.reopen(s.id);
      historyOpen = false;
    } catch (err) {
      fail(err);
    }
  }

  async function deleteAllClosed() {
    try {
      note(tp('aiChat.history.deletedCount', await ai.deleteAllClosed()));
    } catch (err) {
      fail(err);
    }
  }

  async function copyChat(s: AiSession) {
    menu = null;
    try {
      if (await copyText(await ai.markdownOf(s.id))) note(t('aiChat.copiedChat'));
      else fail(new Error(t('aiChat.msg.copyFailed')));
    } catch (err) {
      fail(err);
    }
  }

  function startRename(s: AiSession) {
    menu = null;
    renaming = s.id;
    renameText = s.title_set ? (s.title ?? '') : ai.title(s);
  }

  async function finishRename(save: boolean) {
    const id = renaming;
    renaming = null;
    if (!save || !id) return;
    const s = ai.sessions.find((x) => x.id === id);
    if (!s || renameText.trim() === ai.title(s)) return;
    try {
      await ai.rename(id, renameText);
    } catch (err) {
      fail(err);
    }
  }

  function focusOnMount(el: HTMLInputElement) {
    el.focus();
    el.select();
  }

  function openMenu(s: AiSession, x: number, y: number) {
    const width = 210;
    const height = 150;
    menu = {
      session: s,
      x: Math.max(4, Math.min(x, window.innerWidth - width - 4)),
      y: Math.max(4, Math.min(y, window.innerHeight - height - 4)),
    };
  }

  function menuKeys(e: KeyboardEvent) {
    const list = [...(menuEl?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])];
    const at = list.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const step = e.key === 'ArrowDown' ? 1 : -1;
      list[(at + step + list.length) % list.length]?.focus();
    } else if (e.key === 'Escape' || e.key === 'Tab') {
      e.preventDefault();
      e.stopPropagation();
      menu = null;
    }
  }

  async function answer(id: string, decision: AiPermissionDecision, reason?: string) {
    await ai.answerPermission(id, decision, reason);
  }

  async function saveEdit(index: number, text: string) {
    if (!session) return;
    const ordinal = userOrdinal(items, index);
    editing = null;
    stick = true;
    try {
      await ai.edit(session.id, ordinal, text);
    } catch (err) {
      fail(err);
    }
  }

  /** ↑ in an empty composer: the last message opens in the editor. */
  function editLast(): boolean {
    if (!session || running || lastUser < 0) return false;
    editing = lastUser;
    return true;
  }

  function openLink(href: string) {
    chrome.newTab(href);
  }

  function statusLabel(s: AiSession): string {
    return t(`aiChat.status.${s.status}`);
  }

  function tabTitle(s: AiSession): string {
    const parts = [ai.title(s), ai.providerName(s.provider), statusLabel(s)];
    if (s.model) parts.push(s.model);
    if (s.tabs.length) parts.push(t('aiChat.tabTabs', { tabs: s.tabs.join(', ') }));
    if (s.access?.cwd) parts.push(t('aiChat.access.folder', { cwd: s.access.cwd }));
    return parts.join(' · ');
  }

  function isCodex(s: AiSession): boolean {
    return s.provider === 'codex' || ai.providers.find((p) => p.id === s.provider)?.protocol === 'codex';
  }

  /** The access in effect for a CLI chat, as a short chip label. */
  function accessLabel(s: AiSession): string | null {
    if (!s.access) return null;
    if (isCodex(s)) return t(`aiChat.sandbox.${s.access.sandbox}`);
    return t(`aiChat.mode.${s.access.permission_mode}`);
  }

  async function pickModel(s: AiSession, model: string) {
    try {
      await ai.setModel(s.id, model);
      note(t('aiChat.model.changed', { name: ai.title(s), model }));
    } catch (err) {
      fail(err);
    }
  }
</script>

<svelte:window
  onpointerdown={(e) => {
    if (menu && !menuEl?.contains(e.target as Node)) menu = null;
  }}
  onblur={() => (menu = null)}
/>

{#if view === 'settings'}
  <AiSettings onback={() => (view = 'chat')} />
{:else}
  <div class="ai" bind:this={root}>
    <div class="bar">
      {#if session}
        <span class="dot" style:background={session.color} aria-hidden="true"></span>
        <span class="title" title={tabTitle(session)}>{sessionName}</span>
        <span class="status {session.status}">{statusLabel(session)}</span>
        {#if accessLabel(session)}
          {@const full = session.access?.permission_mode === 'full_access' || session.access?.sandbox === 'full_access'}
          <!-- Only the lock in the narrow bar (its name on hover); full access keeps its warning text. -->
          <span
            class="mode-chip"
            class:warn={full}
            role="img"
            title={`${accessLabel(session)} · ${tabTitle(session)}`}
            aria-label={accessLabel(session)}
          >
            <Icon name={full ? 'lock-open' : 'lock'} size={11} />
            {#if full}{accessLabel(session)}{/if}
          </span>
        {/if}
        <AiModelPicker
          provider={session.provider}
          value={session.model ?? ''}
          allowDefault={false}
          title={t('aiChat.model.chatTitle')}
          onpick={(m) => pickModel(session, m)}
        />
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
        {#if current?.configured !== false}
          <AiModelPicker
            provider={ai.provider}
            value={ai.newModelFor(ai.provider)}
            title={t('aiChat.model.newTitle')}
            onpick={(m) => ai.setNewChatModel(ai.provider, m)}
          />
        {:else}
          <span class="via">{providerHint(ai.provider)}</span>
        {/if}
      {/if}
      <span class="spacer"></span>
      {#if session}
        <button class="icon-btn" title={t('aiChat.copyChatTitle')} aria-label={t('aiChat.copyChat')} onclick={() => copyChat(session)}>
          <Icon name="copy" size={14} />
        </button>
      {/if}
      <button
        class="icon-btn"
        class:on={historyOpen}
        title={t('aiChat.history.openTitle')}
        aria-label={t('aiChat.history.open')}
        aria-expanded={historyOpen}
        onclick={() => (historyOpen = !historyOpen)}
      >
        <Icon name="clock" size={15} />
      </button>
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
          {@const name = ai.title(s)}
          <div class="sess" class:selected={session?.id === s.id} class:editing={renaming === s.id}>
            {#if renaming === s.id}
              <span class="dot" style:background={s.color} aria-hidden="true"></span>
              <input
                class="rename"
                aria-label={t('aiChat.tab.renameLabel')}
                title={t('aiChat.tab.renameHint')}
                bind:value={renameText}
                use:focusOnMount
                onkeydown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    void finishRename(true);
                  } else if (e.key === 'Escape') {
                    e.preventDefault();
                    e.stopPropagation();
                    void finishRename(false);
                  }
                }}
                onblur={() => finishRename(true)}
              />
            {:else}
              <button
                class="pick"
                role="tab"
                aria-selected={session?.id === s.id}
                aria-haspopup="menu"
                title={tabTitle(s)}
                onclick={() => ai.open(s.id)}
                ondblclick={() => startRename(s)}
                oncontextmenu={(e) => {
                  e.preventDefault();
                  openMenu(s, e.clientX, e.clientY);
                }}
                onkeydown={(e) => {
                  if (e.key === 'F2') {
                    e.preventDefault();
                    startRename(s);
                  } else if (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) {
                    e.preventDefault();
                    const r = e.currentTarget.getBoundingClientRect();
                    openMenu(s, r.left, r.bottom + 2);
                  } else if (e.key === 'Delete') {
                    e.preventDefault();
                    requestClose(s);
                  }
                }}
                onauxclick={(e) => {
                  if (e.button === 1) {
                    e.preventDefault();
                    requestClose(s);
                  }
                }}
              >
                <span class="dot" class:pulse={s.status === 'running'} style:background={s.color}></span>
                <span class="label">{name}</span>
                {#if waiting > 0}<span class="badge" title={tp('aiChat.waitingCount', waiting)}>{waiting}</span>{/if}
              </button>
            {/if}
            <button class="x" aria-label={t('aiChat.closeNamed', { name })} title={t('aiChat.closeTitleArchive')} onclick={() => requestClose(s)}>
              <Icon name="x" size={11} stroke={2.25} />
            </button>
          </div>
        {/each}
        {#if finished.length > 0}
          <button class="sess sweep" onclick={closeFinished} title={t('aiChat.closeFinishedTitle')}>
            <Icon name="x" size={12} />
            {t('aiChat.closeFinished')}
          </button>
        {/if}
      </div>
    {/if}

    {#if menu}
      {@const target = menu.session}
      <div
        class="ctx"
        role="menu"
        tabindex="-1"
        aria-label={t('aiChat.tab.menu')}
        bind:this={menuEl}
        style:left="{menu.x}px"
        style:top="{menu.y}px"
        onkeydown={menuKeys}
      >
        <button role="menuitem" title={t('aiChat.tab.renameTitle')} onclick={() => startRename(target)}>
          <Icon name="pencil" size={13} />{t('aiChat.tab.rename')}<kbd>F2</kbd>
        </button>
        <button role="menuitem" onclick={() => copyChat(target)}><Icon name="copy" size={13} />{t('aiChat.tab.copyMarkdown')}</button>
        <div class="sep" role="separator"></div>
        <button role="menuitem" title={t('aiChat.closeTitleArchive')} onclick={() => requestClose(target)}>
          <Icon name="x" size={13} />{t('aiChat.tab.close')}
        </button>
        <button
          role="menuitem"
          class="danger"
          onclick={() => {
            menu = null;
            confirm = { kind: 'delete', session: target };
          }}
        >
          <Icon name="trash" size={13} />{t('aiChat.tab.delete')}
        </button>
      </div>
    {/if}

    {#if confirm}
      {@const target = confirm.session}
      {@const text = confirm.kind === 'close' ? t('aiChat.closeConfirm', { name: ai.title(target) }) : t('aiChat.deleteConfirm', { name: ai.title(target) })}
      <div class="confirm" role="alertdialog" aria-label={text}>
        <span>{text}</span>
        {#if confirm.kind === 'close'}
          <button class="btn danger" onclick={() => close(target)}>{t('aiChat.closeConfirmYes')}</button>
        {:else}
          <button class="btn danger" onclick={() => remove(target)}>{t('aiChat.deleteConfirmYes')}</button>
        {/if}
        <button class="btn" onclick={() => (confirm = null)}>{t('common.cancel')}</button>
      </div>
    {/if}

    <div class="log-wrap">
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
                    <label>
                      <span>{t('aiChat.newChat.addDirs')}</span>
                      <textarea
                        rows="2"
                        spellcheck="false"
                        value={ai.newChat.add_dirs ?? defaults?.add_dirs.join('\n') ?? ''}
                        oninput={(e) => (ai.newChat.add_dirs = e.currentTarget.value)}
                        placeholder={t('aiChat.accessSettings.addDirsPlaceholder')}
                      ></textarea>
                    </label>
                    <p class="hint">{t('aiChat.newChat.addDirsHint')}</p>
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
                    <label>
                      <span>{t('aiChat.newChat.approvals')}</span>
                      <select
                        value={ai.newChat.approval_policy ?? ''}
                        onchange={(e) => (ai.newChat.approval_policy = (e.currentTarget.value || null) as AiCodexApproval | null)}
                      >
                        <option value="">{t('aiChat.newChat.default', { value: t(`aiChat.approval.${defaults?.approval_policy ?? 'on_request'}`) })}</option>
                        {#each APPROVALS as a (a)}<option value={a}>{t(`aiChat.approval.${a}`)}</option>{/each}
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
          {#each rows as row (row.kind === 'tools' ? `g${row.start}` : row.index)}
            {#if row.kind === 'tools'}
              {@const key = `${session.id}:${row.start}`}
              {@const failed = row.items.filter((r) => r.item.ok === false).length}
              {@const busy = row.items.some((r) => r.item.pending)}
              <div class="group" class:open={openGroups[key]}>
                <button class="group-head" aria-expanded={!!openGroups[key]} onclick={() => (openGroups[key] = !openGroups[key])}>
                  <Icon name="chevron-right" size={12} class="chev" />
                  <span>{tp('aiChat.tools.group', row.items.length)}</span>
                  {#if failed > 0}<span class="failed">{t('aiChat.tools.failedCount', { n: failed })}</span>{/if}
                  {#if busy}<span class="spinner" aria-label={t('aiChat.tool.running')}></span>{/if}
                  <span class="names">{[...new Set(row.items.map((r) => r.item.name.split('__').pop()))].join(', ')}</span>
                </button>
                {#if openGroups[key]}
                  <div class="group-body">
                    {#each row.items as r (r.index)}
                      <AiToolRow item={r.item} oncopyfail={(m) => fail(new Error(m))} />
                    {/each}
                  </div>
                {/if}
              </div>
            {:else}
              {@const item = row.item}
              {#if item.kind === 'user' || item.kind === 'assistant'}
                <AiMessage
                  {item}
                  editable={item.kind === 'user' && !item.queued}
                  editBlocked={running ? t('aiChat.msg.editBusy') : null}
                  retry={row.index === lastUser}
                  editing={editing === row.index}
                  onstartedit={() => (editing = row.index)}
                  oncanceledit={() => (editing = null)}
                  onsave={(text) => saveEdit(row.index, text)}
                  onretry={() => item.kind === 'user' && saveEdit(row.index, item.text)}
                  onlink={openLink}
                  oncopyfail={(m) => fail(new Error(m))}
                />
              {:else if item.kind === 'system'}
                <div class="notice"><Icon name="info" size={13} />{noticeText(item)}</div>
              {:else if item.kind === 'permission'}
                <AiPermissionCard
                  request={item.request}
                  agent={ai.providerName(session.provider)}
                  reasonReachesAgent={!isCodex(session)}
                  onanswer={(d, r) => answer(item.request.id, d, r)}
                />
              {:else if item.kind === 'tool'}
                <AiToolRow {item} oncopyfail={(m) => fail(new Error(m))} />
              {:else if item.kind === 'done'}
                {@const text = doneLabel(item.reason)}
                {#if text}<div class="done" class:err={item.reason.startsWith('error')}>{text}</div>{/if}
              {/if}
            {/if}
          {/each}
          {#if running && !(lastRow?.kind === 'assistant' && lastRow.streaming) && ai.pendingFor(session.id).length === 0}
            <div class="thinking" role="status" aria-label={t('aiChat.working')}><span></span><span></span><span></span></div>
          {/if}
          {#if ended}
            <div class="ended">{t(session.status === 'failed' ? 'aiChat.ended.failed' : 'aiChat.ended.stopped')}</div>
          {/if}
        {/if}
      </div>
      {#if session && !atBottom}
        <button class="down" aria-label={t('aiChat.scrollDown')} title={t('aiChat.scrollDown')} onclick={scrollDown}>
          <Icon name="chevron-down" size={16} stroke={2.25} />
        </button>
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

    <AiComposer
      placeholder={running
        ? t('aiChat.placeholder.busy')
        : ended
          ? t('aiChat.placeholder.ended')
          : session
            ? t('aiChat.placeholder.reply', { name: ai.providerName(session.provider) })
            : t('aiChat.placeholder.new', { name: current?.name ?? t('aiChat.placeholder.theAgent') })}
      {running}
      {canSend}
      queued={waitingCount}
      onsend={() => send()}
      oninterrupt={interrupt}
      onstop={stopAgent}
      oneditlast={editLast}
    />

    {#if historyOpen}
      <AiHistory onclose={() => (historyOpen = false)} onreopen={reopen} ondelete={remove} ondeleteall={deleteAllClosed} />
    {/if}
  </div>
{/if}

<style>
  .ai {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 44px;
    padding: 6px 6px 6px 10px;
    border-bottom: 1px solid var(--border);
  }
  .bar > * {
    flex: none;
  }
  .title {
    flex: 0 1 auto;
    min-width: 56px;
    overflow: hidden;
    font-weight: 650;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .bar :global(.model) {
    flex: 0 2 auto;
    min-width: 64px;
  }
  .spacer {
    flex: 1 1 0;
  }
  .icon-btn.on {
    background: var(--accent-soft);
    color: var(--accent);
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
    white-space: nowrap;
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
    max-width: 170px;
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
  div.sess.editing {
    gap: 6px;
    padding-left: 9px;
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
  .label {
    max-width: 150px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rename {
    width: 150px;
    height: 18px;
    padding: 0 4px;
    border: 1px solid var(--accent-line);
    border-radius: var(--r-sm);
    background: var(--surface);
    color: var(--fg);
    font: inherit;
    user-select: text;
    outline: none;
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
  .ctx {
    position: fixed;
    z-index: 50;
    min-width: 200px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    box-shadow: var(--shadow-pop);
    outline: none;
  }
  .ctx button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--fg);
    font-size: var(--fs-sm);
    text-align: left;
  }
  .ctx button:hover,
  .ctx button:focus-visible {
    background: var(--hover);
    outline: none;
  }
  .ctx button.danger {
    color: var(--danger);
  }
  .ctx :global(svg) {
    flex: none;
    color: var(--fg-subtle);
  }
  .ctx .danger :global(svg) {
    color: inherit;
  }
  .ctx kbd {
    margin-left: auto;
    color: var(--fg-subtle);
    font: 10px var(--font-mono);
  }
  .sep {
    height: 1px;
    margin: 4px 2px;
    background: var(--border);
  }
  .mode-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    padding: 1px 7px;
    border-radius: 9px;
    background: var(--sunken);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .bar .mode-chip {
    flex: 0 2 auto;
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
  .access textarea {
    flex: 1;
    min-width: 0;
    padding: 4px 8px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
    font: inherit;
    font-size: var(--fs-xs);
    font-weight: 400;
    resize: vertical;
  }
  .access label:has(textarea) {
    align-items: flex-start;
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
  .log-wrap {
    position: relative;
    display: flex;
    flex: 1;
    min-height: 0;
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
  .down {
    position: absolute;
    right: 14px;
    bottom: 10px;
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: 1px solid var(--border-strong);
    border-radius: 50%;
    background: var(--surface);
    color: var(--fg-muted);
    box-shadow: var(--shadow-pop);
  }
  .down:hover {
    color: var(--fg);
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
  .group {
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    font-size: var(--fs-xs);
  }
  .group-head {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--fg);
    font-size: var(--fs-xs);
    font-weight: 600;
    text-align: left;
  }
  .group-head:hover {
    background: var(--hover);
  }
  .group-head :global(.chev) {
    flex: none;
    color: var(--fg-subtle);
    transition: transform 120ms;
  }
  .group.open .group-head :global(.chev) {
    transform: rotate(90deg);
  }
  .group-head .failed {
    color: var(--danger);
  }
  .group-head .names {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--fg-subtle);
    font-family: var(--font-mono);
    font-weight: 400;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .group-body {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px;
    border-top: 1px solid var(--border);
  }
  .spinner {
    width: 11px;
    height: 11px;
    border: 2px solid var(--accent-soft);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    flex: none;
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
</style>
