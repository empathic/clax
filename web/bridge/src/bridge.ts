/**
 * Runtime bridge injected into every published page.
 * It exposes `window.claude.use(name)`, which resolves the frozen namespace of
 * each capability this view gets and `null` for the rest (see `use.ts`).
 * `db`, `assets`, `user` and `room` call the page's own origin
 * (`/_clax/api/…`, caps/origin.ts), framed or top-level; the others are the
 * framing shell's, and unframed they resolve `null`. Calls to the shell and
 * its answers travel as `clax:use`/`call`/`event` messages (see `protocol.ts`),
 * queued until the shell's welcome. When framed, it greets the shell with `clax:hello`
 * (target "*"; artifact, version, and the page's published file, no page
 * content), then takes orders only from `window.parent` at
 * an origin in `shellOrigins` of its tag's `data-origin` (the daemon's
 * public origin), and replies to that origin only:
 * comment mode (hover outline, element and range picks with anchors and PNG
 * clips within the budget in `clip.ts`, drawn areas clipped to exactly the
 * rectangle, rendered only once the shell's composer for the pick has focus,
 * Option widening with keys the shell forwards), anchor
 * resolution (an area's rectangle projected onto its element's current box),
 * the dashed outline of the focused thread's area, scroll-to, and the live
 * layers (`live.ts`: this viewer's pointer and selection out as `clax:cursor`,
 * other people's drawn from `clax:peers`, open threads' quotes highlighted;
 * see `protocol.ts`). Every HTML
 * page of a version carries the bridge; anchors it builds name this page's
 * file, and anchors on other files never resolve here. Once welcomed, a plain
 * click on a link to another page of the version that the page did not cancel
 * is cancelled and handed to the shell (`clax:navigate`), which follows it
 * with one history entry; a link to this page under another spelling of its
 * path (`index.html` for `/v/<n>/`) is followed in place (`followInPlace`). After the welcome and
 * on every `hashchange` it reports the page's fragment (`clax:hash`). After
 * a welcome with `app: true` (the Clax app's shell) it reports the page's
 * scroll position as it scrolls (`clax:scroll`), and scrolls the page to the
 * position the welcome carries (`restoreScroll`), with `scroll.ts` from the
 * caps part, loaded on that welcome, so a page outside the app loads none of
 * it; with `routes.ts` from the same part it reports the page's own
 * `history.pushState` routes (`clax:route`) and takes the shell back among
 * them (`clax:back`). On
 * `pagehide` it says `clax:bye`, and the shell stops sending until the next
 * document greets. In a frame the shell named `INSPECT_FRAME` it keeps the
 * page's errors from before its first script (`inspect.ts`); it answers the
 * app's inspect view's `clax:inspect` with the clip part's inspection
 * (`clax:inspection`), and there only, its `clax:test` with the test part's
 * run of the draft's test files (`clax:tests`). In a frame named
 * `MIGRATE_FRAME` it ends the page's markup before any of it runs, and
 * answers `clax:migrate` with the test part's run of an app bundle's
 * migration (`clax:migration`).
 * The daemon serves it right after the doctype (first in the skeleton's
 * `<head>` for a fragment), and only the document's first bridge tag runs, so
 * `window.claude` exists before any page script; shell
 * orders that read the page's content wait until the document has parsed.
 * While the page holds a `comments.customAnchors` registration, the bridge's
 * own comment mode, anchor resolution, and scroll-to stand down: the page
 * places the pins, and its placements are re-sent on scroll and resize.
 *
 * Comment mode with anchoring and areas, clip rendering, and the
 * capabilities' page-side members are lazy parts (parts/*.ts) imported from
 * beside this script, never before the page has parsed: comment mode on the
 * first shell order that needs it (in practice right after the welcome),
 * clips when comment mode turns on, on the page's first claude.use() of an
 * origin capability the `origin` part (the site's client, those
 * capabilities' members and the room's socket), on a framed page's first
 * claude.use() of a shell capability the `caps` part (an unframed page gets
 * null for those without loading anything), and `room` and `sample` each a
 * part of their own once granted. A part that cannot load is reported to the
 * shell once (clax:degraded). Waiting for the parse keeps the parts' module
 * loads from ever coming before the page's own import maps; the shell hears
 * of the first need that waits for it and of the parse's end (clax:wait),
 * so a page slow to parse can be named as the wait.
 */
import { loadParts } from "clax-bridge-parts";
import { now } from "../../shell/src/clock";
import type { Resolved } from "./anchor";
import { acceptFromShell, forwardedKey, shellOrigins } from "./channel";
import { commentsContext } from "./comments-context";
import { type ErrorRing, installErrorRing } from "./inspect";
import { hashFor, helloFor, isFirstBridge, readMeta } from "./meta";
import { followInPlace, linkToHandOver } from "./nav";
import type { CommentPart, Parts } from "./parts/types";
import { type Clock, retrying } from "./part-loader";
import { PickFlow } from "./pick";
import { type Anchor, type AnchorResult, type Box, type BridgeToShell, INSPECT_FRAME, MIGRATE_FRAME } from "./protocol";
import { Rpc } from "./rpc";
import type { Routes } from "./routes";
import { whenParsed } from "./parsed";
import { makeUse } from "./use";
import { ORIGIN_CAPABILITIES } from "./capabilities";

/** How long the focus outline flashes after a scroll-to (`showFocus` with
 * `flash` in comment-mode.ts: two 0.9 s pulses). */
const FOCUS_FLASH_MS = 1800;

/** How often, at most, anchors are resolved again while the page scrolls or resizes. */
const REFLOW_MS = 50;

type Mode = InstanceType<CommentPart["CommentMode"]>;
type Cache = InstanceType<CommentPart["AnchorCache"]>;
type Hook = InstanceType<CommentPart["LiveHook"]>;
/** Comment mode and the live layers' hand-off once their part has loaded. */
type Live = { part: CommentPart; mode: Mode; layer: Hook; cache(): Cache; reset(): void };
type PartName = keyof Parts;

(() => {
  // One bridge per document: any bridge tag after the document's first one
  // (a copy the page carried in, or a second injection) stands down.
  const script = document.currentScript as HTMLScriptElement | null;
  if (!isFirstBridge(script)) return;
  // In the app's migration view the page runs none of its own markup: the
  // bridge, its first script, turns the rest of the document into hidden
  // text before the parser reaches any of it, so only the migration writes.
  const migrateFrame = window.parent !== window && window.name === MIGRATE_FRAME;
  if (migrateFrame && document.readyState === "loading") document.write("<plaintext hidden>");
  const meta = readMeta(script);
  (window as any).__clax = meta;
  commentsContext.version = meta.version;
  commentsContext.file = meta.file;
  // The parts' URLs are fixed now, before any page script could change the
  // tag. No part is requested before the page has parsed; the listener for
  // that goes on now, ahead of the page's own.
  const loaders = loadParts(script?.src || location.href);
  let parsed = document.readyState !== "loading";
  const whenDone = new Promise<void>(resolve => whenParsed(document, () => { parsed = true; resolve(); }));
  // The timers and clock as they are now: a page replacing them later can
  // only hold up or hurry its own parts. (`now` is the shell's clock module,
  // which a test build lets the browser tests advance.)
  const clock: Clock = { now, setTimeout: setTimeout.bind(window), clearTimeout: clearTimeout.bind(window) as (t: unknown) => void };

  // The shell's window as it is when the bridge loads: a page script that
  // later replaces `window.parent` can neither read nor alter what the
  // bridge posts, nor pose as the shell.
  const shellWin = window.parent;
  const framed = shellWin !== window;
  // In a frame the shell named for inspection, the page's errors from its
  // first script on (`inspect.ts`); the observer the settle wait uses, as it
  // is now.
  const inspectFrame = framed && window.name === INSPECT_FRAME;
  const ring: ErrorRing = inspectFrame ? installErrorRing(window) : [[], []];
  const Observer = MutationObserver;
  let shellOrigin: string | null = null;
  const post = (m: BridgeToShell, transfer: Transferable[] = []) =>
    shellWin.postMessage(m, shellOrigin ?? "*", transfer);
  /** Reports a failed attempt to load `part`: to the shell once it has
   * welcomed this page (every part is first asked for after that). */
  const failed = (part: PartName) => (e: unknown) => {
    console.warn(`clax: the ${part} part of the bridge could not load`, e);
    if (framed && shellOrigin !== null) post({ type: "clax:degraded", part, message: e instanceof Error ? e.message : String(e) });
  };
  // Each part loads on need, never before the page has parsed (so never
  // before the page's own import maps), within a time limit, and again on a
  // later need after a failure (part-loader.ts). The shell hears of the
  // first need that waits for the parse, and of the parse's end
  // (`clax:wait`).
  let waited = false;
  const wait = () => {
    if (waited || shellOrigin === null) return;
    waited = true;
    post({ type: "clax:wait", waiting: true });
    void whenDone.then(() => post({ type: "clax:wait", waiting: false }));
  };
  const onNeed = <T>(name: PartName, load: (attempt: number) => Promise<T>) => {
    const next = retrying(name, load, failed(name), clock);
    return () => parsed ? next() : (wait(), whenDone.then(next));
  };
  const parts = { comment: onNeed("comment", loaders.comment), clip: onNeed("clip", loaders.clip), caps: onNeed("caps", loaders.caps), origin: onNeed("origin", loaders.origin), room: onNeed("room", loaders.room), sample: onNeed("sample", loaders.sample), live: onNeed("live", loaders.live) };
  const clips = () => parts.clip();
  const rpc = new Rpc(m => post(m));
  // The origin's capabilities load the `origin` part, framed or not; the
  // shell's load the `caps` part, framed only: unframed they are null at once.
  const use = makeUse(name => ORIGIN_CAPABILITIES.includes(name)
    ? parts.origin().then(o => o.originUse(name, framed && rpc, parts.room))
    : framed ? parts.caps().then(c => c.resolveUse(name, { rpc, sample: parts.sample, env: { ctx: commentsContext, clip: clips } })) : null);

  // The URL of this artifact's asset `id`, as `assets` gives it: assets are
  // served per artifact, and a page loads only its own.
  const assetUrl = (id: unknown) => `/_blob/${meta.artifact}/${encodeURIComponent(String(id))}`;

  try {
    Object.defineProperty(window, "claude", {
      value: Object.freeze({ use, assetUrl }),
      writable: false,
      configurable: false,
      enumerable: true,
    });
  } catch (e) {
    // A page that already defined a non-configurable window.claude wins.
    console.warn("clax: could not install window.claude", e);
  }

  if (!framed) return; // opened directly: there is no shell

  // The tag names the shell's origin (`data-origin`): the page's own URL,
  // on its site, does not.
  const origins = shellOrigins(script?.dataset.origin);
  const box = (t: Element | Range): Box => { const r = t.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; };

  /** Where a resolved anchor is now: an area's rectangle projected onto its
   * element, else the range or element. */
  const placeOf = (l: Live, anchor: Anchor, r: Resolved): Box => {
    if (anchor.kind === "area" && anchor.area) return l.part.placeArea(anchor, r.element);
    return box(r.range ?? r.element);
  };

  // Anchors are resolved once per shell request; scroll and resize only
  // re-measure, unless the DOM under a resolved element changed since.
  let anchors: { id: string; anchor: Anchor; sameVersion?: boolean; highlight?: boolean }[] = [];
  let latestResolve: unknown = null;
  // The thread the shell focuses (hovered in its list, or selected); its
  // area, if it is one, is outlined dashed.
  let focusId: string | null = null;
  // The shell's comment mode; the bridge's own hit testing follows it unless
  // a custom-anchors registration is live.
  let shellMode = false;
  let welcomed = false;
  // Welcomed by the Clax app's shell, whose host attests the viewer's finger:
  // a hold-then-drag picks on its release, with no chip to confirm it.
  let appShell = false;
  // The page's own pushState routes, watched once the app shell welcomed.
  let routes: Routes | undefined;

  // A pick's clip is rendered only once the shell's composer for it has
  // focus (see `pick.ts`).
  const picks = new PickFlow(window, post, meta.version);

  let live: Promise<Live> | null = null;
  /** Runs `f` with comment mode once its part has loaded; calls run in the
   * order they were made. */
  const withComment = (f: (l: Live) => void) => {
    if (!live) {
      const loading: Promise<Live> = parts.comment().then(part => {
        // One cache for resolving and focusing. A detached thread is retried
        // when the page changes (content rendered late), not only on scroll or resize.
        let resolutions: Cache | null = null;
        // The live layers (spec 2026-10-11-clax-dev-hosted-design §16.3):
        // this viewer's pointer and selection out, other people's in, from
        // a part of their own loaded on need.
        const layer: Hook = new part.LiveHook(document, post, clock, parts.live);
        const mode: Mode = new part.CommentMode(document, {
          hover: t => post({ type: "clax:hover", selector: t ? part.cssPath(t instanceof Element ? t : part.blockAncestor(t.commonAncestorContainer, window)) : null, rect: t ? box(t) : null }),
          pickElement: el => { void picks.start(part.buildElementAnchor(document, el, meta.file), () => clips().then(c => c.renderTargetClip(el)), part.desc(el)).finally(() => mode.captured()); },
          pickRange: r => { void picks.start(part.buildRangeAnchor(document, r, meta.file), () => clips().then(c => c.renderTargetClip(r))).finally(() => mode.captured()); },
          // The clip is the drawn rectangle, placed on its element at release,
          // cropped out of a render of that element. The rectangle stays drawn
          // (as capturing) until the pick is posted.
          pickArea: r => {
            const el = part.containingElement(document, r);
            const anchor = part.buildAreaAnchor(document, r, meta.file, el);
            const at = part.areaBox(anchor.area!, part.boxOf(el));
            void picks.start(anchor, () => clips().then(c => c.renderAreaClip(el, at))).finally(() => mode.captured());
          },
          cancel: () => { mode.set(false); post({ type: "clax:cancel" }); },
          live: layer,
        }, { confirmDrag: () => !appShell });
        commentsContext.liveChanged = () => mode.set(shellMode && !commentsContext.live);
        return {
          part,
          mode,
          layer,
          cache: () => resolutions ??= new part.AnchorCache(document, undefined, meta.file, () => reflow()),
          reset: () => resolutions?.reset(),
        };
      });
      // A failed load is reported by the loader; the next need tries again.
      loading.catch(() => { if (live === loading) live = null; });
      live = loading;
    }
    live.then(f, () => {});
  };

  /** The pending steps of the last scroll-to's flash. */
  const flashTimers: ReturnType<typeof setTimeout>[] = [];
  const updateFocus = (l: Live, flash = false) => {
    const f = focusId === null || commentsContext.live ? undefined : anchors.find(a => a.id === focusId);
    const r = f?.anchor.kind === "area" ? l.cache().resolve(f.id, f.anchor, f.sameVersion === true) : null;
    l.mode.showFocus(f && r ? placeOf(l, f.anchor, r) : null, flash);
  };
  const resolveAll = (l: Live, requestId: string | null) => {
    const resolved = l.cache();
    const results: AnchorResult[] = anchors.map(({ id, anchor, sameVersion }) => {
      const r = resolved.resolve(id, anchor, sameVersion === true);
      return r ? { id, found: true, method: r.method, rect: placeOf(l, anchor, r) } : { id, found: false, method: null, rect: null };
    });
    // Open threads' quotes stay highlighted in the page (spec §16.3).
    l.layer.quotes(anchors, resolved);
    post({ type: "clax:anchors", requestId, results });
    updateFocus(l);
  };
  let reflowing = false;
  // Anchors are resolved again at most every REFLOW_MS of scrolling or
  // resizing, by a timer: a document the browser treats as hidden (the Clax
  // app's WebView, while the app shows it) runs no animation frames.
  const reflow = () => {
    // While the page anchors threads itself (comments.customAnchors), it
    // re-reports its placements, and the shell's anchors are not resolved.
    commentsContext.reflow?.();
    if (commentsContext.live || !anchors.length || reflowing) return;
    reflowing = true;
    setTimeout(() => { reflowing = false; withComment(l => resolveAll(l, null)); }, REFLOW_MS);
  };
  addEventListener("scroll", reflow, { passive: true, capture: true });
  addEventListener("resize", reflow);

  /** Turns the bridge's comment mode to follow the shell's. The clip part
   * starts loading once comment mode is on, so a pick's screenshot does not
   * wait for it, and a page whose comment part cannot load reports that, not
   * the clip part. Comment mode that never loaded is off already. */
  const followMode = () => {
    if (shellMode || live) withComment(l => {
      l.mode.set(shellMode && !commentsContext.live);
      if (shellMode) void clips().catch(() => {});
    });
  };

  /** Once the page has parsed after the shell's welcome, comment mode loads
   * and is set up (still off), in a task of its own, so it is ready when the
   * viewer turns it on. A part that cannot load is reported then, not only
   * when the viewer presses Comment. */
  let prefetched = false;
  const prefetch = () => {
    if (prefetched) return;
    prefetched = true;
    whenParsed(document, () => setTimeout(() => withComment(() => {})));
  };

  // Bubble phase on the window: the page's own handlers run first, and comment
  // mode's capture-phase handler stops a click before it gets here.
  addEventListener("click", e => {
    const link = linkToHandOver(e, { welcomed, pageUrl: location.href, file: meta.file });
    if (!link) return;
    e.preventDefault();
    if (link.kind === "page") post({ type: "clax:navigate", file: link.file, ...(link.hash ? { hash: link.hash } : {}) });
    else followInPlace(link.hash, location);
  });

  // The shell keeps its address bar's fragment in step with the page's.
  addEventListener("hashchange", () => { if (welcomed) post(hashFor(location.hash)); });

  addEventListener("message", e => {
    const m = acceptFromShell(e, shellWin, origins);
    if (!m) return;
    shellOrigin = e.origin;
    switch (m.type) {
      case "clax:welcome":
        welcomed = true; appShell = m.app === true; shellMode = m.mode === "comment"; followMode(); rpc.connect(); post(hashFor(location.hash));
        prefetch();
        if (m.app) void parts.caps().then(c => { routes = c.app(window, post, meta.file, m.restoreScroll); }, () => {});
        break;
      case "clax:use-result": case "clax:call-result": case "clax:event": rpc.accept(m); break;
      case "clax:comment-mode": shellMode = m.on; followMode(); break;
      // In place, with no entry of its own, and only ever its fragment: the
      // URL up to `#` is the page's own (absolute, so no <base> moves it).
      case "clax:hash": location.replace(location.href.split("#")[0] + "#" + m.hash.slice(1)); break;
      case "clax:greet": post({ ...helloFor(meta), again: true }); routes?.report(); break;
      case "clax:back": routes?.back(m.depth); break;
      case "clax:resolve-anchors": {
        if (commentsContext.live) break;
        // The anchors take effect (for reflows too) once the page has parsed,
        // and only the latest request's.
        latestResolve = m;
        whenParsed(document, () => {
          // No threads, and none resolved before: answered without loading
          // comment mode (nothing is focused or outlined either).
          if (!m.anchors.length && !live) {
            if (latestResolve !== m || commentsContext.live) return;
            anchors = [];
            post({ type: "clax:anchors", requestId: m.requestId, results: [] });
            return;
          }
          withComment(l => {
            if (latestResolve !== m || commentsContext.live) return;
            anchors = m.anchors; l.reset(); resolveAll(l, m.requestId);
          });
        });
        break;
      }
      case "clax:scroll-to": whenParsed(document, () => withComment(l => {
        if (commentsContext.live) return;
        // A later scroll-to replaces an earlier one's flash.
        for (const t of flashTimers.splice(0)) clearTimeout(t);
        const r = l.part.resolveAnchor(document, m.anchor, undefined, meta.file, undefined, m.sameVersion === true);
        if (!r) return;
        const behavior: ScrollBehavior = self.matchMedia?.("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
        if (m.anchor.kind === "area" && m.anchor.area) {
          // The drawn area is centred, not its element (often far taller).
          const a = placeOf(l, m.anchor, r);
          scrollBy({ left: a.x + a.w / 2 - innerWidth / 2, top: a.y + a.h / 2 - innerHeight / 2, behavior });
          // After its flash the outline follows the focused thread again:
          // it stays only while the shell focuses this thread.
          flashTimers.push(setTimeout(() => {
            l.mode.showFocus(placeOf(l, m.anchor, r), true);
            flashTimers.push(setTimeout(() => updateFocus(l), FOCUS_FLASH_MS));
          }, 350));
        } else {
          r.element.scrollIntoView({ block: "center", behavior });
          flashTimers.push(setTimeout(() => l.mode.flash(r.range ?? r.element), 350));
        }
      })); break;
      // The focused thread's area is outlined once the page has parsed.
      // Only a resolved thread can be outlined, so comment mode not yet loaded has none to show.
      case "clax:focus": focusId = typeof m.id === "string" ? m.id : null; whenParsed(document, () => { if (live) withComment(l => updateFocus(l)); }); break;
      // Other people's pointers and selections, drawn once the page has parsed.
      // (The comment part loads only once the page has parsed.)
      case "clax:peers": withComment(l => l.layer.peers(m)); break;
      case "clax:pick-refused": if (typeof m.pickId === "string") picks.answer(m.pickId, false); break;
      case "clax:composer-ready": if (typeof m.pickId === "string") picks.answer(m.pickId, true); break;
      // The app's inspect view asks what the page looks like once it settles
      // (`inspect.ts`), with the clip part; the answer goes to the shell only.
      case "clax:inspect": void clips().then(c => c.inspect(window, ring, m, post, clock, Observer), () => {}); break;
      // The inspect view asks the draft's page to run its test files
      // (`harness.ts`), with the test part, only in the frame it named.
      case "clax:test": if (inspectFrame && typeof m.requestId === "string") {
        const requestId = m.requestId;
        void parts.test().then(t => t.test(window, m, post, clock), err => post({ type: "clax:tests", requestId, error: `the test harness did not load: ${err instanceof Error ? err.message : String(err)}` }));
      } break;
      // And, only in the migration view's frame, an app bundle's migration
      // (`harness.ts`), the same way.
      case "clax:migrate": if (migrateFrame && typeof m.requestId === "string") {
        const requestId = m.requestId;
        void parts.test().then(t => t.migrate(window, m, post, clock), err => post({ type: "clax:migration", requestId, ok: false, error: `the test harness did not load: ${err instanceof Error ? err.message : String(err)}` }));
      } break;
      case "clax:key": {
        const k = forwardedKey(m);
        // Keys count only in comment mode, which loads as it turns on.
        if (k && live) withComment(l => l.mode.key(k.key, k.down));
        break;
      }
    }
  });
  // Before the frame shows another document (a link to another site, say),
  // so the shell sends this page's data to nothing that comes next. A page
  // kept whole in the back/forward cache (`persisted`, with the shell around
  // it) comes back as it was, so it says nothing.
  addEventListener("pagehide", e => { if (!e.persisted) post({ type: "clax:bye" }); });
  post(helloFor(meta));
})();
