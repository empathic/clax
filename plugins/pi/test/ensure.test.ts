// The `ensure` tests, in a file of their own so that Vitest runs them
// alongside the extension's tests: one holds the start lock for 12 s.
import { execFileSync, spawn } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { binaryVersion, ensure, SERVE_TIMEOUT_MS } from "../src/daemon.ts";
import { buildClax, claxBin } from "./daemon-fixture.ts";

let scratch: string;

beforeAll(() => {
  buildClax();
  scratch = mkdtempSync(join(tmpdir(), "clax-pi-ensure-"));
}, 320_000);

afterAll(() => {
  if (scratch) rmSync(scratch, { recursive: true, force: true });
});

/** This process's environment with `CLAX_BIN` set to `bin` and Codex push
 * off (`CLAX_CODEX_BIN` empty) for any daemon it starts. */
function withBin(bin: string): NodeJS.ProcessEnv {
  return { ...process.env, CLAX_BIN: bin, CLAX_CODEX_BIN: "" };
}

describe("ensure", () => {
  it("starts a daemon with `clax serve --json` when none is running", async () => {
    const home = join(scratch, "fresh");
    try {
      const info = await ensure(home, { env: withBin(claxBin), port: 0 });
      expect(info.port).toBeGreaterThan(0);
      expect((await fetch(`http://127.0.0.1:${info.port}/healthz`)).ok).toBe(true);
    } finally {
      execFileSync(claxBin, ["stop"], { env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "" }, stdio: "ignore" });
    }
  }, 30_000);

  it("waits for `clax serve` while another client's daemon replacement holds the start lock", async () => {
    // A replacement holds daemon.lock for up to about 32 s; this one holds it
    // 12 s, longer than the 10 s ensure used to allow.
    expect(SERVE_TIMEOUT_MS).toBeGreaterThanOrEqual(2 * 32_000);
    const home = join(scratch, "locked");
    mkdirSync(home, { recursive: true });
    const holder = spawn("python3", ["-c",
      "import fcntl, sys, time; f = open(sys.argv[1], 'w'); fcntl.flock(f, fcntl.LOCK_EX); print('held', flush=True); time.sleep(12)",
      join(home, "daemon.lock")], { stdio: ["ignore", "pipe", "inherit"] });
    try {
      await new Promise<void>((resolve, reject) => {
        holder.stdout!.once("data", () => resolve());
        holder.once("exit", code => reject(new Error(`the lock holder exited with ${code}`)));
      });
      const t0 = Date.now();
      const info = await ensure(home, { env: withBin(claxBin), port: 0 });
      expect(Date.now() - t0).toBeGreaterThan(10_000);
      expect((await fetch(`http://127.0.0.1:${info.port}/healthz`)).ok).toBe(true);
    } finally {
      holder.kill();
      execFileSync(claxBin, ["stop"], { env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "" }, stdio: "ignore" });
    }
  }, 60_000);

  it("names the install command when no clax binary is found", async () => {
    const e = ensure(join(scratch, "nobin"), { env: { PATH: "" } });
    await expect(e).rejects.toThrow(/`just install` in a Clax checkout/);
    await expect(e).rejects.toThrow(/release installer/);
  });

  it("reads a binary's version only when it reports itself as clax", async () => {
    const other = join(scratch, "other-clax");
    writeFileSync(other, "#!/bin/sh\necho 'other 1.0'\n");
    chmodSync(other, 0o755);
    expect(await binaryVersion(other)).toBeNull();
    expect(await binaryVersion(join(scratch, "no-such-clax"))).toBeNull();
    const version = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;
    expect(await binaryVersion(claxBin)).toBe(version);
  });
});
