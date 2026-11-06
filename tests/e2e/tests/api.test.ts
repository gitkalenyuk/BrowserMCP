// The three API surfaces of one running browser: REST, WebSocket JSON-RPC, MCP (Streamable HTTP).

import { BrowserError, connect, type BrowserClient } from "@browsermcp/sdk";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, mcpUrl, restUrl, until } from "../lib/env.js";

describe("REST", () => {
  it("health", async () => {
    const res = await fetch(`${restUrl()}/health`);
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toMatchObject({ ok: true, name: "BrowserMCP" });
    expect(body.commands).toBeGreaterThan(100);
  });

  it("lists commands, filtered by toolset", async () => {
    const all = await (await fetch(`${restUrl()}/commands`)).json();
    const names: string[] = all.commands.map((c: { name: string }) => c.name);
    expect(names).toEqual(expect.arrayContaining(["tabs.open", "page.snapshot", "net.capture", "system.quit"]));
    const core = await (await fetch(`${restUrl()}/commands?toolsets=core`)).json();
    expect(core.commands.length).toBeLessThan(all.commands.length);
    expect(core.commands.every((c: { toolset: string }) => c.toolset === "core")).toBe(true);
    const bad = await fetch(`${restUrl()}/commands?toolsets=nope`);
    expect(bad.status).toBe(400);
  });

  it("calls a command, and maps errors to statuses", async () => {
    const ping = await fetch(`${restUrl()}/cmd/system.ping`, { method: "POST", body: "{}" });
    expect(await ping.json()).toMatchObject({ name: "BrowserMCP" });
    const missing = await fetch(`${restUrl()}/cmd/tabs.close`, { method: "POST", body: JSON.stringify({ tab: 999999 }) });
    expect(missing.status).toBe(404);
    expect(await missing.json()).toMatchObject({ code: "TAB_NOT_FOUND" });
    const invalid = await fetch(`${restUrl()}/cmd/nav.goto`, { method: "POST", body: "not json" });
    expect(invalid.status).toBe(400);
  });

  it("serves an OpenAPI document with every command", async () => {
    const doc = await (await fetch(`http://127.0.0.1:${env().apiPort}/openapi.json`)).json();
    expect(doc.openapi).toMatch(/^3\./);
    expect(doc.servers[0].url).toBe(`http://127.0.0.1:${env().apiPort}`);
    expect(Object.keys(doc.paths)).toEqual(expect.arrayContaining(["/v1/health", "/v1/cmd/tabs.open", "/v1/cmd/system.quit"]));
    expect(doc.paths["/v1/cmd/tabs.open"].post.requestBody).toBeDefined();
  });

  it("refuses requests from other web origins", async () => {
    const res = await fetch(`${restUrl()}/cmd/system.ping`, {
      method: "POST",
      body: "{}",
      headers: { Origin: "https://evil.example" },
    });
    expect(res.status).toBe(403);
  });

  it("the SDK works over HTTP too", async () => {
    const http = await connect({ url: restUrl() });
    expect((await http.system.ping({})).name).toBe("BrowserMCP");
    await expect(http.tabs.close({ tab: 999999 })).rejects.toMatchObject({ code: "TAB_NOT_FOUND", status: 404 });
  });
});

describe("WebSocket", () => {
  let browser: BrowserClient;
  const tabs: number[] = [];
  beforeAll(async () => {
    browser = await client();
  });
  afterAll(async () => {
    await closeTabs(browser, tabs);
    browser.close();
  });

  it("calls commands with typed results and errors", async () => {
    expect((await browser.system.ping({})).name).toBe("BrowserMCP");
    const err = await browser.tabs.activate({ tab: 999999 }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(BrowserError);
    expect((err as BrowserError).code).toBe("TAB_NOT_FOUND");
  });

  it("delivers tab and command events", async () => {
    const created: number[] = [];
    const finished: string[] = [];
    const offTab = browser.on("tab.created", (e) => created.push((e as { tab: { id: number } }).tab.id));
    const offCmd = browser.on("command.finished", (e) => finished.push(e.name));
    const { id } = await browser.tabs.open({ url: fixture("list.html") });
    tabs.push(id);
    await until("tab.created event", async () => created.includes(id));
    await until("command.finished event", async () => finished.includes("tabs.open"));
    offTab();
    offCmd();
  });
});

describe("MCP (Streamable HTTP)", () => {
  let session: string | undefined;
  let nextId = 1;

  /** One JSON-RPC request; the answer is plain JSON or an SSE stream with one message. */
  async function rpc(method: string, params: unknown = {}, notification = false): Promise<any> {
    const body = notification ? { jsonrpc: "2.0", method, params } : { jsonrpc: "2.0", id: nextId++, method, params };
    const res = await fetch(`${mcpUrl()}?agent=e2e`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json, text/event-stream",
        ...(session ? { "mcp-session-id": session } : {}),
      },
      body: JSON.stringify(body),
    });
    expect(res.status, await res.clone().text()).toBeLessThan(300);
    session ??= res.headers.get("mcp-session-id") ?? undefined;
    if (notification) return undefined;
    const text = await res.text();
    const json = res.headers.get("content-type")?.includes("text/event-stream")
      ? text
          .split("\n")
          .filter((l) => l.startsWith("data:"))
          .map((l) => l.slice(5).trim())
          .filter((l) => l.startsWith("{"))
          .map((l) => JSON.parse(l))
          .find((m) => m.id !== undefined)
      : JSON.parse(text);
    if (json.error) throw new Error(`${method}: ${JSON.stringify(json.error)}`);
    return json.result;
  }

  it("initialize", async () => {
    const result = await rpc("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "bmcp-e2e", version: "0" },
    });
    expect(result.serverInfo).toBeDefined();
    expect(result.capabilities.tools).toBeDefined();
    expect(session).toBeTruthy();
    await rpc("notifications/initialized", {}, true);
  });

  it("tools/list has the core tools with schemas", async () => {
    const { tools } = await rpc("tools/list");
    const names: string[] = tools.map((t: { name: string }) => t.name);
    expect(names).toEqual(expect.arrayContaining(["tabs_open", "page_snapshot", "page_act", "nav_goto"]));
    const open = tools.find((t: { name: string }) => t.name === "tabs_open");
    expect(open.inputSchema.type).toBe("object");
  });

  it("tools/call runs a command", async () => {
    const opened = await rpc("tools/call", { name: "tabs_open", arguments: { url: fixture("list.html") } });
    expect(opened.isError).toBeFalsy();
    const text = opened.content.map((c: { text?: string }) => c.text ?? "").join("\n");
    const id = Number(/"id":\s*(\d+)/.exec(text)?.[1] ?? opened.structuredContent?.id);
    expect(id).toBeGreaterThan(0);
    const snap = await rpc("tools/call", { name: "page_snapshot", arguments: { tab: id } });
    expect(JSON.stringify(snap.content)).toContain("@");
    const bad = await rpc("tools/call", { name: "tabs_close", arguments: { tab: 999999 } });
    expect(bad.isError).toBe(true);
    await rpc("tools/call", { name: "tabs_close", arguments: { tab: id } });
  });
});
