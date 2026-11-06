// The SDK package's main entry (`@empathic/clax-client-sdk`, scripts/build-sdk.mjs):
// what a deployer's page needs to file reports into a queue and read the
// replies, safe to ship to a browser. The overlay is `./overlay`; the
// backend's service client, which holds a queue's service grant, is
// `./service` and never ships to a page.
import type { Recorder } from "./diagnostics";
import type { ReporterAuth } from "./reporter";
import type { ReporterToken } from "./types";
// Type-only: erased at build (scripts/build-sdk.mjs), so this entry never
// bundles the overlay's Svelte components, only their public types.
import type { OverlayHandle, ReporterRef } from "../../overlay/src/mount";
import type { OverlayOptions } from "../../overlay/src/options";

export { ClaxError } from "./client";
export { ReporterClient, type NewReport, type ReporterAuth, type ReporterOptions } from "./reporter";
export type * from "./types";

/** What `window.Clax` is once `<instance>/_clax/overlay.js` has loaded (or
 * the `./overlay` entry's own module, for a bundler): `overlay()` to start
 * it, `diagnostics.install()` to record the page's errors before it
 * starts, `forgetReporter()` for sign-out, and `CONTEXT_TIMEOUT_MS`. Typed
 * here so a host's own page need not hand-type `window.Clax`
 * (docs/feedback.md, "A script tag"; a host's own `ClaxGlobal` of this
 * shape is what `web/changelist`'s `clax.ts` used to hand-type). */
export interface ClaxOverlayGlobal {
  overlay(opts: OverlayOptions, doc?: Document): OverlayHandle;
  diagnostics: { install(win?: Window): Recorder };
  forgetReporter(r: ReporterRef): Promise<void>;
  CONTEXT_TIMEOUT_MS: number;
}

/** `instance` and `doc`, loaded once each: a second call for the same
 * pair (a second feature mounting the overlay, React's StrictMode) reuses
 * the one script tag and its load. */
const loaded = new WeakMap<Document, Map<string, Promise<ClaxOverlayGlobal>>>();

/** Loads `<instance>/_clax/overlay.js` into `doc` (default this page's)
 * and resolves the `Clax` it defines, typed: the client side of
 * docs/feedback.md's "A script tag", generalized so a host need not write
 * its own loader or its own `window.Clax` type (as `web/changelist`'s
 * `clax.ts` `loadOverlay` did before this). Cached per `instance` and
 * `doc`. A failed load (the instance down, a CSP that refuses the script)
 * is forgotten and its `<script>` removed, so a later call tries again
 * with a fresh one rather than replaying the same rejection forever. */
export function loadOverlay(instance: string, doc: Document = document): Promise<ClaxOverlayGlobal> {
  const base = instance.replace(/\/+$/, "");
  const global = () => (doc.defaultView as unknown as { Clax?: ClaxOverlayGlobal } | null)?.Clax;
  const have = global();
  if (have) return Promise.resolve(have);
  let byInstance = loaded.get(doc);
  if (!byInstance) { byInstance = new Map(); loaded.set(doc, byInstance); }
  const pending = byInstance.get(base);
  if (pending) return pending;
  const script = doc.createElement("script");
  script.src = `${base}/_clax/overlay.js`;
  script.async = true;
  const p = new Promise<ClaxOverlayGlobal>((resolve, reject) => {
    script.onload = () => {
      const g = global();
      if (g) resolve(g);
      else reject(new Error("the Clax overlay loaded without its Clax global"));
    };
    script.onerror = () => reject(new Error(`could not load the Clax overlay from ${script.src}`));
    doc.head.appendChild(script);
  }).catch((e: unknown) => {
    byInstance!.delete(base);
    script.remove();
    throw e;
  });
  byInstance.set(base, p);
  return p;
}

/** A `ReporterAuth` that POSTs `{public_key}` as JSON to `url` (the
 * deployer's own route, behind its own sign-in) and reads back `{token,
 * expires_at}`: the client side of docs/feedback.md's "The token route in
 * your backend". `trace`'s `traceparent`, when given, rides as a header
 * of the same name, for a route that wants to join the attempt's trace (a
 * route that ignores it loses nothing). A non-OK answer throws with
 * `code` read from a `{error:{code}}` body, so the overlay's drop-and-
 * retry on `invalid_key` (docs/feedback.md, "What the token route sees")
 * works through this helper exactly as it does through a hand-written
 * `auth`. */
export function reporterTokenFrom(url: string, opts: { fetch?: typeof fetch; headers?: Record<string, string> } = {}): ReporterAuth {
  const doFetch = opts.fetch ?? fetch;
  return async (publicKey, trace) => {
    const headers: Record<string, string> = { "content-type": "application/json", ...opts.headers };
    if (trace?.traceparent) headers.traceparent = trace.traceparent;
    const res = await doFetch(url, { method: "POST", headers, body: JSON.stringify({ public_key: publicKey }) });
    if (!res.ok) {
      const body = (await res.json().catch(() => null)) as { error?: { code?: string; message?: string } } | null;
      const code = body?.error?.code;
      throw Object.assign(new Error(body?.error?.message || `no reporter token: ${code ?? res.status}`), { code });
    }
    return res.json() as Promise<ReporterToken>;
  };
}
