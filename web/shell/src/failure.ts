import { ApiError } from "./api";

export const SEND_FAILED = "Could not send to the agent";
export const RESOLVE_FAILED = "Could not resolve";
export const POST_FAILED = "Could not post";
export const NAME_FAILED = "Could not save your name";
export const LOAD_FAILED = "Could not load comments";

/** `<prefix>: <message>` for an API error, a network failure, or anything thrown. */
export function failureText(prefix: string, e: unknown): string {
  const message = e instanceof ApiError || e instanceof Error ? e.message : String(e);
  return `${prefix}: ${message}`;
}

/** Awaits `p`. On success clears the notice and returns the value; on failure
 * shows `failureText(prefix, e)` and returns `undefined`. Nothing is swallowed. */
export async function report<T>(p: Promise<T>, prefix: string, setNotice: (text: string | null) => void): Promise<T | undefined> {
  try {
    const v = await p;
    setNotice(null);
    return v;
  } catch (e) {
    setNotice(failureText(prefix, e));
    return undefined;
  }
}
