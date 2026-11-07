// The keymap the guide shows (defaults + the user's changes), shared with every piece of guide
// text: an inline `[[cmd:<id>]]` mark names a keymap command and shows its current key.

import { getContext, setContext } from 'svelte';
import { DEFAULT_KEYMAP, type Keymap } from '../../lib/keymap';

const KEY = Symbol('guide.keymap');

/** Guide.svelte: make `get()` (reactive) the keymap of everything below it. */
export function provideKeymap(get: () => Keymap): void {
  setContext(KEY, get);
}

/** A getter for the current keymap; the defaults outside the guide (and in tests). */
export function useKeymap(): () => Keymap {
  return getContext<(() => Keymap) | undefined>(KEY) ?? (() => DEFAULT_KEYMAP);
}
