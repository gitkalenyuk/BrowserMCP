import { describe, expect, it } from 'vitest';
import { fuzzyFilter, fuzzyMatch, highlight } from './fuzzy';

describe('fuzzyMatch', () => {
  it('matches subsequences case-insensitively', () => {
    expect(fuzzyMatch('nt', 'New tab')).not.toBeNull();
    expect(fuzzyMatch('NWTB', 'new tab')).not.toBeNull();
  });

  it('rejects non-subsequences', () => {
    expect(fuzzyMatch('xyz', 'New tab')).toBeNull();
    expect(fuzzyMatch('bat', 'tab')).toBeNull();
  });

  it('empty query matches everything with score 0', () => {
    expect(fuzzyMatch('  ', 'anything')).toEqual({ score: 0, indices: [] });
  });

  it('reports indices of matched characters, preferring word starts', () => {
    expect(fuzzyMatch('ct', 'Close tab')?.indices).toEqual([0, 6]);
    expect(fuzzyMatch('tab', 'Close tab')?.indices).toEqual([6, 7, 8]);
  });

  it('ranks prefix and word-start matches above scattered ones', () => {
    const items = ['Toggle vertical tabs', 'Reload page', 'Open Traffic panel', 'Close tab'];
    const ranked = fuzzyFilter('tab', items, (s) => s).map((r) => r.item);
    expect(ranked[0]).toBe('Close tab');
    expect(fuzzyFilter('rel', items, (s) => s)[0].item).toBe('Reload page');
    expect(fuzzyFilter('vt', items, (s) => s)[0].item).toBe('Toggle vertical tabs');
  });

  it('is stable for equal scores', () => {
    const ranked = fuzzyFilter('a', ['ab', 'ac'], (s) => s).map((r) => r.item);
    expect(ranked).toEqual(['ab', 'ac']);
  });
});

describe('highlight', () => {
  it('splits text into hit and plain parts', () => {
    expect(highlight('Close tab', [0, 6, 7, 8])).toEqual([
      { text: 'C', hit: true },
      { text: 'lose ', hit: false },
      { text: 'tab', hit: true },
    ]);
  });
});
