import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { type DeviceCall, deviceLink } from "./device-link";
import { Coarsener } from "./coarsen";
import { CapError } from "./errors";
import { CapabilityHost, type CapEnv } from "./host";
import { hostArrival } from "./gesture";
import { REGISTRY } from "./registry";

/** Whether the viewer's gesture in the page holds, as `frameGesture` says. */
let gesture = true;
vi.mock("./gesture", async orig => ({ ...(await orig<typeof import("./gesture")>()), frameGesture: () => gesture }));

function env(declared: CapEnv["declared"]) {
  const posted: ShellToBridge[] = [];
  const e: CapEnv = {
    aid: "7q3k9mzx2b4t", version: 1, pinned: false, token: "t",
    viewer: async () => ({ publicId: "u_00000000000000000000aa", name: "Alex" }),
    declared,
    prompt: vi.fn(async () => "allow" as const),
    post: m => posted.push(m),
    reload: vi.fn(),
  };
  return { e, posted };
}

afterEach(() => { deviceLink.call = null; gesture = true; });
/** Links a fake app: each call is kept, with its answer's resolve and reject. */
function link() {
  const sent: (Omit<DeviceCall, "id"> & { resolve(v: unknown): void; reject(e: unknown): void })[] = [];
  deviceLink.call = (cap, method, args) => new Promise((resolve, reject) => { sent.push({ cap, method, args, resolve, reject }); });
  return sent;
}

describe("the device capabilities", () => {
  it("resolve null outside the Clax app, even when declared", async () => {
    expect(deviceLink.call).toBeNull();
    const { e, posted } = env({ notify: {}, camera: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    await host.handle({ type: "clax:use", id: "u1", name: "notify" });
    await host.handle({ type: "clax:call", id: "c1", ns: "camera", method: "take", args: [] });
    expect(posted[0]).toMatchObject({ granted: false, config: null });
    expect(posted[1]).toMatchObject({ ok: false, error: { code: "not_granted" } });
  });

  it("in the app, are granted when declared and forward each call with a request ID", async () => {
    const sent = link();
    const { e, posted } = env({ notify: {}, share: {}, file: {}, camera: {}, location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    for (const name of ["notify", "share", "file", "camera", "location"]) await host.handle({ type: "clax:use", id: name, name });
    expect(posted.map(m => (m as { granted: boolean }).granted)).toEqual([true, true, true, true, true]);
    const at = { id: "water", title: "Water", body: "Drink", at: "2026-10-13T09:00:00Z", route: "#/plants" };
    const done = host.handle({ type: "clax:call", id: "c1", ns: "notify", method: "schedule", args: [at] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ cap: "notify", method: "schedule", args: [at] });
    sent[0].resolve({ id: "water" });
    await done;
    expect(posted.at(-1)).toEqual({ type: "clax:call-result", id: "c1", ok: true, value: { id: "water" } });
  });

  it("is not granted when undeclared, in the app too", async () => {
    link();
    const { e, posted } = env({ db: {} });
    await new CapabilityHost(Promise.resolve(e), REGISTRY, null).handle({ type: "clax:use", id: "u", name: "location" });
    expect(posted[0]).toMatchObject({ granted: false });
  });

  it("passes the app's coded errors to the page", async () => {
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const done = host.handle({ type: "clax:call", id: "c1", ns: "location", method: "current", args: [] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    sent[0].reject(new CapError("not_granted", "the person did not allow location"));
    await done;
    expect(posted.at(-1)).toEqual({ type: "clax:call-result", id: "c1", ok: false, error: { code: "not_granted", message: "the person did not allow location" } });
  });

  it("refuses an unknown method and arguments that are not plain data, asking the app nothing", async () => {
    const sent = link();
    const { e, posted } = env({ share: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    await host.handle({ type: "clax:call", id: "c1", ns: "share", method: "url", args: [] });
    await host.handle({ type: "clax:call", id: "c2", ns: "share", method: "text", args: [new Blob(["x"])] });
    expect(posted.map(m => (m as { error: { code: string } }).error.code)).toEqual(["capability_removed", "bad_request"]);
    expect(sent).toEqual([]);
  });

  it("refuses a call that acts without the viewer's gesture, but lists and cancels reminders without one", async () => {
    const sent = link();
    gesture = false;
    const { e, posted } = env({ notify: {}, camera: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    await host.handle({ type: "clax:call", id: "c1", ns: "camera", method: "take", args: [] });
    await host.handle({ type: "clax:call", id: "c2", ns: "notify", method: "schedule", args: [{ id: "a" }] });
    expect(posted.map(m => (m as { error?: { code: string } }).error?.code)).toEqual(["no_gesture", "no_gesture"]);
    const list = host.handle({ type: "clax:call", id: "c3", ns: "notify", method: "list", args: [] });
    const cancel = host.handle({ type: "clax:call", id: "c4", ns: "notify", method: "cancel", args: ["a"] });
    await vi.waitFor(() => expect(sent.map(c => c.method)).toEqual(["list", "cancel"]));
    sent[0].resolve([]);
    sent[1].resolve({ cancelled: false });
    await Promise.all([list, cancel]);
  });

  it("location.watch gives the app an ID of its own, passes its news to the page under the page's ID, and unwatch needs no gesture", async () => {
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const started = host.handle({ type: "clax:call", id: "c1", ns: "location", method: "watch", args: [{ watch: "w1", minDistance: 20 }] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    const appId = (sent[0].args[0] as { watch: string }).watch;
    expect(appId).not.toBe("w1");
    expect(sent[0].args[0]).toEqual({ watch: appId, minDistance: 20 });
    sent[0].resolve({ watching: true });
    await started;
    for (const f of deviceLink.events) f({ watch: appId, kind: "update", value: { lat: 1, lon: 2, accuracy: 5, precise: true } });
    for (const f of deviceLink.events) f({ watch: "someone-else", kind: "update", value: {} });
    for (const f of deviceLink.events) { f(null); f({ watch: 7, kind: "update" }); f({ watch: appId, kind: "other" }); }
    expect(posted.at(-1)).toEqual({ type: "clax:event", ns: "location", topic: "update", data: { watch: "w1", value: { lat: 1, lon: 2, accuracy: 5, precise: true } } });
    gesture = false;
    const stop = host.handle({ type: "clax:call", id: "c2", ns: "location", method: "unwatch", args: [{ watch: "w1" }] });
    await vi.waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1]).toMatchObject({ method: "unwatch", args: [{ watch: appId }] });
    sent[1].resolve({ stopped: true });
    await stop;
    // Gone: its news no longer reaches the page, and starting one needs a tap.
    const before = posted.length;
    for (const f of deviceLink.events) f({ watch: appId, kind: "update", value: {} });
    await host.handle({ type: "clax:call", id: "c3", ns: "location", method: "watch", args: [{ watch: "w2" }] });
    expect(posted.slice(before)).toEqual([{ type: "clax:call-result", id: "c3", ok: false, error: { code: "no_gesture", message: expect.any(String) } }]);
    host.dispose();
  });

  it("an ended watch tells the page once; the document leaving stops the rest", async () => {
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    for (const [i, w] of ["a", "b"].entries()) {
      hostArrival.presses++;
      const p = host.handle({ type: "clax:call", id: w, ns: "location", method: "watch", args: [{ watch: w }] });
      await vi.waitFor(() => expect(sent).toHaveLength(i + 1));
      sent.at(-1)!.resolve({ watching: true });
      await p;
    }
    const [idA, idB] = sent.map(c => (c.args[0] as { watch: string }).watch);
    for (const f of deviceLink.events) f({ watch: idA, kind: "end", error: { code: "stopped", message: "left the screen" } });
    for (const f of deviceLink.events) f({ watch: idA, kind: "end", error: { code: "stopped", message: "again" } });
    expect(posted.filter(m => m.type === "clax:event")).toEqual([{ type: "clax:event", ns: "location", topic: "end", data: { watch: "a", code: "stopped", message: "left the screen" } }]);
    const listeners = deviceLink.events.size;
    host.dispose();
    expect(sent.slice(2)).toEqual([expect.objectContaining({ method: "unwatch", args: [{ watch: idB }] })]);
    expect(deviceLink.events.size).toBe(listeners - 1);
  });

  it("a location call spends the tap: one press starts one, and after a watch ends a new press is needed", async () => {
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const codes = () => posted.filter(m => m.type === "clax:call-result" && !m.ok).map(m => (m as { error: { code: string } }).error.code);
    hostArrival.presses++;
    const started = host.handle({ type: "clax:call", id: "c1", ns: "location", method: "watch", args: [{ watch: "w1" }] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    // The same tap: neither a second watch nor a one-shot call.
    await host.handle({ type: "clax:call", id: "c2", ns: "location", method: "watch", args: [{ watch: "w2" }] });
    await host.handle({ type: "clax:call", id: "c3", ns: "location", method: "current", args: [] });
    expect(codes()).toEqual(["no_gesture", "no_gesture"]);
    sent[0].resolve({ watching: true });
    await started;
    // The person taps the page again, then stops the watch with the pill.
    hostArrival.presses++;
    const appId = (sent[0].args[0] as { watch: string }).watch;
    for (const f of deviceLink.events) f({ watch: appId, kind: "end", error: { code: "stopped", message: "stopped" } });
    await host.handle({ type: "clax:call", id: "c4", ns: "location", method: "watch", args: [{ watch: "w3" }] });
    expect(codes()).toEqual(["no_gesture", "no_gesture", "no_gesture"]);
    // A tap after the stop starts it again.
    hostArrival.presses++;
    const again = host.handle({ type: "clax:call", id: "c5", ns: "location", method: "watch", args: [{ watch: "w4" }] });
    await vi.waitFor(() => expect(sent).toHaveLength(2));
    sent[1].resolve({ watching: true });
    await again;
    host.dispose();
  });
});

// On iOS the shell makes the calls through WebKit: the app is asked only
// for the grant.
vi.mock("./photo", async orig => ({
  ...(await orig<typeof import("./photo")>()),
  photoOf: vi.fn(async (f: Blob) => ({ mime: "image/jpeg", data: "anBn", width: 1600, height: 1200, bytes: f.size })),
}));

describe("the device capabilities on iOS", () => {
  /** What the next picker the page opens does: a file picked, or closed. */
  let nextPick: File | "cancel" = new File(["hi"], "a.csv", { type: "text/csv" });
  const opened: { accept: string; capture: string | null; inDocument: boolean }[] = [];
  const shared: ShareData[] = [];
  let shareError: Error | null = null;
  type Geo = { ok: PositionCallback; err?: PositionErrorCallback | null; opts?: PositionOptions };
  const geoCalls: Geo[] = [];
  const watching = new Map<number, Geo>();
  let geoSeq = 0;

  beforeEach(() => {
    deviceLink.platform = "ios";
    nextPick = new File(["hi"], "a.csv", { type: "text/csv" });
    opened.length = 0;
    shared.length = 0;
    shareError = null;
    geoCalls.length = 0;
    watching.clear();
    vi.spyOn(HTMLInputElement.prototype, "click").mockImplementation(function (this: HTMLInputElement) {
      opened.push({ accept: this.accept, capture: this.getAttribute("capture"), inDocument: this.isConnected });
      const pick = nextPick;
      queueMicrotask(() => {
        if (pick === "cancel") this.dispatchEvent(new Event("cancel"));
        else {
          Object.defineProperty(this, "files", { value: [pick], configurable: true });
          this.dispatchEvent(new Event("change"));
        }
      });
    });
    Object.defineProperty(navigator, "share", { configurable: true, value: vi.fn(async (d: ShareData) => { shared.push(d); if (shareError) throw shareError; }) });
    Object.defineProperty(navigator, "canShare", { configurable: true, value: () => true });
    Object.defineProperty(navigator, "geolocation", {
      configurable: true,
      value: {
        getCurrentPosition: (ok: PositionCallback, err?: PositionErrorCallback | null, opts?: PositionOptions) => { geoCalls.push({ ok, err, opts }); },
        watchPosition: (ok: PositionCallback, err?: PositionErrorCallback | null, opts?: PositionOptions) => { const id = ++geoSeq; const g = { ok, err, opts }; geoCalls.push(g); watching.set(id, g); return id; },
        clearWatch: (id: number) => { watching.delete(id); },
      },
    });
  });
  afterEach(() => {
    deviceLink.platform = null;
    vi.restoreAllMocks();
    vi.useRealTimers();
    for (const k of ["share", "canShare", "geolocation"]) delete (navigator as unknown as Record<string, unknown>)[k];
  });

  /** A fake app that answers every grant with `grant`, and fails anything
   * else (the test then sees it in `sent`). */
  function app(grant: unknown = { allowed: true }) {
    const sent = link();
    const answer = () => {
      for (const c of sent) if (c.method === "grant" && !(c as { done?: boolean }).done) {
        (c as { done?: boolean }).done = true;
        if (grant instanceof CapError) c.reject(grant);
        else c.resolve(grant);
      }
    };
    const timer = setInterval(answer, 1);
    onTestFinished(() => clearInterval(timer));
    return sent;
  }
  const position = (lat: number, lon: number, accuracy: number) => ({ coords: { latitude: lat, longitude: lon, accuracy } }) as GeolocationPosition;

  async function run(host: CapabilityHost, posted: ShellToBridge[], ns: string, method: string, args: unknown[] = [], before?: () => void) {
    const id = `c${posted.length}-${Math.random()}`;
    hostArrival.presses++;
    const p = host.handle({ type: "clax:call", id, ns, method, args });
    before?.();
    await p;
    return posted.find(m => m.type === "clax:call-result" && m.id === id) as { ok: boolean; value?: unknown; error?: { code: string } };
  }

  it("asks the app for each capability's grant and never for the call itself", async () => {
    const sent = app({ allowed: true, precise: true, shift: [0.3, 0.7] });
    const { e, posted } = env({ share: {}, file: {}, camera: {}, location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const data = btoa("hello");

    nextPick = new File(["\xff\xd8"], "IMG_1.jpg", { type: "image/jpeg" });
    expect(await run(host, posted, "camera", "take")).toMatchObject({ ok: true, value: { mime: "image/jpeg", data: "anBn", width: 1600, height: 1200 } });
    expect(await run(host, posted, "camera", "pick")).toMatchObject({ ok: true });
    nextPick = new File(["a,b\n"], "t.csv", { type: "text/csv" });
    expect(await run(host, posted, "file", "pick", [{ accept: ["csv", "image/*", ".json"] }])).toMatchObject({ ok: true, value: { name: "t.csv", mime: "text/csv", data: btoa("a,b\n") } });
    expect(opened).toEqual([
      { accept: "image/*", capture: "environment", inDocument: true },
      { accept: "image/*", capture: null, inDocument: true },
      { accept: ".csv,image/*,.json", capture: null, inDocument: true },
    ]);
    expect(document.querySelectorAll("input[type=file]")).toHaveLength(0);

    expect(await run(host, posted, "file", "save", [{ name: "n.txt", mime: "text/plain", data }])).toMatchObject({ ok: true, value: { saved: true } });
    expect(await run(host, posted, "share", "file", [{ name: "n.txt", mime: "text/plain", data }])).toMatchObject({ ok: true, value: { shared: true } });
    expect(await run(host, posted, "share", "text", ["hi there"])).toMatchObject({ ok: true, value: { shared: true } });
    expect(shared.map(d => d.text ?? d.files!.map(f => [f.name, f.type, f.size]))).toEqual([[["n.txt", "text/plain", 5]], [["n.txt", "text/plain", 5]], "hi there"]);

    const here = run(host, posted, "location", "current", [], () => {});
    await vi.waitFor(() => expect(geoCalls).toHaveLength(1));
    expect(geoCalls[0].opts).toMatchObject({ enableHighAccuracy: true });
    geoCalls[0].ok(position(52.5, 13.4, 12));
    expect(await here).toMatchObject({ ok: true, value: { lat: 52.5, lon: 13.4, accuracy: 12, precise: true } });

    expect(sent.map(c => `${c.cap}.${c.method}`)).toEqual(["camera.grant", "camera.grant", "file.grant", "file.grant", "share.grant", "share.grant", "location.grant"]);
  });

  it("a refused grant rejects not_granted without opening anything", async () => {
    for (const grant of [{ allowed: false }, new CapError("not_granted", "no")]) {
      const sent = app(grant);
      const { e, posted } = env({ share: {}, file: {}, camera: {}, location: {} });
      const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
      for (const [ns, method, args] of [["camera", "take", []], ["file", "pick", []], ["share", "text", ["x"]], ["location", "current", []]] as const) {
        expect(await run(host, posted, ns, method, [...args])).toMatchObject({ ok: false, error: { code: "not_granted" } });
      }
      expect(sent.every(c => c.method === "grant")).toBe(true);
      expect(opened).toEqual([]);
      expect(shared).toEqual([]);
      expect(geoCalls).toEqual([]);
    }
  });

  it("a dismissed grant question passes its code; a closed picker is cancelled; a closed sheet is not shared", async () => {
    app(new CapError("dismissed", "closed"));
    const { e, posted } = env({ camera: {}, file: {}, share: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    expect(await run(host, posted, "camera", "take")).toMatchObject({ error: { code: "dismissed" } });
    app();
    nextPick = "cancel";
    expect(await run(host, posted, "camera", "take")).toMatchObject({ error: { code: "cancelled" } });
    expect(await run(host, posted, "file", "pick")).toMatchObject({ error: { code: "cancelled" } });
    shareError = Object.assign(new Error("closed"), { name: "AbortError" });
    expect(await run(host, posted, "share", "text", ["x"])).toMatchObject({ ok: true, value: { shared: false } });
  });

  it("a picked file over 4 MiB rejects too_large, as does a page's file to save, and bad arguments are refused before the grant", async () => {
    const sent = app();
    const { e, posted } = env({ file: {}, share: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    nextPick = new File([new Uint8Array(5 * 1024 * 1024)], "big.bin");
    expect(await run(host, posted, "file", "pick")).toMatchObject({ error: { code: "too_large" } });
    const big = "A".repeat(Math.floor(4 * 1024 * 1024 * 4 / 3) + 8);
    expect(await run(host, posted, "file", "save", [{ name: "b", data: big }])).toMatchObject({ error: { code: "too_large" } });
    const grants = sent.length;
    expect(await run(host, posted, "share", "file", [{ name: "a/b", data: "" }])).toMatchObject({ error: { code: "bad_request" } });
    expect(await run(host, posted, "share", "text", [""])).toMatchObject({ error: { code: "bad_request" } });
    expect(await run(host, posted, "file", "pick", [{ accept: "csv" }])).toMatchObject({ error: { code: "bad_request" } });
    expect(sent.length).toBe(grants);
  });

  // The app's own fixture (apps/clax_app/test/platform/device/
  // location_test.dart, "coarsens the shared fixture as the shell does").
  const FIXTURE: [number, number, number, number, number, [number, number, number]][] = [
    [51.5007, -0.1246, 12, 0.3, 0.7, [51.504, -0.134948, 2000]],
    [-33.8568, 151.2153, 3500.4, 0.9, 0.1, [-33.848, 151.216445, 3500]],
    [-16.5, 179.999, 40, 0.25, 0.75, [-16.495, -179.996885, 2000]],
  ];

  it("coarsens as the app does: an approximate grant's fix is its cell's centre on the app's grid", async () => {
    for (const [lat, lon, accuracy, sLat, sLon, [wLat, wLon, wAcc]] of FIXTURE) {
      expect(new Coarsener(sLat, sLon).coarsen({ lat, lon, accuracy })).toEqual({ lat: wLat, lon: wLon, accuracy: wAcc, precise: false });
      geoCalls.length = 0;
      app({ allowed: true, precise: false, shift: [sLat, sLon] });
      const { e, posted } = env({ location: {} });
      const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
      const here = run(host, posted, "location", "current");
      await vi.waitFor(() => expect(geoCalls).toHaveLength(1));
      expect(geoCalls[0].opts).toMatchObject({ enableHighAccuracy: false });
      geoCalls[0].ok(position(lat, lon, accuracy));
      expect(await here).toMatchObject({ ok: true, value: { lat: wLat, lon: wLon, accuracy: wAcc, precise: false } });
    }
  });

  it("a precise grant's fix no better than 2 km (iOS's reduced accuracy) is coarsened too; a refused system permission is not_granted", async () => {
    app({ allowed: true, precise: true, shift: [0.3, 0.7] });
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const here = run(host, posted, "location", "current");
    await vi.waitFor(() => expect(geoCalls).toHaveLength(1));
    geoCalls[0].ok(position(51.5007, -0.1246, 3000));
    expect(await here).toMatchObject({ value: { lat: 51.504, lon: -0.134948, accuracy: 3000, precise: false } });
    const denied = run(host, posted, "location", "current");
    await vi.waitFor(() => expect(geoCalls).toHaveLength(2));
    geoCalls[1].err!({ code: 1, message: "denied" } as GeolocationPositionError);
    expect(await denied).toMatchObject({ error: { code: "not_granted" } });
  });

  it("a watch runs in the shell at the app's cadence, unwatch stops it, and the app hiding the view ends the rest with stopped", async () => {
    vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
    const sent = link();
    const grant = () => { for (const c of sent) if (c.method === "grant") c.resolve({ allowed: true, precise: true, shift: [0.5, 0.5] }); };
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const updates = () => posted.filter(m => m.type === "clax:event").map(m => (m as { topic: string; data: { watch: string; value?: { lat: number } } }).data.watch + ":" + (m as { topic: string }).topic);
    hostArrival.presses++;
    const w1 = host.handle({ type: "clax:call", id: "w1", ns: "location", method: "watch", args: [{ watch: "a", minDistance: 50 }] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    grant();
    await w1;
    expect(posted.at(-1)).toEqual({ type: "clax:call-result", id: "w1", ok: true, value: { watching: true } });
    expect(sent.map(c => c.method)).toEqual(["grant"]);
    const g = [...watching.values()][0];
    expect(g.opts).toMatchObject({ enableHighAccuracy: true });
    g.ok(position(52.5, 13.4, 5));
    // Too near, then far enough but within the second: held to its end.
    g.ok(position(52.5001, 13.4, 5));
    g.ok(position(52.501, 13.4, 5));
    expect(updates()).toEqual(["a:update"]);
    vi.advanceTimersByTime(1000);
    expect(updates()).toEqual(["a:update", "a:update"]);
    // A second watch; then the page stops the first, with no end sent.
    hostArrival.presses++;
    const w2 = host.handle({ type: "clax:call", id: "w2", ns: "location", method: "watch", args: [{ watch: "b" }] });
    await vi.waitFor(() => expect(sent).toHaveLength(2));
    sent[1].resolve({ allowed: true, precise: true, shift: [0.5, 0.5] });
    await w2;
    await host.handle({ type: "clax:call", id: "u1", ns: "location", method: "unwatch", args: [{ watch: "a" }] });
    expect(posted.at(-1)).toMatchObject({ id: "u1", ok: true, value: { stopped: true } });
    expect(watching.size).toBe(1);
    for (const f of deviceLink.hidden) f();
    expect(updates().at(-1)).toBe("b:end");
    expect(posted.at(-1)).toMatchObject({ data: { watch: "b", code: "stopped" } });
    expect(watching.size).toBe(0);
    expect(sent.every(c => c.method === "grant")).toBe(true);
    host.dispose();
  });

  it("an approximate watch reports a new cell only after the hold", async () => {
    vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    hostArrival.presses++;
    const w = host.handle({ type: "clax:call", id: "w", ns: "location", method: "watch", args: [{ watch: "a" }] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    sent[0].resolve({ allowed: true, precise: false, shift: [0.5, 0.5] });
    await w;
    const g = [...watching.values()][0];
    const values = () => posted.filter(m => m.type === "clax:event").map(m => (m as { data: { value: { lat: number } } }).data.value.lat);
    g.ok(position(52.5, 13.4, 5));
    expect(values()).toHaveLength(1);
    // Deep inside the cell to the north: nothing for minutes, then once.
    g.ok(position(52.52, 13.41, 5));
    vi.advanceTimersByTime(3 * 60_000 - 1);
    expect(values()).toHaveLength(1);
    vi.advanceTimersByTime(10 * 60_000);
    expect(values()).toHaveLength(2);
    expect(values()[1]).toBeGreaterThan(values()[0]);
    host.dispose();
    expect(watching.size).toBe(0);
  });

  it("refuses a fifth watch and a repeated ID", async () => {
    const sent = app({ allowed: true, precise: true, shift: [0.5, 0.5] });
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    for (const id of ["a", "b", "c", "d"]) expect(await run(host, posted, "location", "watch", [{ watch: id }])).toMatchObject({ ok: true });
    expect(await run(host, posted, "location", "watch", [{ watch: "a" }])).toMatchObject({ error: { code: "bad_request" } });
    expect(await run(host, posted, "location", "watch", [{ watch: "e" }])).toMatchObject({ error: { code: "bad_request" } });
    expect(sent).toHaveLength(4);
    host.dispose();
  });

  it("notify stays the app's on iOS, and on Android every call is the app's as before", async () => {
    const sent = link();
    const { e, posted } = env({ notify: {}, camera: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const list = host.handle({ type: "clax:call", id: "n", ns: "notify", method: "list", args: [] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ cap: "notify", method: "list" });
    sent[0].resolve([]);
    await list;
    deviceLink.platform = "android";
    const take = host.handle({ type: "clax:call", id: "t", ns: "camera", method: "take", args: [] });
    await vi.waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1]).toMatchObject({ cap: "camera", method: "take", args: [] });
    sent[1].resolve({ mime: "image/jpeg", data: "", width: 1, height: 1 });
    await take;
    expect(opened).toEqual([]);
    expect(posted.at(-1)).toMatchObject({ id: "t", ok: true });
  });
});
