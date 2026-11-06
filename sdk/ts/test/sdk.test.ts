import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { BrowserError, connect, type BrowserClient } from "../src/index.js";
import { MockBrowser, TABS } from "./mock-server.js";

let mock: MockBrowser;
let browser: BrowserClient | undefined;

beforeEach(async () => {
  mock = await MockBrowser.start();
});

afterEach(async () => {
  browser?.close();
  browser = undefined;
  await mock.stop();
});

/** Wait until `check()` is true (polling; the mock answers asynchronously). */
async function until(check: () => boolean, ms = 2000): Promise<void> {
  const end = Date.now() + ms;
  while (!check()) {
    if (Date.now() > end) throw new Error("condition not met in time");
    await new Promise((r) => setTimeout(r, 10));
  }
}

async function caught(promise: Promise<unknown>): Promise<BrowserError> {
  try {
    await promise;
  } catch (err) {
    expect(err).toBeInstanceOf(BrowserError);
    return err as BrowserError;
  }
  throw new Error("expected the call to fail");
}

describe("WebSocket transport", () => {
  it("calls a command and returns its result", async () => {
    browser = await connect({ url: mock.wsUrl });
    const { tabs } = await browser.tabs.list();
    expect(tabs).toEqual(TABS);
    expect(mock.requests.at(-1)).toMatchObject({ method: "tabs.list", params: {} });

    const page = await browser.call("nav.goto", { url: "https://example.com/" });
    expect(page.url).toBe("https://example.com/");
  });

  it("turns a JSON-RPC error into a BrowserError", async () => {
    browser = await connect({ url: mock.wsUrl });
    // @ts-expect-error -- url is required; send it anyway to get INVALID_PARAMS back.
    const err = await caught(browser.nav.goto({}));
    expect(err.code).toBe("INVALID_PARAMS");
    expect(err.retry).toBe("no");
    expect(err.hint).toContain("nav_goto");
    expect(err.status).toBe(-32602);
    expect(err.message).toContain("missing field `url`");

    const unknown = await caught(browser.transport.call("tabs.lst", {}));
    expect(unknown.code).toBe("NOT_FOUND");
    expect(unknown.candidates).toEqual(["tabs.list", "tabs.open"]);
  });

  it("subscribes, delivers typed events and unsubscribes", async () => {
    browser = await connect({ url: mock.wsUrl });
    const seen: string[] = [];
    const off = browser.on("command.finished", (event, topic) => {
      seen.push(`${topic}:${event.name}:${event.ok}:${event.seq}`);
    });
    await browser.tabs.open({ url: "https://example.com/" });
    await until(() => seen.length === 1);
    expect(seen[0]).toMatch(/^command\.finished:tabs\.open:true:\d+$/);
    expect(mock.requests.some((r) => r.method === "events.subscribe")).toBe(true);

    // Patterns work too, and default topics (tab.*) need no subscribe call.
    const tabEvents: unknown[] = [];
    browser.on("tab.*", (data, topic) => tabEvents.push([topic, data]));
    mock.emit("tab.created", { tab: { id: 3 }, index: 2 });
    await until(() => tabEvents.length === 1);
    expect(tabEvents[0]).toEqual(["tab.created", { tab: { id: 3 }, index: 2, seq: expect.any(Number) }]);

    off();
    await until(() => mock.requests.some((r) => r.method === "events.unsubscribe"));
    await browser.tabs.open({ url: "https://example.com/" });
    await new Promise((r) => setTimeout(r, 50));
    expect(seen).toHaveLength(1);
  });

  it("times out a call that gets no answer", async () => {
    browser = await connect({ url: mock.wsUrl });
    const err = await caught(browser.transport.call("test.slow", {}, { timeout: 50 }));
    expect(err.code).toBe("CLIENT_TIMEOUT");
    expect(err.retry).toBe("after_wait");
    // The connection is still fine afterwards.
    expect((await browser.tabs.list()).tabs).toHaveLength(2);
  });

  it("fails pending calls when the connection drops, then reconnects and resubscribes", async () => {
    browser = await connect({ url: mock.wsUrl, minBackoff: 10 });
    const events: unknown[] = [];
    browser.on("command.*", (data) => events.push(data));

    const err = await caught(browser.transport.call("test.drop", {}));
    expect(err.code).toBe("CONNECTION_LOST");

    // The next call waits for the reconnect and then works.
    expect((await browser.tabs.list()).tabs).toHaveLength(2);
    expect(mock.connections).toBe(2);
    const subscribes = mock.requests.filter((r) => r.method === "events.subscribe");
    expect(subscribes.at(-1)?.params).toEqual({ topics: ["command.*"] });
    await browser.tabs.open({ url: "https://example.com/" });
    await until(() => events.length === 1);
  });

  it("keeps calls made while the browser is away until it is back", async () => {
    browser = await connect({ url: mock.wsUrl, minBackoff: 10, maxBackoff: 20 });
    const port = mock.port;
    await mock.stop();
    const call = browser.tabs.list({}, { timeout: 5_000 });
    await new Promise((r) => setTimeout(r, 150)); // several failed reconnect attempts
    mock = await MockBrowser.start(port);
    expect((await call).tabs).toHaveLength(2);
  });

  it("rejects connect() when nothing listens", async () => {
    const url = mock.wsUrl;
    await mock.stop();
    const err = await caught(connect({ url }));
    expect(err.code).toBe("CONNECTION_FAILED");
    mock = await MockBrowser.start(); // for afterEach
  });
});

describe("HTTP transport", () => {
  it("calls POST /v1/cmd/{name}", async () => {
    browser = await connect({ url: mock.httpUrl });
    expect((await browser.tabs.list()).tabs).toEqual(TABS);
    const page = await browser.nav.goto({ url: "https://example.com/" });
    expect(page.title).toBe("Example Domain");
  });

  it("turns an error body into a BrowserError with the HTTP status", async () => {
    browser = await connect({ url: mock.httpUrl });
    const err = await caught(browser.transport.call("nav.goto", {}));
    expect(err.code).toBe("INVALID_PARAMS");
    expect(err.status).toBe(400);
    expect(() => browser!.on("*", () => {})).toThrow(/WebSocket/);
  });
});
