// `URL.createObjectURL` for tests: jsdom has none. Each `blob:` URL made is
// kept with its Blob, so a test reads back what an `<img>` shows.
export function blobUrls() {
  const made = new Map<string, Blob>();
  const revoked: string[] = [];
  let n = 0;
  const was = { create: URL.createObjectURL, revoke: URL.revokeObjectURL };
  URL.createObjectURL = (b: Blob | MediaSource) => { const u = `blob:test/${++n}`; made.set(u, b as Blob); return u; };
  URL.revokeObjectURL = (u: string) => { revoked.push(u); };
  return {
    /** The text of the Blob a `blob:` URL was made from, or null. */
    text: async (u: string | null | undefined) => (u && made.has(u) ? await made.get(u)!.text() : null),
    /** Every URL made so far. */
    made: () => [...made.keys()],
    revoked,
    restore() { URL.createObjectURL = was.create; URL.revokeObjectURL = was.revoke; },
  };
}
