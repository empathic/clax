// Where npm_script.mjs may put its private copy of a project.
//
// The copy must go somewhere writable that Bazel deletes when the action
// ends, however it ends: Bazel stops an interrupted action with a signal
// nothing can catch, so the runner cannot be relied on to clean up. Which
// place that is differs by sandbox, and it is found by trying each in
// turn, not by asking which system this is:
//
//   1. TEST_TMPDIR: a test's own temporary directory, on every platform.
//   2. The action's sandbox directory. Writable under macOS's sandbox,
//      where the temporary directory is the machine's, shared by every
//      action. Read-only under Linux's, so there this is passed over.
//   3. The temporary directory. Under Linux's sandbox that is a mount of
//      the action's own, removed with it. Outside any sandbox it is the
//      machine's, and a copy left there by a killed process is what
//      scripts/bazel-gc.sh sweeps, by the marker the runner writes.
//
// The first place a directory can actually be made in wins.
import { mkdtempSync, statSync } from 'node:fs';
import { join } from 'node:path';

/** The sandbox directory `cwd` is in (…/sandbox/<kind>/<number>), or null. */
export function sandboxOf(cwd) {
  const found = /^(.*\/sandbox\/[^/]+\/\d+)(?:\/|$)/.exec(cwd);
  return found ? found[1] : null;
}

/** The places to try, in order. */
export function candidates({ env, cwd, systemTmp }) {
  const out = [];
  if (env.TEST_TMPDIR) out.push(env.TEST_TMPDIR);
  const sandbox = sandboxOf(cwd);
  if (sandbox) out.push(sandbox);
  out.push(systemTmp);
  return out;
}

/**
 * Makes the private directory in the first place that allows it and returns
 * its path. A place that is missing, not a directory or not writable
 * (EROFS, EACCES, EPERM) is passed over; if none will do, the last error is
 * thrown.
 */
export function makeScratch(places, prefix = 'npm-script-') {
  let last = new Error('no place to put a private copy of the project');
  for (const dir of places) {
    try {
      if (!statSync(dir).isDirectory()) continue;
      return mkdtempSync(join(dir, prefix));
    } catch (error) {
      last = error;
    }
  }
  throw last;
}
