// `Clax.overlay`'s options, in a module of their own: the overlay's public
// types (main.ts) reach the SDK's reporter types and nothing of the bridge.
import type { ClaxPlugin } from "../../sdk/src/plugin";
import type { ReporterAuth } from "../../sdk/src/reporter";
import type { ReportContext } from "../../sdk/src/types";

/** What the host says about the moment of a capture. Only `release`,
 * `flags` and `app` are taken; the time, environment and capture are the
 * overlay's own. */
export type HostContext = Partial<ReportContext>;

/** A value, or a function that gives it (perhaps later). */
export type Given<T> = T | (() => T | Promise<T>);

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
  /** The queue's ID. */
  queue: string;
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
  /** The plugins (`@empathic/clax-sdk/plugins`): every feature the overlay
   * shows comes from one. Default: `classic()`, the overlay as it was before
   * plugins (the DOM render and the exact screenshot, Box/Arrow/Redact/Text,
   * Element, Area and Screen, the corner dock). */
  plugins?: ClaxPlugin[];
  /** How the composer is shown: a presentation plugin's id (`floating`,
   * `dialog`, `drawer`, `sheet`, `window`, `extension`). Default: the first
   * one the plugins give. */
  presentation?: string;
  /** `--clax-*` custom properties: the site's look, over the plugins' themes. */
  theme?: Record<string, string>;
  /** Filters and attachments turned on or off (by id), over each plugin's own default. */
  defaults?: Record<string, boolean>;
}
