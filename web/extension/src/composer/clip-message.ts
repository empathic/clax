// What the composer says for a pick without a screenshot (spec 2026-10-05
// §8.1, §11), after "No screenshot: ". The codes are `captureClip`'s.
import { type Shortcut, clipHint } from "../shortcut";

const MESSAGES: Record<string, string> = {
  restricted_page: "Chrome does not capture this page",
  clip_too_large: "it was over 5 MiB",
};

/** The words for failure `code`, or undefined when there was none; `keys`
 * is the keyboard command's shortcut, which grants the screenshot. */
export function clipMessage(code: string | null, keys: Shortcut = null): string | undefined {
  if (code === null) return undefined;
  if (code === "no_capture_permission") return clipHint(keys);
  return MESSAGES[code] ?? "Chrome could not capture the tab";
}
