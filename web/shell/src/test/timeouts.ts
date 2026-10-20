// Time limits for unit tests whose work is heavy but bounded. Each is a
// guard that ends a test that hangs, never a judge of its speed: it sits
// well above what a loaded machine takes, so no outcome depends on it.

/** The limit for every unit test and hook (`testTimeout` and `hookTimeout`
 * in vitest.config.ts), named by the tests that load and mount a whole view
 * (the artifact view, its controller, the gallery) in a fresh module
 * registry: such a test evaluates the view's module graph again and renders
 * it under jsdom, a few hundred milliseconds of CPU on an idle machine and
 * ten or more times that on a loaded one. */
export const MOUNT_TIMEOUT_MS = 60_000;

/** The limit for a test that runs a child Node (a Vite build, a build
 * script): bounded work that takes up to about a second on an idle machine
 * and many times that on a loaded one. Like [`MOUNT_TIMEOUT_MS`], it is
 * there to end a run that hangs, not to judge its speed. */
export const CHILD_TIMEOUT_MS = 90_000;

/** How long a test polls for a condition before it fails naming what it
 * waited for, and how long `vi.waitFor` and `vi.waitUntil` wait unless told
 * otherwise (shell/test-setup.ts): under [`MOUNT_TIMEOUT_MS`], so that on a
 * loaded machine a test fails only on a condition that never comes, and
 * says which, before the test's own limit ends it. */
export const WAIT_MS = MOUNT_TIMEOUT_MS - 10_000;
