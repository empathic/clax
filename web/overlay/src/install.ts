// The overlay's install: the one ID a host page carries (`<meta
// name="clax-install">` or `OverlayOptions.install`), whose config Clax
// serves (`GET /api/installs/<id>/config`: the queue it files into, its
// name and pages, and the install's wording and switches). The page's own
// options and metas are what the overlay falls back to when there is no
// install, or its config cannot be fetched.
import type { InstallConfig, PublicInstallConfig } from "../../sdk/src/types";
import { reportsModeOf } from "./model";
import type { OverlayOptions, ReportsMode } from "./options";

export type { InstallConfig, PublicInstallConfig };

/** The `<meta name>` a page names its install with. */
export const INSTALL_META = "clax-install";
/** How long the config may take before the overlay starts without it. */
export const INSTALL_TIMEOUT_MS = 3000;

/** The install's ID: `given`, else `<meta name="clax-install">`'s content,
 * else none. */
export function installIdOf(doc: Document, given?: unknown): string | null {
  if (given !== undefined && (typeof given !== "string" || !given.trim())) throw new TypeError("Clax.overlay: install must be a non-empty string");
  const v = (given as string | undefined) ?? doc.querySelector<HTMLMetaElement>(`meta[name="${INSTALL_META}"]`)?.content;
  return v?.trim() || null;
}

/** Whether `v` reads as an install's config. */
function isConfig(v: unknown): v is PublicInstallConfig {
  const c = v as PublicInstallConfig | null;
  return !!c && typeof c.install_id === "string" && typeof c.queue?.id === "string" && typeof c.queue.name === "string"
    && typeof c.config === "object" && c.config !== null;
}

/** Install `id`'s config from `instance`, or null when it cannot be had
 * (refused, gone, not JSON, or slower than `timeoutMs`): the overlay then
 * runs on the page's own options. The browser revalidates it by its ETag. */
export async function fetchInstall(instance: string, id: string, f: typeof fetch = fetch, timeoutMs = INSTALL_TIMEOUT_MS): Promise<PublicInstallConfig | null> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), timeoutMs);
  try {
    const res = await f(`${instance}/api/installs/${encodeURIComponent(id)}/config`, { mode: "cors", credentials: "omit", cache: "no-cache", signal: abort.signal });
    if (!res.ok) return null;
    const v: unknown = await res.json();
    return isConfig(v) ? v : null;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/** The words the overlay shows; null is its own default. */
export interface Wording {
  /** The product the overlay calls itself; null: none, never "Clax". */
  productName: string | null;
  /** The Report button's label. */
  reportLabel: string;
  /** The words before the queue's name in the Report panel. */
  queueLabel: string;
}

/** The overlay's own words, with no install's. */
export const DEFAULT_WORDING: Wording = { productName: null, reportLabel: "Report", queueLabel: "Files into" };

/** The queue the overlay names in its Report panel, when its install says. */
export interface QueueInfo {
  name: string;
  /** The queue's page for its members, on the instance. */
  page: string;
  /** The reporter's page of the queue, on the instance. */
  reporterPage: string;
}

/** What the overlay runs with. */
export interface Settings {
  queue: string;
  reports: ReportsMode;
  picker: boolean;
  wording: Wording;
  /** The queue as the install names it; null without an install's config. */
  queueInfo: QueueInfo | null;
  brandColor: string | null;
}

const text = (s: string | null | undefined, dflt: string) => (typeof s === "string" && s.trim()) || dflt;

/** The overlay's settings: the install's config `cfg` where it has one,
 * else the page's options and metas. The page's `queue` stands when it
 * names another queue than the install (its backend mints tokens for it):
 * the panel then names no queue. */
export function settingsOf(cfg: PublicInstallConfig | null, opts: Pick<OverlayOptions, "queue" | "reports" | "instance">, doc: Document): Settings {
  const fallback: Settings = {
    queue: opts.queue ?? "",
    reports: reportsModeOf(doc, opts.reports),
    picker: true,
    wording: DEFAULT_WORDING,
    queueInfo: null,
    brandColor: null,
  };
  if (!cfg) return fallback;
  const c: Partial<InstallConfig> = cfg.config;
  const same = !opts.queue || opts.queue === cfg.queue.id;
  if (!same) console.warn(`Clax.overlay: the page's queue ${opts.queue} is not install ${cfg.install_id}'s (${cfg.queue.id}); filing into the page's`);
  const base = (opts.instance ?? "").replace(/\/+$/, "");
  return {
    queue: opts.queue || cfg.queue.id,
    reports: c.reports_button === false ? "off" : "button",
    picker: c.picker !== false,
    wording: {
      productName: text(c.product_name, "") || null,
      reportLabel: text(c.report_label, fallback.wording.reportLabel),
      queueLabel: text(c.queue_label, fallback.wording.queueLabel),
    },
    queueInfo: same ? { name: cfg.queue.name, page: `${base}${cfg.queue.page}`, reporterPage: `${base}${cfg.queue.reporter_page}` } : null,
    brandColor: typeof c.brand_color === "string" && /^#[0-9a-f]{6}$/i.test(c.brand_color) ? c.brand_color : null,
  };
}
