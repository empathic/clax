// Comments on the whole artifact (an anchor of kind `artifact`): no spot on
// any page, so no pin, no screenshot, and never Detached. The threads panel's
// New comment box writes them. Only NewComment.svelte imports this module,
// so it loads in that box's own chunk, and it takes from the artifact entry
// (whose budget is tight) only what the entry already exports.
import type { Anchor } from "../../../bridge/src/protocol";
import { type Thread, upsert } from "../threads";
import type { ThreadChange } from "./thread-sync";

/** What the panel and the composer call a thread about the whole artifact. */
export const WHOLE_LABEL = "The whole artifact";
/** The New comment composer's status line: there is nothing to take a screenshot of. */
export const WHOLE_NO_CLIP = "it is about the whole artifact";
/** The notice a failed post shows, as any failed post's (`failure.ts` `POST_FAILED`). */
const POST_FAILED = "Could not post";

/** The anchor a whole-artifact thread is created with: every target field
 * null, and `file` empty, since it is on no page. */
export function wholeAnchor(): Anchor {
  return { kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" };
}

/** `POST /api/artifacts/<aid>/threads` with the whole-artifact anchor and
 * no clip: the new thread, or the daemon's error message thrown. (As
 * `threads.ts` `createThread`, written here so that the artifact entry,
 * which holds that module, grows no export for this chunk.) */
async function create(aid: string, version: number, body: string): Promise<Thread> {
  const form = new FormData();
  form.set("anchor", JSON.stringify(wholeAnchor()));
  form.set("body", body);
  form.set("version", String(version));
  const res = await fetch(`/api/artifacts/${aid}/threads`, { method: "POST", body: form });
  const out = (await res.json().catch(() => ({}))) as { thread?: Thread; error?: { message?: string } };
  if (!res.ok || !out.thread) throw new Error(out.error?.message ?? res.statusText);
  return out.thread;
}

/** What posting needs of the artifact view (its controller). */
export type WholeHost = {
  readonly id: string;
  changeThreads(f: ThreadChange): void;
  set(patch: { selected: string }): void;
  setNotice(u: string | null | ((prev: string | null) => string | null)): void;
};

/** Posts `body` as a new thread about the whole artifact on `version`, adds
 * it to the view and selects it. A failure shows in the notice banner (as
 * any failed post's does, and a later success clears it) and is rethrown,
 * so the composer keeps the text. */
export async function postWhole(host: WholeHost, version: number, body: string): Promise<Thread> {
  try {
    const thread = await create(host.id, version, body);
    host.setNotice(prev => (prev?.startsWith(POST_FAILED) ? null : prev));
    host.changeThreads(ts => upsert(ts, thread));
    host.set({ selected: thread.id });
    return thread;
  } catch (e) {
    host.setNotice(`${POST_FAILED}: ${e instanceof Error ? e.message : String(e)}`);
    throw e;
  }
}
