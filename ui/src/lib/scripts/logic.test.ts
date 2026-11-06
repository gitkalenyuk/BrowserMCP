import { describe, expect, it } from 'vitest';
import { createScriptMocks } from '../api/mock-scripts';
import type { ApiEvents, EventName } from '../api/types';
import { editorOwnsKey, endEntry, freeName, inferInputs, outcomeEntries, toggleLine, validName } from './logic';

describe('inferInputs', () => {
  it('finds dotted, bracketed and destructured inputs once, in order', () => {
    const code = `
      const email = inputs.email;
      const pw = inputs["pass word"];
      const { city, zip: postal, country = "DE", ...rest } = inputs;
      if (inputs?.email) console.log(inputs.retries);
      const notInputs = myinputs.nope;
    `;
    expect(inferInputs(code)).toEqual(['email', 'pass word', 'retries', 'city', 'zip', 'country']);
  });
  it('returns nothing for a script without inputs', () => {
    expect(inferInputs('await browser.tabs.list();')).toEqual([]);
  });
});

describe('console entries', () => {
  it('splits a run outcome and links the error line', () => {
    const entries = outcomeEntries({
      ok: false,
      exit_code: 1,
      duration_ms: 1530,
      result: { a: 1 },
      stdout: 'one\ntwo\n',
      stderr: 'warned\nerror: Uncaught Error: boom\n    at file:///x.ts:7:3\n',
      error: { code: 'NOT_FOUND', command: 'page.find', message: 'no element', line: 7 },
    });
    expect(entries.map((e) => e.kind)).toEqual(['stdout', 'stdout', 'stderr', 'result', 'error']);
    expect(entries[3].text).toBe('{\n  "a": 1\n}');
    expect(entries[4]).toEqual({ kind: 'error', text: 'NOT_FOUND (browser.page.find): no element', line: 7 });
  });
  it('says done / stopped', () => {
    expect(endEntry(true, 120, null, false).text).toBe('done in 120 ms');
    expect(endEntry(false, 2500, null, true).text).toBe('stopped after 2.50 s');
  });
});

describe('small helpers', () => {
  it('toggles breakpoints and picks free names', () => {
    expect(toggleLine([3, 9], 5)).toEqual([3, 5, 9]);
    expect(toggleLine([3, 5, 9], 5)).toEqual([3, 9]);
    expect(freeName('script', ['script', 'script-2'])).toBe('script-3');
    expect(validName('login.v2')).toBe(true);
    expect(validName('../x')).toBe(false);
    expect(validName('-x')).toBe(false);
  });
  it('lets only the editor keys through inside data-own-keys', () => {
    const inside = { closest: () => ({}) };
    const outside = { closest: () => null };
    const key = (k: string, code: string, extra: Partial<KeyboardEvent> = {}) =>
      ({ key: k, code, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...extra }) as KeyboardEvent;
    expect(editorOwnsKey({ ...key('F5', 'F5'), target: inside as unknown as EventTarget })).toBe(true);
    expect(editorOwnsKey({ ...key('F11', 'F11', { shiftKey: true }), target: inside as unknown as EventTarget })).toBe(true);
    expect(editorOwnsKey({ ...key('s', 'KeyS', { ctrlKey: true }), target: inside as unknown as EventTarget })).toBe(true);
    expect(editorOwnsKey({ ...key('t', 'KeyT', { ctrlKey: true }), target: inside as unknown as EventTarget })).toBe(false);
    expect(editorOwnsKey({ ...key('F5', 'F5'), target: outside as unknown as EventTarget })).toBe(false);
  });
});

describe('mock debugger', () => {
  it('pauses on breakpoints, steps and ends', async () => {
    const events: Array<{ event: EventName; payload: unknown }> = [];
    const emit = <E extends EventName>(event: E, payload: ApiEvents[E]) => events.push({ event, payload });
    const api = createScriptMocks(emit, { delay: 0 });
    const tick = () => new Promise((r) => setTimeout(r, 5));
    const code = 'const a = 1;\n\nconsole.log("hi");\nconst b = 2;\nbrowser.output({ b });\n';

    // Line 2 is empty: the breakpoint moves to line 3.
    const started = await api['script.debug_start']({ code, breakpoints: [2] });
    expect(started.breakpoints).toEqual([3]);
    await tick();
    const paused = events.find((e) => e.event === 'script.debug_paused')!.payload as ApiEvents['script.debug_paused'];
    expect(paused.line).toBe(3);
    expect(paused.scopes.at(-1)!.variables).toEqual([{ name: 'a', type: 'number', value: '1' }]);

    events.length = 0;
    await api['script.debug_cmd']({ id: started.id, action: 'step_over' });
    await tick();
    expect(events.map((e) => e.event)).toEqual(['script.debug_resumed', 'script.debug_output', 'script.debug_paused']);
    expect((events[2].payload as ApiEvents['script.debug_paused']).line).toBe(4);

    events.length = 0;
    await api['script.debug_cmd']({ id: started.id, action: 'continue' });
    await tick();
    const ended = events.find((e) => e.event === 'script.debug_ended')!.payload as ApiEvents['script.debug_ended'];
    expect(ended.ok).toBe(true);
    expect(ended.result).toEqual({ mock: true });
    await expect(async () => api['script.debug_cmd']({ id: started.id, action: 'continue' })).rejects.toThrow(/not found/);
  });
});
