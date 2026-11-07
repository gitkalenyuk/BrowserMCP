<script lang="ts">
  /**
   * Settings → General → Language, under the interface language: the languages websites see
   * (Accept-Language, navigator.languages), Chromium's own language and spellcheck
   * (`browser.languages_get|set`, crates/languages). Every change applies at once.
   *
   * Precedence: profiles with a language of their own (a stealth identity fitted to the
   * proxy's country, or profile.create {accept_language}) keep it; this list is for the rest.
   * The card also reports the interface language, so the automatic list follows it.
   */
  import type { BrowserApi } from '../lib/api';
  import type { LanguagesInfo, LanguagesSetArgs } from '../lib/api/types-languages';
  import Icon from '../lib/components/Icon.svelte';
  import { locale, t } from '../lib/i18n/index.svelte';
  import { addLanguage, addable, localName, moveLanguage, nativeName, removeLanguage, resolveLanguage, spellExtras, toggleSpellcheck } from '../lib/settings/languages';

  let { api, notify }: { api: BrowserApi; notify?: (text: string, error?: boolean) => void } = $props();

  let info = $state<LanguagesInfo | null>(null);
  let loadError = $state<string | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let addInput = $state('');
  let addError = $state<string | null>(null);
  /** The last success message, shown inline when the page passes no `notify` (no toasts). */
  let saved = $state<string | null>(null);

  async function load() {
    try {
      info = await api.call('browser.languages_get');
      loadError = null;
    } catch (err) {
      loadError = (err as Error).message;
    }
  }
  $effect(() => {
    void load();
  });

  async function apply(args: LanguagesSetArgs, done?: string): Promise<boolean> {
    busy = true;
    try {
      info = await api.call('browser.languages_set', args);
      error = null;
      saved = done && !notify ? done : null;
      if (done) notify?.(done);
      return true;
    } catch (err) {
      error = (err as Error).message;
      saved = null;
      notify?.(error, true);
      return false;
    } finally {
      busy = false;
    }
  }

  // The automatic website list starts with the interface language: tell the browser which one
  // is shown (only when it differs from what it knows, so opening the page writes nothing).
  // Each language is reported once: after a failure `info` is unchanged, and retrying on every
  // re-run would loop (and toast) for as long as the browser keeps failing. Not reactive on
  // purpose; switching the interface language again reports the new one.
  let reportedUi: string | null = null;
  $effect(() => {
    const ui = locale();
    if (!info || info.ui_language === ui || reportedUi === ui) return;
    reportedUi = ui;
    void apply({ ui_language: ui });
  });

  const web = $derived(info?.web ?? []);
  const spellOn = $derived(info?.spellcheck_supported !== false);
  const options = $derived(addable(web));
  /** Spellcheck dictionaries on but not in the website list (e.g. Chromium's default en-US). */
  const extraSpell = $derived(info ? spellExtras(info.spellcheck, web) : []);

  const setWeb = (list: string[]) => apply({ web: list }, t('languages.saved'));

  async function add() {
    const tag = resolveLanguage(addInput, locale());
    if (!tag) {
      addError = t('languages.err.unknown', { text: addInput.trim() });
      return;
    }
    if (web.some((l) => l.toLowerCase() === tag.toLowerCase())) {
      addError = t('languages.err.listed', { name: nativeName(tag) });
      return;
    }
    addError = null;
    if (await setWeb(addLanguage(web, tag))) addInput = '';
  }

  const isSpell = (tag: string) => !!info?.spellcheck.some((l) => l.toLowerCase() === tag.toLowerCase());

  /** "Deutsch — German (de)", or "English (en)" when both names are the same. */
  function describe(tag: string): { name: string; detail: string } {
    const name = nativeName(tag);
    const local = localName(tag, locale());
    return { name, detail: local.toLowerCase() === name.toLowerCase() ? tag : `${local}, ${tag}` };
  }
</script>

{#if loadError}
  <div class="set-row">
    <div class="text">
      <span class="label">{t('languages.web')}</span>
      <span class="set-err" role="alert">{t('languages.loadError', { error: loadError })}</span>
    </div>
    <div class="ctl"><button class="btn" onclick={load}>{t('settings.hints.retry')}</button></div>
  </div>
{:else if !info}
  <div class="set-row"><span class="set-muted">{t('common.loading')}</span></div>
{:else}
  <div class="set-row stack">
    <div class="text">
      <span class="label">
        {t('languages.web')}
        {#if info.web_automatic}<span class="lib-chip">{t('languages.automatic')}</span>{/if}
      </span>
      <span class="help">{t('languages.webHelp')}</span>
    </div>

    <ol class="langs" aria-label={t('languages.web')}>
      {#each web as tag, i (tag)}
        {@const d = describe(tag)}
        <li>
          <span class="n">{i + 1}</span>
          <span class="name" lang={tag}>{d.name}</span>
          <span class="set-muted detail">{d.detail}</span>
          {#if spellOn}
            <label class="spell" title={t('languages.spellHint')}>
              <input type="checkbox" aria-label={t('languages.spellFor', { name: d.name })} checked={isSpell(tag)} disabled={busy} onchange={() => apply({ spellcheck: toggleSpellcheck(info!.spellcheck, tag, web) }, t('languages.spellSaved'))} />
              {t('languages.spell')}
            </label>
          {/if}
          <button class="icon-btn" aria-label={t('languages.moveUp', { name: d.name })} disabled={busy || i === 0} onclick={() => setWeb(moveLanguage(web, i, -1))}>
            <Icon name="chevron-down" size={13} class="up" />
          </button>
          <button class="icon-btn" aria-label={t('languages.moveDown', { name: d.name })} disabled={busy || i === web.length - 1} onclick={() => setWeb(moveLanguage(web, i, 1))}>
            <Icon name="chevron-down" size={13} />
          </button>
          <button
            class="icon-btn"
            aria-label={t('languages.remove', { name: d.name })}
            title={web.length === 1 ? t('languages.keepOne') : undefined}
            disabled={busy || web.length === 1}
            onclick={() => setWeb(removeLanguage(web, i))}
          >
            <Icon name="x" size={13} />
          </button>
        </li>
      {/each}
    </ol>

    <form class="add" onsubmit={(e) => (e.preventDefault(), void add())}>
      <input
        class="lib-input wide"
        bind:value={addInput}
        list="web-language-options"
        placeholder={t('languages.addPlaceholder')}
        spellcheck="false"
        aria-label={t('languages.addLabel')}
        aria-invalid={!!addError}
      />
      <datalist id="web-language-options">
        {#each options as tag (tag)}<option value={nativeName(tag)}>{localName(tag, locale())} ({tag})</option>{/each}
      </datalist>
      <button class="btn" type="submit" disabled={busy || !addInput.trim()}><Icon name="plus" size={13} /> {t('common.add')}</button>
      {#if !info.web_automatic}
        <button class="btn" type="button" disabled={busy} onclick={() => apply({ web: [] }, t('languages.automaticBack'))} title={t('languages.automaticHint')}>
          {t('languages.useAutomatic')}
        </button>
      {/if}
    </form>
    {#if addError}<p class="set-err">{addError}</p>{/if}
    {#if error}<p class="set-err" role="alert">{error}</p>{/if}
    {#if saved}<p class="set-muted" role="status">{saved}</p>{/if}

    {#if spellOn && extraSpell.length}
      <p class="set-muted note extras">
        {t('languages.spellExtra')}
        {#each extraSpell as tag (tag)}
          <span class="lib-chip" lang={tag}>
            {nativeName(tag)} ({tag})
            <button
              class="icon-btn"
              aria-label={t('languages.spellOff', { name: nativeName(tag) })}
              disabled={busy}
              onclick={() => apply({ spellcheck: toggleSpellcheck(info!.spellcheck, tag, web) }, t('languages.spellSaved'))}
            >
              <Icon name="x" size={11} />
            </button>
          </span>
        {/each}
      </p>
    {/if}

    <p class="set-muted note">
      <Icon name="alert" size={12} />
      {t('languages.precedence')}
      {#if info.own_language.length}
        {t('languages.ownProfiles')}
        {#each info.own_language as p, i (p.profile)}<span class="set-mono">{p.profile}</span> ({p.accept_language.split(',')[0]}){i < info.own_language.length - 1 ? ', ' : '.'}{/each}
      {/if}
    </p>
    {#if !spellOn}<p class="set-muted note">{t('languages.spellUnsupported')}</p>{/if}
  </div>

  <div class="set-row">
    <div class="text">
      <span class="label">{t('languages.browser')}</span>
      <span class="help">{info.browser_locales.length ? t('languages.browserHelp') : t('languages.noLocales')}</span>
      {#if info.restart_required}<span class="set-warn" role="status"><Icon name="reload" size={12} /> {t('languages.restart')}</span>{/if}
    </div>
    <div class="ctl">
      <select
        class="lib-input"
        aria-label={t('languages.browser')}
        value={info.browser_locale ?? 'system'}
        disabled={busy || !info.browser_locales.length}
        onchange={(e) => apply({ browser_locale: e.currentTarget.value }, t('languages.browserSaved'))}
      >
        <option value="system">{t('languages.system')}</option>
        {#each info.browser_locales as l (l)}<option value={l} lang={l}>{nativeName(l)}</option>{/each}
      </select>
    </div>
  </div>
{/if}

<style>
  .langs {
    display: grid;
    gap: 2px;
    margin: 8px 0 0;
    padding: 0;
    list-style: none;
  }
  .langs li {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 32px;
    padding: 2px 6px;
    border-radius: 6px;
  }
  .langs li:hover {
    background: var(--hover);
  }
  .n {
    width: 1.5em;
    color: var(--fg-subtle);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
  .name {
    font-weight: 500;
  }
  .detail {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spell {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--fs-xs);
  }
  .langs :global(.up) {
    transform: rotate(180deg);
  }
  .add {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }
  .add .wide {
    flex: 1;
    min-width: 200px;
  }
  .note {
    margin: 8px 0 0;
    font-size: var(--fs-xs);
    line-height: 1.5;
  }
  .extras {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .extras .lib-chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
  .note :global(svg) {
    margin-right: 4px;
    vertical-align: -2px;
  }
  .set-warn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--warn);
    font-size: var(--fs-xs);
  }
</style>
