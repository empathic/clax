// Git capture against the cases the Rust capture is checked against
// (crates/clax-core/tests/toolpath/git-capture-vectors.json), and the
// version probe's retry rules.
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, unlinkSync, utimesSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { CAPTURE_DEADLINE_MS, capture, encodeHeader, findGit, GitProbe, PROBE_DEADLINE_MS, sanitizeRemote, validate, versionSupported, type GitContext, type GitField } from "../src/git.ts";
import { fakeExe } from "./fake-exe.ts";

const VECTORS = JSON.parse(readFileSync(new URL("../../../crates/clax-core/tests/toolpath/git-capture-vectors.json", import.meta.url), "utf8"));
const GIT = findGit(process.env.PATH)!;

const scratch = mkdtempSync(join(tmpdir(), "clax-pi-git-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

/** A fresh directory, its path resolved through symbolic links. */
function freshDir(): string {
  return realpathSync(mkdtempSync(join(scratch, "case-")));
}

/** Builds `c` of the shared vectors under `base`, as their `about` says. */
function build(c: any, base: string): void {
  for (const step of c.steps) {
    if (step.mkdir !== undefined) mkdirSync(join(base, step.mkdir), { recursive: true });
    else if (step.write !== undefined) writeFileSync(join(base, step.write), step.text, "utf8");
    else if (step.remove !== undefined) unlinkSync(join(base, step.remove));
    else if (step.git !== undefined) {
      const args = VECTORS.fixture.config.flatMap((kv: string) => ["-c", kv]);
      execFileSync(GIT, [...args, ...step.git], { cwd: join(base, step.in ?? ""), env: { ...process.env, ...VECTORS.fixture.env }, stdio: ["ignore", "ignore", "pipe"] });
    } else throw new Error(`unknown step ${JSON.stringify(step)}`);
  }
}

/** A capture in the vectors' form: no `captured_at`, `repo_root` relative
 * to `base`. */
function vectorForm(f: GitField, base: string): unknown {
  if ("capture" in f) return { git_capture: f.capture };
  const { captured_at: _, ...rest } = f.ok;
  return { ...rest, repo_root: relative(base, f.ok.repo_root) };
}

/** Generous, so a loaded machine never turns a case into a timeout. */
const soon = () => performance.now() + 20_000;

describe("git capture", () => {
  it("capture matches the Rust fixture cases", async () => {
    expect(GIT, "git on PATH").toBeTruthy();
    const wrong: string[] = [];
    for (const c of VECTORS.cases) {
      const base = freshDir();
      build(c, base);
      const got = vectorForm(await capture(GIT, join(base, c.cwd), soon()), base);
      try {
        expect(got).toEqual(c.expect);
      } catch {
        wrong.push(`${c.name}: ${JSON.stringify(got)}`);
      }
    }
    expect(wrong).toEqual([]);
  }, 60_000);

  it("a capture is a header the daemon accepts, without contents or names", async () => {
    const base = freshDir();
    build({ steps: VECTORS.cases.find((c: any) => c.name === "dirty_tracked").steps }, base);
    execFileSync(GIT, ["-C", join(base, "app"), "remote", "add", "origin", "https://ghp_TOKEN@github.com/o/r.git"]);
    writeFileSync(join(base, "app/secret-name.txt"), "SECRET-CONTENT");
    const f = await capture(GIT, join(base, "app"), soon());
    if (!("ok" in f)) throw new Error(JSON.stringify(f));
    expect(f.ok.captured_at).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
    const sent = Buffer.from(encodeHeader(f), "base64url").toString("utf8");
    for (const secret of ["SECRET-CONTENT", "secret-name", "a.txt", "ghp_TOKEN"]) expect(sent).not.toContain(secret);
    expect(JSON.parse(sent)).toEqual(f.ok);
    expect(encodeHeader({ capture: "no-cwd" })).toBe(Buffer.from('{"git_capture":"no-cwd"}').toString("base64url"));
    // A context the daemon would refuse is sent as `invalid`.
    const leaky: GitContext = { ...f.ok, remote_url: "https://u:tok@github.com/o/r.git" };
    expect(Buffer.from(encodeHeader({ ok: leaky }), "base64url").toString()).toBe('{"git_capture":"invalid"}');
    const big: GitContext = { ...f.ok, repo_root: `/${"x".repeat(2048)}` };
    expect(Buffer.from(encodeHeader({ ok: big }), "base64url").toString()).toBe('{"git_capture":"invalid"}');
  });

  it("strips credentials from remote URLs as the Rust capture does", () => {
    for (const [input, want] of [
      ["https://u:tok@github.com/o/r.git?x=1#f", "https://github.com/o/r.git"],
      ["https://ghp_token@github.com/o/r.git", "https://github.com/o/r.git"],
      ["http://host:8080/o/r#frag", "http://host:8080/o/r"],
      ["git@github.com:owner/repo.git", "git@github.com:owner/repo.git"],
      ["ssh://git@host/x", "ssh://git@host/x"],
      ["ssh://u:p@host/x", "ssh://host/x"],
      ["ssh://git@host:2222/x?q", "ssh://git@host:2222/x"],
      ["git+ssh://u:p@host/x", "git+ssh://host/x"],
      ["ssh://u:p@host", "ssh://host"],
      ["https://u:p@host?x", "https://host"],
      ["/srv/git/app.git", "/srv/git/app.git"],
      ["file:///srv/git/app.git", "file:///srv/git/app.git"],
      ["https://u:p#x@host/r", "https://host/r"],
      ["https://u:p/x@host/r", "https://host/r"],
      ["https://u:p@w@host/r", "https://host/r"],
      ["ssh://u:p#x@host/r", "ssh://host/r"],
      ["https://u:p@host/path@v1", "https://host/path@v1"],
    ]) expect(sanitizeRemote(input), input).toBe(want);
  });

  it("checks the shape the daemon checks", () => {
    const ok: GitContext = { repo_root: "/r", branch: "feat/x", head: "9c1e5d2b0a7f4e3c8d6b1a2f3e4d5c6b7a8f9e0d", dirty: false, untracked: 0, captured_at: "2026-10-06T14:03:11.512Z" };
    expect(validate(ok)).toBeUndefined();
    for (const bad of [
      { repo_root: "r" },
      { branch: "a..b" },
      { branch: "main‎" },
      { head: "ABC" },
      { untracked: 1 },
      { diff_sha256: `sha256:${"ab".repeat(32)}` },
      { diff_unavailable: true as const },
      { captured_at: "yesterday" },
    ]) expect(validate({ ...ok, ...bad }), JSON.stringify(bad)).toBeTypeOf("string");
    expect(versionSupported("git version 2.50.1 (Apple Git-155)")).toBe(true);
    expect(versionSupported("git version 2.44.0")).toBe(true);
    expect(versionSupported("git version 3.0.0")).toBe(true);
    expect(versionSupported("git version 2.43.5")).toBe(false);
    expect(versionSupported("not git")).toBe(false);
  });

  it("times out at the deadline, killing git", async () => {
    const dir = freshDir();
    const hung = fakeExe(join(dir, "git"), "#!/bin/sh\nexec tail -f /dev/null\n");
    const started = performance.now();
    expect(await capture(hung, dir, started + CAPTURE_DEADLINE_MS)).toEqual({ capture: "timeout" });
    expect(performance.now() - started).toBeLessThan(1_500);
  });

  it("does not capture a missing directory", async () => {
    expect(await capture(GIT, join(scratch, "gone"), soon())).toEqual({ capture: "no-cwd" });
    expect(await new GitProbe(GIT).capture(undefined, soon())).toEqual({ capture: "no-cwd" });
  });
});

describe("git version probe", () => {
  /** A git that answers `version` only once `answer` exists beside it, and
   * otherwise runs the real git; each `version` run adds a line to
   * `versions`. */
  function flakyGit(): { dir: string; git: string; runs: () => number } {
    const dir = freshDir();
    const git = fakeExe(join(dir, "git"), `#!/bin/sh\nhere="$(dirname "$0")"\nfor a in "$@"; do\n  if [ "$a" = version ]; then\n    echo run >> "$here/versions"\n    [ -f "$here/answer" ] || exit 1\n    echo 'git version 2.50.1'\n    exit 0\n  fi\ndone\nexec ${GIT} "$@"\n`);
    const runs = () => (existsSync(join(dir, "versions")) ? readFileSync(join(dir, "versions"), "utf8").trim().split("\n").length : 0);
    return { dir, git, runs };
  }

  function repo(): string {
    const base = freshDir();
    build(VECTORS.cases[0], base);
    return join(base, "app");
  }

  it("retries a probe that got no answer, with backoff, and keeps an answer", async () => {
    const root = repo();
    const { dir, git, runs } = flakyGit();
    let now = 0;
    const probe = new GitProbe(git, () => now);
    expect(await probe.capture(root, soon())).toEqual({ capture: "unavailable" });
    expect(runs()).toBe(1);
    // Within the wait, nothing runs.
    now += 29_000;
    expect(await probe.capture(root, soon())).toEqual({ capture: "unavailable" });
    expect(runs()).toBe(1);
    // After it, the probe runs again; a second miss doubles the wait.
    now += 1_000;
    expect(await probe.capture(root, soon())).toEqual({ capture: "unavailable" });
    expect(runs()).toBe(2);
    now += 59_000;
    expect(await probe.capture(root, soon())).toEqual({ capture: "unavailable" });
    expect(runs()).toBe(2);
    writeFileSync(join(dir, "answer"), "");
    now += 1_000;
    const got = await probe.capture(root, soon());
    expect("ok" in got && got.ok.repo_root).toBe(root);
    expect(runs()).toBe(3);
    // An answer is kept, even once git stops answering.
    unlinkSync(join(dir, "answer"));
    now += 10_000_000;
    expect("ok" in (await probe.capture(root, soon()))).toBe(true);
    expect(runs()).toBe(3);
  });

  it("keeps an answer naming an old git, which runs nothing", async () => {
    const dir = freshDir();
    const old = fakeExe(join(dir, "git"), `#!/bin/sh\necho run >> "$(dirname "$0")/versions"\necho 'git version 2.39.5'\n`);
    const probe = new GitProbe(old);
    for (let i = 0; i < 3; i++) expect(await probe.capture(dir, soon())).toEqual({ capture: "unavailable" });
    expect(readFileSync(join(dir, "versions"), "utf8").trim().split("\n")).toHaveLength(1);
  });

  it("counts the first capture's wait for the version against its deadline", async () => {
    const dir = freshDir();
    const hung = fakeExe(join(dir, "git"), "#!/bin/sh\nexec tail -f /dev/null\n");
    // A probe deadline past the capture's, but short, so the test does not
    // wait out the real one.
    const probeDeadline = CAPTURE_DEADLINE_MS * 2;
    expect(probeDeadline).toBeLessThan(PROBE_DEADLINE_MS);
    const probe = new GitProbe(hung, Date.now, probeDeadline);
    const started = performance.now();
    expect(await probe.capture(dir, started + CAPTURE_DEADLINE_MS)).toEqual({ capture: "timeout" });
    expect(performance.now() - started).toBeLessThan(probeDeadline);
    // The probe goes on to its own deadline, kills git, and gets no answer.
    expect(await probe.capture(dir, soon())).toEqual({ capture: "unavailable" });
  });
});

/** Runs a fixture-building git command in `dir` with the vectors' fixture
 * configuration; its trimmed output. */
function fixtureGit(dir: string, ...args: string[]): string {
  const cfg = VECTORS.fixture.config.flatMap((kv: string) => ["-c", kv]);
  return execFileSync(GIT, [...cfg, ...args], { cwd: dir, env: { ...process.env, ...VECTORS.fixture.env } }).toString().trim();
}

/** A repository `app` under a fresh directory, with one commit of a.txt. */
function committed(): { base: string; root: string } {
  const base = freshDir();
  build(VECTORS.cases[0], base);
  return { base, root: join(base, "app") };
}

/** A marker program: installed once (so macOS assesses it once), it
 * leaves a file named as it is called in `markers` beside the repository's
 * work tree, where git runs hooks and the fsmonitor. */
const MARKER = "#!/bin/sh\ntouch \"../markers/$(basename \"$0\")\"\n";

/** The names of the marker files in `dir`. */
const markers = (dir: string) => readdirSync(dir);

/** Makes the blob of a.txt at HEAD missing, as a partial clone's would be. */
function dropBlob(root: string): void {
  const blob = fixtureGit(root, "rev-parse", "HEAD:a.txt");
  unlinkSync(join(root, ".git/objects", blob.slice(0, 2), blob.slice(2)));
}

describe("git capture runs nothing the repository configures", () => {
  it("runs no filter, textconv, external diff, fsmonitor, pager or index hook", async () => {
    const { base, root } = committed();
    writeFileSync(join(root, "b.txt"), "same\n");
    fixtureGit(root, "add", "b.txt");
    fixtureGit(root, "commit", "-q", "-m", "b");
    const marks = join(base, "markers");
    mkdirSync(marks);
    const m = (name: string) => `sh -c 'touch ${join(marks, name)}; cat'`;
    writeFileSync(join(root, ".gitattributes"), "*.txt filter=x diff=x\n");
    for (const [k, v] of [
      ["filter.x.clean", m("clean")], ["filter.x.smudge", m("smudge")], ["filter.x.process", m("process")],
      ["filter.x.required", "true"], ["filter.with.dots.and=equals.clean", m("dotted")],
      ["diff.x.textconv", m("textconv")], ["diff.x.command", m("diffcmd")], ["diff.external", m("external")],
      ["core.fsmonitor", m("fsmonitor")], ["core.pager", m("pager")],
    ]) fixtureGit(root, "config", k, v);
    const hooks = join(root, ".git/hooks");
    mkdirSync(hooks, { recursive: true });
    for (const hook of ["post-index-change", "reference-transaction"]) fakeExe(join(hooks, hook), MARKER);
    // A stat-only change: a diff that refreshed the index would take
    // index.lock, rewrite the index and run post-index-change.
    utimesSync(join(root, "b.txt"), 1_000_000_000, 1_000_000_000);
    // A tracked change, read through the filter if one ran.
    writeFileSync(join(root, "a.txt"), "changed\n");
    const index = join(root, ".git/index");
    const state = () => [createHash("sha256").update(readFileSync(index)).digest("hex"), statSync(index).mtimeMs];
    const before = state();
    const got = await capture(GIT, root, soon());
    if (!("ok" in got)) throw new Error(JSON.stringify(got));
    expect(got.ok.dirty).toBe(true);
    expect(got.ok.diff_sha256).toMatch(/^sha256:/);
    expect(state()).toEqual(before);
    expect(existsSync(join(root, ".git/index.lock"))).toBe(false);
    expect(markers(marks)).toEqual([]);
    // The control: plain git runs them in this fixture.
    spawnSync(GIT, ["-C", root, "diff", "HEAD", "--stat"], { env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null" } });
    expect(markers(marks).length, "the fixture's programs never run, so the test proves nothing").toBeGreaterThan(0);
  });

  it("ignores an inherited GIT_DIR, GIT_CONFIG_* and GIT_CONFIG_PARAMETERS", async () => {
    const { base, root } = committed();
    const other = committed();
    const marks = join(base, "markers");
    mkdirSync(marks);
    const monitor = fakeExe(join(base, "fsmonitor"), MARKER);
    const env = {
      ...process.env,
      GIT_DIR: join(other.root, ".git"),
      GIT_CONFIG_COUNT: "1",
      GIT_CONFIG_KEY_0: "core.fsmonitor",
      GIT_CONFIG_VALUE_0: monitor,
      GIT_CONFIG_PARAMETERS: `'core.fsmonitor=${monitor}'`,
    };
    const got = await capture(GIT, root, soon(), env);
    if (!("ok" in got)) throw new Error(JSON.stringify(got));
    expect(got.ok.repo_root).toBe(root);
    expect(markers(marks)).toEqual([]);
    // The control: plain git in that environment runs the fsmonitor.
    spawnSync(GIT, ["-C", root, "status", "--porcelain"], { env });
    expect(markers(marks)).toEqual(["fsmonitor"]);
  });

  it("runs every command with the pins and the cleared environment", async () => {
    const { base, root } = committed();
    const log = join(base, "envs");
    const spy = fakeExe(join(base, "git"), `#!/bin/sh\n{ env | sort; echo ---; } >> "$(dirname "$0")/envs"\nexec ${GIT} "$@"\n`);
    expect("ok" in (await capture(spy, root, soon(), { ...process.env, GIT_DIR: "/nowhere", SECRET_VAR: "x" }))).toBe(true);
    const runs = readFileSync(log, "utf8").split("---\n").filter(r => r.trim());
    expect(runs.length).toBeGreaterThanOrEqual(5);
    const allowed = new Set(["PATH", "HOME", "XDG_CONFIG_HOME", "TMPDIR", "GIT_OPTIONAL_LOCKS", "GIT_TERMINAL_PROMPT", "LC_ALL", "GIT_PAGER", "GIT_NO_LAZY_FETCH", "GIT_CONFIG_COUNT", "PWD", "SHLVL", "_", "OLDPWD"]);
    for (const run of runs) {
      const vars = new Map(run.trim().split("\n").map(l => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)] as [string, string]));
      for (const k of vars.keys()) expect(allowed.has(k) || /^GIT_CONFIG_(KEY|VALUE)_\d+$/.test(k), k).toBe(true);
      expect(vars.get("GIT_NO_LAZY_FETCH")).toBe("1");
      const pins = new Map<string, string>();
      for (let i = 0; i < Number(vars.get("GIT_CONFIG_COUNT")); i++) pins.set(vars.get(`GIT_CONFIG_KEY_${i}`)!, vars.get(`GIT_CONFIG_VALUE_${i}`) ?? "");
      expect(Object.fromEntries(pins)).toMatchObject({
        "core.fsmonitor": "false",
        "core.hooksPath": "/dev/null",
        "diff.autoRefreshIndex": "false",
        "core.quotePath": "true",
        "diff.suppressBlankEmpty": "false",
      });
    }
  });

  it("never fetches in a partial clone, and says the diff is unavailable", async () => {
    const { base, root } = committed();
    const marks = join(base, "markers");
    mkdirSync(marks);
    // A remote that has the blob, so a fetch would succeed.
    fixtureGit(base, "clone", "-q", "--bare", "app", "remote.git");
    for (const [k, v] of [
      ["core.repositoryformatversion", "1"],
      ["extensions.partialClone", "origin"],
      ["remote.origin.url", join(base, "remote.git")],
      ["remote.origin.promisor", "true"],
      ["remote.origin.uploadpack", `sh -c 'touch ${join(marks, "fetch")}; exec git-upload-pack "$@"' --`],
    ]) fixtureGit(root, "config", k, v);
    dropBlob(root);
    writeFileSync(join(root, "a.txt"), "changed\n");
    const got = await capture(GIT, root, soon());
    if (!("ok" in got)) throw new Error(JSON.stringify(got));
    expect(markers(marks)).toEqual([]);
    expect([got.ok.dirty, got.ok.diff_unavailable, got.ok.diff_sha256]).toEqual([true, true, undefined]);
    // The control: plain git fetches the blob.
    spawnSync(GIT, ["-C", root, "diff", "HEAD"], { env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null" } });
    expect(markers(marks)).toEqual(["fetch"]);
  });

  it("knows a promisor remote alone makes a partial clone", async () => {
    const { root } = committed();
    fixtureGit(root, "config", "remote.origin.url", "/nonexistent/remote.git");
    fixtureGit(root, "config", "remote.origin.promisor", "true");
    dropBlob(root);
    writeFileSync(join(root, "a.txt"), "changed\n");
    const got = await capture(GIT, root, soon());
    if (!("ok" in got)) throw new Error(JSON.stringify(got));
    expect([got.ok.dirty, got.ok.diff_unavailable]).toEqual([true, true]);
  });

  it("does not look inside a submodule, but sees it moved", async () => {
    const { base, root } = committed();
    const sub = join(base, "sub");
    mkdirSync(sub);
    fixtureGit(sub, "init", "-q");
    writeFileSync(join(sub, "s.txt"), "s\n");
    fixtureGit(sub, "add", "s.txt");
    fixtureGit(sub, "commit", "-q", "-m", "s1");
    fixtureGit(root, "submodule", "add", "-q", sub, "sm");
    fixtureGit(root, "commit", "-q", "-m", "sm");
    const dirty = async () => {
      const got = await capture(GIT, root, soon());
      if (!("ok" in got)) throw new Error(JSON.stringify(got));
      return got.ok.dirty;
    };
    expect(await dirty()).toBe(false);
    writeFileSync(join(root, "sm/s.txt"), "edited\n");
    expect(await dirty()).toBe(false);
    fixtureGit(join(root, "sm"), "commit", "-q", "-am", "s2");
    expect(await dirty()).toBe(true);
  });
});
