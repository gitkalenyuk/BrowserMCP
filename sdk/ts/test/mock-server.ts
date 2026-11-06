// A tiny in-process stand-in for BrowserMCP's /v1/ws and /v1/cmd, enough to test the SDK.
//
// Commands:  tabs.list → two tabs · nav.goto without url → INVALID_PARAMS · tabs.open → emits
// command.finished · test.slow → never answers · test.drop → closes the socket.

import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { WebSocketServer, type WebSocket } from "ws";

const DEFAULT_TOPICS = ["tab.*", "tabs.*"];

export const TABS = [
  { id: 1, title: "Example Domain", url: "https://example.com/", loading: false },
  { id: 2, title: "New Tab", url: "about:blank", loading: false },
];

interface Request {
  id?: number | string;
  method: string;
  params?: Record<string, unknown> | null;
}

export class MockBrowser {
  readonly server: Server;
  readonly wss: WebSocketServer;
  /** Every JSON-RPC request received, in order. */
  readonly requests: Request[] = [];
  connections = 0;
  #seq = 0;
  #subs = new Map<WebSocket, Set<string>>();

  private constructor() {
    this.server = createServer((req, res) => void this.#rest(req, res));
    this.wss = new WebSocketServer({ server: this.server, path: "/v1/ws" });
    this.wss.on("connection", (ws) => {
      this.connections++;
      this.#subs.set(ws, new Set(DEFAULT_TOPICS));
      ws.on("message", (data) => this.#onMessage(ws, JSON.parse(String(data)) as Request));
      ws.on("close", () => this.#subs.delete(ws));
    });
  }

  /** Listen on `port` (default: any free port). */
  static async start(port = 0): Promise<MockBrowser> {
    const mock = new MockBrowser();
    await new Promise<void>((resolve) => mock.server.listen(port, "127.0.0.1", resolve));
    return mock;
  }

  get port(): number {
    return (this.server.address() as AddressInfo).port;
  }

  get wsUrl(): string {
    return `ws://127.0.0.1:${this.port}/v1/ws`;
  }

  get httpUrl(): string {
    return `http://127.0.0.1:${this.port}/v1`;
  }

  /** Send an event to every connection subscribed to it. */
  emit(topic: string, data: Record<string, unknown>): void {
    const seq = ++this.#seq;
    for (const [ws, patterns] of this.#subs) {
      if ([...patterns].some((p) => p === "*" || p === topic || (p.endsWith(".*") && topic.startsWith(p.slice(0, -1))))) {
        ws.send(JSON.stringify({ jsonrpc: "2.0", method: topic, params: { ...data, seq } }));
      }
    }
  }

  /** Close every open WebSocket (simulates the browser dropping the connection). */
  dropAll(): void {
    for (const ws of this.wss.clients) ws.terminate();
  }

  async stop(): Promise<void> {
    this.dropAll();
    await new Promise<void>((resolve) => this.wss.close(() => resolve()));
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
  }

  /** The command logic shared by WS and REST: `{result}` or `{error, status}`. */
  #run(method: string, params: Record<string, unknown>): { result?: unknown; error?: Record<string, unknown>; rpc?: number; http?: number } {
    switch (method) {
      case "tabs.list":
      case "tabs_list":
        return { result: { tabs: TABS } };
      case "nav.goto":
        if (typeof params.url !== "string") {
          return {
            rpc: -32602,
            http: 400,
            error: { code: "INVALID_PARAMS", message: "invalid arguments for nav.goto: missing field `url`", retry: "no", hint: "Check the argument schema of nav_goto in the tool list." },
          };
        }
        return { result: { id: 1, title: "Example Domain", url: params.url, loading: false } };
      case "tabs.open":
        this.emit("command.finished", { call_id: 1, name: "tabs.open", caller: { kind: "ws", conn: 1 }, duration_ms: 3, ok: true, error: null });
        return { result: { tab: 3 } };
      default:
        return {
          rpc: -32601,
          http: 404,
          error: { code: "NOT_FOUND", message: `unknown command '${method}'`, retry: "no", candidates: ["tabs.list", "tabs.open"] },
        };
    }
  }

  #onMessage(ws: WebSocket, req: Request): void {
    this.requests.push(req);
    const params = req.params ?? {};
    const reply = (body: Record<string, unknown>) => {
      if (req.id !== undefined) ws.send(JSON.stringify({ jsonrpc: "2.0", id: req.id, ...body }));
    };
    if (req.method === "events.subscribe" || req.method === "events.unsubscribe") {
      const subs = this.#subs.get(ws)!;
      for (const t of (params.topics as string[] | undefined) ?? []) {
        if (req.method === "events.subscribe") subs.add(t);
        else subs.delete(t);
      }
      return reply({ result: { topics: [...subs].sort() } });
    }
    if (req.method === "test.slow") return; // never answers
    if (req.method === "test.drop") return ws.terminate();
    const out = this.#run(req.method, params);
    if (out.error) reply({ error: { code: out.rpc, message: out.error.message, data: out.error } });
    else reply({ result: out.result });
  }

  async #rest(req: import("node:http").IncomingMessage, res: import("node:http").ServerResponse): Promise<void> {
    let body = "";
    for await (const chunk of req) body += String(chunk);
    const name = decodeURIComponent((req.url ?? "").replace(/^\/v1\/cmd\//, ""));
    const out = this.#run(name, body ? (JSON.parse(body) as Record<string, unknown>) : {});
    res.writeHead(out.error ? (out.http ?? 500) : 200, { "content-type": "application/json" });
    res.end(JSON.stringify(out.error ?? out.result));
  }
}
