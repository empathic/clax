// The DevTools panel's network log, first slice (RECOMMEND-consultant-2026-
// 11-02-clax-extension-devtools Q3): each finished request of the inspected
// tab as `chrome.devtools.network` hands it over (a HAR entry), kept in the
// panel page's memory only (the last 500, cleared on navigation unless
// "Keep log"), never written to storage and never sent anywhere. A row shows
// method, the URL with its query values elided, status, kind, size and time;
// headers show only when asked, with the credentials always `[hidden]`; a
// body loads only on its own click, and is dropped once deselected.
// Attaching requests to a report (D2) is the next slice.

/** The HAR entry fields the log reads (`chrome.devtools.network.Request`). */
export type HarEntry = {
  startedDateTime: string;
  time: number;
  request: { method: string; url: string; headers?: { name: string; value: string }[] };
  response: { status: number; statusText?: string; headers?: { name: string; value: string }[]; bodySize?: number; _transferSize?: number; _error?: string | null; content?: { size?: number; mimeType?: string } };
  timings?: Partial<Record<"blocked" | "dns" | "connect" | "ssl" | "send" | "wait" | "receive", number>>;
  _resourceType?: string;
  getContent?(done: (content: string, encoding: string) => void): void;
};

export const RING = 500;
/** How much of a body "Load body" shows. */
export const BODY_CAP = 64 * 1024;
/** Headers whose values are never shown, even on request. */
export const SENSITIVE = new Set(["authorization", "cookie", "set-cookie", "proxy-authorization"]);
export const FILTERS = [
  { value: "all", label: "All" }, { value: "failed", label: "Failed" }, { value: "fetch", label: "Fetch/XHR" },
  { value: "document", label: "Documents" }, { value: "other", label: "Other sites" },
] as const;
export type Filter = (typeof FILTERS)[number]["value"];

export type Row = {
  n: number;
  method: string;
  /** Origin and path, query values elided: what is shown and copied. */
  url: string;
  status: number;
  failed: boolean;
  error: string | null;
  kind: string;
  /** Bytes over the wire, or the body's size when Chrome gave none; null when unknown. */
  size: number | null;
  ms: number;
  /** Milliseconds since the first request the log kept. */
  at: number;
  otherSite: boolean;
  timings: { phase: string; ms: number }[];
  entry: HarEntry;
};

/** `url` without its fragment, each query value shown as `…`. */
export function elide(url: string): string {
  try {
    const u = new URL(url);
    const q = [...u.searchParams.keys()].map(k => `${encodeURIComponent(k)}=…`).join("&");
    return `${u.origin}${u.pathname}${q ? `?${q}` : ""}`;
  } catch { return url.split(/[?#]/)[0]; }
}

/** A header list as shown: the credentials' values `[hidden]`. */
export const shownHeaders = (hs: { name: string; value: string }[] = []) =>
  hs.map(h => ({ name: h.name, value: SENSITIVE.has(h.name.toLowerCase()) ? "[hidden]" : h.value }));

const PHASES = ["blocked", "dns", "connect", "ssl", "send", "wait", "receive"] as const;

export function toRow(e: HarEntry, n: number, origin: string | null, t0: number): Row {
  const status = e.response?.status ?? 0;
  // HAR's custom fields (Chrome's `_error`, `_transferSize`, `_resourceType`) are read by name.
  const error = e.response?.["_error"] || null;
  const transfer = e.response?.["_transferSize"] ?? e.response?.bodySize ?? -1;
  const size = transfer >= 0 ? transfer : (e.response?.content?.size ?? -1) >= 0 ? e.response.content!.size! : null;
  let other = false;
  try { other = origin !== null && new URL(e.request.url).origin !== origin; } catch { /* not a URL */ }
  const t = Date.parse(e.startedDateTime);
  return {
    n, method: e.request.method, url: elide(e.request.url), status, error,
    failed: !!error || status === 0 || status >= 400,
    kind: e["_resourceType"] ?? "other", size, ms: Math.max(0, Math.round(e.time ?? 0)),
    at: Number.isFinite(t) && Number.isFinite(t0) ? Math.max(0, Math.round(t - t0)) : 0,
    otherSite: other,
    timings: PHASES.map(p => ({ phase: p === "ssl" ? "tls" : p, ms: e.timings?.[p] ?? -1 })).filter(x => x.ms >= 0).map(x => ({ ...x, ms: Math.round(x.ms) })),
    entry: e,
  };
}

export function matches(r: Row, f: Filter, word: string): boolean {
  if (f === "failed" && !r.failed) return false;
  if (f === "fetch" && r.kind !== "fetch" && r.kind !== "xhr") return false;
  if (f === "document" && r.kind !== "document") return false;
  if (f === "other" && !r.otherSite) return false;
  const w = word.trim().toLowerCase();
  return !w || `${r.method} ${r.url} ${r.status} ${r.kind}`.toLowerCase().includes(w);
}

export const bytes = (n: number | null) => n === null ? "—" : n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} kB` : `${(n / 1024 / 1024).toFixed(1)} MB`;

/** The rows as plain text, one per line: only what a row shows (no headers, cookies or bodies). */
export const asText = (rows: Row[]) =>
  rows.map(r => `${r.method} ${r.status || "—"}${r.error ? ` (${r.error})` : ""} ${r.url} ${r.kind} ${bytes(r.size)} ${r.ms} ms`).join("\n");

export class NetworkLog {
  /** Raw (not proxied): each row holds Chrome's own entry, whose `getContent` is called as Chrome gave it. */
  rows = $state.raw<Row[]>([]);
  keep = $state(false);
  /** The inspected page's origin, for "Other sites". */
  origin = $state<string | null>(null);
  private n = 0;
  private t0 = Number.NaN;

  add(e: HarEntry): void {
    if (!e?.request?.url) return;
    const t = Date.parse(e.startedDateTime);
    if (!Number.isFinite(this.t0) && Number.isFinite(t)) this.t0 = t;
    const row = toRow(e, ++this.n, this.origin, this.t0);
    this.rows = this.rows.length >= RING ? [...this.rows.slice(this.rows.length - RING + 1), row] : [...this.rows, row];
  }

  /** The inspected page navigated (to `origin`): the log starts again unless kept. */
  navigated(origin: string | null): void {
    this.origin = origin;
    if (!this.keep) this.clear();
  }

  clear(): void { this.rows = []; this.t0 = Number.NaN; }
}

/** Reads a request's body under its own click: text up to BODY_CAP, or what it is when binary. */
export function loadBody(e: HarEntry): Promise<string> {
  return new Promise(ok => {
    if (!e.getContent) { ok("No body."); return; }
    try {
      e.getContent((content, encoding) => {
        if (content == null || content === "") ok("No body.");
        else if (encoding === "base64") ok(`Binary body (${bytes(Math.floor(content.length * 3 / 4))}), not shown.`);
        else ok(content.length > BODY_CAP ? `${content.slice(0, BODY_CAP)}\n… (cut at ${bytes(BODY_CAP)})` : content);
      });
    } catch { ok("The body could not be read."); }
  });
}
