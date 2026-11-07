// Cron preview for the Triggers tab: parses a spec the way the core does (crates/jobs/src/
// triggers.rs → the Rust `cron` 0.17 crate) and lists the next firings in local time.
//
// Rules of that crate, which differ from Unix cron:
//   * 5 fields get "0" seconds prepended; 6 or 7 fields have seconds first (7th = year).
//   * Weekdays are 1-7 with Sunday = 1 (Unix: 0-6, Sunday = 0). Names (Mon-Fri) are safest.
//   * Day of month AND day of week must both match (Unix: either).
//   * `a/n` means a, a+n, … up to the field's max; `?` = `*` in day fields.
//   * @yearly @monthly @weekly (Sunday) @daily @hourly.
//
// The description and the errors are in the UI language (`runs.cron.*` in lib/i18n).

import { t } from '../i18n/index.svelte';
import { intlLocale } from '../i18n/intl';

export interface CronField {
  values: number[];
  /** Every value of the field (`*`, `?`). */
  all: boolean;
}

export interface CronSchedule {
  second: CronField;
  minute: CronField;
  hour: CronField;
  dom: CronField;
  month: CronField;
  /** 1 = Sunday … 7 = Saturday. */
  dow: CronField;
  year: CronField;
  /** The weekday field used numbers (1-7 count from Sunday here, a common surprise). */
  numericDow: boolean;
}

/** A spec the core would reject; `field` names the bad field (`dow`: a hint about 1 = Sunday). */
export class CronError extends Error {
  constructor(
    message: string,
    readonly field: FieldKey | null = null,
  ) {
    super(message);
  }
}

type FieldKey = 'seconds' | 'minutes' | 'hours' | 'dom' | 'month' | 'dow' | 'year';

interface FieldDef {
  key: FieldKey;
  min: number;
  max: number;
  names?: Record<string, number>;
  any?: boolean;
}

const DOW_NAMES: Record<string, number> = {
  sun: 1, sunday: 1, mon: 2, monday: 2, tue: 3, tues: 3, tuesday: 3, wed: 4, wednesday: 4,
  thu: 5, thurs: 5, thursday: 5, fri: 6, friday: 6, sat: 7, saturday: 7,
};
const MONTH_NAMES: Record<string, number> = {
  jan: 1, january: 1, feb: 2, february: 2, mar: 3, march: 3, apr: 4, april: 4, may: 5,
  jun: 6, june: 6, jul: 7, july: 7, aug: 8, august: 8, sep: 9, sept: 9, september: 9,
  oct: 10, october: 10, nov: 11, november: 11, dec: 12, december: 12,
};

const FIELDS: FieldDef[] = [
  { key: 'seconds', min: 0, max: 59 },
  { key: 'minutes', min: 0, max: 59 },
  { key: 'hours', min: 0, max: 23 },
  { key: 'dom', min: 1, max: 31, any: true },
  { key: 'month', min: 1, max: 12, names: MONTH_NAMES },
  { key: 'dow', min: 1, max: 7, names: DOW_NAMES, any: true },
  { key: 'year', min: 1970, max: 2100 },
];

/** "minutes: 61 is outside 0-59" (`runs.cron.err.<what>`, prefixed with the field's name). */
function fieldError(def: FieldDef, what: string, params: Record<string, string | number> = {}): CronError {
  const field = t(`runs.cron.field.${def.key}`);
  return new CronError(t(`runs.cron.err.${what}`, { field, min: def.min, max: def.max, ...params }), def.key);
}

// Weekdays 1 = Sunday … 7 = Saturday; months 1-12. Separate keys for the grammatical forms
// Ukrainian needs ("у середу", "у січні"); in English they are the same words.
const dowName = (v: number) => t(`runs.cron.dow.${v}`);
const dowOn = (v: number) => t(`runs.cron.dowOn.${v}`);
const dowShort = (v: number) => t(`runs.cron.dowShort.${v}`);
const monthIn = (v: number) => t(`runs.cron.monthIn.${v}`);

function range(a: number, b: number, step = 1): number[] {
  const out: number[] = [];
  for (let v = a; v <= b; v += step) out.push(v);
  return out;
}

function parseValue(tok: string, def: FieldDef): number {
  if (/^\d+$/.test(tok)) {
    const v = Number(tok);
    if (v < def.min || v > def.max) {
      throw fieldError(def, 'outside', { v });
    }
    return v;
  }
  const named = def.names?.[tok.toLowerCase()];
  if (named === undefined) throw fieldError(def, 'invalid', { tok });
  return named;
}

function parseField(text: string, def: FieldDef): CronField {
  if (text === '*' || (def.any && text === '?')) return { values: range(def.min, def.max), all: true };
  const set = new Set<number>();
  for (const item of text.split(',')) {
    if (!item) throw fieldError(def, 'emptyItem', { text });
    const [base, stepText, extra] = item.split('/');
    if (extra !== undefined) throw fieldError(def, 'badStepIn', { item });
    let step = 1;
    if (stepText !== undefined) {
      if (!/^\d+$/.test(stepText)) throw fieldError(def, 'badStep', { step: stepText });
      step = Number(stepText);
      if (step < 1 || step > def.max) throw fieldError(def, 'stepRange');
    }
    let lo: number;
    let hi: number;
    if (base === '*' || (def.any && base === '?')) {
      lo = def.min;
      hi = def.max;
    } else if (base.includes('-')) {
      const [a, b, more] = base.split('-');
      if (more !== undefined || !a || !b) throw fieldError(def, 'badRange', { range: base });
      lo = parseValue(a, def);
      hi = parseValue(b, def);
      if (lo > hi) throw fieldError(def, 'backwards', { range: base });
    } else {
      lo = parseValue(base, def);
      // `5/15` = 5, 20, 35, 50; a lone `5` is just 5.
      hi = stepText !== undefined ? def.max : lo;
    }
    for (const v of range(lo, hi, step)) set.add(v);
  }
  const values = [...set].sort((a, b) => a - b);
  return { values, all: values.length === def.max - def.min + 1 };
}

const SHORTHANDS: Record<string, string> = {
  '@yearly': '0 0 0 1 1 *',
  '@monthly': '0 0 0 1 * *',
  '@weekly': '0 0 0 * * 1',
  '@daily': '0 0 0 * * *',
  '@hourly': '0 0 * * * *',
};

/** Parse a spec like the core does; throws CronError with a readable message. */
export function parseCron(spec: string): CronSchedule {
  const trimmed = spec.trim();
  if (!trimmed) throw new CronError(t('runs.cron.err.empty'));
  const short = SHORTHANDS[trimmed.toLowerCase()];
  let parts = (short ?? trimmed).split(/\s+/);
  if (parts.length === 5) parts = ['0', ...parts];
  if (parts.length !== 6 && parts.length !== 7) {
    throw new CronError(t('runs.cron.err.fields', { n: parts.length }));
  }
  const f = parts.map((p, i) => parseField(p, FIELDS[i]));
  return {
    second: f[0],
    minute: f[1],
    hour: f[2],
    dom: f[3],
    month: f[4],
    dow: f[5],
    year: f[6] ?? { values: range(1970, 2100), all: true },
    numericDow: !short && /\d/.test(parts[5]),
  };
}

/** The next `count` firings strictly after `after`, in local time (empty when it never fires). */
export function nextFires(s: CronSchedule, after: Date, count = 5): Date[] {
  const out: Date[] = [];
  const day = new Date(after.getFullYear(), after.getMonth(), after.getDate());
  const lastYear = Math.min(2100, after.getFullYear() + 9);
  const years = new Set(s.year.values);
  const months = new Set(s.month.values);
  const doms = new Set(s.dom.values);
  const dows = new Set(s.dow.values);
  while (out.length < count && day.getFullYear() <= lastYear) {
    const y = day.getFullYear();
    const m = day.getMonth() + 1;
    const d = day.getDate();
    if (years.has(y) && months.has(m) && doms.has(d) && (s.dow.all || dows.has(day.getDay() + 1))) {
      for (const h of s.hour.values) {
        for (const mi of s.minute.values) {
          for (const sec of s.second.values) {
            const at = new Date(y, m - 1, d, h, mi, sec);
            // Skip local times that do not exist (DST gap: the Date moved).
            if (at.getHours() !== h || at.getDate() !== d) continue;
            if (at.getTime() <= after.getTime()) continue;
            out.push(at);
            if (out.length >= count) return out;
          }
        }
      }
    }
    day.setDate(day.getDate() + 1);
  }
  return out;
}

const pad = (n: number) => String(n).padStart(2, '0');

/** `[1,2,3,5]` → `1–3, 5`, with `fmt` for each value. */
function runs(values: number[], fmt: (v: number) => string = String, sep = '–'): string {
  const parts: string[] = [];
  for (let i = 0; i < values.length; ) {
    let j = i;
    while (j + 1 < values.length && values[j + 1] === values[j] + 1) j++;
    if (j - i >= 2) parts.push(`${fmt(values[i])}${sep}${fmt(values[j])}`);
    else for (let k = i; k <= j; k++) parts.push(fmt(values[k]));
    i = j + 1;
  }
  return joinList(parts);
}

function joinList(parts: string[]): string {
  if (parts.length <= 1) return parts.join('');
  return `${parts.slice(0, -1).join(', ')} ${t('runs.cron.and')} ${parts[parts.length - 1]}`;
}

/** Evenly spaced from the field's minimum: the step, else null. */
function stepOf(f: CronField, min: number): number | null {
  const v = f.values;
  if (v.length < 2 || v[0] !== min) return null;
  const step = v[1] - v[0];
  for (let i = 2; i < v.length; i++) if (v[i] - v[i - 1] !== step) return null;
  return step;
}

/** Part of a description; `periodic`: it repeats within the day ("every 5 minutes"). */
interface Phrase {
  text: string;
  periodic: boolean;
}

/** Comma-joined; periodic when the first phrase is (it leads the sentence). */
function joinPhrases(list: (Phrase | null)[]): Phrase {
  const kept = list.filter((p): p is Phrase => p !== null);
  return { text: kept.map((p) => p.text).join(', '), periodic: kept[0]?.periodic ?? false };
}

function describeTime(s: CronSchedule): Phrase {
  const sec = s.second;
  const secOnly0 = sec.values.length === 1 && sec.values[0] === 0;
  const secStep = stepOf(sec, 0);
  const secPhrase: Phrase | null = secOnly0
    ? null
    : sec.all
      ? { text: t('runs.cron.everySecond'), periodic: true }
      : secStep
        ? { text: t('runs.cron.everyNSeconds', { n: secStep }), periodic: true }
        : { text: t('runs.cron.atSeconds', { list: runs(sec.values) }), periodic: false };

  const { minute: mi, hour: h } = s;
  if (mi.values.length === 1 && h.values.length <= 6 && !h.all) {
    const oneSec = sec.values.length === 1;
    const times = joinList(h.values.map((x) => `${pad(x)}:${pad(mi.values[0])}${oneSec && !secOnly0 ? ':' + pad(sec.values[0]) : ''}`));
    if (oneSec || !secPhrase) return { text: t('runs.cron.at', { times }), periodic: false };
    return { text: t('runs.cron.during', { what: secPhrase.text, times }), periodic: secPhrase.periodic };
  }

  const minStep = stepOf(mi, 0);
  let minPhrase: Phrase | null;
  if (mi.all) minPhrase = secOnly0 ? { text: t('runs.cron.everyMinute'), periodic: true } : null;
  else if (minStep) minPhrase = { text: t('runs.cron.everyNMinutes', { n: minStep }), periodic: true };
  else if (mi.values.length === 1) {
    const text = mi.values[0] === 0 ? t('runs.cron.everyHour') : t('runs.cron.everyHourAt', { mm: pad(mi.values[0]) });
    minPhrase = { text, periodic: true };
  } else minPhrase = { text: t('runs.cron.atMinutes', { list: runs(mi.values, pad) }), periodic: true };

  let hourPhrase: Phrase | null = null;
  if (!h.all) {
    const hs = h.values;
    const contiguous = hs.every((x, i) => i === 0 || x === hs[i - 1] + 1);
    const hourStep = stepOf(h, 0);
    if (hourStep && !contiguous) hourPhrase = { text: t('runs.cron.everyNHours', { n: hourStep }), periodic: true };
    else if (contiguous) {
      hourPhrase = { text: t('runs.cron.fromTo', { from: `${pad(hs[0])}:00`, to: `${pad(hs[hs.length - 1])}:59` }), periodic: false };
    } else hourPhrase = { text: t('runs.cron.duringHours', { list: runs(hs, pad) }), periodic: false };
    // Minute 0 every n hours: one phrase ("every 2 hours on the hour").
    if (mi.values.length === 1 && mi.values[0] === 0 && hourStep && !contiguous) {
      return joinPhrases([secPhrase, { text: t('runs.cron.everyNHoursSharp', { n: hourStep }), periodic: true }]);
    }
  }
  return joinPhrases([secPhrase, minPhrase, hourPhrase]);
}

/** The day part; `everyDay` when no day, month or year field restricts it. */
function describeDays(s: CronSchedule): { text: string; everyDay: boolean } {
  const parts: string[] = [];
  const dowText = (vals: number[]) => {
    if (vals.length === 5 && vals.join() === '2,3,4,5,6') return t('runs.cron.onWeekdays');
    if (vals.length === 2 && vals.join() === '1,7') return t('runs.cron.onWeekends');
    return t('runs.cron.onDays', { days: runs(vals, dowOn) });
  };
  if (!s.dom.all && !s.dow.all) {
    parts.push(t('runs.cron.onDomIf', { days: runs(s.dom.values), dows: joinList(s.dow.values.map(dowName)) }));
  } else if (!s.dom.all) {
    parts.push(t('runs.cron.onDom', { days: runs(s.dom.values) }));
  } else if (!s.dow.all) {
    parts.push(dowText(s.dow.values));
  } else {
    parts.push(t('runs.cron.everyDay'));
  }
  if (!s.month.all) parts.push(t('runs.cron.inMonths', { months: runs(s.month.values, monthIn) }));
  if (!s.year.all) parts.push(t('runs.cron.inYears', { years: runs(s.year.values) }));
  return { text: parts.join(' '), everyDay: s.dom.all && s.dow.all && s.month.all && s.year.all };
}

/** "Every 15 minutes, from 09:00 to 17:59 on weekdays" (in the UI language). */
export function describeCron(s: CronSchedule): string {
  const time = describeTime(s);
  const days = describeDays(s);
  // "Every 5 minutes" says enough; "At 09:00" needs its "every day".
  const text = days.everyDay && time.periodic ? time.text : `${time.text} ${days.text}`.trim();
  return text.charAt(0).toLocaleUpperCase() + text.slice(1);
}

export type CronPreview =
  | { ok: true; text: string; next: Date[]; warning: string | null }
  | { ok: false; error: string; hint: string | null };

/** Description + next firings + warnings, for the trigger form. */
export function cronPreview(spec: string, now: Date = new Date(), count = 5): CronPreview {
  let s: CronSchedule;
  try {
    s = parseCron(spec);
  } catch (err) {
    const error = (err as Error).message;
    const dow = err instanceof CronError && err.field === 'dow';
    return { ok: false, error, hint: dow ? t('runs.cron.dowHint') : null };
  }
  const next = nextFires(s, now, count);
  let warning: string | null = null;
  if (s.numericDow && !s.dow.all) {
    warning = `${t('runs.cron.dowHint')} ${t('runs.cron.dowMeans', { days: runs(s.dow.values, dowShort) })}`;
  }
  if (next.length === 0) warning = t('runs.cron.never');
  return { ok: true, text: describeCron(s), next, warning };
}

/** "Mon 9 Nov, 09:00" (seconds only when not :00), in the UI language. */
export function formatFire(d: Date, now: Date = new Date()): string {
  const sameYear = d.getFullYear() === now.getFullYear();
  const date = d.toLocaleDateString(intlLocale(), { weekday: 'short', day: 'numeric', month: 'short', year: sameYear ? undefined : 'numeric' });
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}${d.getSeconds() ? ':' + pad(d.getSeconds()) : ''}`;
  return `${date}, ${time}`;
}
