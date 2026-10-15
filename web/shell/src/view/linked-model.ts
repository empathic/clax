// How a linked thread reads (spec 2026-10-15-linked-threads-design, "UI"):
// the tag naming its remote, where it is when its version has no twin
// here, and the state of what waits to go up.
import type { Comment, Thread } from "../threads";

/** The tag on a thread from or linked to a remote: its name, and "unlinked" once the link ended. */
export function linkTag(t: Thread): { label: string; title: string; unlinked: boolean } | null {
  const l = t.linked;
  if (!l) return null;
  const where = l.origin === "remote" ? `From ${l.remote}` : `Also on ${l.remote}`;
  return l.unlinked
    ? { label: `${l.remote} · unlinked`, title: `${where}; no longer linked, kept here read-only`, unlinked: true }
    : { label: l.remote, title: where, unlinked: false };
}

/** "on clax.dev v<n>" for a thread on a public version with no twin here. */
export function remoteVersion(t: Thread): { label: string; url: string } | null {
  const l = t.linked;
  if (!l || l.twin || l.remote_version === null) return null;
  return { label: `on ${l.remote} v${l.remote_version}`, url: l.url };
}

/** What the thread's own up-send says while it waits; `retry` when Retry applies. */
export function threadLinkState(t: Thread): { text: string; retry: boolean; failed: boolean } | null {
  const l = t.linked;
  if (!l || l.unlinked || !l.state) return null;
  switch (l.state) {
    case "pending": return { text: `Sending to ${l.remote}…`, retry: false, failed: false };
    case "held": return { text: `Waiting for v${t.version_n} to be pushed to ${l.remote}`, retry: false, failed: false };
    case "failed": return { text: `Not sent: ${l.remote} unreachable`, retry: true, failed: true };
  }
  return null;
}

/** What one comment's up-send says while it waits. */
export function commentLinkState(c: Comment): { text: string; failed: boolean } | null {
  const l = c.linked;
  if (!l?.state) return null;
  return l.state === "failed"
    ? { text: `Not sent: ${l.remote} unreachable`, failed: true }
    : { text: `Sending to ${l.remote}…`, failed: false };
}

/** A comment from the remote: its author's name there and the remote's. */
export function remoteAuthor(c: Comment): { name: string; remote: string } | null {
  const l = c.linked;
  if (!l || l.origin !== "remote") return null;
  return { name: l.author_name || c.author_name, remote: l.remote };
}

/** Whether any of a thread's comments waits on a failed up-send (Retry applies). */
export function anyFailed(t: Thread): boolean {
  return t.linked?.state === "failed" || t.comments.some(c => c.linked?.state === "failed");
}
