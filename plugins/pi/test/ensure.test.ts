// The `ensure` tests, in a file of their own so that Vitest runs them
// alongside the extension's tests: one holds the start lock for 12 s.
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { binaryVersion, ensure, findBinary, SERVE_TIMEOUT_MS, WRAPPER } from "../src/daemon.ts";
import { buildClax, claxBin } from "./daemon-fixture.ts";
import { fakeExe } from "./fake-exe.ts";

let scratch: string;
/** A copy of the wrapper whose limit on `clax --version` (PROBE_SECS, 5 s)
 * is 120 s, as scripts/test-ensure-clax.sh makes its copies: the tests that
 * run the real binary are not about that limit, and on a loaded machine its
 * `--version` can take longer. A case about the limit would use the shipped
 * wrapper. */
let wrapper: string;
/** How long the tests let the real binary's `--version` take, for the same reason. */
const VERSION_MS = 120_000;

beforeAll(() => {
  buildClax();
  scratch = mkdtempSync(join(tmpdir(), "clax-pi-ensure-"));
  wrapper = join(scratch, "ensure-clax.sh");
  const text = readFileSync(WRAPPER, "utf8").replace(/^PROBE_SECS=\d+$/m, "PROBE_SECS=120");
  if (!/^PROBE_SECS=120$/m.test(text)) throw new Error(`${WRAPPER} sets no PROBE_SECS line; update this copy`);
  writeFileSync(wrapper, text);
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
      const info = await ensure(home, { env: withBin(claxBin), port: 0, wrapper });
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
      const info = await ensure(home, { env: withBin(claxBin), port: 0, wrapper });
      expect(Date.now() - t0).toBeGreaterThan(10_000);
      expect((await fetch(`http://127.0.0.1:${info.port}/healthz`)).ok).toBe(true);
    } finally {
      holder.kill();
      execFileSync(claxBin, ["stop"], { env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "" }, stdio: "ignore" });
    }
  }, 60_000);

  it("runs the binary the home's bin setting names, never a clax on PATH", async () => {
    const home = join(scratch, "setting");
    mkdirSync(home, { recursive: true });
    writeFileSync(join(home, "config.toml"), `bin = "${claxBin}"\n`);
    const onPath = join(scratch, "path-bin");
    mkdirSync(onPath, { recursive: true });
    fakeExe(join(onPath, "clax"), "#!/bin/sh\necho 'clax 0.0.1'\n");
    const env: NodeJS.ProcessEnv = { ...process.env, PATH: `${onPath}:${process.env.PATH}` };
    delete env.CLAX_BIN;
    expect(await findBinary(home, env, wrapper)).toBe(claxBin);
    expect(await findBinary(home, withBin(claxBin), wrapper)).toBe(claxBin);
  });

  it("rejects with the wrapper's reason when the binary named is unusable", async () => {
    const home = join(scratch, "bad-setting");
    mkdirSync(home, { recursive: true });
    writeFileSync(join(home, "config.toml"), 'bin = "/no/such/clax"\n');
    const env = { ...process.env };
    delete env.CLAX_BIN;
    await expect(findBinary(home, env)).rejects.toThrow(/sets bin = "\/no\/such\/clax", which is not a usable clax binary/);
    await expect(ensure(home, { env })).rejects.toThrow(/clax bin set/);
    await expect(findBinary(home, withBin("/no/such/clax"))).rejects.toThrow(/CLAX_BIN is set to '\/no\/such\/clax'/);
  });

  it("with no binary named, says how to get one, or that the pinned release could not be installed", async () => {
    // The package's wrapper is the one findBinary runs, so the reason follows
    // its pin. The release base is a closed port on 127.0.0.1: a pinned
    // wrapper's download fails at once and never reaches the network.
    const pin = /^PINNED_VERSION="(.*)"$/m.exec(readFileSync(WRAPPER, "utf8"))?.[1];
    expect(pin).toBeDefined();
    const env: NodeJS.ProcessEnv = { ...process.env, HOME: join(scratch, "nohome"), CLAX_RELEASE_BASE_URL: "http://127.0.0.1:9" };
    delete env.CLAX_BIN;
    const reason = pin
      ? new RegExp(`could not install clax ${pin.replaceAll(".", "\\.")}: .*http://127\\.0\\.0\\.1:9/`)
      : /pins no Clax release yet.*clax bin set/;
    await expect(ensure(join(scratch, "nobin"), { env })).rejects.toThrow(reason);
  });

  it("reads a binary's version only when it reports itself as clax", async () => {
    const other = join(scratch, "other-clax");
    fakeExe(other, "#!/bin/sh\necho 'other 1.0'\n");
    expect(await binaryVersion(other)).toBeNull();
    expect(await binaryVersion(join(scratch, "no-such-clax"))).toBeNull();
    const version = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;
    expect(await binaryVersion(claxBin, process.env, VERSION_MS)).toBe(version);
  });
});
