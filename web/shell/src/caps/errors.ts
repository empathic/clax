/** A capability call's rejection, sent to the page as `{code, message, ...extra}`. */
export class CapError extends Error {
  constructor(readonly code: string, message: string, readonly extra: Record<string, unknown> = {}) {
    super(message);
    this.name = "CapError";
  }
}
