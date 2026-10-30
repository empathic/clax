// `Clax.overlay`'s options, in a module of their own: the overlay's public
// types (main.ts) reach the SDK's reporter types and nothing of the bridge.
import type { ReporterAuth } from "../../sdk/src/reporter";

/** `Clax.overlay`'s options. Every report carries the page's URL with its
 * query and without its fragment, and the page's title. */
export interface OverlayOptions {
  /** The instance's base URL, such as `https://clax.corp.example`. */
  instance: string;
  /** The queue's ID. */
  queue: string;
  /** Gets a reporter token for the reporter's public key, from the deployer's backend. */
  auth: ReporterAuth;
  /** JSON the deployer attaches to every report filed from this page. */
  metadata?: unknown;
  /** The page's artifact. Default: `<meta name="clax-artifact">`'s content,
   * else none, and the instance files the report on the page URL without its route. */
  externalId?: string;
}
