// Cron preview for the Triggers tab: parses a spec the way the core does (crates/jobs/src/
// triggers.rs → the Rust `cron` 0.17 crate) and lists the next firings in local time.
//
// Rules of that crate, which differ from Unix cron:
//   * 5 fields get "0" seconds prepended; 6 or 7 fields have seconds first (7th = year).
//   * Weekdays are 1-7 with Sunday = 1 (Unix: 0-6, Sunday = 0). Names (Mon-Fri) are safest.
//   * Day of month AND day of week must both match (Unix: either).
//   * `a/n` means a, a+n, … up to the field's max; `?` = `*` in day fields.
//   * @yearly @monthly @weekly (Sunday) @daily @hourly.

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

export class CronError extends Error {}

interface FieldDef {
  name: string;
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
  { name: 'seconds', min: 0, max: 59 },
  { name: 'minutes', min: 0, max: 59 },
  { name: 'hours', min: 0, max: 23 },
  { name: 'day of month', min: 1, max: 31, any: true },
  { name: 'month', min: 1, max: 12, names: MONTH_NAMES },
  { name: 'day of week', min: 1, max: 7, names: DOW_NAMES, any: true },
  { name: 'year', min: 1970, max: 2100 },
];

export const WEEKDAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];

function range(a: number, b: number, step = 1): number[] {
  const out: number[] = [];
  for (let v = a; v <= b; v += step) out.push(v);
  return out;
}

function parseValue(tok: string, def: FieldDef): number {
  if (/^\d+$/.test(tok)) {
    const v = Number(tok);
    if (v < def.min || v > def.max) {
      throw new CronError(`${def.name}: ${v} is outside ${def.min}-${def.max}`);
    }
    return v;
  }
  const named = def.names?.[tok.toLowerCase()];
  if (named === undefined) throw new CronError(`${def.name}: '${tok}' is not a valid value`);
  return named;
}

function parseField(text: string, def: FieldDef): CronField {
  if (text === '*' || (def.any && text === '?')) return { values: range(def.min, def.max), all: true };
  const set = new Set<number>();
  for (const item of text.split(',')) {
    if (!item) throw new CronError(`${def.name}: empty list item in '${text}'`);
    const [base, stepText, extra] = item.split('/');
    if (extra !== undefined) throw new CronError(`${def.name}: bad step in '${item}'`);
    let step = 1;
    if (stepText !== undefined) {
      if (!/^\d+$/.test(stepText)) throw new CronError(`${def.name}: bad step '${stepText}'`);
      step = Number(stepText);
      if (step < 1 || step > def.max) throw new CronError(`${def.name}: step must be 1-${def.max}`);
    }
    let lo: number;
    let hi: number;
    if (base === '*' || (def.any && base === '?')) {
      lo = def.min;
      hi = def.max;
    } else if (base.includes('-')) {
      const [a, b, more] = base.split('-');
      if (more !== undefined || !a || !b) throw new CronError(`${def.name}: bad range '${base}'`);
      lo = parseValue(a, def);
      hi = parseValue(b, def);
      if (lo > hi) throw new CronError(`${def.name}: range '${base}' goes backwards`);
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
  if (!trimmed) throw new CronError('Enter a cron expression, e.g. "0 9 * * Mon-Fri"');
  const short = SHORTHANDS[trimmed.toLowerCase()];
  let parts = (short ?? trimmed).split(/\s+/);
  if (parts.length === 5) parts = ['0', ...parts];
  if (parts.length !== 6 && parts.length !== 7) {
    throw new CronError(`Expected 5 fields (min hour day month weekday) or 6-7 with seconds first, got ${parts.length}`);
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
  return `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1]}`;
}

/** Evenly spaced from the field's minimum: the step, else null. */
function stepOf(f: CronField, min: number): number | null {
  const v = f.values;
  if (v.length < 2 || v[0] !== min) return null;
  const step = v[1] - v[0];
  for (let i = 2; i < v.length; i++) if (v[i] - v[i - 1] !== step) return null;
  return step;
}

function describeTime(s: CronSchedule): string {
  const sec = s.second;
  const secOnly0 = sec.values.length === 1 && sec.values[0] === 0;
  const secText = secOnly0
    ? ''
    : sec.all
      ? 'every second'
      : stepOf(sec, 0)
        ? `every ${stepOf(sec, 0)} seconds`
        : `at second ${runs(sec.values)}`;

  const { minute: mi, hour: h } = s;
  let text: string;
  if (mi.values.length === 1 && h.values.length <= 6 && !h.all) {
    const oneSec = sec.values.length === 1;
    const times = h.values.map((x) => `${pad(x)}:${pad(mi.values[0])}${oneSec && !secOnly0 ? ':' + pad(sec.values[0]) : ''}`);
    text = `at ${joinList(times)}`;
    return oneSec ? text : `${secText}, during ${joinList(times)}`;
  }
  let minText: string;
  if (mi.all) minText = secOnly0 ? 'every minute' : '';
  else if (stepOf(mi, 0)) minText = `every ${stepOf(mi, 0)} minutes`;
  else if (mi.values.length === 1) minText = mi.values[0] === 0 ? 'every hour' : `every hour at :${pad(mi.values[0])}`;
  else minText = `at minutes ${runs(mi.values, pad)} past the hour`;

  let hourText = '';
  if (!h.all) {
    const hs = h.values;
    const contiguous = hs.every((x, i) => i === 0 || x === hs[i - 1] + 1);
    if (stepOf(h, 0) && !contiguous) hourText = `every ${stepOf(h, 0)} hours`;
    else if (contiguous) hourText = `from ${pad(hs[0])}:00 to ${pad(hs[hs.length - 1])}:59`;
    else hourText = `during hours ${runs(hs, pad)}`;
    if (mi.values.length === 1 && minText === 'every hour' && stepOf(h, 0) && !contiguous) {
      return [secText, `every ${stepOf(h, 0)} hours on the hour`].filter(Boolean).join(', ');
    }
  }
  return [secText, minText, hourText].filter(Boolean).join(', ');
}

function describeDays(s: CronSchedule): string {
  const parts: string[] = [];
  const dowText = (vals: number[]) => {
    if (vals.length === 5 && vals.join() === '2,3,4,5,6') return 'on weekdays';
    if (vals.length === 2 && vals.join() === '1,7') return 'on weekends';
    return `on ${runs(vals, (v) => WEEKDAYS[v - 1])}`;
  };
  if (!s.dom.all && !s.dow.all) {
    parts.push(`on day ${runs(s.dom.values)} of the month, only if it is a ${joinList(s.dow.values.map((v) => WEEKDAYS[v - 1]))}`);
  } else if (!s.dom.all) {
    parts.push(`on day ${runs(s.dom.values)} of the month`);
  } else if (!s.dow.all) {
    parts.push(dowText(s.dow.values));
  } else {
    parts.push('every day');
  }
  if (!s.month.all) parts.push(`in ${runs(s.month.values, (v) => MONTHS[v - 1])}`);
  if (!s.year.all) parts.push(`in ${runs(s.year.values)}`);
  return parts.join(' ');
}

/** "every 15 minutes, from 09:00 to 17:59, on weekdays". */
export function describeCron(s: CronSchedule): string {
  const time = describeTime(s);
  const days = describeDays(s);
  const text = days === 'every day' && /^(every|at minutes)/.test(time) ? time : `${time} ${days}`.trim();
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export type CronPreview =
  | { ok: true; text: string; next: Date[]; warning: string | null }
  | { ok: false; error: string; hint: string | null };

const DOW_HINT = 'Weekdays here count 1 = Sunday … 7 = Saturday (not Unix 0-6); names like Mon-Fri avoid surprises.';

/** Description + next firings + warnings, for the trigger form. */
export function cronPreview(spec: string, now: Date = new Date(), count = 5): CronPreview {
  let s: CronSchedule;
  try {
    s = parseCron(spec);
  } catch (err) {
    const error = (err as Error).message;
    return { ok: false, error, hint: error.startsWith('day of week') ? DOW_HINT : null };
  }
  const next = nextFires(s, now, count);
  let warning: string | null = null;
  if (s.numericDow && !s.dow.all) warning = `${DOW_HINT} This spec means ${runs(s.dow.values, (v) => WEEKDAYS[v - 1].slice(0, 3))}.`;
  if (next.length === 0) warning = 'This spec never fires (in the next years).';
  return { ok: true, text: describeCron(s), next, warning };
}

/** "Mon 9 Nov, 09:00" (seconds only when not :00). */
export function formatFire(d: Date, now: Date = new Date()): string {
  const sameYear = d.getFullYear() === now.getFullYear();
  const date = d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: sameYear ? undefined : 'numeric' });
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}${d.getSeconds() ? ':' + pad(d.getSeconds()) : ''}`;
  return `${date}, ${time}`;
}
