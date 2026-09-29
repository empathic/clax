import type { Ref } from "preact";
import { contentSrc } from "./origin";

/** The content frame, opened on the version's index; `onLoad` runs on every
 * load of a document in it, navigations inside the frame included. */
export function Frame({ id, n, origin, frameRef, onLoad }: { id: string; n: number; origin: string | null; frameRef?: Ref<HTMLIFrameElement>; onLoad?: () => void }) {
  const src = contentSrc(id, n, origin);
  return origin
    ? <iframe key={`${n}-o`} ref={frameRef} class="frame" title="artifact content" src={src} onLoad={onLoad} allow="clipboard-write; fullscreen" />
    : <iframe key={`${n}-s`} ref={frameRef} class="frame" title="artifact content" src={src} onLoad={onLoad} sandbox="allow-scripts allow-forms allow-modals allow-popups allow-downloads" allow="clipboard-write; fullscreen" />;
}
