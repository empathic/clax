/**
 * Runtime bridge injected into every published page.
 * It exposes `window.claude.use(name)`, which resolves the frozen namespace of
 * each capability the shell grants and `null` for the rest (see `use.ts`);
 * unframed, every name resolves `null`. Capability calls and the shell's
 * answers travel as `clax:use`/`call`/`event` messages (see `protocol.ts`),
 * queued until the shell's welcome. When framed, it greets the shell with `clax:hello`
 * (target "*"; artifact, version, and the page's published file, no page
 * content), then takes orders only from `window.parent` at
 * an origin in `shellOrigins(location.href)` and replies to that origin only:
 * comment mode (hover outline, element and range picks with anchors and PNG
 * clips within the budget in `clip.ts`, drawn areas clipped to exactly the
 * rectangle, rendered only once the shell's composer for the pick has focus,
 * Option widening with keys the shell forwards), anchor
 * resolution (an area's rectangle projected onto its element's current box),
 * the dashed outline of the focused thread's area, and scroll-to (see `protocol.ts`). Every HTML
 * page of a version carries the bridge; anchors it builds name this page's
 * file, and anchors on other files never resolve here. Once welcomed, a plain
 * click on a link to another page of the version that the page did not cancel
 * is cancelled and handed to the shell (`clax:navigate`), which follows it
 * with one history entry; a link to this page under another spelling of its
 * path (`index.html` for `/v/<n>/`) is followed in place (`followInPlace`). After the welcome and
 * on every `hashchange` it reports the page's fragment (`clax:hash`). On
 * `pagehide` it says `clax:bye`, and the shell stops sending until the next
 * document greets.
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
 * clips when comment mode turns on, a capability's members on the first
 * claude.use() of it the shell grants (`room` and `sample` each in a part of
 * its own, the rest in `caps`). A part that cannot load is reported to the
 * shell once (clax:degraded). Waiting for the parse keeps the parts' module
 * loads from ever coming before the page's own import maps.
 */
import { loadParts } from "clax-bridge-parts";
import type { Resolved } from "./anchor";
import { acceptFromShell, forwardedKey, shellOrigins } from "./channel";
import { commentsContext } from "./comments-context";
import { hashFor, helloFor, isFirstBridge, readMeta } from "./meta";
import { followInPlace, linkToHandOver } from "./nav";
import type { CommentPart, Parts } from "./parts/types";
import { type Clock, retrying } from "./part-loader";
import { PickFlow } from "./pick";
import type { Anchor, AnchorResult, Box, BridgeToShell } from "./protocol";
import { Rpc } from "./rpc";
import { whenParsed } from "./parsed";
import { makeUse } from "./use";

type Mode = InstanceType<CommentPart["CommentMode"]>;
type Cache = InstanceType<CommentPart["AnchorCache"]>;
/** Comment mode once its part has loaded. */
type Live = { part: CommentPart; mode: Mode; cache(): Cache; reset(): void };
type PartName = keyof Parts;

(() => {
  // One bridge per document: any bridge tag after the document's first one
  // (a copy the page carried in, or a second injection) stands down.
  const script = document.currentScript as HTMLScriptElement | null;
  if (!isFirstBridge(script)) return;
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
  // only hold up or hurry its own parts.
  const clock: Clock = { now: performance.now.bind(performance), setTimeout: setTimeout.bind(window), clearTimeout: clearTimeout.bind(window) as (t: unknown) => void };

  // The shell's window as it is when the bridge loads: a page script that
  // later replaces `window.parent` can neither read nor alter what the
  // bridge posts, nor pose as the shell.
  const shellWin = window.parent;
  const framed = shellWin !== window;
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
  // later need after a failure (part-loader.ts).
  const onNeed = <T>(name: PartName, load: (attempt: number) => Promise<T>) => {
    const next = retrying(name, load, failed(name), clock);
    return () => parsed ? next() : whenDone.then(next);
  };
  const parts = { comment: onNeed("comment", loaders.comment), clip: onNeed("clip", loaders.clip), caps: onNeed("caps", loaders.caps), room: onNeed("room", loaders.room), sample: onNeed("sample", loaders.sample) };
  const clips = () => parts.clip();
  const rpc = new Rpc(m => post(m));
  const use = makeUse({
    framed,
    rpc,
    locals: (name, r, config) => name === "room"
      ? parts.room().then(p => p.roomNamespace(r))
      : name === "sample"
        ? parts.sample().then(p => p.sampleNamespace(r))
        : parts.caps().then(c => c.localsFor(name, r, config, { ctx: commentsContext, clip: clips })),
  });

  try {
    Object.defineProperty(window, "claude", {
      value: Object.freeze({ use }),
      writable: false,
      configurable: false,
      enumerable: true,
    });
  } catch (e) {
    // A page that already defined a non-configurable window.claude wins.
    console.warn("clax: could not install window.claude", e);
  }

  if (!framed) return; // opened directly: there is no shell

  const origins = shellOrigins(location.href);
  const box = (t: Element | Range): Box => { const r = t.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; };

  /** Where a resolved anchor is now: an area's rectangle projected onto its
   * element, else the range or element. */
  const placeOf = (l: Live, anchor: Anchor, r: Resolved): Box => {
    if (anchor.kind === "area" && anchor.area) return l.part.placeArea(anchor, r.element);
    return box(r.range ?? r.element);
  };

  // Anchors are resolved once per shell request; scroll and resize only
  // re-measure, unless the DOM under a resolved element changed since.
  let anchors: { id: string; anchor: Anchor; sameVersion?: boolean }[] = [];
  let latestResolve: unknown = null;
  // The thread the shell focuses (hovered in its list, or selected); its
  // area, if it is one, is outlined dashed.
  let focusId: string | null = null;
  // The shell's comment mode; the bridge's own hit testing follows it unless
  // a custom-anchors registration is live.
  let shellMode = false;
  let welcomed = false;

  // A pick's clip is rendered only once the shell's composer for it has
  // focus (see `pick.ts`).
  const picks = new PickFlow(window, post, meta.version);
  const pick = (anchor: Anchor, clip: () => Promise<ArrayBuffer>) => picks.start(anchor, clip);

  let live: Promise<Live> | null = null;
  /** Runs `f` with comment mode once its part has loaded; calls run in the
   * order they were made. */
  const withComment = (f: (l: Live) => void) => {
    if (!live) {
      const loading: Promise<Live> = parts.comment().then(part => {
        // One cache for resolving and focusing. A detached thread is retried
        // when the page changes (content rendered late), not only on scroll or resize.
        let resolutions: Cache | null = null;
        const mode: Mode = new part.CommentMode(document, {
          hover: t => post({ type: "clax:hover", selector: t ? part.cssPath(t instanceof Element ? t : part.blockAncestor(t.commonAncestorContainer, window)) : null, rect: t ? box(t) : null }),
          pickElement: el => { void pick(part.buildElementAnchor(document, el, meta.file), () => clips().then(c => c.renderTargetClip(el))).finally(() => mode.captured()); },
          pickRange: r => { void pick(part.buildRangeAnchor(document, r, meta.file), () => clips().then(c => c.renderTargetClip(r))).finally(() => mode.captured()); },
          // The clip is the drawn rectangle, placed on its element at release,
          // cropped out of a render of that element. The rectangle stays drawn
          // (as capturing) until the pick is posted.
          pickArea: r => {
            const el = part.containingElement(document, r);
            const anchor = part.buildAreaAnchor(document, r, meta.file, el);
            const at = part.areaBox(anchor.area!, part.boxOf(el));
            void pick(anchor, () => clips().then(c => c.renderAreaClip(el, at))).finally(() => mode.captured());
          },
          cancel: () => { mode.set(false); post({ type: "clax:cancel" }); },
        });
        commentsContext.liveChanged = () => mode.set(shellMode && !commentsContext.live);
        return {
          part,
          mode,
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
    post({ type: "clax:anchors", requestId, results });
    updateFocus(l);
  };
  let raf = 0;
  const reflow = () => {
    // While the page anchors threads itself (comments.customAnchors), it
    // re-reports its placements, and the shell's anchors are not resolved.
    commentsContext.reflow?.();
    if (commentsContext.live || !anchors.length || raf) return;
    raf = requestAnimationFrame(() => { raf = 0; withComment(l => resolveAll(l, null)); });
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
        welcomed = true; shellMode = m.mode === "comment"; followMode(); rpc.connect(); post(hashFor(location.hash));
        prefetch();
        break;
      case "clax:use-result": case "clax:call-result": case "clax:event": rpc.accept(m); break;
      case "clax:comment-mode": shellMode = m.on; followMode(); break;
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
        const r = l.part.resolveAnchor(document, m.anchor, undefined, meta.file, undefined, m.sameVersion === true);
        if (!r) return;
        const behavior: ScrollBehavior = self.matchMedia?.("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
        if (m.anchor.kind === "area" && m.anchor.area) {
          // The drawn area is centred, not its element (often far taller).
          const a = placeOf(l, m.anchor, r);
          scrollBy({ left: a.x + a.w / 2 - innerWidth / 2, top: a.y + a.h / 2 - innerHeight / 2, behavior });
          setTimeout(() => l.mode.showFocus(placeOf(l, m.anchor, r), true), 350);
        } else {
          r.element.scrollIntoView({ block: "center", behavior });
          setTimeout(() => l.mode.flash(r.range ?? r.element), 350);
        }
      })); break;
      // The focused thread's area is outlined once the page has parsed.
      // Only a resolved thread can be outlined, so comment mode not yet loaded has none to show.
      case "clax:focus": focusId = typeof m.id === "string" ? m.id : null; whenParsed(document, () => { if (live) withComment(l => updateFocus(l)); }); break;
      case "clax:pick-refused": if (typeof m.pickId === "string") picks.answer(m.pickId, false); break;
      case "clax:composer-ready": if (typeof m.pickId === "string") picks.answer(m.pickId, true); break;
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
