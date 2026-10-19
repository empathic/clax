// The device capabilities made through WebKit itself, on iOS (the app's
// `init` names `platform: "ios"`; caps/device-link.ts): `camera`, `file`,
// `share` and `location` use the web's own pickers, share sheet and
// geolocation in the shell's document, each behind the app's per-app grant
// (`grant`, asked first) and iOS's own prompt, so the app holds no native
// path to them. The shell's document is the top one, and the viewer's tap in
// the page's frame gives it the transient activation a picker and the share
// sheet need.
//
// - `camera.take` opens `<input type=file accept="image/*"
//   capture="environment">`, `camera.pick` and `file.pick` an `<input
//   type=file>` (`file.pick` with the page's `accept`); a photo is redrawn
//   as a JPEG at most `PHOTO_MAX_SIDE` on a side (caps/photo.ts).
// - `file.save`, `share.file` and `share.text` open the share sheet
//   (`navigator.share`), whose Save to Files saves.
// - `location.current` and `location.watch` read `navigator.geolocation`,
//   high accuracy only when the grant is precise. A fix the app would not
//   show precisely (an Approximate grant, or a fix no better than
//   `MIN_ACCURACY`: iOS's reduced accuracy) is snapped to its cell on the
//   app's grid (caps/coarsen.ts). A watch keeps the app's cadence: precise
//   updates at most once per `MIN_INTERVAL_MS` and per `minDistance`;
//   approximate ones report a new cell only after the device has been deep
//   inside it for `APPROX_HOLD_MS`, at most once per `APPROX_INTERVAL_MS`.
//
// Every path keeps the app's limits and codes: `deviceMaxBytes` (4 MiB) a
// file (`too_large`), `cancelled` for a closed picker, `not_granted` for a
// refused grant or a refused system permission, `unavailable` where WebKit
// has no way.
import { Coarsener, type Fix, MIN_ACCURACY } from "./coarsen";
import type { Grant, LinkedCap } from "./device-link";
import { CapError } from "./errors";
import { bytesOf, photoOf, toBase64 } from "./photo";

/** The largest file a page may pick, share or save (the app's
 * `deviceMaxBytes`). */
export const DEVICE_MAX_BYTES = 4 * 1024 * 1024;

/** The device capabilities WebKit serves on iOS; `notify` stays the
 * app's. */
export const WEBKIT_CAPS: ReadonlySet<string> = new Set(["share", "file", "camera", "location"]);

/** A watch's cadence, the app's `DeviceHost` rules. */
export const MIN_INTERVAL_MS = 1000;
export const APPROX_HOLD_MS = 3 * 60_000;
export const APPROX_INTERVAL_MS = 10 * 60_000;
export const MAX_WATCHES = 4;
const DEFAULT_DISTANCE = 10, MIN_DISTANCE = 5, MAX_DISTANCE = 10000;

type Ask = (cap: LinkedCap, method: string, args: unknown[]) => Promise<unknown>;

/** The app's grant for `cap`, asked as `cap.grant()`; rejects
 * `not_granted` when the person did not allow it. */
export async function granted(call: Ask, cap: LinkedCap): Promise<Grant> {
  const g = (await call(cap, "grant", [])) as Partial<Grant> | null;
  if (!g || g.allowed !== true) throw new CapError("not_granted", `the person did not allow ${cap} for this app`);
  return { allowed: true, precise: g.precise === true, shift: g.shift };
}

const cancelled = (what: string) => new CapError("cancelled", `no ${what} was picked`);

/** One file the person picks through WebKit's picker: `accept` and
 * `capture` as the input takes them. */
export function pickFile(accept: string, capture?: string): Promise<File> {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    if (accept) input.accept = accept;
    if (capture) input.setAttribute("capture", capture);
    // Out of sight but laid out: a picker opens from a rendered input.
    input.style.cssText = "position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;pointer-events:none";
    document.body.append(input);
    const done = () => input.remove();
    input.addEventListener("change", () => {
      const f = input.files?.[0];
      done();
      if (f) resolve(f);
      else reject(cancelled("file"));
    }, { once: true });
    input.addEventListener("cancel", () => { done(); reject(cancelled("file")); }, { once: true });
    input.click();
  });
}

const tooLarge = () => new CapError("too_large", "the file is over 4 MB");

/** A file `{name, mime, data}` (base64) of the page's, checked as the app
 * checks it. */
export function fileOf(f: unknown): File {
  const o = f as { name?: unknown; mime?: unknown; data?: unknown } | null;
  if (!o || typeof o !== "object" || typeof o.name !== "string" || typeof o.data !== "string") {
    throw new CapError("bad_request", "takes {name, mime, data} with data as base64");
  }
  const name = o.name.trim();
  if (name.length === 0 || name.length > 255 || name.includes("/") || name.includes("\\")) {
    throw new CapError("bad_request", "name is a file name of 1 to 255 characters");
  }
  if (o.data.length > Math.floor(DEVICE_MAX_BYTES * 4 / 3) + 4) throw tooLarge();
  let raw: string;
  try {
    raw = atob(o.data);
  } catch {
    throw new CapError("bad_request", "data is not base64");
  }
  const bytes = Uint8Array.from(raw, c => c.charCodeAt(0));
  if (bytes.length > DEVICE_MAX_BYTES) throw tooLarge();
  const mime = typeof o.mime === "string" && o.mime.includes("/") ? o.mime : "application/octet-stream";
  return new File([bytes], name, { type: mime });
}

/** `accept` of `file.pick({accept})` as the input's attribute:
 * extensions with their dot, MIME types as they are. */
export function acceptOf(opts: unknown): string {
  const accept = opts && typeof opts === "object" ? (opts as { accept?: unknown }).accept : undefined;
  if (accept === undefined || accept === null) return "";
  if (!Array.isArray(accept) || accept.some(a => typeof a !== "string")) {
    throw new CapError("bad_request", "accept is a list of extensions or MIME types");
  }
  return (accept as string[]).map(a => a.trim().toLowerCase()).filter(a => a.length > 0).map(a => (a.includes("/") || a.startsWith(".") ? a : `.${a}`)).join(",");
}

/** Opens the share sheet with `data`: whether the person shared (false
 * when they closed it). */
export async function share(data: ShareData): Promise<boolean> {
  const n = navigator as Navigator & { share?: unknown; canShare?: (d: ShareData) => boolean };
  if (typeof n.share !== "function" || (typeof n.canShare === "function" && !n.canShare(data))) {
    throw new CapError("unavailable", "this device cannot share that");
  }
  try {
    await navigator.share(data);
    return true;
  } catch (e) {
    const name = (e as { name?: unknown } | null)?.name;
    if (name === "AbortError") return false;
    if (name === "NotAllowedError") throw new CapError("no_gesture", "the share sheet needs a new tap: call it from a click handler");
    throw new CapError("unavailable", `could not share: ${String(e)}`);
  }
}

/** Makes `cap.method(...args)` through WebKit, the grant asked of the app
 * first (`location.watch` and `unwatch` are `Watches`'). */
export async function webkitCall(call: Ask, cap: LinkedCap, method: string, args: unknown[]): Promise<unknown> {
  const arg = args[0];
  switch (`${cap}.${method}`) {
    case "share.text": {
      if (typeof arg !== "string" || arg.length === 0) throw new CapError("bad_request", "text takes a non-empty string");
      await granted(call, cap);
      return { shared: await share({ text: arg }) };
    }
    case "share.file":
    case "file.save": {
      const f = fileOf(arg);
      await granted(call, cap);
      const ok = await share({ files: [f] });
      return cap === "share" ? { shared: ok } : { saved: ok };
    }
    case "file.pick": {
      const accept = acceptOf(arg);
      await granted(call, cap);
      const f = await pickFile(accept);
      if (f.size > DEVICE_MAX_BYTES) throw tooLarge();
      return { name: f.name, mime: f.type || "application/octet-stream", data: toBase64(await bytesOf(f)) };
    }
    case "camera.take":
    case "camera.pick": {
      await granted(call, cap);
      const f = await pickFile("image/*", method === "take" ? "environment" : undefined).catch((e: unknown) => {
        throw e instanceof CapError && e.code === "cancelled" ? new CapError("cancelled", "no photo was taken or picked") : e;
      });
      const p = await photoOf(f);
      if (p.bytes > DEVICE_MAX_BYTES) throw tooLarge();
      return { mime: p.mime, data: p.data, width: p.width, height: p.height };
    }
    case "location.current": {
      const g = await granted(call, cap);
      const shown = view(g);
      const pos = await new Promise<GeolocationPosition>((resolve, reject) => {
        navigator.geolocation.getCurrentPosition(resolve, e => reject(geoError(e)), { enableHighAccuracy: g.precise === true, maximumAge: 0 });
      });
      return shown(pos.coords);
    }
  }
  throw new CapError("capability_removed", `${cap}.${method} is not part of this runtime`);
}

/** How a fix reaches the page under grant `g`: as it is when precise and
 * the grant allows it, else snapped to its cell of the app's grid. */
function view(g: Grant): (c: { latitude: number; longitude: number; accuracy: number }) => Fix {
  const grid = gridOf(g);
  return c => {
    const fix = { lat: c.latitude, lon: c.longitude, accuracy: c.accuracy };
    if (g.precise === true && c.accuracy < MIN_ACCURACY) return { ...fix, precise: true };
    if (!grid) throw new CapError("unavailable", "the app sent no grid for approximate location");
    return grid.coarsen(fix);
  };
}

function gridOf(g: Grant): Coarsener | null {
  const s = g.shift;
  if (!Array.isArray(s) || s.length !== 2 || !s.every(x => typeof x === "number" && Number.isFinite(x) && x >= 0 && x < 1)) return null;
  return new Coarsener(s[0], s[1]);
}

/** A geolocation error as the contract's code. */
function geoError(e: GeolocationPositionError): CapError {
  if (e.code === 1) return new CapError("not_granted", "location is not allowed for Clax, or for this page, in iOS");
  return new CapError("unavailable", e.message || "the device could not find where it is");
}

/** Metres between two points (haversine). */
function metres(a: { lat: number; lon: number }, b: { lat: number; lon: number }): number {
  const r = (d: number) => d * Math.PI / 180;
  const h = Math.sin(r(b.lat - a.lat) / 2) ** 2 + Math.cos(r(a.lat)) * Math.cos(r(b.lat)) * Math.sin(r(b.lon - a.lon) / 2) ** 2;
  return 2 * 6371008.8 * Math.asin(Math.min(1, Math.sqrt(h)));
}

type Watch = {
  geo: number | null;
  stopped: boolean;
  grid: Coarsener | null;
  precise: boolean;
  distance: number;
  shown: ((c: GeolocationCoordinates) => Fix) | null;
  lastAt: number | null;
  last: Fix | null;
  trailing: Fix | null;
  timer: ReturnType<typeof setTimeout> | null;
  cell: string | null;
  pending: string | null;
  pendingSince: number;
  pendingFix: Fix | null;
};

/** One document's location watches through WebKit, under the page's IDs:
 * `update` sends a fix to the page, `end` the watch's end with its code. */
export class Watches {
  private readonly all = new Map<string, Watch>();

  constructor(
    private readonly call: Ask,
    private readonly update: (page: string, value: Fix) => void,
    private readonly end: (page: string, code: string, message: string) => void,
  ) {}

  /** Starts `location.watch({watch, minDistance?})`: `{watching: true}`
   * once WebKit follows the device. */
  async watch(arg: unknown): Promise<unknown> {
    const a = arg as { watch?: unknown; minDistance?: unknown } | undefined;
    const page = a?.watch;
    if (typeof page !== "string" || page.length === 0 || page.length > 64) throw new CapError("bad_request", "watch takes {watch, minDistance?} with watch an ID of 1 to 64 characters");
    const md = a?.minDistance;
    if (md !== undefined && md !== null && (typeof md !== "number" || !Number.isFinite(md))) throw new CapError("bad_request", "minDistance is a number of metres");
    if (this.all.has(page)) throw new CapError("bad_request", `watch ${page} is already running`);
    if (this.all.size >= MAX_WATCHES) throw new CapError("bad_request", `at most ${MAX_WATCHES} location watches at once`);
    const w: Watch = {
      geo: null, stopped: false, grid: null, precise: false,
      distance: Math.min(MAX_DISTANCE, Math.max(MIN_DISTANCE, typeof md === "number" ? md : DEFAULT_DISTANCE)),
      shown: null, lastAt: null, last: null, trailing: null, timer: null,
      cell: null, pending: null, pendingSince: 0, pendingFix: null,
    };
    this.all.set(page, w);
    try {
      const g = await granted(this.call, "location");
      if (w.stopped) throw new CapError("stopped", STOPPED);
      w.precise = g.precise === true;
      w.grid = gridOf(g);
      w.shown = view(g);
      w.geo = navigator.geolocation.watchPosition(p => this.fix(page, w, p.coords), e => this.fail(page, w, geoError(e)), { enableHighAccuracy: w.precise, maximumAge: 0 });
      return { watching: true };
    } catch (e) {
      if (this.all.get(page) === w) this.all.delete(page);
      throw e;
    }
  }

  /** The page stopped watch `page`: no end is sent. Whether it ran. */
  unwatch(page: unknown): { stopped: boolean } {
    const w = typeof page === "string" ? this.all.get(page) : undefined;
    if (!w) return { stopped: false };
    this.drop(page as string, w);
    return { stopped: true };
  }

  /** Ends every watch, telling the page `stopped` for those running (the
   * app stopped showing the view). Whether any did. */
  stopAll(tell: boolean): boolean {
    let any = false;
    for (const [page, w] of [...this.all]) {
      const running = w.geo !== null;
      this.drop(page, w);
      if (running && tell) {
        any = true;
        this.end(page, "stopped", STOPPED);
      }
    }
    return any;
  }

  private drop(page: string, w: Watch) {
    w.stopped = true;
    if (w.geo !== null) navigator.geolocation.clearWatch(w.geo);
    if (w.timer !== null) clearTimeout(w.timer);
    w.timer = null;
    if (this.all.get(page) === w) this.all.delete(page);
  }

  private fail(page: string, w: Watch, e: CapError) {
    if (w.stopped) return;
    this.drop(page, w);
    this.end(page, e.code, e.message);
  }

  private send(page: string, w: Watch, f: Fix) {
    w.lastAt = Date.now();
    w.last = f;
    this.update(page, f);
  }

  private later(w: Watch, ms: number, f: () => void) {
    if (w.timer !== null) clearTimeout(w.timer);
    w.timer = setTimeout(() => {
      w.timer = null;
      if (!w.stopped) f();
    }, Math.max(0, ms));
  }

  private fix(page: string, w: Watch, c: GeolocationCoordinates) {
    if (w.stopped || !w.shown) return;
    let shown: Fix;
    try {
      shown = w.shown(c);
    } catch (e) {
      this.fail(page, w, e as CapError);
      return;
    }
    if (shown.precise) this.precise(page, w, shown);
    else this.approximate(page, w, { lat: c.latitude, lon: c.longitude }, shown);
  }

  private precise(page: string, w: Watch, f: Fix) {
    if (w.last && w.last.precise && metres(w.last, f) < w.distance) return;
    const now = Date.now();
    if (w.lastAt === null || now - w.lastAt >= MIN_INTERVAL_MS) {
      w.trailing = null;
      if (w.timer !== null) clearTimeout(w.timer);
      w.timer = null;
      this.send(page, w, f);
      return;
    }
    const waiting = w.trailing !== null;
    w.trailing = f;
    if (waiting) return;
    this.later(w, w.lastAt + MIN_INTERVAL_MS - now, () => {
      const t = w.trailing;
      w.trailing = null;
      if (t) this.send(page, w, t);
    });
  }

  private approximate(page: string, w: Watch, at: { lat: number; lon: number }, shown: Fix) {
    const grid = w.grid;
    if (!grid) return;
    const c = grid.cell(at.lat, at.lon);
    const key = `${c.row},${c.col}`;
    if (w.cell === null) {
      w.cell = key;
      this.send(page, w, shown);
      return;
    }
    if (key === w.cell) {
      w.pending = null;
      if (w.timer !== null) clearTimeout(w.timer);
      w.timer = null;
      return;
    }
    if (!c.deep || key === w.pending) return;
    w.pending = key;
    w.pendingSince = Date.now();
    w.pendingFix = shown;
    this.approxDue(page, w);
  }

  private approxDue(page: string, w: Watch) {
    const fix = w.pendingFix;
    if (w.pending === null || !fix) return;
    let due = w.pendingSince + APPROX_HOLD_MS;
    if (w.lastAt !== null) due = Math.max(due, w.lastAt + APPROX_INTERVAL_MS);
    const now = Date.now();
    if (now >= due) {
      w.cell = w.pending;
      w.pending = null;
      this.send(page, w, fix);
      return;
    }
    this.later(w, due - now, () => this.approxDue(page, w));
  }
}

const STOPPED = "location stopped because the app left the screen; start it again from a tap";
