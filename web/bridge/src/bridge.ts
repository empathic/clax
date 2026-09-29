/**
 * Runtime bridge injected into every published page.
 * It exposes `window.claude.use(name)` and resolves `null` for every
 * capability, so pages written against the claude.ai contract load and
 * degrade correctly. When framed, it greets the shell with `artifax:hello`
 * (target "*", no page data), then takes orders only from `window.parent` at
 * an origin in `shellOrigins(location.href)` and replies to that origin only:
 * comment mode (hover outline, element and range picks with anchors and PNG
 * clips), anchor resolution, and scroll-to (see `protocol.ts`).
 */
import { buildElementAnchor, buildRangeAnchor, cssPath, resolveAnchor } from "./anchor";
import { acceptFromShell, shellOrigins } from "./channel";
import { blockAncestor, renderClip } from "./clip";
import { CommentMode } from "./comment-mode";
import type { Anchor, AnchorResult, Box, BridgeToShell } from "./protocol";

(() => {
  const script = document.currentScript as HTMLScriptElement | null;
  const meta = {
    artifact: script?.dataset.artifact ?? "",
    version: Number(script?.dataset.version ?? "0"),
    contract: script?.dataset.contract ?? "",
  };
  (window as any).__artifax = meta;

  const cache = new Map<string, Promise<null>>();
  function use(name: string): Promise<null> {
    let p = cache.get(name);
    if (!p) {
      p = Promise.resolve().then(() => null);
      cache.set(name, p);
    }
    return p;
  }

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

  if (window.parent === window) return; // opened directly: there is no shell

  const origins = shellOrigins(location.href);
  let shellOrigin: string | null = null;
  const post = (m: BridgeToShell, transfer: Transferable[] = []) =>
    window.parent.postMessage(m, shellOrigin ?? "*", transfer);
  const box = (t: Element | Range): Box => { const r = t.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; };

  let anchors: { id: string; anchor: Anchor }[] = [];
  const resolveAll = (requestId: string | null) => {
    const results: AnchorResult[] = anchors.map(({ id, anchor }) => {
      const r = resolveAnchor(document, anchor);
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

  const pick = async (anchor: Anchor, clipOf: Element) => {
    const pickId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;
    let clipPng: ArrayBuffer | undefined;
    let clipError: string | undefined;
    try { clipPng = await renderClip(clipOf); } catch (e) { clipError = e instanceof Error ? e.message : String(e); }
    post({ type: "artifax:pick", pickId, version: meta.version, anchor, clipPng, clipError }, clipPng ? [clipPng] : []);
  };
  const mode = new CommentMode(document, {
    hover: el => post({ type: "artifax:hover", selector: el ? cssPath(el) : null, rect: el ? box(el) : null }),
    pickElement: el => { void pick(buildElementAnchor(document, el), el); },
    pickRange: r => { void pick(buildRangeAnchor(document, r), blockAncestor(r.commonAncestorContainer, window)); },
    cancel: () => { mode.set(false); post({ type: "artifax:cancel" }); },
  });

  addEventListener("message", e => {
    const m = acceptFromShell(e, window.parent, origins);
    if (!m) return;
    shellOrigin = e.origin;
    switch (m.type) {
      case "artifax:welcome": mode.set(m.mode === "comment"); break;
      case "artifax:comment-mode": mode.set(m.on); break;
      case "artifax:resolve-anchors": anchors = m.anchors; resolveAll(m.requestId); break;
      case "artifax:scroll-to": {
        const r = resolveAnchor(document, m.anchor);
        if (r) { r.element.scrollIntoView({ block: "center", behavior: "smooth" }); setTimeout(() => mode.flash(r.range ?? r.element), 350); }
        break;
      }
    }
  });
  post({ type: "artifax:hello", artifact: meta.artifact, version: meta.version });
})();

