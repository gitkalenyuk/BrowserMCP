<script lang="ts">
  /**
   * Voice dictation for any prompt field: a mic button with a level meter and a status line.
   * Drop it next to a textarea / input:
   *
   *   <VoiceInput {api} target={field} onsubmit={send} context={() => pageTitleAndSite} />
   *
   * Hotkeys work while `target` has focus (Space on an empty prompt by default, see
   * Settings → Voice). Hidden while dictation is off in the settings.
   */
  import type { BrowserApi } from '../../api';
  import { t } from '../../i18n/index.svelte';
  import { VoiceController, type Field } from '../../voice/controller.svelte';
  import Icon from '../Icon.svelte';

  let {
    api,
    target,
    onsubmit,
    context,
    onsetup,
    size = 32,
  }: {
    api: BrowserApi;
    /** The prompt the transcript goes into. */
    target: Field | null | undefined;
    /** Send the prompt (auto-submit after ≥3 words, when enabled). */
    onsubmit?: () => void;
    /** Vocabulary hint for the engine, e.g. "GitHub — github.com" (the current page). */
    context?: () => string | undefined;
    /** Open Settings → Voice (shown with "set it up" errors). */
    onsetup?: () => void;
    size?: number;
  } = $props();

  // svelte-ignore state_referenced_locally
  const voice = new VoiceController(api, { onsubmit, context });
  $effect(() => voice.setCallbacks({ onsubmit, context }));

  $effect(() => {
    void voice.loadSettings();
  });
  $effect(() => {
    if (!target) return;
    return voice.attach(target);
  });

  const key = $derived(voice.hotkeyLabel === 'Space' ? t('voice.key.space') : voice.hotkeyLabel);
  const recording = $derived(voice.phase === 'recording' || voice.phase === 'starting');

  const status = $derived.by((): { text: string; tone: 'rec' | 'busy' | 'info' | 'err' | 'hint'; detail?: string } | null => {
    const n = voice.notice;
    if (voice.phase === 'paused') return { text: t('voice.status.paused', { s: voice.pausedFor }), tone: 'err' };
    if (voice.phase === 'warmup') return { text: t('voice.status.keepHolding'), tone: 'busy' };
    if (recording) {
      if (voice.mode === 'tap') {
        const send = voice.settings?.auto_submit_tap;
        return { text: send ? t('voice.status.recTapSend') : t('voice.status.recTapStop'), tone: 'rec' };
      }
      return { text: voice.interim || voice.phase === 'starting' ? t('voice.status.listening') : t('voice.status.listeningBatch'), tone: 'rec' };
    }
    if (voice.phase === 'transcribing') return { text: t('voice.status.transcribing'), tone: 'busy' };
    if (n) {
      switch (n.kind) {
        case 'denied':
          return { text: t('voice.err.denied'), tone: 'err' };
        case 'no-device':
          return { text: t('voice.err.noDevice'), tone: 'err' };
        case 'unsupported':
          return { text: t('voice.err.unsupported'), tone: 'err' };
        case 'silence':
          return { text: t('voice.status.silence'), tone: 'info' };
        case 'limit':
          return { text: t('voice.status.limit'), tone: 'info' };
        case 'nothing':
          return { text: t('voice.status.nothing'), tone: 'info' };
        case 'setup':
          return { text: t('voice.err.setup'), tone: 'err', detail: n.text };
        case 'paused':
          return null;
        default:
          return { text: t('voice.err.failed', { error: n.text ?? '' }), tone: 'err' };
      }
    }
    if (voice.hint) return { text: voice.settings?.mode === 'tap' ? t('voice.hint.tap', { key }) : t('voice.hint.hold', { key }), tone: 'hint' };
    return null;
  });

  const title = $derived(
    recording ? t('voice.button.stop') : voice.settings?.mode === 'tap' ? t('voice.button.titleTap', { key }) : t('voice.button.titleHold', { key }),
  );
</script>

{#if voice.enabled}
  <div class="voice" style:--size="{size}px" style:--lvl={voice.level.toFixed(3)}>
    <button
      type="button"
      class="mic"
      class:rec={recording}
      class:busy={voice.phase === 'transcribing'}
      aria-pressed={recording}
      aria-label={title}
      {title}
      disabled={voice.phase === 'paused'}
      onmousedown={(e) => e.preventDefault()}
      onpointerdown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
        voice.pointerDown();
      }}
      onpointerup={() => voice.pointerUp()}
      onpointercancel={() => voice.pointerUp()}
      onclick={(e) => {
        // Keyboard activation (Enter / Space on the focused button) has no pointer events.
        if (e.detail === 0) voice.toggle();
      }}
    >
      <span class="ring" aria-hidden="true"></span>
      {#if voice.phase === 'transcribing'}
        <span class="spinner" aria-hidden="true"></span>
      {:else}
        <Icon name="mic" size={Math.round(size * 0.5)} stroke={2} />
      {/if}
    </button>
    {#if status}
      <div class="status {status.tone}" role={status.tone === 'err' ? 'alert' : 'status'} aria-live="polite" title={status.detail}>
        {#if status.tone === 'rec'}<span class="dot" aria-hidden="true"></span>{/if}
        <span class="txt">{status.text}</span>
        {#if recording}
          <span class="meter" aria-hidden="true"><span class="fill"></span></span>
          <kbd>Esc</kbd>
        {/if}
        {#if voice.notice?.kind === 'setup' && onsetup}
          <button type="button" class="link" onclick={() => onsetup?.()}>{t('voice.err.openSettings')}</button>
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .voice {
    position: relative;
    display: inline-flex;
    flex: none;
  }
  .mic {
    position: relative;
    display: inline-grid;
    place-items: center;
    width: var(--size);
    height: var(--size);
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--fg-muted);
    transition:
      background-color 120ms,
      color 120ms;
    touch-action: none;
  }
  .mic:hover:not(:disabled) {
    background: var(--hover);
    color: var(--fg);
  }
  .mic:disabled {
    opacity: 0.4;
  }
  .mic.rec {
    background: color-mix(in oklab, var(--danger) 14%, transparent);
    color: var(--danger);
  }
  .mic :global(svg) {
    position: relative;
  }
  /* The level meter around the button: grows with the microphone level. */
  .ring {
    position: absolute;
    inset: 0;
    border-radius: 50%;
    border: 2px solid color-mix(in oklab, var(--danger) 55%, transparent);
    opacity: 0;
    transform: scale(1);
    transition:
      transform 90ms linear,
      opacity 120ms;
    pointer-events: none;
  }
  .rec .ring {
    opacity: calc(0.35 + var(--lvl) * 0.65);
    transform: scale(calc(1 + var(--lvl) * 0.45));
  }
  .spinner {
    width: calc(var(--size) * 0.42);
    height: calc(var(--size) * 0.42);
    border: 2px solid var(--border-strong);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .status {
    position: absolute;
    left: 0;
    bottom: calc(100% + 8px);
    z-index: 5;
    display: flex;
    align-items: center;
    gap: 7px;
    max-width: min(440px, 80vw);
    padding: 5px 10px;
    border: 1px solid var(--border);
    border-radius: 14px;
    background: var(--surface);
    box-shadow: var(--shadow-pop);
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    line-height: 1.35;
    white-space: nowrap;
  }
  .status .txt {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .status.err {
    color: var(--danger);
    white-space: normal;
  }
  .status.rec {
    color: var(--fg);
    font-weight: 550;
  }
  .status.hint {
    color: var(--fg-subtle);
    box-shadow: none;
  }
  .dot {
    width: 8px;
    height: 8px;
    flex: none;
    border-radius: 50%;
    background: var(--danger);
  }
  .meter {
    position: relative;
    width: 44px;
    height: 4px;
    flex: none;
    overflow: hidden;
    border-radius: 2px;
    background: var(--sunken);
  }
  .meter .fill {
    position: absolute;
    inset: 0;
    background: var(--danger);
    transform-origin: left;
    transform: scaleX(var(--lvl));
    transition: transform 90ms linear;
  }
  .status kbd {
    flex: none;
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    font-weight: 600;
    text-decoration: underline;
    cursor: pointer;
    white-space: nowrap;
  }
  @media (prefers-reduced-motion: reduce) {
    /* No growing ring or sliding meter: the level is shown without movement. */
    .ring,
    .meter .fill {
      transition: none;
    }
    .rec .ring {
      transform: none;
      opacity: 1;
    }
    .meter .fill {
      transform: none;
      opacity: calc(0.25 + var(--lvl) * 0.75);
    }
    .spinner {
      animation-duration: 2.4s;
    }
  }
</style>
