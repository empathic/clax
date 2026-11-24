// A run's content (screenshots, a recording's frames) shown in the shell.
// An `<img src>` or a link the browser loads itself carries no
// owner proof header, so a local daemon refuses it (401) for the
// owner's browser; the shell's `fetch` adds the proof (`owner-proof.ts`).
// So the bytes are fetched and shown from a `blob:` URL, which the caller
// revokes when it stops showing them. The proof is never put in a URL.

/** The path of one item of a run's content. */
export const contentPath = (run: string, id: string) =>
  `/api/bot/runs/${encodeURIComponent(run)}/content/${encodeURIComponent(id)}`;

/** Fetches `url` and answers a `blob:` URL of its bytes; rejects on a
 *  refusal or a failed fetch. The caller revokes it
 *  (`URL.revokeObjectURL`). */
export async function blobUrl(url: string, f: typeof fetch = fetch): Promise<string> {
  const r = await f(url, { credentials: "same-origin" });
  if (!r.ok) throw new Error(String(r.status));
  return URL.createObjectURL(await r.blob());
}
