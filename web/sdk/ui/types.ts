// The UI kit's prop types, in a module of their own: TypeScript reads only
// a component's default export from a .svelte file.

/** A comment as `Thread` shows it: `kind` rules it (`reporter` or `you`
 * for a person writing here, `agent` for an agent, else a neutral). */
export type ThreadComment = { id: string; kind: string; who: string; body: string };
