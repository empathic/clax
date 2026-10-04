// The artifact view's unit tests share this: a view mounted with stubbed
// fetches, a fake stream worker and frame, and the helpers that drive it.
import { afterEach, beforeEach, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { FakeWorker, artifactStreams } from "./fake-worker";

export async function waitFor<T>(check: () => T | null | undefined | false, what: string): Promise<T> {
  const deadline = Date.now() + 2000;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}

export const ID = "7q3k9mzx2b4t";
/** The view's stream, driven by event name. */
export const FakeES = artifactStreams(ID);
export const artifact = (n: number, files: Record<string, unknown> = {}) => ({ artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "2026-09-28T11:00:00Z", current_version: n, pinned: false, owner_harness: "claude" }, versions: [{ artifact_id: ID, n, label: null, created_at: "x", files }] });
export const page = { content_type: "text/html", size: 1 };

export const viewer = { viewer: { public_id: "u_0123456789abcdef012345", display_name: null, created_at: "x" } };

/** The gesture module the mounted view uses (from the same module registry). */
export let gesture: typeof import("../caps/gesture") | undefined;
/** Every gesture module instance a mount loaded: each watches the document
 * until its `unwatchShell` runs. */
export const gestureModules = new Set<typeof import("../caps/gesture")>();
afterEach(() => { for (const g of gestureModules) g.unwatchShell(); gestureModules.clear(); });
/** Every view a test mounted and left mounted; each is unmounted after its test, so its timers stop. */
export const mountedViews = new Set<() => void>();
afterEach(() => { for (const stop of mountedViews) stop(); mountedViews.clear(); });

/** Answers the comment routes (no threads, an anonymous viewer) unless `comments` is given; everything else goes to `fetchImpl`. */
export async function mountView(fetchImpl: (url: string, init?: RequestInit) => Promise<Response>, comments?: (url: string, init?: RequestInit) => Promise<Response>, file?: string, pinned: number | null = null) {
  vi.stubGlobal("SharedWorker", FakeWorker);
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes("/threads") || url.startsWith("/api/viewers/")) {
      if (comments) return comments(url, init);
      return new Response(JSON.stringify(url.includes("/threads") ? { threads: [], next_cursor: null } : viewer));
    }
    return fetchImpl(url, init);
  }));
  sessionStorage.setItem("clax.origin-ok", "0");
  const { mountArtifactView } = await import("../artifact");
  gesture = await import("../caps/gesture");
  gestureModules.add(gesture);
  const root = document.createElement("div");
  document.body.appendChild(root);
  const view = mountArtifactView(root, { id: ID, pinnedVersion: pinned, file });
  let stopped = false;
  const stop = () => { if (!stopped) { stopped = true; mountedViews.delete(stop); view.unmount(); } };
  mountedViews.add(stop);
  return { ...view, root: view.root, unmount: stop };
}



export function stubMedia(matches: Record<string, boolean>) {
  vi.stubGlobal("matchMedia", (q: string) => ({ matches: matches[q] ?? false, media: q, addEventListener() {}, removeEventListener() {} }));
}

/** Whether Post posts when pressed: enabled, and not waiting for the screenshot. */
export function postReady(root: Element): boolean {
  const b = buttonNamed(root, "Post comment");
  return !b.disabled && b.getAttribute("aria-disabled") !== "true";
}

/** A button's text without its `aria-hidden` parts (a keycap), as its accessible name reads. */
export function nameOf(b: HTMLButtonElement): string {
  const c = b.cloneNode(true) as HTMLElement;
  for (const h of c.querySelectorAll("[aria-hidden=true]")) h.remove();
  return (c.textContent ?? "").trim();
}

export function buttonNamed(root: Element, name: string | RegExp): HTMLButtonElement {
  const b = Array.from(root.querySelectorAll("button")).find(x => (typeof name === "string" ? nameOf(x) === name : name.test(nameOf(x))));
  if (!b) throw new Error(`no button ${name}`);
  return b;
}

/** A message from the content frame as a sandboxed (opaque-origin) bridge sends it. */
export function fromFrame(win: Window, data: unknown) {
  dispatchTrusted(window, new MessageEvent("message", { data, origin: "null", source: win }));
}

/** The viewer's gesture in the content frame: transient activation, with
 * the pointer moved onto the frame and focus into it after the shell's own
 * control. */
export function gestureIn(frame: HTMLIFrameElement, active = true) {
  Object.defineProperty(navigator, "userActivation", { value: { isActive: active }, configurable: true });
  const control = document.createElement("button");
  document.body.appendChild(control);
  gesture!.notePointerOver(control, 900, 10);
  gesture!.notePointerAt(900, 10);
  control.focus();
  gesture!.notePointerOver(frame, 100, 100);
  frame.focus();
  control.remove();
}

/** A pick as the bridge sends one for the viewer's click: its start, with the
 * anchor, while the frame holds the gesture, then the pick. */
export function viewerPick(frame: HTMLIFrameElement, m: { pickId: string; [k: string]: unknown }) {
  gestureIn(frame);
  fromFrame(frame.contentWindow!, startOf(m));
  fromFrame(frame.contentWindow!, m);
}

/** The start the bridge sends for the pick `m`. */
export function startOf(m: { pickId: string; [k: string]: unknown }) {
  return { type: "clax:pick-start", pickId: m.pickId, version: m.version, anchor: m.anchor };
}

export function pick(pickId: string, quote: string) {
  return { type: "clax:pick", pickId, version: 1, anchor: { kind: "element", selector: "body > h2", quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" } };
}

/** A trusted pointer's click (`detail` 1) on `el`. */
export function pointerClick(el: Element): void {
  dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
}


/** The hooks every artifact view test runs under (call inside its describe). */
export function artifactViewHooks(): void {
  // `forgetViewer` comes from the fresh module registry, the same `threads`
  // module the test's `import("../artifact")` then loads.
  beforeEach(async () => { vi.resetModules(); (await import("../threads")).forgetViewer(); FakeES.last = undefined; });
  beforeEach(() => { history.replaceState(null, "", `/a/${ID}`); });
  afterEach(() => { vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); history.replaceState(null, "", "/"); });
}
