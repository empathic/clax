/**
 * Runtime bridge injected into every published page.
 * It exposes `window.claude.use(name)`, which resolves the frozen namespace of
 * each capability the shell grants and `null` for the rest (see `use.ts`);
 * unframed, every name resolves `null`. Capability calls and the shell's
 * answers travel as `artifax:use`/`call`/`event` messages (see `protocol.ts`),
 * queued until the shell's welcome. When framed, it greets the shell with `artifax:hello`
 * (target "*"; artifact, version, and the page's published file, no page
 * content), then takes orders only from `window.parent` at
 * an origin in `shellOrigins(location.href)` and replies to that origin only:
 * comment mode (hover outline, element and range picks with anchors and PNG
 * clips within the budget in `clip.ts`, drawn areas clipped to exactly the
 * rectangle, Option widening with keys the shell forwards), anchor
 * resolution (an area's rectangle projected onto its element's current box),
 * the dashed outline of the focused thread's area, and scroll-to (see `protocol.ts`). Every HTML
 * page of a version carries the bridge; anchors it builds name this page's
 * file, and anchors on other files never resolve here. Once welcomed, a plain
 * click on a link to another page of the version that the page did not cancel
 * is cancelled and handed to the shell (`artifax:navigate`), which follows it
 * with one history entry; a link to this page under another spelling of its
 * path (`index.html` for `/v/<n>/`) is followed in place (`followInPlace`). After the welcome and
 * on every `hashchange` it reports the page's fragment (`artifax:hash`).
 * While the page holds a `comments.customAnchors` registration, the bridge's
 * own comment mode, anchor resolution, and scroll-to stand down: the page
 * places the pins, and its placements are re-sent on scroll and resize.
 */
import { AnchorCache, type Resolved, buildElementAnchor, buildRangeAnchor, cssPath, resolveAnchor } from "./anchor";
import { areaBox, buildAreaAnchor, containingElement } from "./area";
import { acceptFromShell, shellOrigins } from "./channel";
import { commentsContext } from "./caps/comments";
import { blockAncestor, renderAreaClip, renderTargetClip } from "./clip";
import { CommentMode } from "./comment-mode";
import { hashFor, helloFor, readMeta } from "./meta";
import { followInPlace, linkToHandOver } from "./nav";
import type { Anchor, AnchorResult, Box, BridgeToShell } from "./protocol";
import { Rpc } from "./rpc";
import { makeUse } from "./use";

(() => {
  const meta = readMeta(document.currentScript as HTMLScriptElement | null);
  (window as any).__artifax = meta;
  commentsContext.version = meta.version;
  commentsContext.file = meta.file;

  const framed = window.parent !== window;
  let shellOrigin: string | null = null;
  const post = (m: BridgeToShell, transfer: Transferable[] = []) =>
    window.parent.postMessage(m, shellOrigin ?? "*", transfer);
  const rpc = new Rpc(m => post(m));
  const use = makeUse({ framed, rpc });

  try {
    Object.defineProperty(window, "claude", {
      value: Object.freeze({ use }),
      writable: false,
      configurable: false,
      enumerable: true,
    });
  } catch (e) {
    // A page that already defined a non-configurable window.claude wins.
    console.warn("artifax: could not install window.claude", e);
  }

  if (!framed) return; // opened directly: there is no shell

  const origins = shellOrigins(location.href);
  const box = (t: Element | Range): Box => { const r = t.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; };

  /** Where a resolved anchor is now: an area's rectangle projected onto its
   * element, else the range or element. */
  const placeOf = (anchor: Anchor, r: Resolved): Box => {
    if (anchor.kind === "area" && anchor.area) { const b = r.element.getBoundingClientRect(); return areaBox(anchor.area, { left: b.left, top: b.top, width: b.width, height: b.height }); }
    return box(r.range ?? r.element);
  };

  // Anchors are resolved once per shell request; scroll and resize only
  // re-measure, unless the DOM under a resolved element changed since.
  let anchors: { id: string; anchor: Anchor }[] = [];
  let resolutions: AnchorCache | null = null;
  // The thread the shell focuses (hovered in its list, or selected); its
  // area, if it is one, is outlined dashed.
  let focusId: string | null = null;
  const updateFocus = (flash = false) => {
    const f = focusId === null || commentsContext.live ? undefined : anchors.find(a => a.id === focusId);
    const r = f?.anchor.kind === "area" ? (resolutions ??= new AnchorCache(document, undefined, meta.file)).resolve(f.id, f.anchor) : null;
    mode.showFocus(f && r ? placeOf(f.anchor, r) : null, flash);
  };
  const resolveAll = (requestId: string | null) => {
    const resolved = resolutions ??= new AnchorCache(document, undefined, meta.file);
    const results: AnchorResult[] = anchors.map(({ id, anchor }) => {
      const r = resolved.resolve(id, anchor);
      return r ? { id, found: true, method: r.method, rect: placeOf(anchor, r) } : { id, found: false, method: null, rect: null };
    });
    post({ type: "artifax:anchors", requestId, results });
    updateFocus();
  };
  let raf = 0;
  const reflow = () => {
    // While the page anchors threads itself (comments.customAnchors), it
    // re-reports its placements, and the shell's anchors are not resolved.
    commentsContext.reflow?.();
    if (commentsContext.live || !anchors.length || raf) return;
    raf = requestAnimationFrame(() => { raf = 0; resolveAll(null); });
  };
  addEventListener("scroll", reflow, { passive: true, capture: true });
  addEventListener("resize", reflow);

  const pick = async (anchor: Anchor, clip: () => Promise<ArrayBuffer>) => {
    const pickId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;
    let clipPng: ArrayBuffer | undefined;
    let clipError: string | undefined;
    try { clipPng = await clip(); } catch (e) { clipError = e instanceof Error ? e.message : String(e); }
    post({ type: "artifax:pick", pickId, version: meta.version, anchor, clipPng, clipError }, clipPng ? [clipPng] : []);
  };
  const mode = new CommentMode(document, {
    hover: t => post({ type: "artifax:hover", selector: t ? cssPath(t instanceof Element ? t : blockAncestor(t.commonAncestorContainer, window)) : null, rect: t ? box(t) : null }),
    pickElement: el => { void pick(buildElementAnchor(document, el, meta.file), () => renderTargetClip(el)); },
    pickRange: r => { void pick(buildRangeAnchor(document, r, meta.file), () => renderTargetClip(r)); },
    // The clip starts at once, from the page as it is at release: the drawn
    // rectangle (clamped into its element) cropped out of its element.
    pickArea: r => {
      const el = containingElement(document, r);
      const anchor = buildAreaAnchor(document, r, meta.file, el);
      const b = el.getBoundingClientRect();
      const clip = renderAreaClip(el, areaBox(anchor.area!, { left: b.left, top: b.top, width: b.width, height: b.height }));
      void pick(anchor, () => clip);
    },
    cancel: () => { mode.set(false); post({ type: "artifax:cancel" }); },
  });

  // The shell's comment mode; the bridge's own hit testing follows it unless
  // a custom-anchors registration is live.
  let shellMode = false;
  commentsContext.liveChanged = () => mode.set(shellMode && !commentsContext.live);

  let welcomed = false;
  // Bubble phase on the window: the page's own handlers run first, and comment
  // mode's capture-phase handler stops a click before it gets here.
  addEventListener("click", e => {
    const link = linkToHandOver(e, { welcomed, pageUrl: location.href, file: meta.file });
    if (!link) return;
    e.preventDefault();
    if (link.kind === "page") post({ type: "artifax:navigate", file: link.file, ...(link.hash ? { hash: link.hash } : {}) });
    else followInPlace(link.hash, location);
  });

  // The shell keeps its address bar's fragment in step with the page's.
  addEventListener("hashchange", () => { if (welcomed) post(hashFor(location.hash)); });

  addEventListener("message", e => {
    const m = acceptFromShell(e, window.parent, origins);
    if (!m) return;
    shellOrigin = e.origin;
    switch (m.type) {
      case "artifax:welcome": welcomed = true; shellMode = m.mode === "comment"; mode.set(shellMode && !commentsContext.live); rpc.connect(); post(hashFor(location.hash)); break;
      case "artifax:use-result": case "artifax:call-result": case "artifax:event": rpc.accept(m); break;
      case "artifax:comment-mode": shellMode = m.on; mode.set(shellMode && !commentsContext.live); break;
      case "artifax:resolve-anchors": if (commentsContext.live) break; anchors = m.anchors; resolutions?.reset(); resolveAll(m.requestId); break;
      case "artifax:scroll-to": {
        if (commentsContext.live) break;
        const r = resolveAnchor(document, m.anchor, undefined, meta.file);
        if (!r) break;
        if (m.anchor.kind === "area" && m.anchor.area) {
          // The drawn area is centred, not its element (often far taller).
          const a = placeOf(m.anchor, r);
          scrollBy({ left: a.x + a.w / 2 - innerWidth / 2, top: a.y + a.h / 2 - innerHeight / 2, behavior: "smooth" });
          setTimeout(() => mode.showFocus(placeOf(m.anchor, r), true), 350);
        } else {
          r.element.scrollIntoView({ block: "center", behavior: "smooth" });
          setTimeout(() => mode.flash(r.range ?? r.element), 350);
        }
        break;
      }
      case "artifax:focus": focusId = typeof m.id === "string" ? m.id : null; updateFocus(); break;
      case "artifax:key":
        if ((m.key === "Alt" || m.key === "ArrowUp" || m.key === "ArrowDown") && typeof m.down === "boolean") mode.key(m.key, m.down);
        break;
    }
  });
  post(helloFor(meta));
})();

