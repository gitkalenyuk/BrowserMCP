<script lang="ts">
  import Icon from '../../lib/components/Icon.svelte';
  import { t } from '../../lib/i18n/index.svelte';
  import { chordLabels, type Keymap } from '../../lib/keymap';
  import { isSettingsSection, type SettingsSection } from '../../lib/settings/open';
  import type { GuideAnchor } from './anchors';
  import ApiExample from './ApiExample.svelte';
  import CodeBlock from './CodeBlock.svelte';
  import InlineText from './InlineText.svelte';
  import type { Block, Chapter, SectionTarget } from './model';

  let {
    block,
    words,
    chapters,
    keymap,
    onfail,
    onopen,
    ongo,
  }: {
    block: Block;
    words: string[];
    chapters: Chapter[];
    keymap: Keymap;
    onfail: (message: string) => void;
    onopen: (section: SettingsSection) => void;
    ongo: (anchor: GuideAnchor) => void;
  } = $props();

  /** Settings sections that exist are named by the shell's nav labels, pending ones by ours. */
  function sectionName(id: SectionTarget): string {
    return t(isSettingsSection(id) ? `settings.nav.${id}` : `guide.pendingSection.${id}`);
  }

  function seeTitle(chapter: string, section?: string): string {
    const ch = chapters.find((c) => c.id === chapter);
    if (!ch) return chapter;
    const s = section ? ch.sections.find((x) => x.id === section) : undefined;
    return s ? `${ch.title} → ${s.title}` : ch.title;
  }
</script>

{#snippet chord(text: string)}<span class="chord">{#each chordLabels(text) as k, i (i)}<kbd>{k}</kbd>{/each}</span>{/snippet}

{#if block.type === 'p'}
  <p><InlineText text={block.text} {words} /></p>
{:else if block.type === 'steps'}
  <ol class="steps">
    {#each block.items as item, i (i)}<li><InlineText text={item} {words} /></li>{/each}
  </ol>
{:else if block.type === 'list'}
  <ul class="list">
    {#each block.items as item, i (i)}<li><InlineText text={item} {words} /></li>{/each}
  </ul>
{:else if block.type === 'tip' || block.type === 'warning'}
  <aside class="callout {block.type}">
    <Icon name={block.type === 'tip' ? 'info' : 'alert'} size={15} />
    <p><strong>{t(block.type === 'tip' ? 'guide.tip' : 'guide.warning')}.</strong> <InlineText text={block.text} {words} /></p>
  </aside>
{:else if block.type === 'code'}
  <CodeBlock code={block.code} lang={block.lang} {words} {onfail} />
{:else if block.type === 'api'}
  <ApiExample command={block.command} args={block.args} text={block.text} {words} {onfail} />
{:else if block.type === 'keys'}
  <table class="keys">
    <tbody>
      {#each block.items as item, i (i)}
        {@const combos = 'keys' in item ? item.keys : keymap[item.command]}
        <tr>
          <td class="combo">
            {#if combos.length}
              {#each combos as combo, j (j)}{#if j > 0}<span class="or">/</span>{/if}{@render chord(combo)}{/each}
            {:else}
              <span class="none">{t('guide.keys.none')}</span>
            {/if}
          </td>
          <td><InlineText text={item.text} {words} /></td>
        </tr>
      {/each}
    </tbody>
  </table>
{:else if block.type === 'settings'}
  <div class="settings-link">
    {#if isSettingsSection(block.section)}
      {@const target = block.section}
      <button class="btn" onclick={() => onopen(target)}>
        <Icon name="settings" size={14} />
        {t('guide.openSettings', { name: sectionName(block.section) })}
      </button>
    {:else}
      <span class="pending"><Icon name="settings" size={14} /> {t('guide.settingsPath', { name: sectionName(block.section) })}</span>
    {/if}
    {#if block.text}<span class="why"><InlineText text={block.text} {words} /></span>{/if}
  </div>
{:else if block.type === 'see'}
  {@const target = { chapter: block.chapter, section: block.section }}
  <p class="see">
    <button class="linkish" onclick={() => ongo(target)}>
      <Icon name="arrow-right" size={13} />
      {t('guide.see', { title: seeTitle(block.chapter, block.section) })}
    </button>
    {#if block.text}<span class="why"><InlineText text={block.text} {words} /></span>{/if}
  </p>
{:else if block.type === 'terms'}
  <dl class="terms">
    {#each block.items as item, i (i)}
      <dt><InlineText text={item.term} {words} /></dt>
      <dd><InlineText text={item.text} {words} /></dd>
    {/each}
  </dl>
{/if}

<style>
  p {
    margin: 8px 0;
    line-height: 1.6;
  }
  .steps,
  .list {
    margin: 8px 0;
    padding-left: 24px;
    line-height: 1.6;
  }
  .steps li,
  .list li {
    margin: 3px 0;
    padding-left: 2px;
  }
  .steps li::marker {
    color: var(--accent);
    font-weight: 650;
  }
  .callout {
    display: flex;
    gap: 10px;
    margin: 10px 0;
    padding: 9px 12px;
    border-radius: var(--r);
    border: 1px solid var(--accent-line);
    background: var(--accent-soft);
  }
  .callout :global(svg) {
    flex: none;
    margin-top: 4px;
    color: var(--accent);
  }
  .callout.warning {
    border-color: color-mix(in oklab, var(--warn) 45%, transparent);
    background: color-mix(in oklab, var(--warn) 10%, transparent);
  }
  .callout.warning :global(svg) {
    color: var(--warn);
  }
  .callout p {
    margin: 0;
  }
  .keys {
    margin: 8px 0;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }
  .keys td {
    padding: 5px 14px 5px 0;
    border-bottom: 1px solid var(--border);
    vertical-align: top;
    line-height: 1.5;
  }
  .keys tr:last-child td {
    border-bottom: 0;
  }
  .combo {
    white-space: nowrap;
  }
  .chord {
    display: inline-flex;
    gap: 2px;
  }
  .or {
    margin: 0 5px;
    color: var(--fg-subtle);
  }
  .none {
    color: var(--fg-subtle);
    font-style: italic;
  }
  .settings-link,
  .see {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    margin: 10px 0;
  }
  .pending {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    font-weight: 550;
  }
  .why {
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
  .linkish {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-weight: 550;
    text-align: left;
  }
  .linkish:hover {
    text-decoration: underline;
  }
  .terms {
    display: grid;
    grid-template-columns: minmax(120px, max-content) minmax(0, 1fr);
    gap: 6px 18px;
    margin: 10px 0;
    line-height: 1.5;
  }
  .terms dt {
    font-weight: 600;
  }
  .terms dd {
    margin: 0;
    color: var(--fg-muted);
  }
  @media (max-width: 720px) {
    .terms {
      grid-template-columns: minmax(0, 1fr);
    }
    .terms dd {
      margin-bottom: 6px;
    }
  }
</style>
