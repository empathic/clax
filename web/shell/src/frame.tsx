import type { Ref } from "preact";
import { contentSrc } from "./origin";

export function Frame({ id, n, origin, frameRef }: { id: string; n: number; origin: string | null; frameRef?: Ref<HTMLIFrameElement> }) {
  const src = contentSrc(id, n, origin);
  return origin
    ? <iframe key={`${n}-o`} ref={frameRef} class="frame" title="artifact content" src={src} allow="clipboard-write; fullscreen" />
    : <iframe key={`${n}-s`} ref={frameRef} class="frame" title="artifact content" src={src} sandbox="allow-scripts allow-forms allow-modals allow-popups allow-downloads" allow="clipboard-write; fullscreen" />;
}
