// The AI panel as it renders (server render with the mock core: markup only). Checks what the
// user sees: chat names instead of "#N", the queue, Markdown answers (sanitised), the hover
// actions, the voice slot, and the Ukrainian strings.
import { render } from 'svelte/server';
import { afterEach, describe, expect, it } from 'vitest';
import { MockApi } from '../../api/mock';
import type { AiSession } from '../../api/types';
import { settings } from '../../settings.svelte';
import { ChromeController } from '../../state/chrome.svelte';
import AiMarkdown from './AiMarkdown.svelte';
import AiPanelHarness from './AiPanelHarness.svelte';

afterEach(() => settings.update({ language: 'system' }));

const chat = (id: string, title: string, extra: Partial<AiSession> = {}): AiSession => ({
  id,
  provider: 'claude-code',
  model: 'haiku',
  color: '#3b82f6',
  status: 'idle',
  tabs: [],
  created_ms: 1,
  title,
  title_set: false,
  archived: false,
  updated_ms: 1,
  queued: 0,
  ...extra,
});

/** Markup without hydration comments and scoped-style classes. */
const clean = (html: string) =>
  html
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/ class="svelte-[a-z0-9]+"/g, '')
    .replace(/ svelte-[a-z0-9]+/g, '');

function panel(): string {
  const chrome = new ChromeController(new MockApi(), { host: null });
  const ai = chrome.ai;
  ai.providers = [
    { id: 'claude-code', name: 'Claude Code', kind: 'cli', protocol: 'claude-code', configured: true, needs_key: false, has_key: false, base_url: '', model: null, models: [], path: null },
  ];
  ai.sessions = [chat('claude-code-24', 'Find direct flights to Lviv', { status: 'running', queued: 1 }), chat('claude-code-3', '')];
  ai.activeId = 'claude-code-24';
  ai.threads['claude-code-24'] = [
    { kind: 'user', text: 'Find direct flights to Lviv', ts: Date.now() },
    { kind: 'assistant', text: '## Flights\n\n| Time | Price |\n|---|--:|\n| 07:40 | €129 |\n\n```ts\nconst x = 1;\n```\n\n<script>alert(1)</script> [bad](javascript:alert(1)) [ok](https://example.com)', streaming: false },
    { kind: 'tool', name: 'tabs_open', args: { url: 'https://e.com' }, pending: false, ok: true, summary: '{"tab":7}' },
    { kind: 'user', text: 'only morning ones', queued: true },
  ];
  return clean(render(AiPanelHarness, { props: { chrome } }).body);
}

describe('AI panel', () => {
  it('names chats by their title (or the provider), never "#N" or the id', () => {
    const html = panel();
    expect(html).toContain('Find direct flights to Lviv');
    expect(html).toContain('>Claude Code</span>'); // the untitled chat
    expect(html).not.toMatch(/#(24|3)\b/); // (colors like #3b82f6 are fine)
    expect(html).not.toContain('claude-code-24');
  });

  it('queues messages while it works: the pill, the count, the stop button next to send', () => {
    const html = panel();
    expect(html).toContain('queued</span>');
    expect(html).toContain('1 queued');
    expect(html).toContain('Esc interrupts');
    expect(html).toContain('aria-label="Interrupt"');
    expect(html).toContain('aria-label="More ways to stop"');
    expect(html).toContain('aria-label="Send"');
    expect(html).toContain('placeholder="Add to the task while it works… (Enter sends)"');
  });

  it('answers render as Markdown, raw HTML from the model stays text', () => {
    const html = panel();
    expect(html).toContain('<table>');
    expect(html).toContain('Copy code');
    expect(html).not.toContain('<script>alert(1)');
    expect(html).toContain('&lt;script>alert(1)&lt;/script>');
    expect(html).not.toContain('javascript:');
    expect(html).toContain('href="https://example.com"');
    expect(html).toContain('target="_blank"');
  });

  it('messages have Copy / Edit / Retry, tools Copy; the model picker, history and the voice slot are there', () => {
    const html = panel();
    expect(html).toContain('aria-label="Edit"');
    expect(html).toContain('aria-label="Retry"');
    expect(html).toContain('title="Copy arguments"');
    expect(html).toContain('aria-label="Copy chat (Markdown)"');
    expect(html).toContain('aria-label="History"');
    expect(html).toContain('title="Model of this chat. A change takes effect from the next answer."');
    expect(html).toContain('class="composer-tools');
  });

  it('switches to Ukrainian', () => {
    settings.update({ language: 'uk' });
    const html = panel();
    expect(html).toContain('у черзі');
    expect(html).toContain('Esc — перервати');
    expect(html).toContain('aria-label="Історія"');
    expect(html).toContain('aria-label="Редагувати"');
    expect(html).not.toContain('aria-label="Edit"');
  });
});

describe('AiMarkdown', () => {
  const md = (text: string) => clean(render(AiMarkdown, { props: { text, onlink: () => {} } }).body);

  it('renders lists, inline marks and code without extra spaces', () => {
    const html = md('- **bold** and `code`\n- [x](https://a.io)');
    expect(html).toContain('<ul>');
    expect(html).toMatch(/<strong>bold<\/strong> and <code>code<\/code>/);
    expect(html).toMatch(/<a href="https:\/\/a.io"[^>]*>x<\/a>/);
  });

  it('never emits elements from the model text', () => {
    const html = md('<img src=x onerror=alert(1)> <iframe src="https://e.com"></iframe>\n\n![p](https://e.com/p.png)');
    expect(html).not.toMatch(/<img|<iframe/);
    expect(html).toContain('&lt;img');
  });
});
