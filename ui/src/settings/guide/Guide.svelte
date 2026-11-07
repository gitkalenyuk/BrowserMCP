<script lang="ts">
  // The user guide inside Settings: table of contents, search across every chapter, and one
  // chapter at a time. Content: content/<locale>/*.ts (follows the UI language).
  import { tick, untrack } from 'svelte';
  import Icon from '../../lib/components/Icon.svelte';
  import { locale, t, tp } from '../../lib/i18n/index.svelte';
  import { effectiveKeymap, loadOverrides, onOverrides, type KeymapOverrides } from '../../lib/settings/keys';
  import type { SettingsSection } from '../../lib/settings/open';
  import { resolveAnchor, type GuideAnchor } from './anchors';
  import { GUIDE } from './content';
  import GuideBlock from './GuideBlock.svelte';
  import { highlight, queryWords } from './inline';
  import { provideKeymap } from './keymap-context';
  import { lastChapter, rememberChapter } from './prefs.svelte';
  import { searchGuide } from './search';

  let {
    target,
    onconsumed,
    onopen,
    onfail,
  }: {
    /**
     * Where to go once (the Help link, a deep link). The guide calls `onconsumed` after going
     * there and the parent clears it, so coming back to the guide later shows the remembered
     * chapter instead of replaying an old jump.
     */
    target: { anchor: GuideAnchor | null } | null;
    onconsumed: () => void;
    onopen: (section: SettingsSection) => void;
    onfail: (message: string) => void;
  } = $props();

  const chapters = $derived(GUIDE[locale()]);
  let chapterId = $state<string | null>(lastChapter());
  let query = $state('');
  /** A search result was opened: show the chapter (matches stay marked) instead of the results. */
  let reading = $state(false);
  let root = $state<HTMLElement>();
  let overrides = $state<KeymapOverrides>(loadOverrides());

  $effect(() => onOverrides(() => (overrides = loadOverrides())));
  const keymap = $derived(effectiveKeymap(overrides));
  // Inline `[[cmd:<id>]]` marks in any block show the same (current) keys.
  provideKeymap(() => keymap);

  const index = $derived(chapters.findIndex((c) => c.id === chapterId));
  const chapter = $derived(index >= 0 ? chapters[index] : null);
  const words = $derived(queryWords(query));
  const results = $derived(searchGuide(chapters, words));
  const showResults = $derived(words.length > 0 && !reading);
  const marks = $derived(reading ? words : []);

  $effect(() => {
    const next = target;
    // Only a new target navigates: go() reads state that must not re-run this effect.
    if (next)
      untrack(() => {
        void go(next.anchor);
        onconsumed();
      });
  });

  /** Show a chapter (null = the overview), scrolled to `anchor.section` when given. */
  async function go(anchor: GuideAnchor | null, fromSearch = false) {
    const found = resolveAnchor(chapters, anchor);
    chapterId = found?.chapter ?? null;
    reading = fromSearch;
    if (!fromSearch) query = '';
    rememberChapter(chapterId);
    await tick();
    const el = found?.section ? document.getElementById(sectionDomId(found.chapter, found.section)) : null;
    if (el) el.scrollIntoView({ block: 'start' });
    else root?.scrollIntoView({ block: 'start' });
    // Move keyboard focus to what is now shown (screen readers announce it).
    (el?.querySelector('h3') ?? root?.querySelector<HTMLElement>('.focus-target'))?.focus({ preventScroll: true });
  }

  function sectionDomId(ch: string, s: string): string {
    return `guide-${ch}-${s}`;
  }

  function onSearchInput() {
    reading = false;
  }
</script>

<div class="guide" bind:this={root}>
  <header class="top">
    <div class="titles">
      <h2 class="focus-target" tabindex="-1">{t('guide.title')}</h2>
      <p>{t('guide.lead')}</p>
    </div>
    <div class="lib-search" role="search">
      <Icon name="search" size={14} />
      <input
        class="lib-input"
        type="search"
        bind:value={query}
        oninput={onSearchInput}
        placeholder={t('guide.search.placeholder')}
        aria-label={t('guide.search.label')}
      />
    </div>
  </header>

  <div class="layout">
    <nav class="toc" aria-label={t('guide.contents')}>
      <button class="toc-item" aria-current={chapterId === null && !showResults ? 'page' : undefined} onclick={() => go(null)}>
        <span class="num"><Icon name="menu" size={12} /></span><span>{t('guide.overview')}</span>
      </button>
      {#each chapters as c, i (c.id)}
        <button class="toc-item" aria-current={c.id === chapterId && !showResults ? 'page' : undefined} onclick={() => go({ chapter: c.id })}>
          <span class="num">{i + 1}</span><span>{c.title}</span>
        </button>
      {/each}
    </nav>

    <label class="toc-select">
      <span class="sr-only">{t('guide.chapterSelect')}</span>
      <select class="lib-input" value={chapterId ?? ''} onchange={(e) => go(e.currentTarget.value ? { chapter: e.currentTarget.value } : null)}>
        <option value="">{t('guide.overview')}</option>
        {#each chapters as c, i (c.id)}<option value={c.id}>{i + 1}. {c.title}</option>{/each}
      </select>
    </label>

    <div class="content">
      {#if showResults}
        <section aria-live="polite">
          <h3 class="results-head">{tp('guide.results', results.length)}</h3>
          {#if !results.length}
            <p class="empty">{t('guide.noResults', { query })}</p>
          {/if}
          {#each results as r (r.chapter)}
            <article class="result">
              <button class="linkish title" onclick={() => go({ chapter: r.chapter }, true)}>
                {#each highlight(r.title, words) as piece, i (i)}{#if piece.hit}<mark>{piece.text}</mark>{:else}{piece.text}{/if}{/each}
              </button>
              <p class="summary">{r.summary}</p>
              {#each r.sections.slice(0, 4) as s (s.section)}
                <button class="hit" onclick={() => go({ chapter: r.chapter, section: s.section }, true)}>
                  <span class="hit-title">{s.title}</span>
                  <span class="hit-text"
                    >{#each highlight(s.snippet, words) as piece, i (i)}{#if piece.hit}<mark>{piece.text}</mark>{:else}{piece.text}{/if}{/each}</span
                  >
                </button>
              {/each}
            </article>
          {/each}
        </section>
      {:else if chapter}
        <article class="chapter">
          <header>
            <p class="kicker">{t('guide.chapterOf', { n: index + 1, total: chapters.length })}</p>
            <h2 class="focus-target" tabindex="-1">{chapter.title}</h2>
            <p class="summary">{chapter.summary}</p>
            {#if reading && words.length}
              <button class="linkish back" onclick={() => (reading = false)}><Icon name="arrow-left" size={13} /> {t('guide.backToResults')}</button>
            {/if}
            <nav class="onpage" aria-label={t('guide.onThisPage')}>
              {#each chapter.sections as s (s.id)}
                <button class="lib-chip" onclick={() => go({ chapter: chapter.id, section: s.id }, reading)}>{s.title}</button>
              {/each}
            </nav>
          </header>

          {#each chapter.sections as s (s.id)}
            <section id={sectionDomId(chapter.id, s.id)} aria-labelledby="{sectionDomId(chapter.id, s.id)}-h">
              <h3 id="{sectionDomId(chapter.id, s.id)}-h" tabindex="-1">
                {#each highlight(s.title, marks) as piece, i (i)}{#if piece.hit}<mark>{piece.text}</mark>{:else}{piece.text}{/if}{/each}
              </h3>
              {#each s.blocks as block, i (i)}
                <GuideBlock {block} words={marks} {chapters} {keymap} {onfail} {onopen} ongo={(a) => go(a)} />
              {/each}
            </section>
          {/each}

          <footer class="pager">
            {#if index > 0}
              <button class="btn" onclick={() => go({ chapter: chapters[index - 1].id })}>
                <Icon name="arrow-left" size={14} />
                {t('guide.prev', { title: chapters[index - 1].title })}
              </button>
            {/if}
            <span class="grow"></span>
            {#if index < chapters.length - 1}
              <button class="btn" onclick={() => go({ chapter: chapters[index + 1].id })}>
                {t('guide.next', { title: chapters[index + 1].title })}
                <Icon name="arrow-right" size={14} />
              </button>
            {/if}
          </footer>
        </article>
      {:else}
        <section class="overview">
          <h3>{t('guide.overviewHead')}</h3>
          <div class="cards">
            {#each chapters as c, i (c.id)}
              <button class="card" onclick={() => go({ chapter: c.id })}>
                <span class="card-num">{i + 1}</span>
                <span class="card-title">{c.title}</span>
                <span class="card-text">{c.summary}</span>
              </button>
            {/each}
          </div>
        </section>
      {/if}
    </div>
  </div>
</div>

<style>
  .guide {
    max-width: 1180px;
    padding: 26px 32px 80px;
    scroll-margin-top: 0;
  }
  /* Body text of the guide may be selected and copied (the UI disables selection elsewhere). */
  .content {
    user-select: text;
    min-width: 0;
  }
  .guide :global(mark) {
    border-radius: 2px;
    background: color-mix(in oklab, var(--warn) 38%, transparent);
    color: inherit;
  }
  .guide :global(code) {
    padding: 0 3px;
    border-radius: 4px;
    background: var(--sunken);
    font: 0.92em var(--font-mono);
    overflow-wrap: anywhere;
  }
  .guide :global(pre code) {
    padding: 0;
    background: none;
    font: inherit;
    overflow-wrap: normal;
  }
  .top {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    justify-content: space-between;
    gap: 12px 24px;
    margin-bottom: 20px;
  }
  .titles h2 {
    margin: 0 0 4px;
    font-size: 20px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .titles p {
    margin: 0;
    max-width: 68ch;
    color: var(--fg-muted);
    line-height: 1.5;
  }
  h2:focus,
  h3:focus {
    outline: none;
  }
  .layout {
    display: grid;
    grid-template-columns: 230px minmax(0, 1fr);
    gap: 28px;
    align-items: start;
  }
  .toc {
    position: sticky;
    top: 12px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    max-height: calc(100vh - 24px);
    overflow-y: auto;
  }
  .toc-item {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 5px 8px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    line-height: 1.35;
    text-align: left;
  }
  .toc-item:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .toc-item[aria-current='page'] {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .num {
    flex: none;
    width: 18px;
    color: var(--fg-subtle);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
  .toc-select {
    display: none;
  }
  .chapter > header {
    margin-bottom: 8px;
  }
  .kicker {
    margin: 0 0 2px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 650;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .chapter > header h2 {
    margin: 0 0 6px;
    font-size: 24px;
    font-weight: 680;
    letter-spacing: -0.015em;
  }
  .summary {
    margin: 0 0 10px;
    max-width: 75ch;
    color: var(--fg-muted);
    line-height: 1.5;
  }
  .onpage {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 10px 0 6px;
  }
  .onpage .lib-chip {
    border: 0;
  }
  .chapter section {
    max-width: 78ch;
    padding-top: 14px;
    scroll-margin-top: 10px;
  }
  .chapter section h3 {
    margin: 10px 0 4px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    font-size: var(--fs-lg);
    font-weight: 640;
  }
  .pager {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    max-width: 78ch;
    margin-top: 32px;
    padding-top: 16px;
    border-top: 1px solid var(--border);
  }
  .pager .grow {
    flex: 1;
  }
  .linkish {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-weight: 600;
    text-align: left;
  }
  .linkish:hover {
    text-decoration: underline;
  }
  .back {
    margin-bottom: 4px;
    font-size: var(--fs-sm);
  }
  .results-head,
  .overview h3 {
    margin: 0 0 12px;
    font-size: var(--fs-lg);
    font-weight: 640;
  }
  .empty {
    color: var(--fg-muted);
  }
  .result {
    max-width: 80ch;
    margin-bottom: 18px;
    padding-bottom: 14px;
    border-bottom: 1px solid var(--border);
  }
  .result .title {
    font-size: var(--fs-lg);
  }
  .result .summary {
    margin: 2px 0 6px;
    font-size: var(--fs-sm);
  }
  .hit {
    display: flex;
    flex-direction: column;
    gap: 1px;
    width: 100%;
    padding: 6px 10px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    text-align: left;
  }
  .hit:hover {
    background: var(--hover);
  }
  .hit-title {
    font-weight: 600;
    font-size: var(--fs-sm);
  }
  .hit-text {
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 10px;
  }
  .card {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-content: start;
    gap: 2px 10px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    text-align: left;
    transition: border-color 120ms;
  }
  .card:hover {
    border-color: var(--accent-line);
  }
  .card-num {
    grid-row: span 2;
    color: var(--accent);
    font-weight: 700;
    font-variant-numeric: tabular-nums;
  }
  .card-title {
    font-weight: 620;
  }
  .card-text {
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }
  @media (max-width: 980px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
      gap: 12px;
    }
    .toc {
      display: none;
    }
    .toc-select {
      display: block;
    }
    .toc-select select {
      width: 100%;
    }
  }
  @media (max-width: 720px) {
    .guide {
      padding: 18px 16px 60px;
    }
  }
</style>
