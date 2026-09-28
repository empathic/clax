export type ArtifactEvent = { type: "version"; artifact_id: string; n: number } | { type: "artifact_deleted"; artifact_id: string };

export function subscribe(artifactId: string, onEvent: (e: ArtifactEvent) => void): () => void {
  const es = new EventSource(`/api/events?artifact=${artifactId}`);
  const handler = (e: MessageEvent) => { try { onEvent(JSON.parse(e.data)); } catch { /* ignore malformed */ } };
  es.addEventListener("version", handler);
  es.addEventListener("artifact_deleted", handler);
  return () => es.close();
}
