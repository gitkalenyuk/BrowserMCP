// Settings → Connections & API: the ready-to-copy commands and the toolsets URL.
// Pure functions (tested in e2-connections.test.ts); the section only renders them.

import type { ApiToolset } from '../api/types-e2-settings';

/** Name the MCP server gets in Claude Code / Codex. */
export const SERVER_NAME = 'browsermcp';
export const DEFAULT_PORT = 47800;

/** Toolsets in the order the picker shows them (unknown ones from a newer core go last). */
const ORDER = ['core', 'net', 'record', 'script', 'run', 'profile', 'stealth', 'data', 'ai', 'ui', 'system'];

export function sortToolsets(list: ApiToolset[]): ApiToolset[] {
  const rank = (name: string) => {
    const i = ORDER.indexOf(name);
    return i < 0 ? ORDER.length : i;
  };
  return [...list].sort((a, b) => rank(a.name) - rank(b.name) || a.name.localeCompare(b.name));
}

/** The toolsets listed when the URL names none. */
export function defaultToolsets(list: ApiToolset[]): string[] {
  return list.filter((t) => t.default).map((t) => t.name);
}

/**
 * `http://127.0.0.1:47800/mcp` plus `?toolsets=a,b` when the choice differs from the default
 * set (the server lists only the defaults without the parameter). An empty choice falls back
 * to the defaults: an MCP server with no tools is never what the user wants.
 */
export function mcpUrl(base: string, selected: readonly string[], defaults: readonly string[], all: readonly string[] = []): string {
  const order = (names: readonly string[]) => {
    const known = all.length ? all : ORDER;
    return [...new Set(names)].sort((a, b) => {
      const ia = known.indexOf(a);
      const ib = known.indexOf(b);
      return (ia < 0 ? known.length : ia) - (ib < 0 ? known.length : ib) || a.localeCompare(b);
    });
  };
  const chosen = order(selected);
  const same = chosen.length === 0 || chosen.join() === order(defaults).join();
  return same ? base : `${base}?toolsets=${chosen.join(',')}`;
}

/** Quote a Windows path for PowerShell / cmd when it has spaces (or is empty). */
export function quotePath(path: string): string {
  return /[\s&()^;,]/.test(path) || !path ? `"${path}"` : path;
}

/** `?` and `&` mean something to shells (glob, background job): quote such URLs. */
export function quoteUrl(url: string): string {
  return /[?&*]/.test(url) ? `"${url}"` : url;
}

export function claudeHttpCommand(url: string): string {
  return `claude mcp add --transport http ${SERVER_NAME} ${quoteUrl(url)}`;
}

/** stdio variant: Claude Code starts `browsermcp.exe mcp-stdio`, which finds (or starts) the browser. */
export function claudeStdioCommand(exe: string): string {
  return `claude mcp add ${SERVER_NAME} -- ${quotePath(exe)} mcp-stdio`;
}

export function codexStdioCommand(exe: string): string {
  return `codex mcp add ${SERVER_NAME} -- ${quotePath(exe)} mcp-stdio`;
}

/** A TOML literal string ('…'): no escapes, so Windows backslashes stay as they are. */
export function tomlString(text: string): string {
  // A literal string cannot contain `'`; fall back to a basic string with escapes.
  if (!text.includes("'") && !/[\n\r]/.test(text)) return `'${text}'`;
  return `"${text.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\r/g, '\\r')}"`;
}

/** `~/.codex/config.toml` entry that runs the stdio bridge (works with every Codex version). */
export function codexStdioToml(exe: string): string {
  return [`[mcp_servers.${SERVER_NAME}]`, `command = ${tomlString(exe)}`, `args = ["mcp-stdio"]`].join('\n');
}

/** `~/.codex/config.toml` entry for the HTTP endpoint (Codex versions with streamable HTTP). */
export function codexHttpToml(url: string): string {
  return [`[mcp_servers.${SERVER_NAME}]`, `url = ${tomlString(url)}`].join('\n');
}

/** JSON for clients configured with an `mcpServers` file (Cursor, Claude Desktop, …). */
export function mcpJson(url: string): string {
  return JSON.stringify({ mcpServers: { [SERVER_NAME]: { type: 'http', url } } }, null, 2);
}

export function cdpUrl(port: number): string {
  return `http://127.0.0.1:${port}`;
}

export function playwrightSnippet(port: number): string {
  return `const browser = await chromium.connectOverCDP('${cdpUrl(port)}');`;
}

export function puppeteerSnippet(port: number): string {
  return `const browser = await puppeteer.connect({ browserURL: '${cdpUrl(port)}' });`;
}

/** Problems with a CDP port typed in the form, as an i18n key ('' = fine). */
export function cdpPortError(text: string, apiPort: number | null): string {
  if (!/^\d+$/.test(text.trim())) return 'settings.connections.cdp.portNumber';
  const n = Number(text);
  if (n < 1024 || n > 65535) return 'settings.connections.cdp.portRange';
  if (n === (apiPort ?? DEFAULT_PORT)) return 'settings.connections.cdp.portTaken';
  return '';
}

/** The name of the agent holding a tab lock (its id when the agent is not listed). */
export function agentName(owner: string, agents: readonly { id: string; name: string }[]): string {
  return agents.find((a) => a.id === owner)?.name ?? owner;
}
