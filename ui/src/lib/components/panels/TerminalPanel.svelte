<script lang="ts">
  import EmptyPanel from './EmptyPanel.svelte';
</script>

<div class="term" aria-label="Terminal (not connected)">
  <div class="screen">
    <span class="prompt">PS C:\BrowserMCP&gt;</span><span class="cursor" aria-hidden="true"></span>
  </div>
  <EmptyPanel
    icon="terminal"
    title="Terminal is not connected"
    text="A real PTY shell (xterm.js) with Claude Code and Codex one keystroke away. It arrives with the built-in AI in phase 5."
  />
</div>

<style>
  .term {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .screen {
    margin: 10px;
    padding: 10px 12px;
    border-radius: var(--r);
    background: #0d0f12;
    color: #c9d1d9;
    font: var(--fs-sm) / 1.5 var(--font-mono);
  }
  .prompt {
    color: #7ee787;
  }
  .cursor {
    display: inline-block;
    width: 7px;
    height: 14px;
    margin-left: 6px;
    vertical-align: -2px;
    background: #c9d1d9;
    animation: blink 1.1s steps(1) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
</style>
