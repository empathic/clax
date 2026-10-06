// How a document says it has a live overlay (spec 2026-10-05 §11 "extension
// reloaded"). The overlay marks the isolated world's global with a check of
// its own liveness: the extension context it runs in is still there, and the
// worker that injected it is of the same load of the extension (the boot
// nonce the worker keeps in session storage, which a reload of the extension
// clears). An overlay left over from an earlier load of the extension, whose
// global Chromium may keep for the new load's scripts, is so not present:
// the worker injects again, and the new overlay stops the old one.

/** The global the overlay sets: its liveness check and its stop. */
export const RUNNING = "claxOverlayStarted";
/** The global the worker sets, before it injects the overlay, to its boot nonce. */
export const BOOT = "claxBoot";

export type Running = { alive(boot: unknown): boolean; stop(): void };

const running = (g: object): Running | null => {
  const r = (g as Record<string, unknown>)[RUNNING] as Partial<Running> | undefined;
  return r && typeof r.alive === "function" && typeof r.stop === "function" ? (r as Running) : null;
};

/** The world's overlay, unless it is gone or of another load of the extension. */
export function liveOverlay(g: object): Running | null {
  const r = running(g);
  try {
    return r && r.alive((g as Record<string, unknown>)[BOOT]) ? r : null;
  } catch {
    return null;
  }
}
export const overlayRunning = (g: object) => liveOverlay(g) !== null;

/** Stops an overlay the world still marks that is no longer live. */
export function stopStale(g: object): void {
  const r = running(g);
  if (!r || liveOverlay(g)) return;
  try { r.stop(); } catch { /* its context is gone */ }
  delete (g as Record<string, unknown>)[RUNNING];
}
