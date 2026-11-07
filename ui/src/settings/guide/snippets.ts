// One command, four ways to call it: what the guide's API examples show and copy.

/** The default API port of the browser (crates/app/src/cli.rs DEFAULT_API_PORT). */
export const API_BASE = 'http://127.0.0.1:47800';

export type ApiFlavor = 'mcp' | 'powershell' | 'curl' | 'sdk';
export const API_FLAVORS: ApiFlavor[] = ['mcp', 'powershell', 'curl', 'sdk'];

/** MCP tool name: the dot becomes `_` (the Anthropic API refuses dots in tool names). */
export function wireName(command: string): string {
  return command.replace('.', '_');
}

/** JSON on one line when short, else indented (also valid JavaScript for the SDK example). */
export function argsJson(args: Record<string, unknown>, indent = ''): string {
  // JSON escapes newlines inside strings, so every raw newline is layout and may be collapsed.
  const flat = JSON.stringify(args, null, 1).replace(/\n\s*/g, ' ');
  if (flat.length <= 72) return flat;
  return JSON.stringify(args, null, 2).replace(/\n/g, `\n${indent}`);
}

export function snippet(flavor: ApiFlavor, command: string, args: Record<string, unknown>): string {
  const empty = Object.keys(args).length === 0;
  const body = JSON.stringify(args);
  switch (flavor) {
    case 'mcp':
      return `${wireName(command)} ${argsJson(args)}`;
    case 'powershell':
      // Single quotes: PowerShell takes the JSON literally (no escaping of " needed). The charset
      // matters: without it Windows PowerShell 5.1 sends the body as ISO-8859-1, which mangles
      // any non-Latin text (a Ukrainian prompt) before it reaches the API.
      return `Invoke-RestMethod -Method Post ${API_BASE}/v1/cmd/${command} -ContentType 'application/json; charset=utf-8' -Body '${body.replace(/'/g, "''")}'`;
    case 'curl':
      return `curl -X POST ${API_BASE}/v1/cmd/${command} -H "content-type: application/json" -d '${body.replace(/'/g, "'\\''")}'`;
    case 'sdk': {
      const [ns, method] = command.split('.');
      return `await browser.${ns}.${method}(${empty ? '{}' : argsJson(args)});`;
    }
  }
}
