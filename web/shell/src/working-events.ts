// The gallery's working stream, apart from `events.ts` so the gallery's lazy
// working module does not share the artifact entry's event code.
import type { ArtifactEvent } from "./events";
/** Subscribes to every artifact's `working` events (the gallery's chips);
 * a no-op where `EventSource` is undefined. */
export function subscribeWorking(onEvent: (e: ArtifactEvent) => void): () => void {
  if (typeof EventSource === "undefined") return () => {};
  const es = new EventSource("/api/events?types=working");
  es.addEventListener("working", (e: MessageEvent) => { try { onEvent(JSON.parse(e.data)); } catch { /* ignore malformed */ } });
  es.addEventListener("ready", () => onEvent({ type: "ready" }));
  es.addEventListener("error", () => onEvent({ type: "stream_down" }));
  es.addEventListener("resync", (e: MessageEvent) => {
    try { onEvent({ type: "resync", dropped: Number(JSON.parse(e.data).dropped) || 0 }); } catch { onEvent({ type: "resync", dropped: 0 }); }
  });
  return () => es.close();
}
