<script lang="ts">
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
  const STATUS = { accepted: 'Accepted', rejected: 'Rejected', superseded: 'Replaced', pending: 'Waiting' } as const;
</script>

{#if hs.applied && !paused}
  <div class="rr-note ok applied" role="status">
    <span>
      <Icon name="check" size={12} stroke={2.25} />
      {hs.applied.auto ? 'Patched automatically' : 'Patch accepted'}: continues with <span class="rr-mono">{hs.applied.script}</span>,
      {hs.applied.requeued} row{hs.applied.requeued === 1 ? '' : 's'} back in the queue.
    </span>
  </div>
{/if}

{#if paused}
  <div class="ai" aria-live="polite">
    {#if pending}
      {@const level = confidenceLevel(pending.confidence)}
      <div class="card" role="group" aria-label="AI patch proposal">
        <div class="top">
          <span class="lbl"><Icon name="sparkles" size={13} /> AI patch</span>
          <span class="conf {level}" title="The model's own estimate. Auto mode applies from 80%.">{confidencePct(pending.confidence)} confident</span>
          <span class="rr-hint">attempt {pending.attempt}/{pending.max_attempts}{pending.provider ? ` · ${pending.provider}${pending.model ? ` ${pending.model}` : ''}` : ''}</span>
        </div>
        {#if pending.explanation}<p class="expl">{pending.explanation}</p>{/if}
        {#if pending.checks.length}
          <div class="checks" aria-label="Checks passed">
            {#each pending.checks as c (c)}<span class="chk"><Icon name="check" size={10} stroke={2.5} />{c}</span>{/each}
          </div>
        {/if}
        {#if hs.note}<p class="rr-hint">Not applied automatically: {hs.note}</p>{/if}
        <DiffView diff={pending.diff} />
        {#if rejecting}
          <label class="rr-field">
            <span>What is wrong with it? <span class="opt">the AI gets this for the next attempt</span></span>
            <textarea rows="2" bind:value={feedback} placeholder="e.g. the button is called “Log in” now, keep the wait" spellcheck="true"></textarea>
          </label>
          <div class="rr-row">
            <button class="btn primary" disabled={busy !== null} onclick={() => reject(pending)}>
              {busy === 'reject' ? 'Asking for another…' : pending.attempt >= pending.max_attempts ? 'Reject' : 'Reject & try again'}
            </button>
            <button class="btn" disabled={busy !== null} onclick={() => ((rejecting = false), (feedback = ''))}>Back</button>
          </div>
        {:else}
          <div class="rr-row">
            <button class="btn primary" disabled={busy !== null} onclick={() => accept(pending)}>
              <Icon name="check" size={13} stroke={2.25} />
              {busy === 'accept' ? 'Applying…' : 'Accept & continue'}
            </button>
            <button class="btn" disabled={busy !== null} onclick={() => (rejecting = true)}>Reject…</button>
            {#if pending.patched_code}
              <button class="btn" disabled={busy !== null} onclick={() => onedit(pending.patched_code, pending.script)} title="Open the patched script in the editor below">
                <Icon name="pencil" size={12} /> Edit by hand
              </button>
            {/if}
          </div>
        {/if}
      </div>
    {:else}
      <div class="card none">
        {#if hs.failed}
          <p class="err-line" role="alert">
            <b>No usable patch</b>{hs.failed.attempts ? ` after ${hs.failed.attempts} attempt${hs.failed.attempts === 1 ? '' : 's'}` : ''}: {hs.failed.error}
          </p>
        {:else if busy === 'propose'}
          <p class="rr-hint">The AI is reading the error, the page snapshot and the script…</p>
        {:else}
          <p class="rr-hint">
            {mode === 'off'
              ? 'Self-healing is off for this run: ask the AI once, or write the patch yourself.'
              : 'No AI proposal yet. With healing on, one usually arrives by itself; you can also ask now.'}
          </p>
        {/if}
        <label class="rr-field">
          <span>Hint for the AI <span class="opt">optional</span></span>
          <input bind:value={feedback} placeholder="e.g. the sign-in button moved into the header" spellcheck="true" />
        </label>
        <div class="rr-row">
          <button class="btn primary" disabled={busy !== null} onclick={propose}>
            <Icon name="sparkles" size={13} />
            {busy === 'propose' ? 'Asking the AI…' : hs.failed ? 'Propose again' : 'Propose a patch'}
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
      <Icon name={showEarlier ? 'chevron-down' : 'chevron-right'} size={12} /> Earlier proposals ({earlier.length})
    </button>
    {#if showEarlier}
      {#each earlier as p (p.proposal_id)}
        <details class="old">
          <summary>
            <span class="rr-pill {p.status === 'accepted' ? 'ok' : p.status === 'rejected' ? 'failed' : ''}">{STATUS[p.status]}</span>
            attempt {p.attempt} · {confidencePct(p.confidence)} · {p.explanation}
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
