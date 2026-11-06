<script lang="ts">
  import { diffStats, parseDiff } from '../../runs/heal';

  let { diff, maxHeight = 260 }: { diff: string; maxHeight?: number } = $props();

  const lines = $derived(parseDiff(diff));
  const stats = $derived(diffStats(lines));
</script>

<div class="diff" role="region" aria-label="Patch diff, {stats.added} lines added, {stats.removed} removed">
  <div class="head">
    <span class="add">+{stats.added}</span>
    <span class="del">−{stats.removed}</span>
  </div>
  <pre style:max-height="{maxHeight}px">{#each lines as l, i (i)}<span class="l {l.kind}"><i>{l.old ?? ''}</i><i>{l.new ?? ''}</i><b aria-hidden="true">{l.kind === 'add' ? '+' : l.kind === 'del' ? '−' : ' '}</b>{l.text}
</span>{:else}<span class="l meta"><i></i><i></i><b></b>(empty diff)
</span>{/each}</pre>
</div>

<style>
  .diff {
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--sunken);
    overflow: hidden;
  }
  .head {
    display: flex;
    gap: 8px;
    padding: 3px 8px;
    border-bottom: 1px solid var(--border);
    font: 600 10.5px / 16px var(--font-mono);
  }
  .add {
    color: var(--ok);
  }
  .del {
    color: var(--danger);
  }
  pre {
    margin: 0;
    padding: 3px 0;
    overflow: auto;
    font: 11px / 1.55 var(--font-mono);
  }
  .l {
    display: block;
    padding-right: 8px;
    white-space: pre;
  }
  .l i {
    display: inline-block;
    width: 3.5ch;
    padding-right: 4px;
    color: var(--fg-subtle);
    font-style: normal;
    text-align: right;
    user-select: none;
  }
  .l b {
    display: inline-block;
    width: 2ch;
    text-align: center;
    font-weight: 400;
    user-select: none;
  }
  .l.add {
    background: color-mix(in oklab, var(--ok) 15%, transparent);
  }
  .l.add b {
    color: var(--ok);
  }
  .l.del {
    background: color-mix(in oklab, var(--danger) 13%, transparent);
  }
  .l.del b {
    color: var(--danger);
  }
  .l.hunk {
    color: var(--accent);
    background: color-mix(in oklab, var(--accent) 8%, transparent);
  }
  .l.meta {
    color: var(--fg-subtle);
  }
</style>
