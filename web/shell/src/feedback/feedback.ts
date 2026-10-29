// clax.dev reports problems with itself through its own report queue, the
// way any product embeds the Clax overlay (spec 2026-10-29-feedback-loop
// §10.3; docs/feedback.md shows this file as the worked example). Nothing
// here reaches past the overlay's public surface: the overlay is loaded from
// the instance (`<instance>/_clax/overlay.js`, which defines `window.Clax`),
// the reporter's token comes from the product's own backend route
// (`POST /_app/feedback/token`), and what a report says of the product comes
// from the product's own config route (`GET /_app/feedback/config`).
//
// Only types are imported from the overlay (its published `overlay.d.ts`);
// no overlay code is bundled into the shell.
import type { HostContext, OverlayHandle, ReporterAuth, ReporterToken } from "../../../overlay/overlay";
import { type AppView, type ClaxGlobal, type FeedbackConfig, TOKEN_PATH } from "./config";

export { type AppView, type ClaxGlobal, type FeedbackConfig, CONFIG_PATH, TOKEN_PATH } from "./config";

/** The overlay's `auth`: asks the shell's backend for a reporter token for
 * the overlay's key. The session cookie says who the reporter is; the
 * backend signs the mint with the queue's service grant. */
export function tokenAuth(f: typeof fetch = fetch): ReporterAuth {
  return async (publicKey: string): Promise<ReporterToken> => {
    const r = await f(TOKEN_PATH, {
      method: "POST",
      credentials: "same-origin",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ public_key: publicKey }),
    });
    if (!r.ok) {
      let why = `${r.status}`;
      try { why = ((await r.json()) as { error?: { code?: string } })?.error?.code ?? why; } catch { /* the status says it */ }
      throw new Error(`the feedback token was refused (${why})`);
    }
    const t = (await r.json()) as ReporterToken;
    if (typeof t?.token !== "string" || typeof t?.expires_at !== "string") throw new Error("the feedback token answer has no token");
    return t;
  };
}

/** The shell view at `pathname`: an artifact's (`/a/<id>/…`), a gallery
 * page's (`/queues/…` is `queues`), or the gallery's. */
export function viewOf(pathname: string): AppView {
  const a = /^\/a\/([^/]+)/.exec(pathname);
  if (a) {
    let id = a[1];
    try { id = decodeURIComponent(id); } catch { /* kept as written */ }
    return { view: "artifact", artifact: id };
  }
  const page = /^\/([a-z]+)/.exec(pathname)?.[1];
  return { view: page ?? "gallery", artifact: null };
}

/** The overlay's `context`, asked when a capture starts: the release and
 * flags the page loaded with (from the config: the release the person ran,
 * not one deployed since), and the app's view and artifact at that moment. */
export function hostContext(cfg: FeedbackConfig, where: () => AppView): () => HostContext {
  return () => {
    const app = cfg.context.app && typeof cfg.context.app === "object" ? cfg.context.app as Record<string, unknown> : {};
    return { release: cfg.context.release, flags: cfg.context.flags, app: { ...app, ...where() } };
  };
}

/** Loads `<instance>/_clax/overlay.js` into `doc` once and answers its
 * global; a page that has it already answers that. */
export function loadOverlay(doc: Document, instance: string): Promise<ClaxGlobal> {
  const win = doc.defaultView as (Window & { Clax?: ClaxGlobal }) | null;
  if (win?.Clax) return Promise.resolve(win.Clax);
  return new Promise((resolve, reject) => {
    const s = doc.createElement("script");
    s.src = `${instance}/_clax/overlay.js`;
    s.async = true;
    s.dataset.claxIgnore = "";
    s.onload = () => (win?.Clax ? resolve(win.Clax) : reject(new Error("overlay.js defined no Clax")));
    s.onerror = () => reject(new Error(`could not load ${s.src}`));
    doc.head.append(s);
  });
}

/** Whether this browser runs on an Apple platform, whose shortcuts use ⌘. */
export function applePlatform(nav: Pick<Navigator, "platform" | "userAgent"> = navigator): boolean {
  return /Mac|iPhone|iPad|iPod/.test(nav.platform || nav.userAgent || "");
}

/** The shortcut that starts a report: Ctrl+Shift+B, or ⌘⇧B on Apple
 * platforms. `keys` are the keys sheet's, `label` the menu's, `aria` the
 * menu item's `aria-keyshortcuts`. */
export function shortcut(apple: boolean): { keys: string[]; label: string; aria: string } {
  return apple
    ? { keys: ["⌘", "Shift", "B"], label: "⌘⇧B", aria: "Meta+Shift+B" }
    : { keys: ["Ctrl", "Shift", "B"], label: "Ctrl+Shift+B", aria: "Control+Shift+B" };
}

export type ShortcutKey = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey" | "isComposing" | "repeat" | "defaultPrevented" | "target">;

function typing(t: EventTarget | null): boolean {
  if (!t || typeof (t as Element).closest !== "function") return false;
  const el = t as HTMLElement;
  return el.localName === "input" || el.localName === "textarea" || el.localName === "select" || el.isContentEditable || el.contentEditable === "true" || !!el.closest("[contenteditable=''], [contenteditable='true']");
}

/** Whether `e` is the report shortcut: Mod+Shift+B, with Mod ⌘ on Apple
 * platforms and Ctrl elsewhere, and no other modifier, not typed into a
 * text field or inside a modal dialog, not repeated, not composed. */
export function isShortcut(e: ShortcutKey, apple: boolean): boolean {
  if (e.defaultPrevented || e.isComposing || e.repeat || e.altKey || !e.shiftKey) return false;
  if (apple ? !e.metaKey || e.ctrlKey : !e.ctrlKey || e.metaKey) return false;
  if (e.key !== "B" && e.key !== "b") return false;
  if (typing(e.target)) return false;
  return !(e.target instanceof Element && e.target.closest("[aria-modal=true]"));
}

/** A running feedback integration: the overlay, with no launcher of its
 * own; the shell's menu and shortcut drive it. */
export class Feedback {
  /** How many of the person's reports wait on them (the overlay's `badge`). */
  badge = 0;
  readonly shortcut: { keys: string[]; label: string; aria: string };
  private readonly listeners = new Set<(n: number) => void>();
  private readonly unBadge: () => void;
  private readonly onKey: (e: KeyboardEvent) => void;

  constructor(private readonly handle: OverlayHandle, private readonly win: Window, apple: boolean) {
    this.shortcut = shortcut(apple);
    this.unBadge = handle.on("badge", n => {
      this.badge = n;
      for (const fn of [...this.listeners]) fn(n);
    });
    this.onKey = e => {
      if (!isShortcut(e, apple)) return;
      e.preventDefault();
      this.report();
    };
    win.addEventListener("keydown", this.onKey);
  }

  /** Starts a report: the overlay's mode bar. */
  report(): void { this.handle.report(); }
  /** Opens the person's reports. */
  showReports(): void { this.handle.showReports(); }
  /** Hears each badge count, the current one at once; the result stops it. */
  onBadge(fn: (n: number) => void): () => void {
    this.listeners.add(fn);
    fn(this.badge);
    return () => { this.listeners.delete(fn); };
  }
  /** Removes the shortcut and the overlay. */
  close(): void {
    this.win.removeEventListener("keydown", this.onKey);
    this.unBadge();
    this.listeners.clear();
    this.handle.close();
  }
}

export interface StartDeps {
  doc?: Document;
  fetch?: typeof fetch;
  /** Where the person is in the shell; default from the page's address. */
  where?: () => AppView;
  /** Loads the overlay's global; default `loadOverlay`. */
  load?: (doc: Document, instance: string) => Promise<ClaxGlobal>;
  apple?: boolean;
}

/** Starts the overlay for `cfg`: loads it from the instance, starts its
 * diagnostics recorder at once (so what goes wrong from here on is kept,
 * before the overlay itself mounts), then mounts it with no launcher,
 * the shell's token route as its `auth` and `hostContext` as its context.
 * It passes no `nonce`: the shell's pages restrict no style or script
 * source (their policy is `frame-ancestors 'none'`) and use no web fonts,
 * so the overlay's styles and clips trip nothing. A product whose
 * `style-src` is strict passes its nonce here. */
export async function startFeedback(cfg: FeedbackConfig, deps: StartDeps = {}): Promise<Feedback> {
  const doc = deps.doc ?? document;
  const win = doc.defaultView ?? window;
  const f = deps.fetch ?? ((...a: Parameters<typeof fetch>) => win.fetch(...a));
  const Clax = await (deps.load ?? loadOverlay)(doc, cfg.instance);
  Clax.diagnostics.install(win);
  const where = deps.where ?? (() => viewOf(win.location.pathname));
  const handle = Clax.overlay({
    instance: cfg.instance,
    queue: cfg.queue,
    auth: tokenAuth(f),
    launcher: false,
    context: hostContext(cfg, where),
  }, doc);
  return new Feedback(handle, win, deps.apple ?? applePlatform(win.navigator));
}
