<script lang="ts">
  import { hostOf, isNewTab } from '../urls';
  import Icon from './Icon.svelte';

  let {
    url,
    src = null,
    loading = false,
    size = 16,
  }: { url: string; src?: string | null; loading?: boolean; size?: number } = $props();

  let failed = $state<string | null>(null);
  const host = $derived(hostOf(url));
  const letter = $derived((host.match(/[a-z0-9]/i)?.[0] ?? '?').toUpperCase());
  // Stable hue per host so the same site always gets the same monogram color.
  const hue = $derived([...host].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 7));
  const showImg = $derived(!!src && failed !== src);
</script>

<span class="fav" style:--size="{size}px" aria-hidden="true">
  {#if loading}
    <span class="spinner"></span>
  {:else if showImg}
    <img {src} alt="" width={size} height={size} onerror={() => (failed = src)} />
  {:else if isNewTab(url)}
    <Icon name="layers" size={size - 2} />
  {:else if host}
    <span class="mono" style:--h={hue}>{letter}</span>
  {:else}
    <Icon name="globe" size={size - 2} />
  {/if}
</span>

<style>
  .fav {
    display: inline-grid;
    place-items: center;
    width: var(--size);
    height: var(--size);
    flex: none;
    color: var(--fg-subtle);
  }
  img {
    display: block;
    border-radius: 3px;
  }
  .mono {
    display: grid;
    place-items: center;
    width: 100%;
    height: 100%;
    border-radius: 4px;
    background: oklch(0.62 0.13 var(--h));
    color: #fff;
    font-size: calc(var(--size) * 0.62);
    font-weight: 700;
    line-height: 1;
  }
  .spinner {
    width: calc(var(--size) - 2px);
    height: calc(var(--size) - 2px);
    border-radius: 50%;
    border: 2px solid color-mix(in oklab, var(--accent) 22%, transparent);
    border-top-color: var(--accent);
    animation: spin 700ms linear infinite;
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation-duration: 2.4s;
    }
  }
</style>
