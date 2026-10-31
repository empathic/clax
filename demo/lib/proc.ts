// The child-process helpers the local runs share (demo/local.ts,
// demo/remote-e2e.ts, lib/build.ts, lib/worker.ts): waiting for a child to
// exit, and stopping one by its PID or its process group.

import type { ChildProcess } from "node:child_process";

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Resolves with `child`'s exit code (null when a signal ended it) once it has exited. */
export function exited(child: ChildProcess): Promise<number | null> {
  return new Promise((ok) => {
    if (child.exitCode !== null || child.signalCode !== null) ok(child.exitCode);
    else child.once("exit", (code) => ok(code));
  });
}

/** Polls `get` every 100 ms until it answers non-null, for at most `ms`. */
export async function until<T>(what: string, get: () => Promise<T | null | undefined>, ms = 30_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await get();
    if (v !== null && v !== undefined) return v;
    if (Date.now() > end) throw new Error(`${what}: not within ${ms / 1000} s`);
    await sleep(100);
  }
}

/** `signal`s `child` (or its process group), then SIGKILL after 10 s. */
async function stop(child: ChildProcess, signal: (sig: NodeJS.Signals) => void): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  signal("SIGTERM");
  if (await Promise.race([exited(child).then(() => true), sleep(10_000).then(() => false)])) return;
  signal("SIGKILL");
  await Promise.race([exited(child), sleep(10_000)]);
}

/** Stops `child` by its PID: SIGTERM, then SIGKILL after 10 s. */
export function stopChild(child: ChildProcess): Promise<void> {
  return stop(child, (sig) => child.kill(sig));
}

/** Stops a child started with `detached: true` by its process group:
 * SIGTERM, then SIGKILL after 10 s. */
export function stopGroup(child: ChildProcess): Promise<void> {
  const pid = child.pid;
  if (pid === undefined) return Promise.resolve();
  return stop(child, (sig) => {
    try {
      process.kill(-pid, sig);
    } catch {
      // already gone
    }
  });
}
