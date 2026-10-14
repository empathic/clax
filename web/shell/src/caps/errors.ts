/** What a write is refused with while the view is a spectator (beyond the
 * artifact's live seats, spec 2026-10-11-clax-dev-hosted-design §16.1),
 * under the contract's `unavailable` (`upstream_error` for room): the page
 * may try again once the viewer joins live. */
export const SPECTATING = "Lots of people are here, so you're viewing; join live to make changes";

/** A capability call's rejection, sent to the page as `{code, message, ...extra}`. */
export class CapError extends Error {
  constructor(readonly code: string, message: string, readonly extra: Record<string, unknown> = {}) {
    super(message);
    this.name = "CapError";
  }
}
