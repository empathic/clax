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

/** How long the DOM must be still before the automatic snapshot (spec L11). */
export const QUIET_SNAPSHOT_MS = 1000;
/** The least time between two automatic snapshots. */
export const SNAPSHOT_EVERY_MS = 10_000;
/** The keep-alive ping to the worker (spec §9.5). */
export const PING_MS = 20_000;
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

  // The pins' host: in the top layer (a manual popover), styled so the
  // page's rules cannot hide or move it, passing every pointer event but
  // the pins' and the composer's through.
  const host = doc.createElement(OVERLAY_TAG);
  host.setAttribute("popover", "manual");
  const HOST_CSS = "all:initial!important;position:fixed!important;inset:0!important;display:block!important;pointer-events:none!important;background:transparent!important;border:0!important;margin:0!important;padding:0!important;overflow:visible!important;width:auto!important;height:auto!important;opacity:1!important;z-index:2147483647!important;";
  host.style.cssText = HOST_CSS;
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `<style>${PIN_CSS}iframe{position:fixed;z-index:2147483647;width:360px;height:236px;border:0;border-radius:10px;box-shadow:0 8px 28px rgba(0,0,0,.28);color-scheme:normal;pointer-events:auto}</style>`;
  const setHidden = (hidden: boolean) => host.style.setProperty("visibility", hidden ? "hidden" : "visible", "important");
  const attach = () => {
    doc.documentElement.appendChild(host);
    try { host.showPopover(); } catch { /* no popover support, or already shown */ }
  };
  attach();
  stops.push(() => host.remove());

  // A page that removes the host gets it back once; then the overlay gives
  // up and the worker tells the side panel (spec §10.4).
  let reattached = false;
  const guard = new MutationObserver(() => {
    if (host.isConnected) return;
    if (!reattached) { reattached = true; attach(); watch(); return; }
    guard.disconnect();
    void send({ t: "removed" });
  });
  const watch = () => {
    guard.disconnect();
    guard.observe(doc, { childList: true });
    guard.observe(doc.documentElement, { childList: true });
  };
  watch();
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
  /** The results go to the worker when what was found changed, not on
   * every scroll. */
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
  stops.push(() => mode.set(false));

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
    const left = Math.min(Math.max(8, rect.x + rect.w + 12), win.innerWidth - 368);
    const top = Math.min(Math.max(8, rect.y), win.innerHeight - 244);
    f.style.left = `${Math.max(8, left)}px`;
    f.style.top = `${Math.max(8, top)}px`;
    root.appendChild(f);
    composer = { pickId, frame: f };
  }

  // The automatic snapshot after an agent addressed a thread (spec L11):
  // the page visible, an address pending, and the DOM quiet for a second;
  // at most one every SNAPSHOT_EVERY_MS, unless the worker asks for one.
  let lastQuiet = -Infinity;
  stops.push(every(() => {
    if (!state?.pending || doc.visibilityState !== "visible") return;
    const t = now();
    if (t - resolver.lastMutation < QUIET_SNAPSHOT_MS || t - lastQuiet < SNAPSHOT_EVERY_MS) return;
    lastQuiet = t;
    const s = serializeSnapshot(doc);
    if (!s.error) void send({ t: "quiet", url: win.location.href, title: doc.title.slice(0, MAX_TITLE), snapshot: s.html });
  }, QUIET_SNAPSHOT_MS));
  stops.push(every(() => void send({ t: "ping" }), PING_MS));

  // Same-document navigations: the worker looks the new URL up (spec §11
  // "SPA route change"). A tab that holds only activeTab has no loader.
  const route = () => void send({ t: "route", url: win.location.href });
  const nav = (win as { navigation?: EventTarget }).navigation;
  const navTarget: EventTarget = nav ?? win;
  const navEvent = nav ? "navigatesuccess" : "popstate";
  navTarget.addEventListener(navEvent, route);
  win.addEventListener("hashchange", route);
  stops.push(() => { navTarget.removeEventListener(navEvent, route); win.removeEventListener("hashchange", route); });

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
      case "stream-status": break;
    }
  };
  runtime.onMessage.addListener(onMessage);
  stops.push(() => runtime.onMessage.removeListener(onMessage));

  route();
  return stop;
}
