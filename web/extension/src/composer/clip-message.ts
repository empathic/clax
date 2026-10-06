// What the composer says for a pick without a screenshot (spec 2026-10-05
// §8.1, §11), after "No screenshot: ". The codes are `captureClip`'s.
const MESSAGES: Record<string, string> = {
  no_capture_permission: "click the Clax button or press ⌥⇧C to comment with a screenshot",
  restricted_page: "Chrome does not capture this page",
  clip_too_large: "it was over 5 MiB",
};

/** The words for failure `code`, or undefined when there was none. */
export function clipMessage(code: string | null): string | undefined {
  if (code === null) return undefined;
  return MESSAGES[code] ?? "Chrome could not capture the tab";
}
