// Starting and stopping processes for the e2e suite: the browser under test and the fixture
// server. Only processes started here are ever stopped, by their own PID (never by image name).

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = resolve(fileURLToPath(new URL("../../../", import.meta.url)));
const EXE_SUFFIX = process.platform === "win32" ? ".exe" : "";

/** `target/debug` of this checkout (or `$CARGO_TARGET_DIR/debug`), unless `BMCP_EXE` is set. */
export function browserExe(): string {
  if (process.env.BMCP_EXE) return resolve(process.env.BMCP_EXE);
  const target = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(ROOT, "target");
  return join(target, "debug", `browsermcp${EXE_SUFFIX}`);
}

/** The V8 script host next to the browser (`cargo build -p bmcp-script`), if it was built. */
export function scriptHostExe(exe: string): string | null {
  const host = join(dirname(exe), `bmcp-script-host${EXE_SUFFIX}`);
  return existsSync(host) ? host : null;
}

export function hasFfmpeg(): boolean {
  const out = spawnSync("ffmpeg", ["-version"], { stdio: "ignore", windowsHide: true });
  return out.status === 0;
}

/** A port that was free a moment ago on 127.0.0.1. */
export function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : 0;
      server.close(() => resolvePort(port));
    });
  });
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** `POST /v1/cmd/<name>` on the browser's REST API; throws with the error body on failure. */
export async function rest<T = any>(port: number, name: string, args: unknown = {}): Promise<T> {
  const res = await fetch(`http://127.0.0.1:${port}/v1/cmd/${name}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args),
  });
  const body = await res.json();
  if (!res.ok) throw new Error(`${name} -> ${res.status} ${JSON.stringify(body)}`);
  return body as T;
}

export async function healthy(port: number): Promise<boolean> {
  try {
    const res = await fetch(`http://127.0.0.1:${port}/v1/health`, { signal: AbortSignal.timeout(2000) });
    return res.ok;
  } catch {
    return false;
  }
}

/** Has process `pid` exited? */
export function exited(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return false;
  } catch (err) {
    return (err as NodeJS.ErrnoException).code === "ESRCH";
  }
}

export async function waitExit(pid: number, ms: number): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (exited(pid)) return true;
    await sleep(200);
  }
  return exited(pid);
}

/** Terminate exactly this PID (Windows: TerminateProcess; CEF helpers exit with their parent). */
export function killPid(pid: number): void {
  try {
    process.kill(pid);
  } catch {
    // already gone
  }
}

export interface BrowserProcess {
  pid: number;
  apiPort: number;
  dataDir: string;
  log: string;
  child: ChildProcess;
  /** Resolves with the exit code once the process has exited. */
  exitCode: Promise<number | null>;
  /** `system.quit`, wait for the process to exit; kill it if it does not. True = it quit by itself. */
  stop(timeoutMs?: number): Promise<boolean>;
}

export interface StartOptions {
  exe: string;
  dataDir: string;
  apiPort: number;
  /** Extra arguments, e.g. `["--cdp-port", "9333"]`. `--headless` is always passed. */
  args?: string[];
  /** Seconds to wait for /v1/health (default 90). */
  timeoutSecs?: number;
}

export async function startBrowser(opts: StartOptions): Promise<BrowserProcess> {
  if (!existsSync(opts.exe)) {
    throw new Error(`${opts.exe} not found: build it first (cargo xtask build), or run cargo xtask e2e`);
  }
  if (await healthy(opts.apiPort)) throw new Error(`port ${opts.apiPort} already answers: another browser?`);
  mkdirSync(join(opts.dataDir, "logs"), { recursive: true });
  const log = join(opts.dataDir, "logs", "e2e-browser.log");
  const fd = openSync(log, "a");
  const child = spawn(
    opts.exe,
    [
      "--headless",
      "--api-port",
      String(opts.apiPort),
      "--data-dir",
      opts.dataDir,
      "--url",
      "about:blank",
      ...(opts.args ?? []),
    ],
    { stdio: ["ignore", fd, fd], windowsHide: true, env: { ...process.env, RUST_LOG: process.env.RUST_LOG ?? "info" } },
  );
  closeSync(fd);
  const pid = child.pid;
  if (pid === undefined) throw new Error(`cannot start ${opts.exe}`);
  const exitCode = new Promise<number | null>((resolveExit) => child.once("exit", (code) => resolveExit(code)));
  child.unref();

  const stop = async (timeoutMs = 20_000) => {
    if (exited(pid)) return true;
    try {
      await rest(opts.apiPort, "system.quit");
    } catch {
      // API already gone or unreachable: fall through to the kill
    }
    const quit = await waitExit(pid, timeoutMs);
    if (!quit) killPid(pid);
    return quit;
  };

  const deadline = Date.now() + (opts.timeoutSecs ?? 90) * 1000;
  while (!(await healthy(opts.apiPort))) {
    if (exited(pid) || Date.now() > deadline) {
      killPid(pid);
      throw new Error(`the browser did not come up; see ${log}${startupReport(child.exitCode, log)}`);
    }
    await sleep(300);
  }
  return { pid, apiPort: opts.apiPort, dataDir: opts.dataDir, log, child, exitCode, stop };
}

/** Exit code and the end of the log, so a CI failure shows its cause without the log file. */
function startupReport(code: number | null, log: string): string {
  const parts: string[] = [];
  if (code !== null) {
    // Windows NTSTATUS codes arrive as large unsigned numbers.
    const unsigned = code >>> 0;
    const hint = unsigned === 0xc0000135 ? " (a DLL is missing next to the exe: libcef.dll?)" : "";
    parts.push(`exit code ${code} (0x${unsigned.toString(16).toUpperCase()})${hint}`);
  }
  try {
    const text = readFileSync(log, "utf8").trimEnd();
    parts.push(text ? `last log lines:\n${text.split(/\r?\n/).slice(-40).join("\n")}` : "the log is empty");
  } catch {
    parts.push("no log was written");
  }
  return parts.map((p) => `\n${p}`).join("");
}

/**
 * Remove a data folder. CEF helper processes may hold files for a moment after the browser
 * exited, so retry for a while; give up with a warning (never fail a test over it).
 */
export async function removeDir(dir: string): Promise<void> {
  for (let i = 0; i < 40; i++) {
    try {
      rmSync(dir, { recursive: true, force: true });
      return;
    } catch {
      await sleep(250);
    }
  }
  console.warn(`[e2e] could not remove ${dir}`);
}

export interface FixtureServer {
  pid: number;
  /** http://localhost:<port> */
  base: string;
  /** Port of the second (cross-site) listener, as iframe.html's `?xport=`. */
  xport: number;
  stop(): void;
}

export function startFixtures(): Promise<FixtureServer> {
  const script = join(ROOT, "tests", "e2e", "fixtures", "server.mjs");
  const child = spawn(process.execPath, [script], { stdio: ["ignore", "pipe", "inherit"], windowsHide: true });
  return new Promise((resolveStart, reject) => {
    let out = "";
    const timer = setTimeout(() => reject(new Error("fixture server did not start")), 15_000);
    child.once("exit", (code) => reject(new Error(`fixture server exited (${code})`)));
    child.stdout!.on("data", (chunk: Buffer) => {
      out += chunk.toString();
      const line = out.split("\n")[0];
      if (!out.includes("\n")) return;
      clearTimeout(timer);
      const { port, xport } = JSON.parse(line) as { port: number; xport: number };
      child.unref();
      (child.stdout as any).unref?.();
      resolveStart({
        pid: child.pid!,
        base: `http://localhost:${port}`,
        xport,
        stop: () => killPid(child.pid!),
      });
    });
  });
}
