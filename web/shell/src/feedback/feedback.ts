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
      let code: string | undefined;
      try { code = ((await r.json()) as { error?: { code?: string } })?.error?.code; } catch { /* the status says it */ }
      // The route's code rides on the error: `invalid_key` has the overlay
      // drop its kept key and ask once with a fresh one.
      throw Object.assign(new Error(`the feedback token was refused (${code ?? r.status})`), { code });
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

/** The key that starts a report: F, as the shell's other keys are single
 * letters (C, ?). `keys` are the keys sheet's, `label` the menu's, `aria`
 * the menu item's `aria-keyshortcuts`. */
export const SHORTCUT = { keys: ["F"], label: "F", aria: "F" } as const;

export type ShortcutKey = Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "isComposing" | "repeat" | "defaultPrevented" | "target" | "composedPath">;

/** What a key is typed into: a form field, editable content, or a custom
 * text field or combo box. */
const TYPING = "input, textarea, select, [contenteditable]:not([contenteditable=false]), [role=textbox], [role=combobox]";

/** Whether `e` is the report key: F (either case, or the key in F's place,
 * `KeyF`, on a layout where it types another letter) with no modifier but
 * Shift, not repeated or composed, not already handled, and not typed into a
 * text field (the event's innermost target, through open shadow roots) or
 * inside a dialog. */
export function isShortcut(e: ShortcutKey): boolean {
  if (e.defaultPrevented || e.isComposing || e.repeat || e.altKey || e.ctrlKey || e.metaKey) return false;
  if (e.key !== "f" && e.key !== "F" && e.code !== "KeyF") return false;
  const t = (typeof e.composedPath === "function" ? e.composedPath()[0] : null) ?? e.target;
  if (!(t instanceof Element)) return true;
  if ((t as HTMLElement).isContentEditable || t.closest(TYPING)) return false;
  return !t.closest("[role=dialog], [aria-modal=true]");
}

/** A running feedback integration: the overlay, with no launcher of its
 * own; the shell's menu and key drive it. */
export class Feedback {
  /** How many of the person's reports wait on them (the overlay's `badge`). */
  badge = 0;
  readonly shortcut = SHORTCUT;
  private readonly listeners = new Set<(n: number) => void>();
  private readonly unBadge: () => void;
  private readonly onKey: (e: KeyboardEvent) => void;

  constructor(private readonly handle: OverlayHandle, private readonly win: Window) {
    this.unBadge = handle.on("badge", n => {
      this.badge = n;
      for (const fn of [...this.listeners]) fn(n);
    });
    this.onKey = e => {
      if (!isShortcut(e)) return;
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
  /** Forgets the person's reporter key and token (sign-out). */
  forget(): Promise<void> { return this.handle.forget(); }
  /** Removes the key and the overlay. */
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
    ...(cfg.user ? { user: cfg.user } : {}),
    launcher: false,
    context: hostContext(cfg, where),
  }, doc);
  return new Feedback(handle, win);
}
