<script lang="ts">
  // A miniature browser window painted with one palette variant: tab strip, toolbar with the
  // address bar, a page with text lines, an accent button and a tab group chip. The tokens are
  // set on the preview itself, so it shows that palette whatever the page around it uses.
  import { TOKEN_NAMES, type Tokens } from '../theme/palettes';

  let { tokens }: { tokens: Tokens } = $props();
  const vars = $derived(TOKEN_NAMES.map((n) => `--${n}: ${tokens[n]}`).join('; '));
</script>

<div class="pv" style={vars} aria-hidden="true">
  <div class="strip">
    <span class="tab on"><i class="fav"></i><i class="ln"></i></span>
    <span class="tab"><i class="ln"></i></span>
    <span class="grp"></span>
  </div>
  <div class="bar"><span class="omni"><i class="ln"></i></span><span class="dot"></span></div>
  <div class="page">
    <i class="ln fg w80"></i>
    <i class="ln muted w60"></i>
    <i class="ln subtle w40"></i>
    <span class="row"><span class="btn"><i></i></span><span class="ok"></span><span class="warn"></span><span class="danger"></span></span>
  </div>
</div>

<style>
  .pv {
    display: flex;
    flex-direction: column;
    width: 100%;
    aspect-ratio: 16 / 10;
    overflow: hidden;
    border-radius: 6px;
    background: var(--frame);
    color-scheme: normal;
  }
  i {
    display: block;
  }
  .strip {
    display: flex;
    align-items: flex-end;
    gap: 3px;
    height: 20%;
    padding: 0 6px;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 3px;
    width: 30%;
    height: 72%;
    padding: 0 5px;
    border-radius: 4px 4px 0 0;
  }
  .tab .ln {
    flex: 1;
    height: 3px;
    border-radius: 2px;
    background: var(--fg-muted);
    opacity: 0.7;
  }
  .tab.on {
    background: var(--toolbar);
  }
  .tab.on .ln {
    background: var(--fg);
    opacity: 1;
  }
  .fav {
    width: 6px;
    height: 6px;
    border-radius: 2px;
    background: var(--accent);
  }
  .grp {
    align-self: center;
    width: 14%;
    height: 34%;
    margin-left: auto;
    border-radius: 3px;
    background: var(--g-green);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 20%;
    padding: 0 6px;
    background: var(--toolbar);
    border-bottom: 1px solid var(--border);
  }
  .omni {
    display: flex;
    align-items: center;
    flex: 1;
    height: 62%;
    padding: 0 6px;
    border-radius: 99px;
    background: var(--field);
  }
  .omni .ln {
    width: 46%;
    height: 3px;
    border-radius: 2px;
    background: var(--fg-subtle);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--fg-muted);
  }
  .page {
    display: flex;
    flex-direction: column;
    gap: 5px;
    flex: 1;
    margin: 6px 6px 0;
    padding: 8px;
    border: 1px solid var(--border);
    border-bottom: 0;
    border-radius: 4px 4px 0 0;
    background: var(--surface);
  }
  .page .ln {
    height: 4px;
    border-radius: 2px;
  }
  .fg {
    background: var(--fg);
  }
  .muted {
    background: var(--fg-muted);
  }
  .subtle {
    background: var(--fg-subtle);
  }
  .w80 {
    width: 80%;
  }
  .w60 {
    width: 60%;
  }
  .w40 {
    width: 40%;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: auto;
  }
  .btn {
    width: 32%;
    height: 9px;
    border-radius: 3px;
    display: grid;
    place-items: center;
    background: var(--accent);
  }
  .btn i {
    width: 55%;
    height: 2px;
    border-radius: 1px;
    background: var(--on-accent);
  }
  .ok,
  .warn,
  .danger {
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }
  .ok {
    background: var(--ok);
  }
  .warn {
    background: var(--warn);
  }
  .danger {
    background: var(--danger);
  }
</style>
