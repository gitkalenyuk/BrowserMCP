import { describe, expect, it } from 'vitest';
import { cronPreview, describeCron, nextFires, parseCron } from './cron';

// Fri 6 Nov 2026, 10:30:00 local time.
const NOW = new Date(2026, 10, 6, 10, 30, 0);
const hm = (d: Date) => `${d.getDate()}/${d.getMonth() + 1} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;

describe('parseCron', () => {
  it('prepends seconds to 5 fields', () => {
    const s = parseCron('*/15 9-17 * * Mon-Fri');
    expect(s.second.values).toEqual([0]);
    expect(s.minute.values).toEqual([0, 15, 30, 45]);
    expect(s.hour.values).toEqual([9, 10, 11, 12, 13, 14, 15, 16, 17]);
    expect(s.dow.values).toEqual([2, 3, 4, 5, 6]);
    expect(s.dom.all).toBe(true);
  });

  it('counts weekdays from Sunday = 1 like the cron crate', () => {
    expect(parseCron('0 9 * * 1').dow.values).toEqual([1]);
    expect(parseCron('0 9 * * Sun').dow.values).toEqual([1]);
    expect(parseCron('0 9 * * sat').dow.values).toEqual([7]);
    expect(() => parseCron('0 9 * * 0')).toThrow(/day of week: 0 is outside 1-7/);
  });

  it('accepts 6 and 7 fields, lists, steps from a point, ? and names', () => {
    const s = parseCron('30 5/20 1,13 ? jan-mar,dec * 2027');
    expect(s.second.values).toEqual([30]);
    expect(s.minute.values).toEqual([5, 25, 45]);
    expect(s.hour.values).toEqual([1, 13]);
    expect(s.dom.all).toBe(true);
    expect(s.month.values).toEqual([1, 2, 3, 12]);
    expect(s.year.values).toEqual([2027]);
  });

  it('knows the shorthands', () => {
    expect(parseCron('@weekly').dow.values).toEqual([1]);
    expect(parseCron('@hourly').minute.values).toEqual([0]);
  });

  it('rejects bad specs with a readable message', () => {
    expect(() => parseCron('')).toThrow(/Enter a cron/);
    expect(() => parseCron('* * *')).toThrow(/got 3/);
    expect(() => parseCron('61 * * * *')).toThrow(/minutes: 61/);
    expect(() => parseCron('0 17-9 * * *')).toThrow(/backwards/);
    expect(() => parseCron('*/0 * * * *')).toThrow(/step/);
    expect(() => parseCron('0 9 * * Funday')).toThrow(/'Funday'/);
  });
});

describe('nextFires', () => {
  it('lists the next weekday mornings', () => {
    const next = nextFires(parseCron('0 9 * * Mon-Fri'), NOW, 3);
    // Friday 09:00 already passed: Mon 9, Tue 10, Wed 11 Nov.
    expect(next.map(hm)).toEqual(['9/11 09:00', '10/11 09:00', '11/11 09:00']);
  });

  it('is strictly after now and steps through minutes', () => {
    expect(nextFires(parseCron('*/15 * * * *'), NOW, 3).map(hm)).toEqual(['6/11 10:45', '6/11 11:00', '6/11 11:15']);
  });

  it('needs day of month AND weekday to match', () => {
    // Friday the 13th: Nov 2026, then Aug 2027 (not every 13th, not every Friday).
    const next = nextFires(parseCron('0 0 13 * Fri'), NOW, 3);
    expect(next.map((d) => `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`)).toEqual(['2026-11-13', '2027-8-13', '2028-10-13']);
  });

  it('finds leap days and gives up on impossible dates', () => {
    expect(nextFires(parseCron('0 0 29 2 *'), NOW, 1)[0].getFullYear()).toBe(2028);
    expect(nextFires(parseCron('0 0 30 2 *'), NOW, 1)).toEqual([]);
  });

  it('handles seconds', () => {
    const next = nextFires(parseCron('*/20 * * * * *'), NOW, 3);
    expect(next.map((d) => d.getSeconds())).toEqual([20, 40, 0]);
  });
});

describe('describeCron', () => {
  const d = (spec: string) => describeCron(parseCron(spec));
  it('reads common specs', () => {
    expect(d('0 9 * * Mon-Fri')).toBe('At 09:00 on weekdays');
    expect(d('30 8,12 * * *')).toBe('At 08:30 and 12:30 every day');
    expect(d('*/15 9-17 * * Mon-Fri')).toBe('Every 15 minutes, from 09:00 to 17:59 on weekdays');
    expect(d('* * * * *')).toBe('Every minute');
    expect(d('0 * * * *')).toBe('Every hour');
    expect(d('0 0 1 * *')).toBe('At 00:00 on day 1 of the month');
    expect(d('0 0 9 * * Sat,Sun')).toBe('At 09:00 on weekends');
    expect(d('0 7 * * Mon,Wed,Fri')).toBe('At 07:00 on Monday, Wednesday and Friday');
    expect(d('@yearly')).toBe('At 00:00 on day 1 of the month in January');
    expect(d('*/10 * * * * *')).toBe('Every 10 seconds');
  });
});

describe('cronPreview', () => {
  it('warns about numeric weekdays', () => {
    const p = cronPreview('0 9 * * 1-5', NOW);
    expect(p.ok).toBe(true);
    if (p.ok) {
      expect(p.text).toBe('At 09:00 on Sunday–Thursday');
      expect(p.warning).toMatch(/1 = Sunday/);
      expect(p.warning).toMatch(/Sun–Thu/);
      expect(p.next).toHaveLength(5);
    }
  });

  it('explains weekday errors', () => {
    const p = cronPreview('0 9 * * 0-4', NOW);
    expect(p.ok).toBe(false);
    if (!p.ok) expect(p.hint).toMatch(/1 = Sunday/);
  });

  it('flags specs that never fire', () => {
    const p = cronPreview('0 0 31 2 *', NOW);
    expect(p.ok && p.warning).toMatch(/never fires/);
  });
});
