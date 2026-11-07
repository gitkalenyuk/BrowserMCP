<script lang="ts">
  // Settings → General → Appearance: colour theme gallery (live previews, click applies at once),
  // light / dark mode, accent, density and text size. Every page of the UI follows through the
  // shared settings store (localStorage + storage events).
  import Icon from '../lib/components/Icon.svelte';
  import ThemePreview from '../lib/components/ThemePreview.svelte';
  import { t } from '../lib/i18n/index.svelte';
  import { ACCENTS, PALETTE_ACCENT, settings, themeTokens, type Density, type TextSize, type ThemePref } from '../lib/settings.svelte';
  import { PALETTES } from '../lib/theme/palettes';

  // Labels are looked up in markup (t() follows the interface language).
  const THEMES: ThemePref[] = ['system', 'light', 'dark'];
  const DENSITIES: Density[] = ['comfortable', 'compact'];
  const TEXT_SIZES: TextSize[] = ['small', 'default', 'large'];
  const s = $derived(settings.value);
  const mode = $derived(settings.effectiveTheme);
  const paletteAccent = $derived(themeTokens({ palette: s.palette, accent: PALETTE_ACCENT }, mode).accent);
</script>

<section class="set-card" aria-labelledby="g-look">
  <h3 id="g-look">{t('settings.general.appearance')}</h3>
  <div class="set-row stack">
    <div class="text"><span class="label" id="g-palette">{t('settings.general.palette')}</span><span class="help">{t('settings.general.paletteHelp')}</span></div>
    <div class="gallery" role="radiogroup" aria-labelledby="g-palette">
      {#each PALETTES as p (p.id)}
        {@const on = s.palette === p.id}
        <button
          class="theme"
          role="radio"
          aria-checked={on}
          aria-label={t(`theme.palette.${p.id}`)}
          title={t(`theme.palette.${p.id}.hint`)}
          data-palette-id={p.id}
          onclick={() => settings.update({ palette: p.id })}
        >
          <ThemePreview tokens={themeTokens({ palette: p.id, accent: s.accent }, mode)} />
          <span class="name">
            <span class="title">{t(`theme.palette.${p.id}`)}</span>
            {#if p.level === 'AAA'}<span class="badge" title={t('theme.aaaTitle')}>{t('theme.aaa')}</span>{/if}
            {#if on}<span class="tick" title={t('settings.general.paletteCurrent')}><Icon name="check" size={12} stroke={2.75} /></span>{/if}
          </span>
          <span class="hint">{t(`theme.palette.${p.id}.hint`)}</span>
        </button>
      {/each}
    </div>
  </div>
  <div class="set-row">
    <div class="text"><span class="label">{t('settings.general.theme')}</span><span class="help">{t('settings.general.themeHelp')}</span></div>
    <div class="ctl">
      <div class="set-seg" role="radiogroup" aria-label={t('settings.general.theme')}>
        {#each THEMES as theme (theme)}
          <button role="radio" aria-checked={s.theme === theme} onclick={() => settings.update({ theme })}>{t(`settings.general.theme.${theme}`)}</button>
        {/each}
      </div>
    </div>
  </div>
  <div class="set-row">
    <div class="text"><span class="label">{t('settings.general.accent')}</span><span class="help">{t('settings.general.accentHelp')}</span></div>
    <div class="ctl swatches" role="radiogroup" aria-label={t('settings.general.accent')}>
      <button class="auto" role="radio" aria-checked={s.accent === PALETTE_ACCENT} style:--c={paletteAccent} onclick={() => settings.update({ accent: PALETTE_ACCENT })}>
        <span class="sw-dot"></span>{t('settings.general.accentPalette')}
      </button>
      {#each ACCENTS as c (c)}
        <button
          class="sw"
          role="radio"
          aria-checked={s.accent === c}
          aria-label={t('settings.general.accentN', { color: c })}
          style:--c={c}
          onclick={() => settings.update({ accent: c })}
        ></button>
      {/each}
      <label class="custom" title={t('settings.general.customColor')}>
        <input type="color" value={settings.effectiveAccent} oninput={(e) => settings.update({ accent: e.currentTarget.value })} aria-label={t('settings.general.customAccent')} />
      </label>
    </div>
  </div>
  <div class="set-row">
    <div class="text"><span class="label">{t('settings.general.density')}</span><span class="help">{t('settings.general.densityHelp')}</span></div>
    <div class="ctl">
      <div class="set-seg" role="radiogroup" aria-label={t('settings.general.density')}>
        {#each DENSITIES as density (density)}
          <button role="radio" aria-checked={(s.density ?? 'comfortable') === density} onclick={() => settings.update({ density })}>{t(`settings.general.density.${density}`)}</button>
        {/each}
      </div>
    </div>
  </div>
  <div class="set-row">
    <div class="text"><span class="label">{t('settings.general.textSize')}</span><span class="help">{t('settings.general.textSizeHelp')}</span></div>
    <div class="ctl">
      <div class="set-seg" role="radiogroup" aria-label={t('settings.general.textSize')}>
        {#each TEXT_SIZES as textSize (textSize)}
          <button role="radio" aria-checked={s.textSize === textSize} onclick={() => settings.update({ textSize })}>{t(`settings.general.textSize.${textSize}`)}</button>
        {/each}
      </div>
    </div>
  </div>
</section>

<style>
  .gallery {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(148px, 1fr));
    gap: 10px;
  }
  .theme {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
    padding: 6px 6px 8px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    color: var(--fg);
    text-align: left;
    transition: border-color 120ms, box-shadow 120ms;
  }
  .theme:hover {
    border-color: var(--border-strong);
  }
  .theme[aria-checked='true'] {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .name {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    padding: 0 2px;
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    padding: 0 4px;
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    line-height: 14px;
  }
  .tick {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    margin-left: auto;
    flex: none;
    border-radius: 50%;
    background: var(--accent);
    color: var(--on-accent);
  }
  .hint {
    display: -webkit-box;
    padding: 0 2px;
    overflow: hidden;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    line-height: 1.35;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .swatches {
    flex-wrap: wrap;
    gap: 6px;
  }
  .auto {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px 0 4px;
    border: 1px solid var(--border-strong);
    border-radius: 99px;
    background: var(--surface);
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    font-weight: 550;
  }
  .auto[aria-checked='true'] {
    border-color: var(--c);
    color: var(--fg);
    box-shadow: 0 0 0 1px var(--c);
  }
  .sw-dot {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--c);
  }
  .sw {
    width: 22px;
    height: 22px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 50%;
    background: var(--c);
    box-shadow: inset 0 0 0 2px var(--surface);
  }
  .sw[aria-checked='true'] {
    border-color: var(--c);
  }
  .custom {
    display: grid;
    width: 26px;
    height: 26px;
    overflow: hidden;
    border: 1px dashed var(--border-strong);
    border-radius: 50%;
  }
  .custom input {
    width: 40px;
    height: 40px;
    margin: -8px;
    border: 0;
    padding: 0;
    background: none;
  }
</style>
