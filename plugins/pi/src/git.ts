// The git state of the session's working directory (spec
// 2026-10-06-toolpath-audit-design §9), captured the way the MCP shim does
// (crates/clax-core/src/gitctx.rs) and sent in the `x-clax-git` header.
// crates/clax-core/tests/toolpath/git-capture-vectors.json holds cases both
// captures must report alike.
import { spawn, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import { statSync } from "node:fs";
import { isAbsolute, join } from "node:path";

/** How long a capture may run; past it, every git child is killed and the
 * outcome is `timeout`. */
export const CAPTURE_DEADLINE_MS = 300;
/** How long the version probe waits for `git version`. */
export const PROBE_DEADLINE_MS = 2_000;
/** The wait before probing again after a probe that got no answer; it
 * doubles with each miss, up to [`PROBE_RETRY_MAX_MS`]. */
export const PROBE_RETRY_FIRST_MS = 30_000;
export const PROBE_RETRY_MAX_MS = 600_000;
/** The largest header value sent, in bytes of the encoded value. */
export const MAX_HEADER_BYTES = 2048;
/** The most diff output hashed; past it the diff is `diff_truncated`. */
export const DIFF_CAP = 64 * 1024 * 1024;
/** The oldest git capture runs: 2.44, the first to honour
 * `GIT_NO_LAZY_FETCH` (and `GIT_CONFIG_COUNT`, which carries every pin,
 * came in 2.31). Under an older git nothing runs. */
export const MIN_GIT: [number, number] = [2, 44];

/** The flags of the diff hashed, after `diff HEAD` (or `diff --cached` on an
 * unborn branch): each pins what a user's configuration could change. */
export const DIFF_FLAGS = [
  "--binary",
  "--no-color",
  "--no-ext-diff",
  "--no-textconv",
  "--full-index",
  "--no-relative",
  "--src-prefix=a/",
  "--dst-prefix=b/",
  "--no-renames",
  "--diff-algorithm=myers",
  "--indent-heuristic",
  "--unified=3",
  "--inter-hunk-context=0",
  "-O/dev/null",
  "--ignore-submodules=dirty",
  "--no-color-moved",
];

/** The variables kept from this process's environment; every other is
 * cleared, so nothing (`GIT_DIR`, `GIT_CONFIG_*`, `GIT_EXEC_PATH`,
 * `GIT_TRACE*`, …) points git elsewhere or injects configuration. */
const KEPT_VARS = ["PATH", "HOME", "XDG_CONFIG_HOME", "TMPDIR"];

/** Configuration every command runs with: no fsmonitor, no hooks, no index
 * refresh (which would take `index.lock` and write the index), and the
 * diff settings no flag pins. */
const BASE_CONFIG: [string, string][] = [
  ["core.fsmonitor", "false"],
  ["core.hooksPath", "/dev/null"],
  ["diff.autoRefreshIndex", "false"],
  ["core.quotePath", "true"],
  ["diff.suppressBlankEmpty", "false"],
];

/** The most filter drivers neutralized; a configuration naming more is
 * `unavailable`. */
const MAX_FILTERS = 64;

/** A captured working-directory state: hashes and counts, never contents
 * or file names. */
export interface GitContext {
  repo_root: string;
  remote?: string;
  remote_url?: string;
  branch?: string;
  head?: string;
  dirty: boolean;
  diff_sha256?: string;
  diff_bytes?: number;
  diff_truncated?: true;
  diff_unavailable?: true;
  untracked: number;
  captured_at: string;
}

/** Why there is no context. */
export type Outcome = "not-a-repo" | "timeout" | "unavailable" | "no-cwd" | "invalid";

/** A capture's result: a context, or the outcome without one. */
export type GitField = { ok: GitContext } | { capture: Outcome };

class Failed extends Error {
  constructor(readonly outcome: Outcome) {
    super(outcome);
  }
}

// ---- Shape checks (the daemon's, spec §9.1) --------------------------------

/** Unicode format characters (category Cf), the bidi controls among them. */
const FORMAT_RANGES: [number, number][] = [
  [0x00ad, 0x00ad], [0x0600, 0x0605], [0x061c, 0x061c], [0x06dd, 0x06dd], [0x070f, 0x070f],
  [0x0890, 0x0891], [0x08e2, 0x08e2], [0x180e, 0x180e], [0x200b, 0x200f], [0x202a, 0x202e],
  [0x2060, 0x2064], [0x2066, 0x206f], [0xfeff, 0xfeff], [0xfff9, 0xfffb], [0x110bd, 0x110bd],
  [0x110cd, 0x110cd], [0x13430, 0x1343f], [0x1bca0, 0x1bca3], [0x1d173, 0x1d17a], [0xe0001, 0xe0001],
  [0xe0020, 0xe007f],
];

function isControl(cp: number): boolean {
  return cp <= 0x1f || (cp >= 0x7f && cp <= 0x9f);
}

function isFormat(cp: number): boolean {
  return FORMAT_RANGES.some(([a, b]) => cp >= a && cp <= b);
}

function codePoints(s: string): number[] {
  return [...s].map(c => c.codePointAt(0)!);
}

/** An error when `v` is empty or holds a control or format character. */
function text(name: string, v: string): string | undefined {
  if (v === "") return `${name} is empty`;
  if (codePoints(v).some(cp => isControl(cp) || isFormat(cp))) return `${name} holds a control or format character`;
  return undefined;
}

/** Whether `git check-ref-format --branch` would accept `b`. */
function isBranchName(b: string): boolean {
  return !(
    b === "" ||
    b === "@" ||
    b.startsWith("-") || b.startsWith("/") ||
    b.endsWith("/") || b.endsWith(".") ||
    b.includes("..") || b.includes("@{") || b.includes("//") ||
    codePoints(b).some(cp => isControl(cp) || " ~^:?*[\\".includes(String.fromCodePoint(cp))) ||
    b.split("/").some(part => part.startsWith(".") || part.endsWith(".lock"))
  );
}

const isLowerHex = (s: string) => /^[0-9a-f]*$/.test(s);
const isSha256Ref = (s: string) => /^sha256:[0-9a-f]{64}$/.test(s);
const RFC3339 = /^\d{4}-\d{2}-\d{2}[Tt ]\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/;

/** The reason `c` would be refused, or undefined when it has the shape the
 * daemon accepts. */
export function validate(c: GitContext): string | undefined {
  if (!isAbsolute(c.repo_root)) return "repo_root is not an absolute path";
  const t = text("repo_root", c.repo_root);
  if (t) return t;
  for (const [name, v] of [["remote", c.remote], ["remote_url", c.remote_url], ["branch", c.branch]] as const) {
    if (v !== undefined) {
      const e = text(name, v);
      if (e) return e;
    }
  }
  if (c.branch !== undefined && !isBranchName(c.branch)) return "branch is not a valid git branch name";
  if (c.remote_url !== undefined && sanitizeRemote(c.remote_url) !== c.remote_url) return "remote_url carries a credential, query or fragment";
  if (c.head !== undefined && !((c.head.length === 40 || c.head.length === 64) && isLowerHex(c.head))) return "head is not a 40- or 64-digit lowercase hex object name";
  if (c.diff_sha256 !== undefined) {
    if (!isSha256Ref(c.diff_sha256)) return "diff_sha256 is not sha256:<64 lowercase hex>";
    if (!c.dirty) return "diff_sha256 on a clean tree";
    if (c.diff_unavailable) return "diff_sha256 with diff_unavailable";
  } else if (c.diff_unavailable && !c.dirty) {
    return "diff_unavailable on a clean tree";
  } else if (c.diff_bytes !== undefined || c.diff_truncated) {
    return "diff_bytes or diff_truncated without diff_sha256";
  }
  if (!c.dirty && c.untracked > 0) return "untracked paths on a clean tree";
  if (!RFC3339.test(c.captured_at)) return "captured_at is not RFC 3339";
  return undefined;
}

/** A remote URL with any credential, query and fragment removed. For
 * `ssh`-family schemes a bare user (`ssh://git@host/x`) is kept and a
 * `user:password@` removed; for every other scheme all userinfo is removed.
 * Userinfo ends at the last `@` before the first `/` that follows any `@`.
 * The scp form `git@host:owner/repo.git` and local paths are kept. */
export function sanitizeRemote(url: string): string {
  const sep = url.indexOf("://");
  if (sep < 0) return url;
  const scheme = url.slice(0, sep);
  const rest = url.slice(sep + 3);
  let userinfo: string | undefined;
  let after = rest;
  const first = rest.indexOf("@");
  if (first >= 0) {
    const slash = rest.indexOf("/", first);
    const limit = slash < 0 ? rest.length : slash;
    const at = rest.lastIndexOf("@", limit - 1);
    userinfo = rest.slice(0, at);
    after = rest.slice(at + 1);
  }
  const hostEnd = after.search(/[/?#]/);
  const host = hostEnd < 0 ? after : after.slice(0, hostEnd);
  let path = hostEnd < 0 ? "" : after.slice(hostEnd);
  const q = path.search(/[?#]/);
  if (q >= 0) path = path.slice(0, q);
  const ssh = scheme.split("+").some(s => s.toLowerCase() === "ssh");
  if (userinfo !== undefined && ssh && !/[:/?#]/.test(userinfo)) return `${scheme}://${userinfo}@${host}${path}`;
  return `${scheme}://${host}${path}`;
}

/** The `x-clax-git` value for `field`: base64url JSON. A context that fails
 * [`validate`] or would pass [`MAX_HEADER_BYTES`] is sent as `invalid`. */
export function encodeHeader(field: GitField): string {
  const enc = (v: unknown) => Buffer.from(JSON.stringify(v), "utf8").toString("base64url");
  if ("capture" in field) return enc({ git_capture: field.capture });
  if (validate(field.ok) === undefined) {
    const h = enc(field.ok);
    if (h.length <= MAX_HEADER_BYTES) return h;
  }
  return enc({ git_capture: "invalid" });
}

// ---- Finding and probing git ------------------------------------------------

/** The executable named `git` on `path` (a `PATH` value), if any. */
export function findGit(path: string | undefined): string | undefined {
  for (const dir of (path ?? "").split(":")) {
    if (!isAbsolute(dir)) continue;
    const p = join(dir, "git");
    try {
      const st = statSync(p);
      if (st.isFile() && (st.mode & 0o111) !== 0) return p;
    } catch { /* not there */ }
  }
  return undefined;
}

/** Whether `git version` output `line` names [`MIN_GIT`] or later. */
export function versionSupported(line: string): boolean {
  if (!line.startsWith("git version ")) return false;
  const parts = line.slice("git version ".length).split(/[^0-9]/);
  const num = (s: string | undefined) => (s !== undefined && /^\d+$/.test(s) ? Number(s) : undefined);
  const major = num(parts[0]);
  const minor = num(parts[1]);
  if (major === undefined || minor === undefined) return false;
  return major > MIN_GIT[0] || (major === MIN_GIT[0] && minor >= MIN_GIT[1]);
}

/** A promise settled no later than `deadline` (`performance.now()` time):
 * `p`'s value, or `TIMEOUT`. */
const TIMEOUT = Symbol("timeout");
function within<T>(p: Promise<T>, deadline: number): Promise<T | typeof TIMEOUT> {
  let timer: NodeJS.Timeout | undefined;
  const late = new Promise<typeof TIMEOUT>(r => {
    timer = setTimeout(() => r(TIMEOUT), Math.max(0, deadline - performance.now()));
  });
  return Promise.race([p, late]).finally(() => clearTimeout(timer));
}

/**
 * One git executable whose version is read once per process. Only an answer
 * is kept: a probe that gets none within [`PROBE_DEADLINE_MS`] (a git that
 * hangs, or cannot be run) is tried again on a capture
 * [`PROBE_RETRY_FIRST_MS`] later, the wait doubling up to
 * [`PROBE_RETRY_MAX_MS`], and the captures in between are `unavailable`
 * without running anything. A capture waits for the probe only until its own
 * deadline: the first capture's wait for the version counts against it.
 */
export class GitProbe {
  private supported: boolean | undefined;
  private running: Promise<void> | undefined;
  private retryAt: number | undefined;
  private backoff = PROBE_RETRY_FIRST_MS;

  /** `now` is the clock of the retry waits (`Date.now` unless a test
   * moves time on by hand); `probeDeadlineMs` how long a probe waits for
   * `git version` (tests shorten it). */
  constructor(
    readonly path: string,
    private readonly now: () => number = Date.now,
    private readonly probeDeadlineMs = PROBE_DEADLINE_MS,
  ) {}

  /** Starts the probe unless there is an answer, one is running, or the
   * retry wait has not passed; the running probe, or undefined. */
  start(env: NodeJS.ProcessEnv = process.env): Promise<void> | undefined {
    if (this.supported !== undefined) return undefined;
    if (this.running) return this.running;
    if (this.retryAt !== undefined && this.now() < this.retryAt) return undefined;
    const runner = new Runner(this.path, "/", env);
    this.running = (async () => {
      let line: string | undefined;
      try {
        const out = await within(runner.output(["version"]), performance.now() + this.probeDeadlineMs);
        if (out !== TIMEOUT) line = out.line();
      } catch { /* no answer */ } finally {
        runner.cancel();
      }
      if (line !== undefined) {
        this.supported = versionSupported(line);
      } else {
        this.retryAt = this.now() + this.backoff;
        this.backoff = Math.min(this.backoff * 2, PROBE_RETRY_MAX_MS);
      }
      this.running = undefined;
    })();
    return this.running;
  }

  /** Captures `cwd` by [`capture`], by `deadline` (`performance.now()`
   * time), the wait for git's version included, in `env` (as [`capture`]). */
  async capture(cwd: string | undefined, deadline: number, env: NodeJS.ProcessEnv = process.env): Promise<GitField> {
    if (!cwd || !isDir(cwd)) return { capture: "no-cwd" };
    while (this.supported === undefined) {
      const p = this.start(env);
      if (!p) return { capture: "unavailable" };
      if ((await within(p, deadline)) === TIMEOUT) return { capture: "timeout" };
    }
    if (!this.supported) return { capture: "unavailable" };
    return capture(this.path, cwd, deadline, env);
  }
}

const probes = new Map<string, GitProbe>();

/** The process's probe of the git at `path`. */
export function probeFor(path: string): GitProbe {
  let p = probes.get(path);
  if (!p) probes.set(path, (p = new GitProbe(path)));
  return p;
}

/** The git found on each `PATH` value looked through, found once. */
const found = new Map<string, string | undefined>();

/** Captures `cwd` with the git on `env`'s `PATH` (none: `unavailable`),
 * in `env` (as [`capture`]), within `deadlineMs` from now. */
export function captureCwd(cwd: string | undefined, deadlineMs = CAPTURE_DEADLINE_MS, env: NodeJS.ProcessEnv = process.env): Promise<GitField> {
  const deadline = performance.now() + deadlineMs;
  if (!cwd || !isDir(cwd)) return Promise.resolve({ capture: "no-cwd" });
  const path = env.PATH ?? "";
  if (!found.has(path)) found.set(path, findGit(path));
  const git = found.get(path);
  if (!git) return Promise.resolve({ capture: "unavailable" });
  return probeFor(git).capture(cwd, deadline, env);
}

function isDir(p: string): boolean {
  try {
    return statSync(p).isDirectory();
  } catch {
    return false;
  }
}

// ---- Running git -------------------------------------------------------------

/** A finished command: whether it exited 0, and its standard output. */
class Out {
  constructor(readonly ok: boolean, readonly stdout: Buffer) {}

  /** The first line, when the command succeeded and printed one; `invalid`
   * when it is not UTF-8. */
  line(): string | undefined {
    if (!this.ok) return undefined;
    let s = utf8(this.stdout, "invalid");
    if (s.endsWith("\n")) s = s.slice(0, -1);
    if (s.endsWith("\r")) s = s.slice(0, -1);
    return s === "" ? undefined : s;
  }
}

/** `b` decoded as UTF-8, failing with `outcome` when it is not. */
function utf8(b: Buffer, outcome: Outcome): string {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(b);
  } catch {
    throw new Failed(outcome);
  }
}

interface Diff {
  sha256: string;
  bytes: number;
  truncated: boolean;
}

/** The children of a capture's runners, and whether it was cancelled. */
interface Children {
  running: Set<ChildProcess>;
  cancelled: boolean;
}

/** Runs git commands in one working directory, and kills them all on
 * [`Runner.cancel`]. */
class Runner {
  /** `env` is where the kept variables are read from. */
  constructor(
    readonly git: string,
    readonly cwd: string,
    readonly env: NodeJS.ProcessEnv,
    readonly config: [string, string][] = BASE_CONFIG,
    private readonly children: Children = { running: new Set(), cancelled: false },
  ) {}

  /** This runner with every filter driver in `drivers` turned off; its
   * children are this runner's, so [`Runner.cancel`] kills them too. */
  withoutFilters(drivers: string[]): Runner {
    const config = [...this.config];
    for (const d of drivers) {
      for (const [v, value] of [["clean", ""], ["smudge", ""], ["process", ""], ["required", "false"]]) config.push([`filter.${d}.${v}`, value]);
    }
    return new Runner(this.git, this.cwd, this.env, config, this.children);
  }

  /** Kills every child still running, and refuses to start more. */
  cancel(): void {
    this.children.cancelled = true;
    for (const c of this.children.running) c.kill("SIGKILL");
  }

  private spawn(args: string[]): ChildProcess {
    if (this.children.cancelled) throw new Failed("timeout");
    const env: Record<string, string> = {};
    for (const v of KEPT_VARS) {
      const value = this.env[v];
      if (value !== undefined) env[v] = value;
    }
    Object.assign(env, {
      GIT_OPTIONAL_LOCKS: "0",
      GIT_TERMINAL_PROMPT: "0",
      LC_ALL: "C",
      GIT_PAGER: "cat",
      // Never fetch a partial clone's missing objects (git 2.44+).
      GIT_NO_LAZY_FETCH: "1",
      GIT_CONFIG_COUNT: String(this.config.length),
    });
    this.config.forEach(([k, v], i) => {
      env[`GIT_CONFIG_KEY_${i}`] = k;
      env[`GIT_CONFIG_VALUE_${i}`] = v;
    });
    const child = spawn(this.git, ["-C", this.cwd, ...args], { env, stdio: ["ignore", "pipe", "ignore"] });
    const running = this.children.running;
    running.add(child);
    child.once("close", () => running.delete(child));
    return child;
  }

  /** Runs a command to completion, keeping its standard output. */
  output(args: string[]): Promise<Out> {
    return new Promise((resolve, reject) => {
      let child: ChildProcess;
      try {
        child = this.spawn(args);
      } catch (e) {
        return reject(e);
      }
      const chunks: Buffer[] = [];
      child.stdout!.on("data", (c: Buffer) => chunks.push(c));
      child.once("error", () => reject(new Failed("unavailable")));
      child.once("close", code => resolve(new Out(code === 0, Buffer.concat(chunks))));
    });
  }

  /** Runs a diff, hashing its output as it streams, up to [`DIFF_CAP`]. */
  diff(args: string[]): Promise<Diff> {
    return new Promise((resolve, reject) => {
      let child: ChildProcess;
      try {
        child = this.spawn(args);
      } catch (e) {
        return reject(e);
      }
      const hash = createHash("sha256");
      let bytes = 0;
      let full = false;
      let truncated = false;
      child.stdout!.on("data", (c: Buffer) => {
        if (truncated || c.length === 0) return;
        if (full) {
          truncated = true;
          child.kill("SIGKILL");
          return;
        }
        const take = Math.min(c.length, DIFF_CAP - bytes);
        hash.update(c.subarray(0, take));
        bytes += take;
        if (take < c.length) {
          truncated = true;
          child.kill("SIGKILL");
        } else if (bytes === DIFF_CAP) {
          full = true;
        }
      });
      child.once("error", () => reject(new Failed("unavailable")));
      child.once("close", code => {
        if (!truncated && code !== 0) return reject(new Failed("unavailable"));
        resolve({ sha256: `sha256:${hash.digest("hex")}`, bytes, truncated });
      });
    });
  }
}

// ---- The capture ------------------------------------------------------------

/** What the configuration says before anything else runs. */
interface Scan {
  drivers: string[];
  partialClone: boolean;
}

/** Reads the [`Scan`] with `git config`, which runs nothing. */
async function scan(r: Runner): Promise<Scan> {
  const out = await r.output(["config", "-z", "--get-regexp", "^(filter\\..*|extensions\\.partialclone|remote\\..*\\.promisor)$"]);
  const s: Scan = { drivers: [], partialClone: false };
  let start = 0;
  const b = out.stdout;
  while (start < b.length) {
    let end = b.indexOf(0, start);
    if (end < 0) end = b.length;
    const entry = b.subarray(start, end);
    start = end + 1;
    if (entry.length === 0) continue;
    // The key must be UTF-8; a value (a filter's command, say) is compared
    // as bytes and may be anything.
    const nl = entry.indexOf(0x0a);
    const key = utf8(nl < 0 ? entry : entry.subarray(0, nl), "unavailable");
    const value = nl < 0 ? Buffer.alloc(0) : entry.subarray(nl + 1);
    if (key.startsWith("filter.")) {
      const rest = key.slice("filter.".length);
      const dot = rest.lastIndexOf(".");
      if (dot < 0) continue;
      const driver = rest.slice(0, dot);
      if (!s.drivers.includes(driver)) s.drivers.push(driver);
    } else if (key === "extensions.partialclone") {
      s.partialClone ||= value.length > 0;
    } else if (key.endsWith(".promisor")) {
      const v = Buffer.from(value).toString("latin1").toLowerCase();
      s.partialClone ||= !["false", "no", "off", "0"].includes(v);
    }
  }
  if (s.drivers.length > MAX_FILTERS) throw new Failed("unavailable");
  return s;
}

/** The branch, then its upstream remote (else `origin`, else the first
 * remote) and that remote's URL, sanitized. */
async function remote(r: Runner): Promise<{ branch?: string; remote?: [string, string] }> {
  const branch = (await r.output(["symbolic-ref", "-q", "--short", "HEAD"])).line();
  let name: string | undefined;
  if (branch !== undefined) {
    const up = (await r.output(["config", "--get", `branch.${branch}.remote`])).line();
    if (up !== undefined && up !== ".") name = up;
  }
  if (name === undefined) {
    const list = await r.output(["remote"]);
    const names = utf8(list.stdout, "invalid").split("\n").map(l => (l.endsWith("\r") ? l.slice(0, -1) : l)).filter(l => l !== "");
    name = names.includes("origin") ? "origin" : names[0];
  }
  let rem: [string, string] | undefined;
  if (name !== undefined) {
    const url = (await r.output(["remote", "get-url", name])).line();
    if (url !== undefined) rem = [name, sanitizeRemote(url)];
  }
  return { branch, remote: rem };
}

/** HEAD, then the diff of staged and unstaged changes to tracked files
 * against it (against the empty tree on an unborn branch); in a partial
 * clone a diff that needs a missing object fails and is unavailable. */
async function head(r: Runner, partialClone: boolean): Promise<{ head?: string; diff?: Diff; unavailable: boolean }> {
  const h = (await r.output(["rev-parse", "-q", "--verify", "HEAD"])).line();
  try {
    return { head: h, diff: await r.diff(["diff", h !== undefined ? "HEAD" : "--cached", ...DIFF_FLAGS]), unavailable: false };
  } catch (e) {
    if (e instanceof Failed && e.outcome === "unavailable" && partialClone) return { head: h, unavailable: true };
    throw e;
  }
}

/** Whether `git status` prints anything, and how many untracked paths it
 * names. */
async function status(r: Runner): Promise<{ dirty: boolean; untracked: number }> {
  const out = await r.output(["status", "--porcelain=v1", "-z", "--untracked-files=normal", "--ignore-submodules=dirty", "--no-renames"]);
  if (!out.ok) throw new Failed("unavailable");
  let untracked = 0;
  let start = 0;
  const b = out.stdout;
  while (start < b.length) {
    let end = b.indexOf(0, start);
    if (end < 0) end = b.length;
    // `--no-renames`: every entry is one field.
    if (end > start && b[start] === 0x3f && b[start + 1] === 0x3f) untracked += 1;
    start = end + 1;
  }
  return { dirty: b.length > 0, untracked };
}

async function run(r: Runner): Promise<GitContext> {
  const [root, sc] = await Promise.all([
    r.output(["rev-parse", "--show-toplevel"]).then(o => {
      const l = o.line();
      if (l === undefined) throw new Failed("not-a-repo");
      return l;
    }),
    scan(r),
  ]);
  const f = r.withoutFilters(sc.drivers);
  const [rem, hd, st] = await Promise.all([remote(f), head(f, sc.partialClone), status(f)]);
  const diff = hd.diff && hd.diff.bytes > 0 ? hd.diff : undefined;
  const dirty = st.dirty || diff !== undefined;
  const c: GitContext = { repo_root: root, dirty, untracked: dirty ? st.untracked : 0, captured_at: new Date().toISOString() };
  if (rem.remote) {
    c.remote = rem.remote[0];
    c.remote_url = rem.remote[1];
  }
  if (rem.branch !== undefined) c.branch = rem.branch;
  if (hd.head !== undefined) c.head = hd.head;
  if (diff) {
    c.diff_sha256 = diff.sha256;
    c.diff_bytes = diff.bytes;
    if (diff.truncated) c.diff_truncated = true;
  }
  if (hd.unavailable && dirty) c.diff_unavailable = true;
  return c;
}

/**
 * Captures the git state of `cwd` with the (already probed) git at `git`,
 * by spec §9.1 and §9.2, finishing by `deadline` (`performance.now()` time),
 * with git's environment cleared down to `env`'s `PATH`, `HOME`,
 * `XDG_CONFIG_HOME` and `TMPDIR`.
 * It never fails: without a context it is the outcome `no-cwd`,
 * `unavailable`, `not-a-repo`, `timeout` (every git child still running is
 * killed) or `invalid`. No command runs a program the repository or the
 * user configured, writes the repository or fetches: the environment is
 * cleared, configuration is pinned through `GIT_CONFIG_COUNT`, every
 * configured filter driver is emptied, and diffs pass `--no-ext-diff
 * --no-textconv`.
 */
export async function capture(git: string, cwd: string, deadline: number, env: NodeJS.ProcessEnv = process.env): Promise<GitField> {
  if (!cwd || !isDir(cwd)) return { capture: "no-cwd" };
  const r = new Runner(git, cwd, env);
  try {
    const got = await within(run(r), deadline);
    if (got === TIMEOUT) return { capture: "timeout" };
    return validate(got) === undefined ? { ok: got } : { capture: "invalid" };
  } catch (e) {
    return { capture: e instanceof Failed ? e.outcome : "unavailable" };
  } finally {
    r.cancel();
  }
}
