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
 * clips within the budget in `clip.ts`), anchor resolution, and scroll-to (see `protocol.ts`). Every HTML
 * page of a version carries the bridge; anchors it builds name this page's
 * file, and anchors on other files never resolve here. Once welcomed, a plain
 * click on a link to another page of the version that the page did not cancel
 * is cancelled and handed to the shell (`artifax:navigate`), which follows it
 * with one history entry; a link to this page under another spelling of its
 * path (`index.html` for `/v/<n>/`) is followed in place (`followInPlace`). After the welcome and
 * on every `hashchange` it reports the page's fragment (`artifax:hash`).
 */
import { AnchorCache, buildElementAnchor, buildRangeAnchor, cssPath, resolveAnchor } from "./anchor";
import { acceptFromShell, shellOrigins } from "./channel";
import { blockAncestor, renderTargetClip } from "./clip";
import { CommentMode } from "./comment-mode";
import { hashFor, helloFor, readMeta } from "./meta";
import { followInPlace, linkToHandOver } from "./nav";
import type { Anchor, AnchorResult, Box, BridgeToShell } from "./protocol";
import { Rpc } from "./rpc";
import { makeUse } from "./use";

(() => {
  const meta = readMeta(document.currentScript as HTMLScriptElement | null);
  (window as any).__artifax = meta;

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

  // Anchors are resolved once per shell request; scroll and resize only
  // re-measure, unless the DOM under a resolved element changed since.
  let anchors: { id: string; anchor: Anchor }[] = [];
  let resolutions: AnchorCache | null = null;
  const resolveAll = (requestId: string | null) => {
    const resolved = resolutions ??= new AnchorCache(document, undefined, meta.file);
    const results: AnchorResult[] = anchors.map(({ id, anchor }) => {
      const r = resolved.resolve(id, anchor);
      return r ? { id, found: true, method: r.method, rect: box(r.range ?? r.element) } : { id, found: false, method: null, rect: null };
    });
    post({ type: "artifax:anchors", requestId, results });
  };
  let raf = 0;
  const reflow = () => {
    if (!anchors.length || raf) return;
    raf = requestAnimationFrame(() => { raf = 0; resolveAll(null); });
  };
  addEventListener("scroll", reflow, { passive: true, capture: true });
  addEventListener("resize", reflow);

  const pick = async (anchor: Anchor, target: Element | Range) => {
    const pickId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;
    let clipPng: ArrayBuffer | undefined;
    let clipError: string | undefined;
    try { clipPng = await renderTargetClip(target); } catch (e) { clipError = e instanceof Error ? e.message : String(e); }
    post({ type: "artifax:pick", pickId, version: meta.version, anchor, clipPng, clipError }, clipPng ? [clipPng] : []);
  };
  const mode = new CommentMode(document, {
    hover: t => post({ type: "artifax:hover", selector: t ? cssPath(t instanceof Element ? t : blockAncestor(t.commonAncestorContainer, window)) : null, rect: t ? box(t) : null }),
    pickElement: el => { void pick(buildElementAnchor(document, el, meta.file), el); },
    pickRange: r => { void pick(buildRangeAnchor(document, r, meta.file), r); },
    cancel: () => { mode.set(false); post({ type: "artifax:cancel" }); },
  });

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
      case "artifax:welcome": welcomed = true; mode.set(m.mode === "comment"); rpc.connect(); post(hashFor(location.hash)); break;
      case "artifax:use-result": case "artifax:call-result": case "artifax:event": rpc.accept(m); break;
      case "artifax:comment-mode": mode.set(m.on); break;
      case "artifax:resolve-anchors": anchors = m.anchors; resolutions?.reset(); resolveAll(m.requestId); break;
      case "artifax:scroll-to": {
        const r = resolveAnchor(document, m.anchor, undefined, meta.file);
        if (r) { r.element.scrollIntoView({ block: "center", behavior: "smooth" }); setTimeout(() => mode.flash(r.range ?? r.element), 350); }
        break;
      }
    }
  });
  post(helloFor(meta));
})();

