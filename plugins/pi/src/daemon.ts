// Finding, and when needed starting, the Artifax daemon for an Artifax home.
import { execFile } from "node:child_process";
import { accessSync, constants, readFileSync, statSync } from "node:fs";
import { isIP, isIPv6 } from "node:net";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { probe, type Endpoint } from "./client.ts";

/** The command that installs the Artifax CLI, named when no binary is found. */
export const INSTALL_HINT =
  "install it with `cargo install --path crates/artifax-cli` from a clone of https://github.com/empathic/artifax " +
  "(or download a release from https://github.com/empathic/artifax/releases), or set ARTIFAX_BIN to its path";

/** How long `artifax serve --json` may take; it gives up on its own after
 * about 5 s when the daemon does not become ready. */
const SERVE_TIMEOUT_MS = 10_000;

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
  /** Environment for locating (`ARTIFAX_BIN`, `PATH`) and running the binary;
   * defaults to this process's. */
  env?: NodeJS.ProcessEnv;
  /** Port for a daemon this call starts (0 = any free port); the CLI's default
   * when absent. */
  port?: number;
}

/** `$ARTIFAX_HOME`, else `$HOME/.artifax`; an empty variable counts as unset. */
export function artifaxHome(env: NodeJS.ProcessEnv = process.env): string {
  if (env.ARTIFAX_HOME) return env.ARTIFAX_HOME;
  return join(env.HOME || homedir(), ".artifax");
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

/** The `artifax` binary: `ARTIFAX_BIN` when set (which must then be
 * executable), else the first `artifax` on `PATH`. Throws naming the install
 * command when there is none. */
export function findBinary(env: NodeJS.ProcessEnv = process.env): string {
  if (env.ARTIFAX_BIN) {
    if (executable(env.ARTIFAX_BIN)) return env.ARTIFAX_BIN;
    throw new Error(`ARTIFAX_BIN is set to '${env.ARTIFAX_BIN}', which is not an executable file; ${INSTALL_HINT}`);
  }
  for (const dir of (env.PATH ?? "").split(delimiter)) {
    if (!dir) continue;
    const candidate = join(dir, "artifax");
    if (executable(candidate)) return candidate;
  }
  throw new Error(`the artifax CLI was not found on PATH; ${INSTALL_HINT}`);
}

/** The running daemon for `home`, starting one with `artifax serve --json`
 * (which returns once the daemon answers) when discovery finds none. */
export async function ensure(home: string, opts: DaemonOptions = {}): Promise<DaemonInfo> {
  const found = await discover(home);
  if (found) return found;
  const env = opts.env ?? process.env;
  const bin = findBinary(env);
  const args = ["serve", "--json", ...(opts.port === undefined ? [] : ["--port", String(opts.port)])];
  await new Promise<void>((resolve, reject) => {
    execFile(bin, args, { env: { ...env, ARTIFAX_HOME: home }, timeout: SERVE_TIMEOUT_MS }, (err, _stdout, stderr) => {
      if (!err) return resolve();
      const detail = String(stderr).trim() || err.message;
      reject(new Error(`\`${bin} serve\` failed: ${detail}; see ${logPath(home)}`));
    });
  });
  const started = await discover(home);
  if (!started) throw new Error(`the artifax daemon did not answer after \`${bin} serve\`; see ${logPath(home)}`);
  return started;
}
