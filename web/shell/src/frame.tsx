import type { Ref } from "preact";
import { pageSrc } from "./origin";

/** The content frame, opened on the page published at `file` (the index by
 * default); `onLoad` runs on every load of a document in it, navigations
 * inside the frame included. */
export function Frame({ id, n, origin, file = "index.html", frameRef, onLoad }: { id: string; n: number; origin: string | null; file?: string; frameRef?: Ref<HTMLIFrameElement>; onLoad?: () => void }) {
  const src = pageSrc(id, n, origin, file);

  return origin
    ? <iframe key={`${n}-o`} ref={frameRef} class="frame" title="artifact content" src={src} onLoad={onLoad} allow="clipboard-write; fullscreen" />
    : <iframe key={`${n}-s`} ref={frameRef} class="frame" title="artifact content" src={src} onLoad={onLoad} sandbox="allow-scripts allow-forms allow-modals allow-popups allow-downloads" allow="clipboard-write; fullscreen" />;
}
