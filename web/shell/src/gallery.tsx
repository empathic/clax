import { useEffect, useState } from "preact/hooks";
import { type Artifact, deleteArtifact, getToken, listArtifacts, patchArtifact } from "./api";
import { relativeTime } from "./format";
import { filterArtifacts, publisherText } from "./view/gallery-model";

export default function Gallery() {
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const refresh = () => listArtifacts().then(a => { setError(null); setArtifacts(a); }, e => setError(describe(e)));
  useEffect(() => { refresh(); getToken().then(setToken); }, []);

  const act = (op: () => Promise<unknown>) => op().then(refresh, e => setError(describe(e)));
  const shown = artifacts && filterArtifacts(artifacts, query);

  return (
    <>
      <header class="topbar">
        <h1>Clax</h1><span class="muted hide-sm">local artifacts</span>
        <input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" value={query}
          onInput={e => setQuery((e.currentTarget as HTMLInputElement).value)} />
      </header>
      <main class="wrap">
        {error && <p class="empty">Could not load artifacts: {error}</p>}
        {artifacts && artifacts.length === 0 && (
          <p class="empty">No artifacts yet. Publish one with <code>clax publish index.html</code>.</p>
        )}
        {artifacts && artifacts.length > 0 && shown && shown.length === 0 && (
          <p class="empty">No artifacts match your search.</p>
        )}
        {shown && shown.length > 0 && (
          <div class="grid">
            {shown.map(a => {
              const publisher = publisherText(a);
              return (
                <div class="card-wrap" key={a.id}>
                  <a class="card" href={`/a/${a.id}`}>
                    {a.pinned && <span class="pin" title="Pinned">★</span>}
                    <h2>{a.title}</h2>
                    {a.description && <p>{a.description}</p>}
                    <div class="meta">
                      <span>v{a.current_version}</span>
                      <span>{relativeTime(a.updated_at)}</span>
                      {publisher ? (
                        <span class="publisher">
                          {a.owner_live && <span class="live-dot" role="img" aria-label="session is live" title="Session is live" />}
                          {publisher}
                        </span>
                      ) : <span>published from the command line</span>}
                    </div>
                  </a>
                  {token && (
                    <div class="card-tools">
                      <button type="button" title={a.pinned ? "Unpin" : "Pin"} aria-label={a.pinned ? `Unpin ${a.title}` : `Pin ${a.title}`}
                        onClick={() => act(() => patchArtifact(a.id, { pinned: !a.pinned }, token))}>{a.pinned ? "★" : "☆"}</button>
                      <button type="button" title="Delete"
                        onClick={() => { if (confirm(`Delete "${a.title}"? This removes every version.`)) act(() => deleteArtifact(a.id, token)); }}>Delete</button>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </main>
    </>
  );
}
