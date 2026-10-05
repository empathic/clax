// Stand-in executables for the tests (a fake `clax`, an opener), as
// crates/clax-fake-exe makes them for the Rust tests and scripts/fake-exe.sh
// for the script gates.
//
// macOS assesses every new executable file the first time it runs
// (syspolicyd, XProtect); on a loaded machine that first run can take
// seconds, longer than the limit the code under test puts on a program it
// starts, while later runs of the same file take milliseconds. The
// assessment belongs to the file: a new file with the same text is assessed
// again, a hard link to an assessed file is not. So each text is stored
// once, read-only, in the temporary directory under a hash of the text, run
// once with CLAX_FAKE_EXE_WARMUP set (a line added after the #! line makes
// that run exit at once), and the path asked for is a hard link to it.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, linkSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const warmed = new Set<string>();

function warm(path: string): void {
  execFileSync(path, [], { env: { ...process.env, CLAX_FAKE_EXE_WARMUP: "1" }, stdio: "ignore" });
}

/** Puts an executable script with `text` (a #! line naming a shell, then
 * its body) at `at`, replacing whatever is there, and returns `at`. The file
 * is read-only: to change a script, install another text at the same path. */
export function fakeExe(at: string, text: string): string {
  const nl = text.indexOf("\n");
  const first = nl < 0 ? text : text.slice(0, nl);
  if (!first.startsWith("#!")) throw new Error(`a fake executable starts with a #! line: ${first}`);
  const full = `${first}\n[ -z "\${CLAX_FAKE_EXE_WARMUP:-}" ] || exit 0\n${nl < 0 ? "" : text.slice(nl + 1)}`;
  const dir = join(tmpdir(), "clax-fake-exe");
  const shared = join(dir, `sha256-${createHash("sha256").update(full).digest("hex")}`);
  if (!warmed.has(shared)) {
    mkdirSync(dir, { recursive: true });
    if (existsSync(shared)) {
      // Assessed when it was stored; run again in case that was before a restart.
      warm(shared);
    } else {
      // Written, made read-only and run under a name of its own, then
      // renamed into place, so a concurrent test never sees a partial or
      // unassessed copy.
      const tmp = join(dir, `.${process.pid}.${Math.random().toString(36).slice(2)}`);
      writeFileSync(tmp, full);
      chmodSync(tmp, 0o555);
      warm(tmp);
      renameSync(tmp, shared);
    }
    warmed.add(shared);
  }
  mkdirSync(dirname(at), { recursive: true });
  rmSync(at, { force: true });
  try {
    linkSync(shared, at);
  } catch {
    // Another file system: a copy of its own, warmed here.
    copyFileSync(shared, at);
    warm(at);
  }
  return at;
}
