// The commands behind Settings → Connections & API, Recording & scripts, Proxies against the real
// browser (started by the harness with --cdp-port and a temporary data folder).

import { describe, expect, it } from "vitest";

import { env, restUrl } from "../lib/env.js";

async function cmd<T = Record<string, any>>(name: string, args: object = {}): Promise<T> {
  const res = await fetch(`${restUrl()}/cmd/${name}`, { method: "POST", body: JSON.stringify(args) });
  const body = await res.json();
  if (!res.ok) throw new Error(`${name}: ${res.status} ${JSON.stringify(body)}`);
  return body as T;
}

describe("settings API", () => {
  it("api.info describes this browser", async () => {
    const info = await cmd("api.info");
    expect(info.listening).toBe(true);
    expect(info.port).toBe(env().apiPort);
    expect(info.urls.mcp).toBe(`http://127.0.0.1:${env().apiPort}/mcp`);
    expect(info.urls.openapi).toBe(`http://127.0.0.1:${env().apiPort}/openapi.json`);
    expect(info.endpoint_present).toBe(true);
    expect(info.exe.toLowerCase()).toMatch(/browsermcp(\.exe)?$/);
    expect(info.data_dir.toLowerCase()).toBe(env().dataDir.toLowerCase());
    const core = info.toolsets.find((t: { name: string }) => t.name === "core");
    expect(core).toMatchObject({ default: true });
    expect(core.commands).toBeGreaterThan(10);
    // The harness opened the CDP port on the command line; nothing is saved.
    expect(info.cdp).toMatchObject({ active_port: env().cdpPort, enabled: false, refused: false });
  });

  it("api.config saves the CDP setting without touching the running port", async () => {
    const saved = await cmd("api.config", { cdp_port: 9333 });
    expect(saved.config).toEqual({ cdp_enabled: false, cdp_port: 9333 });
    const info = await cmd("api.info");
    expect(info.cdp.active_port).toBe(env().cdpPort);
    await cmd("api.config", { cdp_port: 9222 });
  });

  it("record.settings and script.settings report their tools and folders", async () => {
    const rec = await cmd("record.settings");
    expect(rec.settings).toEqual(rec.defaults);
    expect(rec.ffmpeg.found).toBe(env().ffmpeg);
    expect(rec.videos_dir.toLowerCase()).toContain(env().dataDir.toLowerCase());
    const scr = await cmd("script.settings");
    expect(scr.host.found).toBe(env().scriptHost);
    expect(scr.recommended_parallel).toBeGreaterThanOrEqual(1);
    const changed = await cmd("script.settings", { run_retries: 2 });
    expect(changed.settings.run_retries).toBe(2);
    expect((await cmd("script.settings", { reset: true })).settings.run_retries).toBe(1);
  });

  it("proxy.rules_get lists routing with passwords masked", async () => {
    await cmd("proxy.rules_set", {
      profile: "e2e-routes",
      rules: [{ match: "*.example.com", via: "direct" }],
      default: "http://user:secret@127.0.0.1:9",
    });
    try {
      const got = await cmd("proxy.rules_get", { profile: "e2e-routes" });
      expect(JSON.stringify(got)).not.toContain("secret");
      expect(got.routes[0].default).toBe("http://user:***@127.0.0.1:9");
    } finally {
      await cmd("proxy.rules_set", { profile: "e2e-routes" });
    }
  });
});
