// The page-view report: one per page load, to the page's own site, when the
// bridge tag carries `data-count="1"` (a hosted Clax counting this page).

import type { PageMeta } from "./meta";
import type { Clock } from "./part-loader";
import { INSPECT_FRAME, MIGRATE_FRAME } from "./protocol";

/** Where the report goes, on the page's own origin. */
export const COUNT_PATH = "/_clax/api/analytics";

const LCP = "largest-contentful-paint";

/** What the report reads and calls, as the bridge captured it when it ran. */
export interface CountWorld {
  /** The bridge tag's dataset. */
  data?: DOMStringMap;
  meta: PageMeta;
  /** `window.parent !== window` as it was when the bridge ran. */
  framed: boolean;
  /** `window.name`: the app's inspect and migration frames report nothing. */
  name: string;
  /** `location.origin`, so a page's `<base>` cannot move the report. */
  origin: string;
  win: Pick<Window, "addEventListener">;
  nav: { globalPrivacyControl?: boolean };
  doc: Pick<Document, "readyState" | "referrer" | "addEventListener"> & { prerendering?: boolean };
  perf: Pick<Performance, "getEntriesByType">;
  Observer?: typeof PerformanceObserver;
  fetch: typeof fetch;
  clock: Pick<Clock, "setTimeout">;
}

/** Sends the report in the first task after the document's `load` (at once
 * if that has passed), after a prerendered document is shown, and never
 * with Global Privacy Control on. The navigation's timings count from the
 * prerender's activation. A failed send is ignored. */
export function count({ data, meta, framed, name, origin, win, nav, doc, perf, Observer, fetch, clock }: CountWorld): void {
  if (!data || data.count !== "1" || nav.globalPrivacyControl || framed && (name === INSPECT_FRAME || name === MIGRATE_FRAME)) return;
  let lcp: number | null = null;
  const ob = Observer && (Observer.supportedEntryTypes || []).includes(LCP) ? new Observer(l => { for (const e of l.getEntries()) lcp = e.startTime; }) : null;
  if (ob) ob.observe({ type: LCP, buffered: true });
  const send = () => clock.setTimeout(() => {
    if (ob) ob.disconnect();
    const n = perf.getEntriesByType("navigation")[0] as PerformanceNavigationTiming & { activationStart?: number } | undefined;
    const ms = (t: number | null | undefined) => t == null ? null : Math.max(0, Math.round(t - (n && n.activationStart || 0)));
    fetch(origin + COUNT_PATH, {
      method: "POST", keepalive: true, headers: { "content-type": "application/json" },
      body: JSON.stringify({
        file: meta.file, version: meta.version, framed,
        referrer: doc.referrer && new URL(doc.referrer).hostname,
        load_ms: ms(n && n.loadEventStart), lcp_ms: ms(lcp),
      }),
    }).catch(() => {});
  }, 0);
  const loaded = () => doc.readyState === "complete" ? send() : win.addEventListener("load", send);
  if (doc.prerendering) doc.addEventListener("prerenderingchange", loaded, { once: true }); else loaded();
}
