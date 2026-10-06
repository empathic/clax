import { describe, expect, it } from "vitest";
import type { WorkerToOverlay } from "../messages";
import { ApiFailure } from "./api";
import { PICK_TTL_MS, Picks } from "./picks";

// jsdom's Blob has no `arrayBuffer` or `text` (a worker's has both): read through FileReader.
const read = (b: Blob, as: "text" | "buffer") => new Promise<unknown>((ok, fail) => {
  const r = new FileReader();
  r.onload = () => ok(r.result);
  r.onerror = () => fail(r.error);
  if (as === "text") r.readAsText(b); else r.readAsArrayBuffer(b);
});
if (!Blob.prototype.arrayBuffer) Blob.prototype.arrayBuffer = function () { return read(this, "buffer") as Promise<ArrayBuffer>; };
if (!Blob.prototype.text) Blob.prototype.text = function () { return read(this, "text") as Promise<string>; };

const anchor = { kind: "element", selector: "#save", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const T1 = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const P1 = "01J9PPPPPPPPPPPPPPPPPPPPP1";
const P2 = "01J9PPPPPPPPPPPPPPPPPPPPP2";
const RECT = { x: 10, y: 20, w: 30, h: 40 };
const page = { artifact_id: "7q3k9mzx2b4t", origin: "http://localhost:5173", path: "/", page_url: "http://localhost:5173/", title: "Home", current_version: 2, url: "http://localhost:7480/a/7q3k9mzx2b4t" };

function port(name: string, tabId: number) {
  const sent: unknown[] = [];
  const listeners: ((m: unknown) => void)[] = [];
  const gone: (() => void)[] = [];
  const p = {
    name, sent, sender: { tab: { id: tabId } }, cut: false,
    postMessage: (m: unknown) => sent.push(m),
    onMessage: { addListener: (l: (m: unknown) => void) => listeners.push(l) },
    onDisconnect: { addListener: (l: () => void) => gone.push(l) },
    disconnect: () => { p.cut = true; },
    fire: (m: unknown) => listeners.forEach(l => l(m)),
    /** The composer page went away. */
    close: () => gone.forEach(l => l()),
  };
  return p;
}
const flush = () => new Promise(r => setTimeout(r, 0));
const capture = (pickId: string, rect = RECT) => ({ t: "capture", pickId, rect, dpr: 1 }) as const;
const pick = (pickId: string, extra: Record<string, unknown> = {}) =>
  ({ t: "pick", pickId, anchor: anchor as never, url: "http://localhost:5173/", title: "Home", snapshot: "<p>", snapshotError: null, ...extra }) as never;

function setup(opts: { clip?: { png: Blob } | { error: string }; fail?: unknown; pending?: string[] } = {}) {
  const posted: { form: FormData; pending: string[] }[] = [];
  const snapshots: { form: FormData; pending: string[] }[] = [];
  const overlay: { tabId: number; m: WorkerToOverlay }[] = [];
  const told: { tabId: number; page: unknown }[] = [];
  const captures: unknown[][] = [];
  let now = 0;
  let pending = opts.pending ?? [];
  let fail = opts.fail;
  let snapshotFail: unknown = null;
  const picks = new Picks({
    api: {
      postThread: async (f: FormData, p: string[]) => {
        if (fail) { const e = fail; fail = undefined; throw e; }
        posted.push({ form: f, pending: p });
        return { thread: { id: T1 }, page, version: 3 };
      },
      postSnapshot: async (f: FormData, p: string[]) => {
        snapshots.push({ form: f, pending: p });
        if (snapshotFail) throw snapshotFail;
        return { page, version: 3, linked: p };
      },
    } as never,
    capture: async (...a) => { captures.push(a); return opts.clip ?? { png: new Blob([new Uint8Array([137, 80, 78, 71])], { type: "image/png" }) }; },
    toOverlay: (tabId, m) => overlay.push({ tabId, m }),
    pendingIds: () => pending,
    posted: (tabId, p) => told.push({ tabId, page: p }),
    now: () => now,
  });
  return {
    picks, posted, snapshots, overlay, told, captures,
    advance: (ms: number) => { now += ms; },
    setPending: (p: string[]) => { pending = p; },
    failSnapshots: (e: unknown) => { snapshotFail = e; },
  };
}
const ID = (n: number) => String(n).padStart(32, "0");

describe("Picks", () => {
  it("captures for the pick the overlay names, then opens its composer", async () => {
    const { picks, overlay, captures } = setup();
    const r = await picks.capture(5, 9, capture(ID(1)));
    expect(r).toEqual({ t: "captured", pickId: ID(1), ok: true });
    expect(captures[0]).toEqual([9, RECT, 1]);
    expect(overlay).toEqual([{ tabId: 5, m: { t: "open-composer", pickId: ID(1), rect: RECT } }]);
  });

  it("says why there is no screenshot, and still opens the composer", async () => {
    const { picks, overlay } = setup({ clip: { error: "no_capture_permission" } });
    expect(await picks.capture(5, 9, capture(ID(1)))).toEqual({ t: "captured", pickId: ID(1), ok: false, error: "no_capture_permission" });
    expect(overlay.at(-1)?.m.t).toBe("open-composer");
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    await picks.attach(5, pick(ID(1)));
    expect(p.sent.at(-1)).toEqual({ t: "draft", anchor, clipUrl: null, clipError: "no_capture_permission", capturing: false });
  });

  it("refuses a second capture under the tab's current pick ID", async () => {
    const { picks, captures } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    expect(await picks.capture(5, 9, capture(ID(1)))).toBeNull();
    expect(captures).toHaveLength(1);
  });

  it("posts once the body and the snapshot are both in, whichever comes first", async () => {
    const { picks, posted, overlay, told } = setup({ pending: [P1] });
    await picks.capture(5, 9, capture(ID(1)));
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    p.fire({ t: "ready" });
    p.fire({ t: "post", body: "Too wide" });
    expect(posted).toHaveLength(0);
    await picks.attach(5, pick(ID(1)));
    await flush();
    expect(posted).toHaveLength(1);
    const f = posted[0].form;
    expect(f.get("body")).toBe("Too wide");
    expect(f.get("url")).toBe("http://localhost:5173/");
    expect(f.get("title")).toBe("Home");
    expect(JSON.parse(f.get("anchor") as string)).toEqual(anchor);
    expect(await (f.get("snapshot") as File).text()).toBe("<p>");
    expect((f.get("clip") as File).type).toBe("image/png");
    expect(posted[0].pending).toEqual([P1]);
    expect(p.sent.at(-1)).toEqual({ t: "posted", threadId: T1 });
    expect(overlay.at(-1)).toEqual({ tabId: 5, m: { t: "close-composer", pickId: ID(1), posted: true } });
    expect(told).toEqual([{ tabId: 5, page }]);
  });

  it("names the threads pending before the page was serialized, not those addressed after", async () => {
    const { picks, posted, setPending } = setup({ pending: [P1] });
    await picks.capture(5, 9, capture(ID(1)));
    // The overlay serializes once it hears open-composer; an address made since is not in it.
    setPending([P1, P2]);
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    await picks.attach(5, pick(ID(1)));
    p.fire({ t: "post", body: "Too wide" });
    await flush();
    expect(posted[0].pending).toEqual([P1]);
  });

  it("sends the draft with the clip as a PNG data URL once the composer is ready and the anchor is in", async () => {
    const { picks } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    p.fire({ t: "ready" });
    await flush();
    expect(p.sent).toHaveLength(0);
    await picks.attach(5, pick(ID(1)));
    expect(p.sent[0]).toEqual({ t: "draft", anchor, clipUrl: "data:image/png;base64,iVBORw==", clipError: null, capturing: false });
  });

  it("posts a pick whose page was too large to snapshot, and only once", async () => {
    const { picks, posted } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    await picks.attach(5, pick(ID(1), { snapshot: null, snapshotError: "too_large" }));
    p.fire({ t: "post", body: "One" });
    p.fire({ t: "post", body: "Two" });
    await flush();
    expect(posted).toHaveLength(1);
    expect(posted[0].form.get("body")).toBe("One");
  });

  it("tells the composer a failed post and lets it post again", async () => {
    const { picks, posted } = setup({ fail: new ApiFailure("daemon_unreachable", "The daemon is not reachable.") });
    await picks.capture(5, 9, capture(ID(1)));
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    await picks.attach(5, pick(ID(1)));
    p.fire({ t: "post", body: "Too wide" });
    await flush();
    expect(p.sent.at(-1)).toEqual({ t: "failed", message: "The daemon is not reachable." });
    p.fire({ t: "post", body: "Too wide" });
    await flush();
    expect(posted).toHaveLength(1);
  });

  it("refuses a composer for another tab, an unknown pick, a second port or a stale pick", async () => {
    const { picks, advance } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    const other = port(`composer:${ID(1)}`, 6);
    picks.attachComposer(other as never, 6);
    expect(other.cut).toBe(true);
    const unknown = port(`composer:${"f".repeat(32)}`, 5);
    picks.attachComposer(unknown as never, 5);
    expect(unknown.cut).toBe(true);
    const first = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(first as never, 5);
    expect(first.cut).toBe(false);
    const second = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(second as never, 5);
    expect(second.cut).toBe(true);
    await picks.capture(5, 9, capture(ID(2)));
    advance(PICK_TTL_MS);
    const late = port(`composer:${ID(2)}`, 5);
    picks.attachComposer(late as never, 5);
    expect(late.cut).toBe(true);
  });

  it("drops what the composer sends that its validator refuses", async () => {
    const { picks, posted } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    await picks.attach(5, pick(ID(1)));
    p.fire({ t: "post", body: "x", extra: 1 });
    p.fire({ t: "post", body: "   " });
    await flush();
    expect(posted).toHaveLength(0);
  });

  it("takes the pick's anchor and snapshot once, from its own tab", async () => {
    const { picks, posted } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    await picks.attach(6, pick(ID(1)));
    await picks.attach(5, pick(ID(2)));
    expect(p.sent).toHaveLength(0);
    await picks.attach(5, pick(ID(1)));
    await picks.attach(5, pick(ID(1), { title: "Changed" }));
    p.fire({ t: "post", body: "Hi" });
    await flush();
    expect(posted[0].form.get("title")).toBe("Home");
  });

  it("cancels from the composer or the overlay, and closes the composer", async () => {
    const { picks, overlay, posted } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    p.fire({ t: "cancel" });
    expect(p.cut).toBe(true);
    expect(overlay.at(-1)).toEqual({ tabId: 5, m: { t: "close-composer", pickId: ID(1), posted: false } });
    await picks.attach(5, pick(ID(1)));
    p.fire({ t: "post", body: "Hi" });
    await flush();
    expect(posted).toHaveLength(0);

    await picks.capture(5, 9, capture(ID(2)));
    picks.cancel(5, ID(3));
    expect(overlay.at(-1)?.m.t).toBe("open-composer");
    picks.cancel(5, ID(2));
    expect(overlay.at(-1)).toEqual({ tabId: 5, m: { t: "close-composer", pickId: ID(2), posted: false } });
  });

  it("drops the pick when its composer goes away unposted", async () => {
    const { picks, overlay } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    p.close();
    expect(overlay.at(-1)).toEqual({ tabId: 5, m: { t: "close-composer", pickId: ID(1), posted: false } });
    const again = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(again as never, 5);
    expect(again.cut).toBe(true);
  });

  it("forgets a closed tab's pick without telling it", async () => {
    const { picks, overlay } = setup();
    await picks.capture(5, 9, capture(ID(1)));
    const n = overlay.length;
    picks.close(5);
    expect(overlay).toHaveLength(n);
    const p = port(`composer:${ID(1)}`, 5);
    picks.attachComposer(p as never, 5);
    expect(p.cut).toBe(true);
  });

  it("posts a quiet snapshot for the threads still pending of those the overlay named", async () => {
    const { picks, snapshots, failSnapshots } = setup({ pending: [P1] });
    const quiet = (pending: string[]) => ({ t: "quiet", url: "http://localhost:5173/", title: "Home", snapshot: "<p>", pending }) as const;
    await picks.quiet(5, quiet([P1, P2]));
    expect(snapshots).toHaveLength(1);
    expect(snapshots[0].pending).toEqual([P1]);
    expect(snapshots[0].form.get("url")).toBe("http://localhost:5173/");
    expect(await (snapshots[0].form.get("snapshot") as File).text()).toBe("<p>");
    // None of the named threads is pending now: nothing is posted.
    await picks.quiet(5, quiet([P2]));
    expect(snapshots).toHaveLength(1);
    // The daemon's 409 (nothing pending any more) is not a failure.
    failSnapshots(new ApiFailure("nothing_pending", "", 409));
    await expect(picks.quiet(5, quiet([P1]))).resolves.toBeUndefined();
  });
});
