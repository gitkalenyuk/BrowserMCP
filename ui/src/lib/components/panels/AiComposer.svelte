<script lang="ts">
  // The AI panel's message box. It stays usable while the agent works: Enter sends (the
  // message is queued until the agent takes it), the stop button sits next to send (a click
  // or Esc interrupts the answer; its menu also offers "Stop the agent"). It grows with its
  // text up to ~40% of the panel; Shift+Enter is a new line; ↑ in an empty box edits the last
  // message. The text lives in `chrome.ai.draft` (the page menu "Ask AI about this" and the
  // voice button fill it too).
  import { tick } from 'svelte';
  import { t, tp } from '../../i18n/index.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  let {
    placeholder,
    running,
    canSend,
    queued = 0,
    onsend,
    oninterrupt,
    onstop,
    oneditlast,
  }: {
    placeholder: string;
    /** The shown chat works now: the stop button and the Esc hint show. */
    running: boolean;
    canSend: boolean;
    /** Messages of this chat waiting for the agent. */
    queued?: number;
    onsend: () => void;
    oninterrupt: () => void;
    onstop: () => void;
    /** ↑ in an empty box: edit the last message; false when there is none. */
    oneditlast?: () => boolean;
  } = $props();

  const ai = useChrome().ai;

  let area = $state<HTMLTextAreaElement>();
  let menuOpen = $state(false);
  let menuBtn = $state<HTMLButtonElement>();

  /** Grow with the text, up to 40% of the panel's height. */
  function grow() {
    const el = area;
    if (!el) return;
    const panel = el.closest<HTMLElement>('.ai');
    const max = Math.max(120, (panel?.clientHeight ?? 500) * 0.4);
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight + 2, max)}px`;
    el.style.overflowY = el.scrollHeight + 2 > max ? 'auto' : 'hidden';
  }

  // Text set from outside (Ask AI about this, voice input, a cleared box after sending).
  $effect(() => {
    void ai.draft;
    void tick().then(grow);
  });

  $effect(() => {
    if (!running) menuOpen = false;
  });

  /** The composer's textarea gets the focus (after "Ask AI about this", say). */
  export function focus() {
    area?.focus();
  }

  function onKey(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      if (canSend) onsend();
    } else if (e.key === 'ArrowUp' && !ai.draft && !e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey) {
      if (oneditlast?.()) e.preventDefault();
    }
  }

  /** Text dropped on the box but outside the textarea goes in at the end. */
  function onDrop(e: DragEvent) {
    if (e.target === area) return; // the textarea inserts at the drop point itself
    const text = e.dataTransfer?.getData('text/plain');
    if (!text) return;
    e.preventDefault();
    ai.draft = ai.draft ? `${ai.draft}${/\s$/.test(ai.draft) ? '' : ' '}${text}` : text;
    area?.focus();
  }

  function closeMenu(focusButton = false) {
    menuOpen = false;
    if (focusButton) menuBtn?.focus();
  }
</script>

<svelte:window
  onclick={(e) => {
    if (menuOpen && !(e.target as Element).closest?.('.stop-group')) closeMenu();
  }}
/>

<form
  class="composer"
  onsubmit={(e) => {
    e.preventDefault();
    if (canSend) onsend();
  }}
  ondragover={(e) => {
    if (e.dataTransfer?.types.includes('text/plain')) e.preventDefault();
  }}
  ondrop={onDrop}
>
  <textarea
    bind:this={area}
    bind:value={ai.draft}
    rows="2"
    {placeholder}
    aria-label={t('aiChat.message')}
    aria-describedby="ai-composer-hint"
    oninput={grow}
    onkeydown={onKey}
  ></textarea>
  <div class="tools">
    <div class="composer-tools" role="group" aria-label={t('aiChat.composerTools')}>
      <!--
        Voice button goes here (bottom-left of the composer): the VOICE stream mounts its
        microphone button in this slot. It writes what it hears into `chrome.ai.draft`
        (the composer grows and Enter sends it like typed text).
      -->
    </div>
    {#if queued > 0}
      <span class="queued" title={t('aiChat.queuedTitle')}><Icon name="clock" size={11} />{tp('aiChat.queuedCount', queued)}</span>
    {/if}
    <span class="hint" id="ai-composer-hint">{running ? t('aiChat.escHint') : t('aiChat.newlineHint')}</span>
    <span class="spacer"></span>
    {#if running}
      <div class="stop-group">
        <button class="stop" type="button" aria-label={t('aiChat.interrupt')} title={t('aiChat.interruptTitle')} onclick={oninterrupt}>
          <Icon name="stop" size={13} stroke={2.25} />
        </button>
        <button
          class="more"
          type="button"
          bind:this={menuBtn}
          aria-label={t('aiChat.stopMenu')}
          title={t('aiChat.stopMenu')}
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          onclick={() => (menuOpen = !menuOpen)}
        >
          <Icon name="chevron-down" size={11} stroke={2.25} />
        </button>
        {#if menuOpen}
          <div class="menu" role="menu" aria-label={t('aiChat.stopMenu')}>
            <button
              role="menuitem"
              type="button"
              onclick={() => {
                closeMenu();
                onstop();
              }}
              onkeydown={(e) => {
                if (e.key === 'Escape') {
                  e.stopPropagation();
                  closeMenu(true);
                }
              }}
            >
              <Icon name="stop" size={13} />
              <span>
                <b>{t('aiChat.stopAgent')}</b>
                <small>{t('aiChat.stopAgentHint')}</small>
              </span>
            </button>
          </div>
        {/if}
      </div>
    {/if}
    <button class="send" type="submit" aria-label={t('aiChat.send')} disabled={!canSend}>
      <Icon name="send" size={15} stroke={2} />
    </button>
  </div>
</form>

<style>
  .composer {
    position: relative;
    margin: 0 10px 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    background: var(--surface);
  }
  .composer:focus-within {
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  textarea {
    display: block;
    width: 100%;
    min-height: 44px;
    padding: 9px 11px 2px;
    border: 0;
    border-radius: var(--r-lg) var(--r-lg) 0 0;
    background: transparent;
    resize: none;
    overflow-y: hidden;
    font-size: var(--fs);
    line-height: 1.45;
    user-select: text;
    outline: none;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 36px;
    padding: 0 6px 6px 8px;
  }
  .composer-tools {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .composer-tools:empty {
    display: none;
  }
  .queued {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 0 7px;
    height: 18px;
    border-radius: 9px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 650;
    white-space: nowrap;
  }
  .hint {
    min-width: 0;
    overflow: hidden;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .spacer {
    flex: 1;
  }
  .send,
  .stop,
  .more {
    display: grid;
    place-items: center;
    height: 28px;
    border: 0;
    flex: none;
  }
  .send {
    width: 28px;
    border-radius: var(--r);
    background: var(--accent);
    color: var(--on-accent);
  }
  .send:hover:not(:disabled) {
    background: color-mix(in oklab, var(--accent) 88%, var(--fg));
  }
  .send:disabled {
    background: var(--sunken);
    color: var(--fg-subtle);
  }
  .stop-group {
    position: relative;
    display: flex;
    border-radius: var(--r);
    background: var(--fg);
  }
  .stop {
    width: 28px;
    border-radius: var(--r) 0 0 var(--r);
    background: transparent;
    color: var(--surface);
  }
  .more {
    width: 18px;
    border-left: 1px solid color-mix(in oklab, var(--surface) 25%, transparent);
    border-radius: 0 var(--r) var(--r) 0;
    background: transparent;
    color: var(--surface);
  }
  .stop:hover,
  .more:hover {
    background: color-mix(in oklab, var(--surface) 15%, transparent);
  }
  .menu {
    position: absolute;
    right: 0;
    bottom: calc(100% + 6px);
    z-index: 10;
    width: 240px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    box-shadow: var(--shadow-pop);
  }
  .menu button {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    width: 100%;
    padding: 7px 8px;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--fg);
    text-align: left;
  }
  .menu button:hover,
  .menu button:focus-visible {
    background: var(--hover);
  }
  .menu :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--danger);
  }
  .menu b {
    display: block;
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .menu small {
    display: block;
    margin-top: 2px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    line-height: 1.35;
  }
</style>
