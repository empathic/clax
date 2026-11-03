// The DevTools panel's link to the inspected page's app dev settings (the
// hook of web/sdk/src/devtools.ts): it evaluates the hook's expressions in
// the page through `chrome.devtools.inspectedWindow.eval` (no permission,
// no content script), checks every answer with the SDK's own validator, and
// reads again when the page's `rev` moves (polled only while the settings
// view is in sight), when the page navigates, and after each change.
import { type SetResult, type Snapshot, type Value, REV_EXPR, SNAPSHOT_EXPR, parseSetResult, parseSnapshot, resetExpr, setExpr } from "../../../sdk/src/devtools";

export type InspectEnv = {
  /** Evaluates `expr` in the inspected page's top document; its value, or null when it threw or could not run. */
  eval(expr: string): Promise<unknown>;
  /** Hears each navigation of the inspected page, until the returned function runs. */
  onNavigated(f: () => void): () => void;
};

export const chromeInspect = (): InspectEnv => ({
  eval: expr => new Promise(ok => {
    try { chrome.devtools.inspectedWindow.eval(expr, (r, e) => ok(e ? null : r)); } catch { ok(null); }
  }),
  onNavigated: f => {
    const l = () => f();
    chrome.devtools.network.onNavigated.addListener(l);
    return () => chrome.devtools.network.onNavigated.removeListener(l);
  },
});

/** How often the page's `rev` is read while the settings are in sight. */
export const POLL_MS = 1000;

export class SettingsLink {
  /** `loading` until the first read; `none` when the page has no hook (or answered something off). */
  status = $state<"loading" | "none" | "ready">("loading");
  snapshot = $state<Snapshot | null>(null);
  private rev: number | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;
  private reading: Promise<void> | null = null;
  private readonly off: () => void;

  constructor(private readonly env: InspectEnv) {
    this.off = env.onNavigated(() => { this.rev = null; void this.refresh(); });
  }

  /** Reads while `on` (the settings in sight), every POLL_MS; stops otherwise. */
  active(on: boolean): void {
    if (on && this.timer === null) {
      void this.refresh();
      this.timer = setInterval(() => void this.refresh(), POLL_MS);
    } else if (!on && this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  /** Reads the page's `rev`, then its snapshot when it moved (or `force`). */
  refresh(force = false): Promise<void> {
    if (this.reading && !force) return this.reading;
    const p = this.read(force).finally(() => { if (this.reading === p) this.reading = null; });
    this.reading = p;
    return p;
  }

  private async read(force: boolean): Promise<void> {
    const rev = await this.env.eval(REV_EXPR);
    if (typeof rev !== "number" || !Number.isSafeInteger(rev)) { this.rev = null; this.snapshot = null; this.status = "none"; return; }
    if (!force && rev === this.rev && this.status === "ready") return;
    const snap = parseSnapshot(await this.env.eval(SNAPSHOT_EXPR));
    this.rev = snap ? snap.rev : null;
    this.snapshot = snap;
    this.status = snap ? "ready" : "none";
  }

  set(section: string, field: string, value: Value): Promise<SetResult> { return this.change(setExpr(section, field, value)); }
  reset(section: string, field: string): Promise<SetResult> { return this.change(resetExpr(section, field)); }

  private async change(expr: string): Promise<SetResult> {
    const r = parseSetResult(await this.env.eval(expr)) ?? { ok: false as const, reason: "The page did not answer. Reload it and try again." };
    await this.refresh(true);
    return r;
  }

  close(): void { this.active(false); this.off(); }
}
