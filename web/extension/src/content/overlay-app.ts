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
import type { Thread } from "../../../shell/src/threads";
import { isFromWorker, MAX_TITLE, type OverlayToWorker, PICK_ID, type Rect, type WorkerToOverlay } from "../messages";
import { PIN_CSS, Pins } from "./pins";
import { type Placed, realTimers, Resolver, type Timers } from "./resolver";
import { serializeSnapshot } from "./snapshot";

/** How often the overlay checks whether the automatic snapshot is due (spec L11). */
export const SNAPSHOT_CHECK_MS = 1000;
/** The least time between two automatic snapshots. */
export const SNAPSHOT_EVERY_MS = 10_000;
/** The keep-alive ping to the worker (spec §9.5). */
export const PING_MS = 20_000;
/** The least time between two `route` messages: a burst of navigations
 * sends the last URL once, on the trailing edge. */
export const ROUTE_MS = 250;
/** The most results one `resolved` carries (`isFromOverlay`'s bound). */
const MAX_RESULTS = 500;
/** The global, in the isolated world only, that marks a started overlay.
 * It is the overlay's own; the worker's `claxOverlayLoaded` flag is set and
 * read by the worker alone. */
const STARTED = "claxOverlayStarted";

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
};

/** Starts the overlay unless this world already has one; returns what
 * stops it (after which it may start again), or undefined when it was
 * running already. */
export function startOnce(env: OverlayEnv, g: object = globalThis): (() => void) | undefined {
  const G = g as Record<string, unknown>;
  if (G[STARTED]) return undefined;
  G[STARTED] = true;
  let stopped = false;
  const stop = startOverlay(env, () => { if (!stopped) { stopped = true; G[STARTED] = false; } });
  return () => { stop(); };
}

/** What the overlay shows of a state: a state that changes none of it
 * (`pending` alone, say) resolves nothing again. */
const shownKey = (threads: Thread[], route: string | null) =>
  JSON.stringify([route, threads.map(t => [t.id, t.status, t.anchor])]);

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
   * reloaded or removed) stops the overlay. */
  const send = (m: OverlayToWorker): Promise<unknown> => {
    if (!live) return Promise.resolve(null);
    try {
      if (!runtime.id) throw new Error("extension context invalidated");
      return runtime.sendMessage(m).catch(() => null);
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
  root.innerHTML = `<style>${PIN_CSS}iframe{position:fixed;z-index:2147483647;width:360px;height:236px;border:0;border-radius:10px;box-shadow:0 8px 28px rgba(0,0,0,.28);color-scheme:normal;pointer-events:auto}</style>`;
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

  let composer: { pickId: string; frame: HTMLIFrameElement } | null = null;
  const closeComposer = () => { composer?.frame.remove(); composer = null; };
  stops.push(closeComposer);

  /** A pick: the screenshot is taken with nothing of Clax drawn, then the
   * composer opens for the pick the worker issued, and the anchor and the
   * page's snapshot follow. */
  async function pick(anchor: Anchor, rect: Rect): Promise<void> {
    mode.setVisible(false);
    setHidden(true);
    await new Promise<void>(r => timers.frame(() => timers.frame(r)));
    const reply = await send({ t: "capture", rect, dpr: Math.min(8, win.devicePixelRatio || 1) });
    mode.setVisible(true);
    setHidden(false);
    mode.captured();
    const pickId = (reply as { pickId?: unknown } | null)?.pickId;
    if (!live || typeof pickId !== "string" || !PICK_ID.test(pickId)) return;
    mode.set(false);
    openComposer(pickId, rect);
    timers.set(() => {
      if (!live) return;
      const s = serializeSnapshot(doc);
      void send({ t: "pick", pickId, anchor, url: win.location.href, title: doc.title.slice(0, MAX_TITLE), snapshot: s.html, snapshotError: s.error });
    }, 0);
  }

  function openComposer(pickId: string, rect: Rect): void {
    closeComposer();
    const f = doc.createElement("iframe");
    f.src = `${runtime.getURL("composer.html")}#${pickId}`;
    // The composer loads once; a second load is a navigation the page made
    // (a parent may navigate a child frame), so the frame goes.
    let loads = 0;
    f.addEventListener("load", () => {
      if (++loads < 2 || composer?.frame !== f) return;
      closeComposer();
      void send({ t: "cancel", pickId });
    });
    const left = Math.min(Math.max(8, rect.x + rect.w + 12), win.innerWidth - 368);
    const top = Math.min(Math.max(8, rect.y), win.innerHeight - 244);
    f.style.left = `${Math.max(8, left)}px`;
    f.style.top = `${Math.max(8, top)}px`;
    root.appendChild(f);
    composer = { pickId, frame: f };
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
    const s = serializeSnapshot(doc);
    if (!s.error) void send({ t: "quiet", url: win.location.href, title: doc.title.slice(0, MAX_TITLE), snapshot: s.html });
  }
  stops.push(every(maybeSnapshot, SNAPSHOT_CHECK_MS));
  stops.push(every(() => void send({ t: "ping" }), PING_MS));

  // Same-document navigations: the worker looks the new URL up (spec §11
  // "SPA route change"). A tab that holds only activeTab has no loader.
  // Only the browser's own events count (a page can dispatch these), the
  // URL is read from `location`, an unchanged one is not sent, and a burst
  // sends its last URL once, at most one every ROUTE_MS.
  let lastRoute = win.location.href;
  let lastRouteAt = now();
  let routeTimer: unknown = null;
  const sendRoute = () => {
    const href = win.location.href;
    if (href === lastRoute) return;
    lastRoute = href;
    lastRouteAt = now();
    void send({ t: "route", url: href });
  };
  const onNav = (e: Event) => {
    if (!e.isTrusted || routeTimer !== null) return;
    routeTimer = timers.set(() => { routeTimer = null; if (live) sendRoute(); }, Math.max(0, lastRouteAt + ROUTE_MS - now()));
  };
  const nav = (win as { navigation?: EventTarget }).navigation;
  const navTarget: EventTarget = nav ?? win;
  const navEvent = nav ? "navigatesuccess" : "popstate";
  navTarget.addEventListener(navEvent, onNav);
  win.addEventListener("hashchange", onNav);
  stops.push(() => {
    navTarget.removeEventListener(navEvent, onNav);
    win.removeEventListener("hashchange", onNav);
    if (routeTimer !== null) timers.clear(routeTimer);
  });

  const onMessage = (m: unknown, sender: chrome.runtime.MessageSender) => {
    if (!live || sender.id !== runtime.id || sender.tab || !isFromWorker(m)) return;
    switch (m.t) {
      case "state": {
        state = m;
        mode.set(m.commentMode && composer === null);
        const key = shownKey(m.threads, m.route);
        if (key !== shown) { shown = key; resolver.set(m.threads, m.route); }
        break;
      }
      case "comment-mode": mode.set(m.on && composer === null); break;
      case "close-composer":
        if (composer?.pickId !== m.pickId) break;
        closeComposer();
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
      case "snapshot-now": lastQuiet = -Infinity; break;
      case "resend":
        // Sent now, or by the resolution already due.
        told = "";
        if (!resolver.busy) tellResolved(placed);
        break;
      case "stream-status": break;
    }
  };
  runtime.onMessage.addListener(onMessage);
  stops.push(() => runtime.onMessage.removeListener(onMessage));

  void send({ t: "route", url: lastRoute });
  return stop;
}
