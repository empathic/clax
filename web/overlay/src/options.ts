// `Clax.overlay`'s options, and the other names of its public interface
// that main.ts does not define, in a module of their own: the overlay's
// public types (main.ts) reach this module and the SDK's reporter types,
// and nothing of the bridge or of the overlay's model.
import type { ReporterAuth } from "../../sdk/src/reporter";
import type { ReportContext } from "../../sdk/src/types";

/** What the reporter captures: a pick, a drawn area, or the visible viewport. */
export type Mode = "element" | "area" | "screen";

/** How long the host's `context` and `metadata` functions may take before
 * the capture goes on without them. */
export const CONTEXT_TIMEOUT_MS = 2000;

/** What the host says about the moment of a capture. Only `release`,
 * `flags` and `app` are taken; the time, environment and capture are the
 * overlay's own. */
export type HostContext = Partial<ReportContext>;

/** A value, or a function that gives it (perhaps later). */
export type Given<T> = T | (() => T | Promise<T>);

/** Where "Your reports" shows: `OverlayOptions.reports`. */
export type ReportsMode = "button" | "off";

/** `Clax.overlay`'s options. Every report carries the page's URL with its
 * query and without its fragment, and the page's title.
 *
 * A host built from configuration passes it through: clax.dev's shell, from
 * its `/_app/feedback/config` (the queue, whether this person may file, and
 * a context of release, commit, app and flags), passes the queue as
 * `queue`, whether the person may file as `canFile`, and the context as
 * `context`, its commit as `release.commit`. */
export interface OverlayOptions {
  /** The instance's base URL, such as `https://clax.corp.example`. */
  instance: string;
  /** The queue's ID. Optional with an install (`install`, or the page's
   * `<meta name="clax-install">`), whose config names its queue; given,
   * it is the queue filed into. */
  queue?: string;
  /** The install's ID, whose config Clax serves: the queue it files into,
   * its wording, and whether it shows "Your reports" and the element
   * picker; edited in Clax, on the queue's Installs. Default: `<meta
   * name="clax-install">`'s content. The overlay starts once the config is
   * fetched; when it cannot be (in `INSTALL_TIMEOUT_MS`), it starts on this
   * page's own options and metas. */
  install?: string;
  /** Gets a reporter token for the reporter's public key, from the deployer's backend. */
  auth: ReporterAuth;
  /** `"corner"` (the default): the Report and "Your reports" buttons in the
   * page's bottom right corner. `false`: none; the host starts a report
   * with `report()` and opens the list with `showReports()` from its own
   * controls, and shows the count `on("badge")` gives. */
  launcher?: "corner" | false;
  /** Whether this reporter may file reports (default true). False: no
   * Report button, and `report()` does nothing; "Your reports" stays. */
  canFile?: boolean;
  /** The "Your reports" entry point: `"button"` (the default) puts it beside
   * Report in the launcher; `"off"` leaves it out, with its panel, its
   * polling and the badge's, for a page that shows the reports itself
   * (`showReports()` then does nothing). Report stays either way. Default:
   * `<meta name="clax-reports">`'s content, else `"button"`. An install's
   * config, once fetched, sets it in their place. */
  reports?: ReportsMode;
  /** The host's part of each capture's context (spec
   * 2026-10-29-feedback-loop §5.1): `release`, `flags` and `app`, or a
   * function giving them, called when the reporter starts a capture (and
   * when they give a verdict), so it describes that moment. A function that
   * throws, or takes longer than `CONTEXT_TIMEOUT_MS`, files the report
   * with `app: {context_error}` in their place. */
  context?: Given<HostContext>;
  /** JSON the deployer attaches to each report, or a function giving it,
   * called once per report when its capture starts. One that throws or
   * takes longer than `CONTEXT_TIMEOUT_MS` files the report without. */
  metadata?: Given<unknown>;
  /** Whether "Include recent errors" starts ticked (default false). The
   * page's recent errors leave it only when the reporter keeps it ticked. */
  diagnostics?: { default?: boolean };
  /** The page's artifact. Default: `<meta name="clax-artifact">`'s content,
   * else none, and the instance files the report on the page URL without its route. */
  externalId?: string;
  /** The nonce the page's Content-Security-Policy gives its own inline
   * styles, on a page whose `style-src` refuses 'unsafe-inline' (or any
   * strict string, such as `"strict"`, where it uses hashes). The overlay
   * styles itself without inline style; given this, its page renders
   * (clips) are drawn without the page's web fonts from the first, as reading
   * them takes an inline <style> such a policy refuses, so the overlay trips
   * no violation. A policy in a `<meta http-equiv>` is read without it. */
  nonce?: string;
  /** A stable ID of the signed-in user, in the host's own terms (hashed
   * before it is stored). With it, the reporter's key (a non-extractable
   * CryptoKey) and its reporter token are kept in IndexedDB for this
   * instance, queue and user and reused across page loads until shortly
   * before the token expires, so `auth` is called about once per 15
   * minutes per user rather than on every page load. Without it, every
   * page load asks `auth` for a token. */
  user?: string;
}
