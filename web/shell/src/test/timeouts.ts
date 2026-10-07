// Time limits for unit tests whose work is heavy but bounded.

/** The limit for a test that loads and mounts a whole view (the artifact
 * view, its controller, the gallery) in a fresh module registry: it
 * evaluates the view's module graph again and renders it under jsdom, a few
 * hundred milliseconds of CPU on an idle machine and ten or more times that
 * on a loaded one. The limit is there to end a test that hangs, not to judge
 * its speed, so it sits well above what a loaded machine takes. */
export const MOUNT_TIMEOUT_MS = 30_000;

/** The limit for a test that runs a child Node (a Vite build, a build
 * script): bounded work that takes up to about a second on an idle machine
 * and many times that on a loaded one. Like [`MOUNT_TIMEOUT_MS`], it is
 * there to end a run that hangs, not to judge its speed. */
export const CHILD_TIMEOUT_MS = 60_000;

/** How long a test polls for a condition before it fails naming what it
 * waited for: just under [`MOUNT_TIMEOUT_MS`], so that on a loaded machine
 * a test fails only on a condition that never comes, and says which. Tests
 * that poll with it run under `MOUNT_TIMEOUT_MS`, so the poll gives up first. */
export const WAIT_MS = MOUNT_TIMEOUT_MS - 5_000;
