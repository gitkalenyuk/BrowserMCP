import { afterEach, describe, expect, it } from 'vitest';
import type { BrowserApi } from '../api/client';
import { settings } from '../settings.svelte';
import { TerminalStore } from './store.svelte';

/** Just enough of the API for `start()`: records the event handlers so the test can fire them. */
function fakeApi() {
  const handlers = new Map<string, (payload: never) => void>();
  const api = {
    on: (event: string, cb: (payload: never) => void) => {
      handlers.set(event, cb);
      return () => handlers.delete(event);
    },
    onStatus: () => () => {},
    call: () => Promise.resolve([]),
  } as unknown as BrowserApi;
  const fire = (event: string, payload: unknown) => handlers.get(event)?.(payload as never);
  return { api, fire };
}

afterEach(() => settings.update({ language: 'system' }));

describe('terminal exit note', () => {
  function exitNote(code: number | null): string {
    const { api, fire } = fakeApi();
    const store = new TerminalStore(api);
    store.start();
    fire('term.exit', { id: 't1', code });
    const text = store.attach('t1', () => {});
    store.stop();
    return text;
  }

  it('is written in the UI language', () => {
    expect(exitNote(0)).toContain('[process exited with code 0]');
    expect(exitNote(null)).toContain('[process exited]');
    settings.update({ language: 'uk' });
    expect(exitNote(2)).toContain('[процес завершився з кодом 2]');
    expect(exitNote(null)).toContain('[процес завершився]');
  });
});
