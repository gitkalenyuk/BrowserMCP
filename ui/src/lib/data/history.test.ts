import { describe, expect, it } from 'vitest';
import type { HistoryEntry } from '../api/types-data';
import { clearPlan, dayLabel, groupByDay, startOfDay } from './history';

// Local times, so the tests hold in every time zone.
const at = (d: number, h: number, m = 0) => new Date(2026, 10, d, h, m).getTime();
const entry = (url: string, last_visit: number): HistoryEntry => ({ url, title: url, visit_count: 1, last_visit });
const NOW = at(6, 15, 30); // Friday, November 6, 2026, 15:30

describe('groupByDay', () => {
  it('groups by local day, newest day and newest visit first', () => {
    const groups = groupByDay(
      [
        entry('a', at(6, 9)),
        entry('b', at(5, 23, 59)),
        entry('c', at(6, 14)),
        entry('d', at(3, 0, 1)),
        entry('e', at(5, 0, 0)),
      ],
      NOW,
    );
    expect(groups.map((g) => g.key)).toEqual(['2026-11-06', '2026-11-05', '2026-11-03']);
    expect(groups.map((g) => g.label)).toEqual(['Today', 'Yesterday', 'Tuesday, November 3, 2026']);
    expect(groups[0].entries.map((e) => e.url)).toEqual(['c', 'a']);
    expect(groups[1].entries.map((e) => e.url)).toEqual(['b', 'e']);
    expect(groups[2].start).toBe(at(3, 0));
  });

  it('returns nothing for no entries', () => {
    expect(groupByDay([], NOW)).toEqual([]);
  });

  it('labels across a month boundary', () => {
    const now = new Date(2026, 11, 1, 8).getTime(); // December 1
    expect(dayLabel(startOfDay(new Date(2026, 10, 30, 22).getTime()), now)).toBe('Yesterday');
    expect(dayLabel(startOfDay(new Date(2026, 10, 29, 22).getTime()), now)).toBe('Sunday, November 29, 2026');
  });
});

describe('clearPlan', () => {
  it('uses `before` for old data and "everything", `since` for recent ranges', () => {
    expect(clearPlan('all', NOW)).toEqual({ before: NOW + 1 });
    expect(clearPlan('older_month', NOW)).toEqual({ before: NOW - 28 * 86_400_000 });
    expect(clearPlan('hour', NOW)).toEqual({ since: NOW - 3_600_000 });
    expect(clearPlan('week', NOW)).toEqual({ since: NOW - 7 * 86_400_000 });
  });
});
