// Finding, and when needed starting, the Clax daemon for an Clax home.
import { execFile } from "node:child_process";
import { accessSync, constants, readFileSync, statSync } from "node:fs";
import { isIP, isIPv6 } from "node:net";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { probe, type Endpoint } from "./client.ts";

/** How to get a binary when none is found. Pi never downloads. */
export const INSTALL_HINT =
  "install clax with `just install` in a Clax checkout (it puts clax in ~/.cargo/bin), or with the release installer " +
  "(~/.local/bin), and start Pi from a shell whose PATH includes that directory; or set CLAX_BIN to a clax binary";

/** The longest one daemon replacement can hold the start lock, as
 * `crates/clax-cli/src/client.rs` bounds it: stopping the old daemon (a 2 s
 * shutdown request, 7 s for it to exit, then SIGTERM and 3 s more), starting
 * the new one (5 s for `/healthz`, then SIGTERM, 3 s, SIGKILL and 2 s more
 * when it is late) and the same again to roll back to the old executable;
 * 32 s, rounded up for the `/healthz` probes in between. */
const REPLACEMENT_MAX_MS = 35_000;

/** How long `clax serve --json` may take. It runs only when no daemon
 * answers, and it waits on the start lock (`daemon.lock`) as every Clax
 * client does. That lock may be held by another client's replacement of the
 * daemon, after which this binary may find an older daemon and replace it in
 * turn: two replacements, plus 5 s to spare. Killing it sooner would report a
 * failure while a daemon is about to answer. */
export const SERVE_TIMEOUT_MS = 2 * REPLACEMENT_MAX_MS + 5_000;

/** The contents of `<home>/daemon.json`. */
export interface DaemonInfo {
  port: number;
  pid: number;
  token: string;
  started_at: string;
  bind: string;
  version: string;
}

export interface DaemonOptions {
  /** Environment for locating (`CLAX_BIN`, `PATH`) and running the binary;
   * defaults to this process's. */
  env?: NodeJS.ProcessEnv;
  /** Port for a daemon this call starts (0 = any free port); the CLI's default
   * when absent. */
  port?: number;
}

/** `$CLAX_HOME`, else `$HOME/.clax`; an empty variable counts as unset. */
export function claxHome(env: NodeJS.ProcessEnv = process.env): string {
  if (env.CLAX_HOME) return env.CLAX_HOME;
  return join(env.HOME || homedir(), ".clax");
}

/** The daemon log in `home`, named when the daemon cannot be reached. */
export function logPath(home: string): string {
  return join(home, "logs", "daemon.log");
}

/** The host part of a URL that reaches a daemon bound to `bind`: an unspecified
 * address maps to the same-family loopback, a specific address is used as-is,
 * and IPv6 is bracketed. */
export function probeHost(bind: string): string {
  if (bind === "0.0.0.0") return "127.0.0.1";
  if (isIPv6(bind)) return bind === "::" ? "[::1]" : `[${bind}]`;
  return bind;
}

/** The host a browser on this machine should use: `localhost` for loopback
 * binds, otherwise the [`probeHost`] address. */
export function browserHost(bind: string): string {
  const loopback = (isIP(bind) === 4 && bind.startsWith("127.")) || bind === "::1";
  return loopback ? "localhost" : probeHost(bind);
}

/** The endpoint of the daemon `info` describes. */
export function endpointOf(info: DaemonInfo): Endpoint {
  return {
    base: `http://${probeHost(info.bind)}:${info.port}`,
    browserBase: `http://${browserHost(info.bind)}:${info.port}`,
    token: info.token,
  };
}

function pidAlive(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0 || pid > 0x7fffffff) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as NodeJS.ErrnoException).code === "EPERM";
  }
}

function readInfo(home: string): DaemonInfo | null {
  try {
    const info = JSON.parse(readFileSync(join(home, "daemon.json"), "utf8"));
    const ok = typeof info?.port === "number" && typeof info.pid === "number" && typeof info.token === "string" && typeof info.bind === "string";
    return ok ? (info as DaemonInfo) : null;
  } catch {
    return null;
  }
}

/** The live daemon named by `<home>/daemon.json` that answers `/healthz` within
 * 1 s, or null. Never starts one. */
export async function discover(home: string): Promise<DaemonInfo | null> {
  const info = readInfo(home);
  if (!info || !pidAlive(info.pid)) return null;
  return (await probe(`${endpointOf(info).base}/healthz`)) ? info : null;
}

function executable(path: string): boolean {
  try {
    accessSync(path, constants.X_OK);
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

/** The `clax` binary: `CLAX_BIN` when set (which must then be
 * executable), else the first `clax` on `PATH`. Throws naming the install
 * command when there is none. */
export function findBinary(env: NodeJS.ProcessEnv = process.env): string {
  if (env.CLAX_BIN) {
    if (executable(env.CLAX_BIN)) return env.CLAX_BIN;
    throw new Error(`CLAX_BIN is set to '${env.CLAX_BIN}', which is not an executable file; ${INSTALL_HINT}`);
  }
  for (const dir of (env.PATH ?? "").split(delimiter)) {
    if (!dir) continue;
    const candidate = join(dir, "clax");
    if (executable(candidate)) return candidate;
  }
  throw new Error(`the clax CLI was not found on PATH; ${INSTALL_HINT}`);
}

/** The version `bin --version` reports (`clax 0.3.0` gives `0.3.0`), or
 * null when it does not run, does not answer within 3 s, or is not clax. */
export function binaryVersion(bin: string, env: NodeJS.ProcessEnv = process.env): Promise<string | null> {
  return new Promise((resolve) => {
    execFile(bin, ["--version"], { env, timeout: 3_000 }, (err, stdout) => {
      const m = /^clax (\S+)/.exec(String(stdout).split("\n")[0] ?? "");
      resolve(!err && m ? m[1] : null);
    });
  });
}

/** The failed upgrade that keeps the daemon in `home` at an older version,
 * as `bin status --json` reports it (`upgrade_held`), or null when there is
 * none, or `bin` does not answer within 3 s. `clax status` never starts a
 * daemon. */
export function upgradeHeld(bin: string, home: string, env: NodeJS.ProcessEnv = process.env): Promise<Record<string, unknown> | null> {
  return new Promise((resolve) => {
    execFile(bin, ["status", "--json"], { env: { ...env, CLAX_HOME: home }, timeout: 3_000 }, (err, stdout) => {
      if (err) return resolve(null);
      try {
        const held = JSON.parse(String(stdout))?.upgrade_held;
        resolve(held && typeof held === "object" ? held : null);
      } catch {
        resolve(null);
      }
    });
  });
}

/** The running daemon for `home`, starting one with `clax serve --json`
 * (which returns once the daemon answers) when discovery finds none. */
export async function ensure(home: string, opts: DaemonOptions = {}): Promise<DaemonInfo> {
  const found = await discover(home);
  if (found) return found;
  const env = opts.env ?? process.env;
  const bin = findBinary(env);
  const args = ["serve", "--json", ...(opts.port === undefined ? [] : ["--port", String(opts.port)])];
  await new Promise<void>((resolve, reject) => {
    execFile(bin, args, { env: { ...env, CLAX_HOME: home }, timeout: SERVE_TIMEOUT_MS }, (err, _stdout, stderr) => {
      if (!err) return resolve();
      const late = err.killed ? `it did not finish within ${SERVE_TIMEOUT_MS / 1000} s and was stopped` : "";
      const detail = [String(stderr).trim(), late].filter(Boolean).join("; ") || err.message;
      reject(new Error(`\`${bin} serve\` failed: ${detail}; see ${logPath(home)}`));
    });
  });
  const started = await discover(home);
  if (!started) throw new Error(`the clax daemon did not answer after \`${bin} serve\`; see ${logPath(home)}`);
  return started;
}
