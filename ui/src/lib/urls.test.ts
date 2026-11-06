import { describe, expect, it } from 'vitest';
import { omniboxIntent, resolveOmniboxInput } from './urls';

const S = 'https://search.test/?q=%s';

describe('resolveOmniboxInput', () => {
  it('passes full URLs through', () => {
    expect(resolveOmniboxInput('https://example.com/a?b=1', S)).toBe('https://example.com/a?b=1');
    expect(resolveOmniboxInput('bmcp://newtab/', S)).toBe('bmcp://newtab/');
    expect(resolveOmniboxInput('about:blank', S)).toBe('about:blank');
  });

  it('adds a scheme to host-like input', () => {
    expect(resolveOmniboxInput('example.com', S)).toBe('https://example.com');
    expect(resolveOmniboxInput('docs.rs/cef', S)).toBe('https://docs.rs/cef');
    expect(resolveOmniboxInput('example.com:8080/x', S)).toBe('https://example.com:8080/x');
    expect(resolveOmniboxInput('localhost:5173', S)).toBe('http://localhost:5173');
    expect(resolveOmniboxInput('127.0.0.1:47800/v1', S)).toBe('http://127.0.0.1:47800/v1');
  });

  it('searches everything else', () => {
    expect(resolveOmniboxInput('svelte runes', S)).toBe('https://search.test/?q=svelte%20runes');
    expect(resolveOmniboxInput('rust', S)).toBe('https://search.test/?q=rust');
    expect(resolveOmniboxInput('javascript:alert(1)', S)).toBe('https://search.test/?q=javascript%3Aalert(1)');
    expect(resolveOmniboxInput('   ', S)).toBeNull();
  });

  it('classifies intent', () => {
    expect(omniboxIntent('example.com', S)).toBe('url');
    expect(omniboxIntent('what is cef', S)).toBe('search');
    expect(omniboxIntent('', S)).toBeNull();
  });
});
