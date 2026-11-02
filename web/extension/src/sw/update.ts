// Following the build on disk. `clax extension install` rewrites the
// unpacked extension in place (a deployer runs it after each new daemon),
// but Chrome keeps running what it loaded. Chrome serves the extension's
// own URLs from the files on disk, while `chrome.runtime.getManifest()` is
// the manifest it loaded, so the worker compares the build stamp of each
// (`version_name`, scripts/extension-manifest.mjs) and reloads the
// extension when they differ, at a quiet moment: no pick open (a composer
// and its text), no request waiting for its answer, no side panel holding
// unsent text (QuietGate). The files must look whole first (the manifest
// parses, its service worker is there): `install` swaps the directory in two
// renames, and reloading an unpacked extension whose files are broken leaves
// it unloaded until someone loads it again by hand. Chrome also disables an
// unpacked extension that reloads while chrome://extensions has Developer
// mode off (seen in Chromium 153: disable reason 1 << 24); Load unpacked
// needs it on, and it must stay on for these reloads.

/** How often a running worker looks at the files on disk. */
export const CHECK_MS = 60_000;
/** A reload for a build asked within this long is not asked again for the
 * same build: one whose reload did not take is not reloaded in a loop. */
export const RELOAD_GUARD_MS = 10 * 60_000;
const GUARD_KEY = "reloadedForBuild";

/** Something whose being busy holds a reload back, and that says when it is not. */
export type Source = { busy(): boolean; whenIdle(fn: () => void): void };

/** Runs one function at a moment none of `sources` is busy, checking every
 * source again at that moment (one going idle says nothing of the others). */
export class QuietGate {
  private waiting: (() => void) | null = null;
  private readonly armed = new Set<Source>();
  constructor(private readonly sources: readonly Source[]) {}

  /** Whether no source is busy now. */
  quiet(): boolean { return this.sources.every(s => !s.busy()); }

  /** Runs `fn` at the first quiet moment (at once when it is quiet now).
   * One function waits at a time: a later one replaces it. */
  when(fn: () => void): void {
    this.waiting = fn;
    this.poke();
  }

  private poke(): void {
    if (!this.waiting) return;
    const busy = this.sources.filter(s => s.busy());
    if (!busy.length) {
      const fn = this.waiting;
      this.waiting = null;
      fn();
      return;
    }
    for (const s of busy) {
      if (this.armed.has(s)) continue;
      this.armed.add(s);
      s.whenIdle(() => { this.armed.delete(s); this.poke(); });
    }
  }
}

type Area = { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
export type UpdateEnv = {
  /** The manifest Chrome loaded (`chrome.runtime.getManifest()`). */
  loaded: { version: string; version_name?: string };
  /** One of the extension's own files as it is on disk now; null when it cannot be read. */
  read(path: string): Promise<string | null>;
  local: Area;
  gate: QuietGate;
  /** `chrome.runtime.reload()`. */
  reload(): void;
  now(): number;
};

/** A manifest's build: its stamp, else (a build from before stamps) its version. */
export const buildOf = (m: { version: string; version_name?: string }) => m.version_name ?? m.version;

export class Updater {
  /** The build on disk, when it is whole, not the one running, and not one
   * a reload was asked for within RELOAD_GUARD_MS. */
  newer: string | null = null;
  private checking: Promise<string | null> | null = null;
  constructor(private readonly env: UpdateEnv) {}

  /** Looks at the files on disk; when they are another, whole build, asks
   * for a reload at the next quiet moment. Resolves to that build (`newer`), else null. */
  check(): Promise<string | null> {
    this.checking ??= this.look().finally(() => { this.checking = null; });
    return this.checking;
  }

  private async look(): Promise<string | null> {
    const disk = await this.onDisk();
    const build = disk !== null && disk !== buildOf(this.env.loaded) ? disk : null;
    this.newer = null;
    if (build === null) return null;
    const g = (await this.env.local.get(GUARD_KEY))[GUARD_KEY] as { build?: unknown; at?: unknown } | undefined;
    if (g?.build === build && typeof g.at === "number" && this.env.now() - g.at < RELOAD_GUARD_MS) return null;
    this.newer = build;
    this.env.gate.when(() => void this.reloadFor(build));
    return build;
  }

  private async reloadFor(build: string): Promise<void> {
    await this.env.local.set({ [GUARD_KEY]: { build, at: this.env.now() } });
    // Checked again after the write: something may have opened meanwhile.
    this.env.gate.when(() => this.env.reload());
  }

  /** The build on disk, when its manifest parses and names a service worker that is there. */
  private async onDisk(): Promise<string | null> {
    const text = await this.env.read("manifest.json");
    if (text === null) return null;
    let m: unknown;
    try { m = JSON.parse(text); } catch { return null; }
    if (typeof m !== "object" || m === null) return null;
    const { version, version_name, manifest_version, background } = m as Record<string, unknown>;
    if (manifest_version !== 3 || typeof version !== "string" || (version_name !== undefined && typeof version_name !== "string")) return null;
    const sw = (background as { service_worker?: unknown } | undefined)?.service_worker;
    if (typeof sw !== "string" || !(await this.env.read(sw))) return null;
    return buildOf({ version, version_name: version_name as string | undefined });
  }
}

/** The text at `url` (one of the extension's own, as on disk), through
 * `fetchFn`; null on any failure. */
export async function readOwn(url: string, fetchFn: typeof fetch = (...a) => fetch(...a)): Promise<string | null> {
  try {
    const r = await fetchFn(url, { cache: "no-store" });
    return r.ok ? await r.text() : null;
  } catch {
    return null;
  }
}
