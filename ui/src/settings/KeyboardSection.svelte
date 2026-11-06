<script lang="ts">
  import Icon from '../lib/components/Icon.svelte';
  import { chordId, chordLabels, parseChord, type CommandId } from '../lib/keymap';
  import {
    conflicts,
    effectiveKeymap,
    filterRows,
    keymapRows,
    loadOverrides,
    onOverrides,
    recordChord,
    resetBinding,
    saveOverrides,
    setBinding,
    type KeymapOverrides,
  } from '../lib/settings/keys';

  let { notify }: { notify: (t: string, e?: boolean) => void } = $props();

  let overrides = $state<KeymapOverrides>(loadOverrides());
  let query = $state('');
  /** The command waiting for a key press. */
  let recording = $state<CommandId | null>(null);

  $effect(() => onOverrides(() => (overrides = loadOverrides())));

  const keymap = $derived(effectiveKeymap(overrides));
  const rows = $derived(filterRows(keymapRows(keymap, overrides), query));
  const clashes = $derived(conflicts(keymap));
  const customCount = $derived(Object.keys(overrides).length);

  function commit(next: KeymapOverrides) {
    overrides = next;
    saveOverrides(next);
  }

  function removeKey(id: CommandId, text: string) {
    commit(setBinding(overrides, id, keymap[id].filter((k) => k !== text)));
  }

  function onRecordKey(e: KeyboardEvent) {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape' && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      recording = null;
      return;
    }
    const text = recordChord(e);
    if (!text) return; // a modifier alone, or a plain letter
    const id = recording;
    recording = null;
    const c = parseChord(text);
    if (c && keymap[id].some((k) => sameKey(k, chordId(c)))) return;
    const next = setBinding(overrides, id, [...keymap[id], text]);
    commit(next);
    const other = c ? keymapRows(effectiveKeymap(next), next).find((r) => r.id !== id && r.keys.some((k) => sameKey(k, chordId(c)))) : undefined;
    if (other) notify(`${text} is also bound to “${other.title}”: the later command in the list wins`, true);
  }

  function sameKey(text: string, key: string): boolean {
    const c = parseChord(text);
    return !!c && chordId(c) === key;
  }

  function clashFor(text: string): boolean {
    const c = parseChord(text);
    return !!c && clashes.has(chordId(c));
  }
</script>

<svelte:window onkeydowncapture={onRecordKey} />

<div class="set-page">
  <header>
    <h2>Keyboard</h2>
    <p>
      Every shortcut of the browser chrome. Changes apply at once in this browser and are kept on this computer (they will move
      to <span class="set-mono">data/config/keymap.json</span> later). Keys pressed while a web page has the focus still use the
      built-in map for now.
    </p>
  </header>

  <section class="set-card" aria-labelledby="k-h">
    <h3 id="k-h">
      <span class="grow">Shortcuts</span>
      {#if customCount}
        <button class="btn" onclick={() => (commit({}), notify('All shortcuts are back to the defaults'))}>Reset all ({customCount})</button>
      {/if}
    </h3>
    <div class="set-row">
      <label class="lib-search">
        <Icon name="search" size={14} />
        <input class="lib-input" type="search" bind:value={query} placeholder="Search commands or keys (ctrl+t, history…)" aria-label="Search shortcuts" />
      </label>
      {#if clashes.size}<span class="set-err">{clashes.size} shortcut{clashes.size === 1 ? ' is' : 's are'} used twice</span>{/if}
    </div>
    <div class="set-table-wrap">
      <table class="set-table keys">
        <thead><tr><th>Command</th><th>Shortcuts</th><th><span class="sr">Actions</span></th></tr></thead>
        <tbody>
          {#each rows as r (r.id)}
            <tr>
              <td>
                <span class="title">{r.title}</span>
                <span class="set-mono id">{r.id}</span>
                {#if r.custom}<span class="lib-chip">changed</span>{/if}
              </td>
              <td>
                <span class="chords">
                  {#each r.keys as k (k)}
                    <span class="chord" class:clash={clashFor(k)} title={clashFor(k) ? 'Also bound to another command' : undefined}>
                      {#each chordLabels(k) as cap (cap)}<kbd>{cap}</kbd>{/each}
                      <button class="icon-btn" aria-label="Remove {k} from {r.title}" onclick={() => removeKey(r.id, k)}><Icon name="x" size={11} /></button>
                    </span>
                  {/each}
                  {#if recording === r.id}
                    <span class="rec" role="status">Press the keys… <kbd>Esc</kbd> cancels</span>
                  {:else if !r.keys.length}
                    <span class="set-muted">—</span>
                  {/if}
                </span>
              </td>
              <td class="r acts">
                <button class="btn sm" onclick={() => (recording = recording === r.id ? null : r.id)} aria-pressed={recording === r.id}>
                  <Icon name="plus" size={12} /> Add
                </button>
                {#if r.custom}
                  <button class="btn sm" onclick={() => commit(resetBinding(overrides, r.id))}>Reset</button>
                {/if}
              </td>
            </tr>
          {:else}
            <tr><td colspan="3" class="set-muted">No command matches “{query}”.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  </section>
</div>

<style>
  .lib-search {
    width: min(420px, 100%);
  }
  .keys td {
    padding-top: 7px;
    padding-bottom: 7px;
  }
  .title {
    margin-right: 8px;
  }
  .id {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .chords {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .chord {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 1px 1px 1px 4px;
    border-radius: var(--r-sm);
  }
  .chord:hover {
    background: var(--hover);
  }
  .chord .icon-btn {
    width: 18px;
    height: 18px;
    opacity: 0.55;
  }
  .chord:hover .icon-btn {
    opacity: 1;
  }
  .chord.clash kbd {
    border-color: var(--danger);
    color: var(--danger);
  }
  .rec {
    color: var(--accent);
    font-size: var(--fs-sm);
    font-weight: 550;
  }
  .acts {
    white-space: nowrap;
  }
  .btn.sm {
    height: 24px;
    padding: 0 8px;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
