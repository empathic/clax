// Comments on the whole artifact (an anchor of kind `artifact`): no spot on
// any page, so no pin, no screenshot, and never Detached. The threads panel's
// New comment box writes them. This module loads with the panel's lazy
// chunk; nothing in the artifact entry imports it.
import { type Anchor, INDEX_FILE } from "../../../bridge/src/protocol";
import { POST_FAILED, failureText, scopedNotice } from "../failure";
import { type Thread, anchorLabel, createThread, upsert } from "../threads";
import type { Draft } from "./composer-model";
import type { ThreadChange } from "./thread-sync";

/** What the panel and the composer call a thread about the whole artifact. */
export const WHOLE_LABEL = "The whole artifact";
/** The composer's status line for it: there is nothing to take a screenshot of. */
export const WHOLE_NO_CLIP = "it is about the whole artifact";

/** Whether `a` is about the whole artifact. */
export const isWhole = (a: Anchor): boolean => a.kind === "artifact";

/** A thread's label: `WHOLE_LABEL` for one about the whole artifact, else its anchor's (`anchorLabel`). */
export function threadLabel(a: Anchor): string {
  return isWhole(a) ? WHOLE_LABEL : anchorLabel(a);
}

/** The anchor a whole-artifact thread is created with: every target field
 * null, and `file` empty, since it is on no page. */
export function wholeAnchor(): Anchor {
  return { kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" };
}

/** The usual composer's draft for a whole-artifact comment on `version`: it
 * shows `WHOLE_LABEL` in place of a quote and says why there is no
 * screenshot. Its anchor names the index only so the composer shows no page
 * label; the post sends `wholeAnchor()`. */
export function wholeDraft(version: number, pickId = `whole-${Date.now()}`): Draft {
  return { pickId, anchor: { ...wholeAnchor(), file: INDEX_FILE }, version, clip: null, clipError: WHOLE_NO_CLIP, label: WHOLE_LABEL };
}

/** What posting needs of the artifact view. */
export type WholeHost = {
  readonly id: string;
  changeThreads(f: ThreadChange): void;
  set(patch: { selected: string }): void;
  setNotice(u: string | null | ((prev: string | null) => string | null)): void;
};

/** Posts `body` as a new thread about the whole artifact on `version`, adds
 * it to the view and selects it. A failure shows in the notice banner (as
 * any failed post) and is rethrown, so the composer keeps the text. */
export async function postWhole(host: WholeHost, version: number, body: string): Promise<Thread> {
  const notice = scopedNotice(u => host.setNotice(u), POST_FAILED);
  try {
    const { thread } = await createThread(host.id, { anchor: wholeAnchor(), body, version, clip: null });
    notice(null);
    host.changeThreads(ts => upsert(ts, thread));
    host.set({ selected: thread.id });
    return thread;
  } catch (e) {
    notice(failureText(POST_FAILED, e));
    throw e;
  }
}
