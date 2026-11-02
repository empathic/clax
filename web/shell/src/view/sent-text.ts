// What a send naming an agent says once it is done (the shell's and the
// extension worker's alike; no imports, so the worker may take it).

/** Whom a send naming an agent reached, as the daemon answers it (`to`):
 * its name, the chitin title it registered with, and whether the send waits
 * for it (it is not live on the page now). */
export type SentTo = { name: string; chitin_title: string | null; queued: boolean };

/** What a send naming an agent says: over chitin to the agent's title when
 * it has one, else that it waits for the agent; null for a send to a live
 * agent without a title, or to no one named. */
export function sentText(to: SentTo | null | undefined): string | null {
  if (!to) return null;
  if (to.chitin_title) return `Sent over chitin to ${to.chitin_title}.`;
  if (to.queued) return `Sent; ${to.name} will get it the next time it works.`;
  return null;
}
