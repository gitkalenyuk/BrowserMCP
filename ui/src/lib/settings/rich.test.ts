import { describe, expect, it } from 'vitest';
import { richParts } from './rich';

describe('richParts', () => {
  it('splits a sentence at its slots', () => {
    expect(richParts('Plugins are in {dir} with a {file}.')).toEqual([
      { text: 'Plugins are in ', slot: null },
      { text: '', slot: 'dir' },
      { text: ' with a ', slot: null },
      { text: '', slot: 'file' },
      { text: '.', slot: null },
    ]);
    expect(richParts('{method} first')).toEqual([
      { text: '', slot: 'method' },
      { text: ' first', slot: null },
    ]);
    expect(richParts('no slots')).toEqual([{ text: 'no slots', slot: null }]);
  });
});
