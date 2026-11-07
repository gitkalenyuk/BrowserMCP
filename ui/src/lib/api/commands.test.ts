// Every literal `api.call('name')` in the UI must be a command the core really has: the mock
// accepts anything it implements, so a made-up name (`runs.list` instead of `run.list`) only
// showed up as "unknown command" in the real browser. The list of real commands is the
// generated protocol (`cargo xtask gen-sdk` → sdk/protocol/src/generated/commands.ts).
import { describe, expect, it } from 'vitest';
import generated from '../../../../sdk/protocol/src/generated/commands.ts?raw';

/** Commands the UI already calls that another work stream adds to the core (same shape). */
const PENDING = new Set<string>();

/** `"tabs.open": { args: …; output: … };` lines of the `Commands` interface. */
function realCommands(src: string): Set<string> {
  return new Set([...src.matchAll(/^ {2}"([a-z][\w]*\.[\w.]+)": \{ args: /gm)].map((m) => m[1]));
}

/** `api.call('x')`, `this.api.call<…>("x", …)`, `chrome.api.call(`x`)` … */
function calledCommands(src: string): string[] {
  return [...src.matchAll(/\.call(?:<[^>()]*>)?\(\s*['"`]([a-z][\w]*\.[\w.]+)['"`]/g)].map((m) => m[1]);
}

describe('UI calls only commands the core has', () => {
  const sources = import.meta.glob<string>(['../../**/*.{ts,svelte}', '!../../**/*.test.ts'], {
    query: '?raw',
    import: 'default',
    eager: true,
  });
  const real = realCommands(generated);

  it('reads the generated command list', () => {
    expect(real.size).toBeGreaterThan(100);
    expect(real.has('run.list')).toBe(true);
    expect(real.has('tabs.open')).toBe(true);
  });

  it('finds calls in every form', () => {
    expect(calledCommands(`api.call('run.list'); this.api.call<X>("tabs.open", {}); chrome.api.call(\`nav.goto\`, p)`)).toEqual([
      'run.list',
      'tabs.open',
      'nav.goto',
    ]);
  });

  it('every api.call literal is a real command', () => {
    const unknown: string[] = [];
    let calls = 0;
    for (const [file, src] of Object.entries(sources)) {
      for (const name of calledCommands(src)) {
        calls++;
        if (!real.has(name) && !PENDING.has(name)) unknown.push(`${file}: ${name}`);
      }
    }
    expect(calls).toBeGreaterThan(100);
    expect(unknown).toEqual([]);
  });
});
