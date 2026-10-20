import { vi } from "vitest";

/** Starts a fresh module registry once every import in flight has landed.
 *
 * A lazy import a mounted view started (a chunk loaded after the first
 * paint) keeps evaluating after `vi.resetModules()`, and the modules it
 * reaches then are loaded into the new registry from the old graph. A
 * component mounted next that imports the same module, the Svelte runtime
 * among them, can then be handed its exports half-evaluated (`from_html is
 * not a function`). Waiting first, while the registry still tracks those
 * imports, leaves none to land after the reset. A test that resets the
 * registry part-way through, or in its own `afterEach`, calls this instead. */
export async function freshModules(): Promise<void> {
  await vi.dynamicImportSettled();
  vi.resetModules();
}
