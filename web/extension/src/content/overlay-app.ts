// The overlay (spec 2026-10-05 §6.4, §10, §11, O4, L7, L8): runs in the
// page's isolated world, once per document. Pins and comment mode's drawing
// live in closed shadow roots on <html>, so page scripts cannot read or
// forge them; the hosts carry no thread text or IDs, only neutral
// attributes. The composer is an extension page in an iframe inside the
// pins' root, so nothing typed into it reaches the page. Comment mode starts
// only when the worker says so (after the icon, the command or the context
// menu); no key on the page starts it. The overlay hears only the worker,
// every message checked by `isFromWorker`, and tells it only what §9.4
// lists.
import { buildElementAnchor, buildRangeAnchor, OVERLAY_TAG } from "../../../bridge/src/anchor";
import { type AreaRect, buildAreaAnchor } from "../../../bridge/src/area";
import { CommentMode } from "../../../bridge/src/comment-mode";
import type { Anchor, AnchorResult } from "../../../bridge/src/protocol";
import { rectOf } from "../../../bridge/src/target";
import { isFromWorker, MAX_TITLE, type OverlayThread, type OverlayToWorker, PICK_ID, pageUrl, type Rect, type SnapshotError, URL_TOO_LONG, type WorkerToOverlay, waitsForSnapshot } from "../messages";
import { PIN_CSS, Pins } from "./pins";
import { BOOT, RUNNING, liveOverlay, stopStale } from "./presence";
import { type Placed, realTimers, Resolver, type Timers } from "./resolver";
import { ROUTE_MS, watchRoutes } from "./routes";
import { serializeSnapshot } from "./snapshot";

export { ROUTE_MS };

/** How often the overlay checks whether the automatic snapshot is due (spec L11). */
export const SNAPSHOT_CHECK_MS = 1000;
/** The least time between two automatic snapshots. */
export const SNAPSHOT_EVERY_MS = 10_000;
/** The keep-alive ping to the worker (spec §9.5). */
export const PING_MS = 20_000;
/** The composer frame's size in CSS pixels: room for the quote, the clip's
 * thumbnail, three lines of text, the buttons and a notice. */
const FRAME_W = 360;
const FRAME_H = 300;
/** How long a composer frame waits, hidden, for the worker to confirm its
 * page connected before the overlay closes it (the worker's own wait is shorter). */
export const COMPOSER_CONFIRM_MS = 10_000;
/** How long a notice over the page stays. */
export const NOTICE_MS = 6000;
/** The most results one `resolved` carries (`isFromOverlay`'s bound). */
const MAX_RESULTS = 500;

export type OverlayRuntime = {
  readonly id: string | undefined;
  sendMessage(m: OverlayToWorker): Promise<unknown>;
  getURL(path: string): string;
  onMessage: {
    addListener(l: (m: unknown, sender: chrome.runtime.MessageSender) => void): void;
    removeListener(l: (m: unknown, sender: chrome.runtime.MessageSender) => void): void;
  };
};
export type OverlayEnv = {
  doc: Document;
  runtime: OverlayRuntime;
  timers?: Timers;
  /** Runs `fn` every `ms`; returns what stops it. */
  every?(fn: () => void, ms: number): () => void;
  now?(): number;
  /** A new pick's ID: 128 random bits, in hex. */
  randomId?(): string;
};

/** Starts the overlay unless this world already has a live one; returns
 * what stops it (after which it may start again), or undefined when one was
 * running already. One left over from an earlier load of the extension
 * (`presence.ts`) is stopped first. The world's mark (`RUNNING`) carries
 * the overlay's liveness: its extension context is there, and the boot
 * nonce it started under is the worker's. */
export function startOnce(env: OverlayEnv, g: object = globalThis): (() => void) | undefined {
  const G = g as Record<string, unknown>;
  if (liveOverlay(g)) return undefined;
  stopStale(g);
  const boot = G[BOOT];
  let live = true;
  const alive = (b: unknown) => {
    try { return live && b === boot && !!env.runtime.id; } catch { return false; }
  };
  const mark = { alive, stop: () => stop() };
  G[RUNNING] = mark;
  const stop = startOverlay(env, () => {
    live = false;
    if (G[RUNNING] === mark) delete G[RUNNING];
  });
  return () => { stop(); };
}

/** The worker's answer to a message it does not admit. */
const isRefusal = (r: unknown) => typeof r === "object" && r !== null && (r as { off?: unknown }).off === true;

/** 128 random bits in hex, from the isolated world's own `crypto`. */
const randomPickId = () => [...crypto.getRandomValues(new Uint8Array(16))].map(b => b.toString(16).padStart(2, "0")).join("");

/** The open threads a state shows waiting for a snapshot (spec L11). */
const waitingIds = (threads: OverlayThread[]) => threads.filter(waitsForSnapshot).map(t => t.id);

/** What the overlay shows of a state: a state that changes none of it
 * (`pending` alone, say) resolves nothing again. */
const shownKey = (threads: OverlayThread[], route: string | null) =>
  JSON.stringify([route, threads.map(t => [t.id, t.status, t.anchor, t.from])]);

function startOverlay(env: OverlayEnv, onStop: () => void): () => void {
  const { doc, runtime } = env;
  const win = doc.defaultView!;
  const now = env.now ?? (() => performance.now());
  const timers: Timers = { ...(env.timers ?? realTimers), now };
  const every = env.every ?? ((fn, ms) => { const h = setInterval(fn, ms); return () => clearInterval(h); });
  const stops: (() => void)[] = [];
  let live = true;

  const stop = () => {
    if (!live) return;
    live = false;
    for (const s of stops.splice(0)) s();
    onStop();
  };
  /** Sends `m` to the worker; a context the extension left (it was
   * reloaded or removed) stops the overlay, as does the worker's refusal
   * (`{off: true}`: Clax is not on in this tab, or not for this origin). */
  const send = (m: OverlayToWorker): Promise<unknown> => {
    if (!live) return Promise.resolve(null);
    try {
      if (!runtime.id) throw new Error("extension context invalidated");
      return runtime.sendMessage(m).then(r => {
        if (isRefusal(r)) { stop(); return null; }
        return r;
      }, () => null);
    } catch {
      stop();
      return Promise.resolve(null);
    }
  };

  // The pins' host: a child of <html> in the top layer (a manual popover),
  // styled so the page's rules cannot hide or move it, passing every pointer
  // event but the pins' and the composer's through. Its only attributes are
  // `popover` and `style`.
  const host = doc.createElement(OVERLAY_TAG);
  const HOST_CSS = "all:initial!important;position:fixed!important;inset:0!important;display:block!important;pointer-events:none!important;background:transparent!important;border:0!important;margin:0!important;padding:0!important;overflow:visible!important;width:auto!important;height:auto!important;opacity:1!important;z-index:2147483647!important;";
  let hidden = false;
  let ownStyle = "";
  const applyHost = () => {
    host.setAttribute("popover", "manual");
    host.style.cssText = HOST_CSS + (hidden ? "visibility:hidden!important;" : "");
    ownStyle = host.getAttribute("style") ?? "";
  };
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `<style>${PIN_CSS}.notice{position:fixed;left:50%;bottom:16px;transform:translateX(-50%);max-width:min(420px,calc(100vw - 32px));padding:8px 12px;border-radius:8px;background:#1c1b19;color:#fff;font:13px/1.4 system-ui,sans-serif;box-shadow:0 4px 16px rgba(0,0,0,.25)}iframe{position:fixed;z-index:2147483647;width:${FRAME_W}px;height:${FRAME_H}px;border:0;border-radius:10px;box-shadow:0 8px 28px rgba(0,0,0,.28);color-scheme:normal;pointer-events:auto}</style>`;
  const setHidden = (on: boolean) => { hidden = on; applyHost(); };
  const popoverOpen = () => {
    try { return typeof host.showPopover !== "function" || host.matches(":popover-open"); } catch { return true; }
  };
  const intact = () => host.parentNode === doc.documentElement && host.attributes.length === 2
    && host.getAttribute("popover") === "manual" && host.getAttribute("style") === ownStyle && popoverOpen();
  const attach = () => {
    applyHost();
    doc.documentElement.appendChild(host);
    try { host.showPopover(); } catch { /* no popover support, or already shown */ }
  };
  attach();
  stops.push(() => host.remove());

  // A page that removes, moves, restyles or closes the host gets it back
  // once; the next time the overlay gives up and the worker tells the side
  // panel (spec §10.4, §11).
  let repaired = false;
  let gaveUp = false;
  const check = () => {
    if (!live || gaveUp || intact()) return;
    if (!repaired) { repaired = true; attach(); watch(); return; }
    gaveUp = true;
    guard.disconnect();
    void send({ t: "removed" });
  };
  const guard = new MutationObserver(check);
  const watch = () => {
    guard.disconnect();
    guard.observe(doc, { childList: true });
    guard.observe(doc.documentElement, { childList: true });
    guard.observe(host, { attributes: true });
  };
  watch();
  host.addEventListener("toggle", check);
  stops.push(() => guard.disconnect());

  let state: Extract<WorkerToOverlay, { t: "state" }> | null = null;
  let shown = "";
  let selected: string | null = null;
  let placed: Placed[] = [];
  let told = "";

  const pins = new Pins(root, id => {
    selected = id;
    pins.draw(placed, selected);
    void send({ t: "pin", threadId: id });
  });
  /** The results go to the worker when what was found changed (or it asks
   * with `resend`), not on every scroll: a result's `rect` is where its
   * anchor was when they were sent. */
  const tellResolved = (p: Placed[]) => {
    const results: AnchorResult[] = p.slice(0, MAX_RESULTS).map(x => ({ id: x.id, found: x.box !== null, method: x.method, rect: x.box }));
    const key = JSON.stringify(results.map(r => [r.id, r.found, r.method]));
    if (key === told) return;
    told = key;
    void send({ t: "resolved", results });
  };
  const resolver = new Resolver(doc, p => {
    placed = p;
    pins.draw(p, selected);
    tellResolved(p);
    maybeSnapshot();
  }, timers);
  stops.push(() => resolver.stop());

  // Scrolls and resizes move the pins: measured once per frame.
  let measuring = false;
  const remeasure = () => {
    if (measuring) return;
    measuring = true;
    timers.frame(() => { measuring = false; if (live) resolver.measure(); });
  };
  win.addEventListener("scroll", remeasure, { capture: true, passive: true });
  win.addEventListener("resize", remeasure, { passive: true });
  stops.push(() => { win.removeEventListener("scroll", remeasure, { capture: true }); win.removeEventListener("resize", remeasure); });

  const box = (r: DOMRect): Rect => ({ x: r.left, y: r.top, w: r.width, h: r.height });
  const mode = new CommentMode(doc, {
    hover: () => {},
    pickElement: el => void pick(buildElementAnchor(doc, el), box(rectOf(el))),
    pickRange: r => void pick(buildRangeAnchor(doc, r), box(rectOf(r))),
    pickArea: (r: AreaRect) => void pick(buildAreaAnchor(doc, r), { x: r.left, y: r.top, w: r.width, h: r.height }),
    cancel: () => { mode.set(false); void send({ t: "comment-mode", on: false }); },
  }, { shadow: "closed" });
  stops.push(() => mode.destroy());

  /** The composer's frame; `shown` once the worker confirmed the pick's
   * composer page connected; `lost` once the worker said it no longer holds
   * the pick (comment mode is then back, and a new pick replaces the frame). */
  let composer: { pickId: string; frame: HTMLIFrameElement; shown: boolean; lost: boolean; wait: unknown } | null = null;
  /** Comment mode as the worker's state has it, unless a composer holds the page. */
  const syncMode = () => mode.set(!!state?.commentMode && (composer === null || composer.lost));
  const closeComposer = () => {
    if (composer) timers.clear(composer.wait);
    composer?.frame.remove();
    composer = null;
  };
  stops.push(closeComposer);
  /** The pick whose screenshot is being taken, until the worker opens its composer. */
  let picking: { pickId: string; anchor: Anchor } | null = null;
  const newPickId = env.randomId ?? randomPickId;

  /** Clax's drawing is back after a capture. */
  const reveal = () => {
    mode.setVisible(true);
    setHidden(false);
  };

  /** A pick (spec §3.2, §8.1): Clax's drawing is hidden for two frames, the
   * worker captures the tab for the pick ID the overlay names, then tells the
   * overlay to open the pick's composer (`open-composer`), and the anchor and
   * the page's snapshot follow. */
  async function pick(anchor: Anchor, rect: Rect): Promise<void> {
    // The daemon takes no address over MAX_URL: the person is told at once,
    // and nothing is captured.
    if (pageUrl(win.location.href) === null) {
      mode.captured();
      notify(URL_TOO_LONG);
      return;
    }
    const pickId = newPickId();
    picking = { pickId, anchor };
    mode.setVisible(false);
    setHidden(true);
    await new Promise<void>(r => timers.frame(() => timers.frame(r)));
    const reply = await send({ t: "capture", pickId, anchor, rect, dpr: Math.min(8, win.devicePixelRatio || 1) });
    reveal();
    mode.captured();
    // A worker that refused the capture opens no composer for it.
    const ok = isFromWorker(reply) && reply.t === "captured" && reply.pickId === pickId;
    if (!ok && picking?.pickId === pickId) picking = null;
  }

  /** The worker took the pick: its composer's frame opens beside `rect`,
   * hidden until the worker confirms the composer page connected. */
  function composerFor(pickId: string, rect: Rect): void {
    const p = picking;
    if (!p || p.pickId !== pickId || !PICK_ID.test(pickId)) return;
    picking = null;
    reveal();
    mode.set(false);
    openComposer(pickId, rect);
  }

  /** The pick's composer page connected to the worker: its frame is shown
   * and focused, and the page is serialized while the person types (spec
   * §3.2, §8.2). The composer is an extension frame in its own process, so
   * the serialization here does not hold up its typing. */
  function composerReady(pickId: string): void {
    const c = composer;
    if (!c || c.pickId !== pickId || c.shown) return;
    c.shown = true;
    timers.clear(c.wait);
    c.frame.style.visibility = "";
    c.frame.focus();
    timers.set(() => {
      if (!live) return;
      let html: string | null = null;
      let error: SnapshotError | null = null;
      try {
        const s = serializeSnapshot(doc);
        html = s.html;
        error = s.error;
      } catch {
        // A DOM the serializer cannot take: the thread is posted with no snapshot of it.
        error = "failed";
      }
      void send({ t: "pick", pickId, url: pageUrl(win.location.href), title: doc.title.slice(0, MAX_TITLE), snapshot: html, snapshotError: error });
    }, 0);
  }

  /** A short notice over the page, in the closed root, for NOTICE_MS. */
  let notice: { el: HTMLElement; timer: unknown } | null = null;
  const clearNotice = () => { if (notice) { timers.clear(notice.timer); notice.el.remove(); notice = null; } };
  stops.push(clearNotice);
  function notify(text: string): void {
    clearNotice();
    const el = doc.createElement("div");
    el.className = "notice";
    el.setAttribute("role", "status");
    el.textContent = text;
    root.appendChild(el);
    notice = { el, timer: timers.set(clearNotice, NOTICE_MS) };
  }

  function openComposer(pickId: string, rect: Rect): void {
    closeComposer();
    const f = doc.createElement("iframe");
    f.src = `${runtime.getURL("composer.html")}#${pickId}`;
    // The frame stays hidden, and so cannot take focus, until the worker
    // confirms the composer page connected (`composer-ready`): a page may
    // navigate a child frame, even before the composer first loads, and
    // what it puts there is never shown. The composer loads once; a second
    // load is such a navigation, so the frame goes, as does a frame whose
    // page never connects.
    f.style.visibility = "hidden";
    let loads = 0;
    const abandon = () => {
      if (composer?.frame !== f) return;
      closeComposer();
      void send({ t: "cancel", pickId });
    };
    f.addEventListener("load", () => { if (++loads >= 2) abandon(); });
    const left = Math.min(Math.max(8, rect.x + rect.w + 12), win.innerWidth - FRAME_W - 8);
    const top = Math.min(Math.max(8, rect.y), win.innerHeight - FRAME_H - 8);
    f.style.left = `${Math.max(8, left)}px`;
    f.style.top = `${Math.max(8, top)}px`;
    root.appendChild(f);
    composer = { pickId, frame: f, shown: false, lost: false, wait: timers.set(abandon, COMPOSER_CONFIRM_MS) };
  }

  // The automatic snapshot after an agent addressed a thread (spec L11):
  // the page visible, an address pending, and the DOM settled as the
  // resolver judges it (quiet for QUIET_MS, or MAX_WAIT_MS into a run of
  // changes); at most one every SNAPSHOT_EVERY_MS, unless the worker asks
  // for one. It is tried after each resolution and every SNAPSHOT_CHECK_MS.
  let lastQuiet = -Infinity;
  function maybeSnapshot(): void {
    if (!live || !state?.pending || doc.visibilityState !== "visible" || resolver.busy) return;
    const t = now();
    if (t - lastQuiet < SNAPSHOT_EVERY_MS) return;
    lastQuiet = t;
    // Read before serializing: the snapshot covers the addresses made before it.
    const url = pageUrl(win.location.href);
    if (url === null) return;
    const pending = waitingIds(state.threads);
    const s = serializeSnapshot(doc);
    if (!s.error) void send({ t: "quiet", url, title: doc.title.slice(0, MAX_TITLE), snapshot: s.html, pending });
  }
  stops.push(every(maybeSnapshot, SNAPSHOT_CHECK_MS));
  stops.push(every(() => void send({ t: "ping" }), PING_MS));

  // Same-document navigations: the worker looks the new URL up (spec §11
  // "SPA route change"), as `watchRoutes` filters and throttles them.
  stops.push(watchRoutes(win, href => void send({ t: "route", url: pageUrl(href) }), timers, now));
  // A page restored from the back/forward cache asks the worker again: Clax
  // may have turned off in the tab meanwhile, and the `off` went unheard.
  const onShow = (e: PageTransitionEvent) => { if (e.isTrusted && e.persisted) void send({ t: "route", url: pageUrl(win.location.href) }); };
  win.addEventListener("pageshow", onShow);
  stops.push(() => win.removeEventListener("pageshow", onShow));

  const onMessage = (m: unknown, sender: chrome.runtime.MessageSender) => {
    if (!live || sender.id !== runtime.id || sender.tab || !isFromWorker(m)) return;
    switch (m.t) {
      case "state": {
        state = m;
        syncMode();
        const key = shownKey(m.threads, m.route);
        if (key !== shown) { shown = key; resolver.set(m.threads, m.route); }
        break;
      }
      case "captured": break; // the answer to `capture`
      case "open-composer": composerFor(m.pickId, m.rect); break;
      case "composer-ready": composerReady(m.pickId); break;
      case "close-composer":
        if (composer?.pickId !== m.pickId) break;
        closeComposer();
        if (m.reason === "timeout") notify("The comment box did not open. Pick again to comment.");
        if (m.posted || state?.commentMode) mode.set(true);
        break;
      case "focus": case "scroll-to": {
        selected = m.threadId;
        pins.draw(placed, selected);
        const target = m.threadId === null ? null : resolver.target(m.threadId);
        if (m.t === "scroll-to" && target) {
          const el = target instanceof Range ? target.startContainer.parentElement : target;
          el?.scrollIntoView({ block: "center", behavior: "smooth" });
          mode.flash(target);
        }
        break;
      }
      case "resend":
        // Sent now, or by the resolution already due.
        told = "";
        if (!resolver.busy) tellResolved(placed);
        break;
      case "pick-lost":
        if (composer?.pickId !== m.pickId) break;
        composer.lost = true;
        syncMode();
        break;
      case "off": stop(); break;
    }
  };
  runtime.onMessage.addListener(onMessage);
  stops.push(() => runtime.onMessage.removeListener(onMessage));

  void send({ t: "route", url: pageUrl(win.location.href) });
  return stop;
}
