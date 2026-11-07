import { describe, expect, it } from 'vitest';
import type { ApiToolset } from '../api/types-e2-settings';
import {
  agentName,
  cdpPortError,
  claudeHttpCommand,
  claudeStdioCommand,
  codexHttpToml,
  codexStdioCommand,
  codexStdioToml,
  defaultToolsets,
  mcpJson,
  mcpUrl,
  playwrightSnippet,
  quotePath,
  sortToolsets,
  tomlString,
} from './e2-connections';

const BASE = 'http://127.0.0.1:47800/mcp';
const ts = (name: string, def = false): ApiToolset => ({ name, commands: 3, default: def });

describe('toolsets', () => {
  it('sorts in the picker order and finds the defaults', () => {
    const list = [ts('system', true), ts('zzz'), ts('net'), ts('core', true)];
    expect(sortToolsets(list).map((t) => t.name)).toEqual(['core', 'net', 'system', 'zzz']);
    expect(defaultToolsets(list)).toEqual(['system', 'core']);
  });

  it('adds ?toolsets= only when the choice differs from the defaults', () => {
    const all = ['core', 'net', 'record', 'system'];
    expect(mcpUrl(BASE, ['system', 'core'], ['core', 'system'], all)).toBe(BASE);
    expect(mcpUrl(BASE, [], ['core', 'system'], all)).toBe(BASE);
    expect(mcpUrl(BASE, ['net', 'core', 'net'], ['core', 'system'], all)).toBe(`${BASE}?toolsets=core,net`);
    expect(mcpUrl(BASE, ['system', 'record', 'core'], ['core', 'system'], all)).toBe(`${BASE}?toolsets=core,record,system`);
  });
});

describe('client commands', () => {
  const exe = 'C:\\Program Files\\BrowserMCP\\browsermcp.exe';

  it('quotes paths with spaces only', () => {
    expect(quotePath('C:\\bmcp\\browsermcp.exe')).toBe('C:\\bmcp\\browsermcp.exe');
    expect(quotePath(exe)).toBe(`"${exe}"`);
  });

  it('builds the Claude Code and Codex commands', () => {
    expect(claudeHttpCommand(BASE)).toBe(`claude mcp add --transport http browsermcp ${BASE}`);
    expect(claudeHttpCommand(`${BASE}?toolsets=core,net`)).toBe(`claude mcp add --transport http browsermcp "${BASE}?toolsets=core,net"`);
    expect(claudeStdioCommand(exe)).toBe(`claude mcp add browsermcp -- "${exe}" mcp-stdio`);
    expect(codexStdioCommand('C:\\b\\browsermcp.exe')).toBe('codex mcp add browsermcp -- C:\\b\\browsermcp.exe mcp-stdio');
  });

  it('writes TOML that keeps Windows backslashes', () => {
    expect(tomlString('C:\\b\\x.exe')).toBe("'C:\\b\\x.exe'");
    // A quote forces a basic string with escapes.
    expect(tomlString("C:\\it's\\x.exe")).toBe('"C:\\\\it\'s\\\\x.exe"');
    expect(codexStdioToml('C:\\b\\browsermcp.exe')).toBe(
      "[mcp_servers.browsermcp]\ncommand = 'C:\\b\\browsermcp.exe'\nargs = [\"mcp-stdio\"]",
    );
    expect(codexHttpToml(BASE)).toBe(`[mcp_servers.browsermcp]\nurl = '${BASE}'`);
  });

  it('writes mcpServers JSON and CDP snippets', () => {
    expect(JSON.parse(mcpJson(BASE))).toEqual({ mcpServers: { browsermcp: { type: 'http', url: BASE } } });
    expect(playwrightSnippet(9222)).toContain("connectOverCDP('http://127.0.0.1:9222')");
  });
});

describe('cdpPortError', () => {
  it('checks number, range and the API port', () => {
    expect(cdpPortError('9222', 47800)).toBe('');
    expect(cdpPortError('', 47800)).toBe('settings.connections.cdp.portNumber');
    expect(cdpPortError('92a', 47800)).toBe('settings.connections.cdp.portNumber');
    expect(cdpPortError('80', 47800)).toBe('settings.connections.cdp.portRange');
    expect(cdpPortError('70000', 47800)).toBe('settings.connections.cdp.portRange');
    expect(cdpPortError('47800', 47800)).toBe('settings.connections.cdp.portTaken');
    expect(cdpPortError('47801', 47801)).toBe('settings.connections.cdp.portTaken');
    expect(cdpPortError('47800', null)).toBe('settings.connections.cdp.portTaken');
  });
});

describe('tab locks', () => {
  it('names the agent holding a lock', () => {
    const agents = [{ id: 'a-1', name: 'Claude Code' }];
    expect(agentName('a-1', agents)).toBe('Claude Code');
    expect(agentName('a-9', agents)).toBe('a-9');
  });
});
