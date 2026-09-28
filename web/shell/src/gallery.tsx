import { useEffect, useState } from "preact/hooks";
import { type Artifact, listArtifacts } from "./api";
import { relativeTime } from "./format";

export default function Gallery() {
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { listArtifacts().then(setArtifacts, e => setError(String(e))); }, []);
  return (
    <>
      <header class="topbar"><h1>Artifax</h1><span class="muted hide-sm">local artifacts</span></header>
      <main class="wrap">
        {error && <p class="empty">Could not load artifacts: {error}</p>}
        {artifacts && artifacts.length === 0 && (
          <p class="empty">No artifacts yet. Publish one with <code>artifax publish index.html</code>.</p>
        )}
        {artifacts && artifacts.length > 0 && (
          <div class="grid">
            {artifacts.map(a => (
              <a class="card" href={`/a/${a.id}`} key={a.id}>
                {a.pinned && <span class="pin" title="Pinned">★</span>}
                <h2>{a.title}</h2>
                {a.description && <p>{a.description}</p>}
                <div class="meta">
                  <span>v{a.current_version}</span>
                  <span>{relativeTime(a.updated_at)}</span>
                  <span>{a.owner_session_id ? "published by an agent" : "published from the command line"}</span>
                </div>
              </a>
            ))}
          </div>
        )}
      </main>
    </>
  );
}
