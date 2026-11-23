// The haiku list (haiku.json), fetched rather than imported, once per page:
// what asks first starts the request, and once it is in, a haiku line
// renders with its text at once (`haikuNow`). The artifact view's sidebar
// chunk waits for it (`ui/sidebar-chunk.ts`), so a working strip is drawn
// whole, its haiku included.
import listUrl from "./haiku.json?url&no-inline";

let list: string[] | null = null;
let pending: Promise<string[] | null> | null = null;

/** The list, asked once; null when it could not be read (asked again next time). */
export function haikuList(): Promise<string[] | null> {
  pending ??= fetch(listUrl).then(r => r.json() as Promise<string[]>).then(l => (list = l), () => { pending = null; return null; });
  return pending;
}

/** The list once it is in, else null. */
export const haikuNow = (): string[] | null => list;
