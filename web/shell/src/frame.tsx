import { contentSrc } from "./origin";

export function Frame({ id, n, origin }: { id: string; n: number; origin: string | null }) {
  const src = contentSrc(id, n, origin);
  return origin
    ? <iframe key={`${n}-o`} class="frame" title="artifact content" src={src} allow="clipboard-write; fullscreen" />
    : <iframe key={`${n}-s`} class="frame" title="artifact content" src={src} sandbox="allow-scripts allow-forms allow-modals allow-popups allow-downloads" allow="clipboard-write; fullscreen" />;
}
