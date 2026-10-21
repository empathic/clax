// The Markdown module (`markdown.ts`, with its parser), loaded on demand so
// no entry carries it. Until it is here a comment shows as its text, as
// typed; reading `md.m` in a template or `$derived` renders again once it
// arrives.
type Md = typeof import("./markdown");

export const md: { m: Md | null } = $state({ m: null });
let loading: Promise<Md | null> | null = null;

/** Loads the Markdown module (once); resolves to it, or to null when it
 * did not load, and the next call tries again. */
export function loadMarkdown(): Promise<Md | null> {
  return (loading ??= import("./markdown").then(m => (md.m = m), () => { loading = null; return null; }));
}

/** A comment body as plain text for a preview: its Markdown syntax taken
 * away once the module is here, else as typed. */
export const plainText = (body: string): string => (md.m ? md.m.plain(body) : body);
