<script lang="ts">
  import { t, tp } from '../../i18n/index.svelte';
  import type { RunInfo } from '../../api/types-record-runs';
  import type { HealProposal } from '../../api/types-settings';
  import { confidenceLevel, confidencePct, emptyHeal, pendingProposal } from '../../runs/heal';
  import { runsStore } from '../../runs/store.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import DiffView from './DiffView.svelte';

  /**
   * The AI side of a paused run (crates/heal): the pending proposal with its diff, Accept /
   * Reject with feedback, "Propose" when there is none (or after a failure), earlier attempts.
   * `paused`: the run waits for a patch (run.heal_needed). `onedit` opens the proposal's code in
   * the manual patch editor.
   */
  let { run, paused, onedit }: { run: RunInfo; paused: boolean; onedit: (code: string, script: string) => void } = $props();

  const store = runsStore(useChrome().api);
  const hs = $derived(store.heal[run.id] ?? emptyHeal());
  const pending = $derived(pendingProposal(hs));
  const earlier = $derived(hs.proposals.filter((p) => p !== pending));

  let busy = $state<'propose' | 'accept' | 'reject' | null>(null);
  let error = $state<string | null>(null);
  let rejecting = $state(false);
  let feedback = $state('');
  let showEarlier = $state(false);

  $effect(() => {
    // Proposals live in the core's memory: pick up what is there when the run is opened.
    void store.loadProposals(run.id);
  });

  async function act(kind: 'propose' | 'accept' | 'reject', f: () => Promise<unknown>) {
    busy = kind;
    error = null;
    try {
      await f();
      if (kind !== 'accept') feedback = '';
      rejecting = false;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = null;
    }
  }

  const propose = () => act('propose', () => store.propose(run.id, feedback));
  const accept = (p: HealProposal) => act('accept', () => store.accept(p));
  const reject = (p: HealProposal) => act('reject', () => store.reject(p, feedback));

  const mode = $derived(run.options.heal_mode ?? (run.options.heal ? 'ask' : 'off'));
</script>

{#if hs.applied && !paused}
  <div class="rr-note ok applied" role="status">
    <span>
      <Icon name="check" size={12} stroke={2.25} />
      {hs.applied.auto ? t('runs.heal.auto') : t('runs.heal.accepted')}: {t('runs.heal.continues')} <span class="rr-mono">{hs.applied.script}</span>,
      {t('runs.heal.requeued', { rows: tp('runs.form.rows', hs.applied.requeued) })}
    </span>
  </div>
{/if}

{#if paused}
  <div class="ai" aria-live="polite">
    {#if pending}
      {@const level = confidenceLevel(pending.confidence)}
      <div class="card" role="group" aria-label={t('runs.heal.proposal')}>
        <div class="top">
          <span class="lbl"><Icon name="sparkles" size={13} /> {t('runs.heal.aiPatch')}</span>
          <span class="conf {level}" title={t('runs.heal.confidenceHint')}>{t('runs.heal.confident', { pct: confidencePct(pending.confidence) })}</span>
          <span class="rr-hint">{t('runs.heal.attempt', { n: pending.attempt, max: pending.max_attempts })}{pending.provider ? ` · ${pending.provider}${pending.model ? ` ${pending.model}` : ''}` : ''}</span>
        </div>
        {#if pending.explanation}<p class="expl">{pending.explanation}</p>{/if}
        {#if pending.checks.length}
          <div class="checks" aria-label={t('runs.heal.checks')}>
            {#each pending.checks as c (c)}<span class="chk"><Icon name="check" size={10} stroke={2.5} />{c}</span>{/each}
          </div>
        {/if}
        {#if hs.note}<p class="rr-hint">{t('runs.heal.notAuto', { note: hs.note })}</p>{/if}
        <DiffView diff={pending.diff} />
        {#if rejecting}
          <label class="rr-field">
            <span>{t('runs.heal.whatWrong')} <span class="opt">{t('runs.heal.whatWrongHint')}</span></span>
            <textarea rows="2" bind:value={feedback} placeholder={t('runs.heal.whatWrongPlaceholder')} spellcheck="true"></textarea>
          </label>
          <div class="rr-row">
            <button class="btn primary" disabled={busy !== null} onclick={() => reject(pending)}>
              {busy === 'reject' ? t('runs.heal.askingAnother') : pending.attempt >= pending.max_attempts ? t('newtab.patch.reject') : t('runs.heal.rejectRetry')}
            </button>
            <button class="btn" disabled={busy !== null} onclick={() => ((rejecting = false), (feedback = ''))}>{t('common.back')}</button>
          </div>
        {:else}
          <div class="rr-row">
            <button class="btn primary" disabled={busy !== null} onclick={() => accept(pending)}>
              <Icon name="check" size={13} stroke={2.25} />
              {busy === 'accept' ? t('runs.heal.applying') : t('runs.heal.accept')}
            </button>
            <button class="btn" disabled={busy !== null} onclick={() => (rejecting = true)}>{t('runs.heal.rejectDots')}</button>
            {#if pending.patched_code}
              <button class="btn" disabled={busy !== null} onclick={() => onedit(pending.patched_code, pending.script)} title={t('runs.heal.editHint')}>
                <Icon name="pencil" size={12} /> {t('runs.heal.edit')}
              </button>
            {/if}
          </div>
        {/if}
      </div>
    {:else}
      <div class="card none">
        {#if hs.failed}
          <p class="err-line" role="alert">
            <b>{t('runs.heal.noPatch')}</b>{hs.failed.attempts ? ` ${t('runs.heal.after', { attempts: tp('runs.detail.attempts', hs.failed.attempts) })}` : ''}: {hs.failed.error}
          </p>
        {:else if busy === 'propose'}
          <p class="rr-hint">{t('runs.heal.reading')}</p>
        {:else}
          <p class="rr-hint">
            {mode === 'off'
              ? t('runs.heal.off')
              : t('runs.heal.none')}
          </p>
        {/if}
        <label class="rr-field">
          <span>{t('runs.heal.hint')} <span class="opt">{t('runs.heal.optional')}</span></span>
          <input bind:value={feedback} placeholder={t('runs.heal.hintPlaceholder')} spellcheck="true" />
        </label>
        <div class="rr-row">
          <button class="btn primary" disabled={busy !== null} onclick={propose}>
            <Icon name="sparkles" size={13} />
            {busy === 'propose' ? t('runs.heal.asking') : hs.failed ? t('runs.heal.proposeAgain') : t('runs.heal.propose')}
          </button>
        </div>
      </div>
    {/if}
    {#if error}<p class="err-line" role="alert">{error}</p>{/if}
  </div>
{/if}

{#if earlier.length}
  <div class="earlier">
    <button class="link" aria-expanded={showEarlier} onclick={() => (showEarlier = !showEarlier)}>
      <Icon name={showEarlier ? 'chevron-down' : 'chevron-right'} size={12} /> {t('runs.heal.earlier', { n: earlier.length })}
    </button>
    {#if showEarlier}
      {#each earlier as p (p.proposal_id)}
        <details class="old">
          <summary>
            <span class="rr-pill {p.status === 'accepted' ? 'ok' : p.status === 'rejected' ? 'failed' : ''}">{t(`runs.heal.status.${p.status}`)}</span>
            {t('runs.heal.attemptN', { n: p.attempt })} · {confidencePct(p.confidence)} · {p.explanation}
          </summary>
          <DiffView diff={p.diff} maxHeight={180} />
        </details>
      {/each}
    {/if}
  </div>
{/if}

<style>
  .ai {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 9px;
    border: 1px solid color-mix(in oklab, var(--accent) 30%, var(--border));
    border-radius: var(--r);
    background: var(--surface);
  }
  .card.none {
    border-color: var(--border);
  }
  .top {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .lbl {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 650;
  }
  .conf {
    padding: 0 7px;
    border-radius: 9px;
    font-size: 10.5px;
    font-weight: 650;
    line-height: 17px;
  }
  .conf.high {
    background: color-mix(in oklab, var(--ok) 14%, transparent);
    color: var(--ok);
  }
  .conf.medium {
    background: color-mix(in oklab, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .conf.low {
    background: color-mix(in oklab, var(--danger) 14%, transparent);
    color: var(--danger);
  }
  .expl {
    margin: 0;
    font-size: var(--fs-sm);
    line-height: 1.45;
  }
  .checks {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chk {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 0 6px;
    border-radius: 8px;
    background: var(--sunken);
    color: var(--fg-muted);
    font: 10.5px / 16px var(--font-mono);
  }
  .chk :global(svg) {
    color: var(--ok);
  }
  .err-line {
    margin: 0;
    color: var(--danger);
    font-size: var(--fs-xs);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  .applied {
    border: 0;
  }
  .applied span :global(svg) {
    vertical-align: -1px;
  }
  .earlier {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    align-self: flex-start;
    padding: 2px 0;
    border: 0;
    background: none;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .old summary {
    padding: 3px 0;
    overflow: hidden;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }
</style>
