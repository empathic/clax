import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";
import { type Artifact, type Version, getArtifact } from "./api";
import { subscribe } from "./events";
import { Frame } from "./frame";
import { artifactOrigin, contentSrc, probeOrigin } from "./origin";

type Props = { id: string; pinnedVersion: number | null };

export default function ArtifactView({ id, pinnedVersion }: Props) {
  const [data, setData] = useState<{ artifact: Artifact; versions: Version[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [origin, setOrigin] = useState<string | null | undefined>(undefined);
  const [newer, setNewer] = useState<number | null>(null);
  const [deleted, setDeleted] = useState(false);

  useEffect(() => { getArtifact(id).then(setData, e => setError(String(e).includes("404") ? "Artifact not found" : String(e))); }, [id]);
  useEffect(() => {
    const o = artifactOrigin(id);
    if (!o) { setOrigin(null); return; }
    probeOrigin(o).then(ok => setOrigin(ok ? o : null));
  }, [id]);

  const shown = pinnedVersion ?? data?.artifact.current_version ?? 0;
  useEffect(() => subscribe(id, e => {
    if (e.type === "version" && e.n > shown) setNewer(e.n);
    if (e.type === "artifact_deleted") setDeleted(true);
  }), [id, shown]);

  if (error) return <Shell title="Artifax"><p class="empty">{error}</p></Shell>;
  if (!data || origin === undefined) return <Shell title="Artifax"><p class="empty muted">Loading…</p></Shell>;
  const { artifact, versions } = data;
  const latest = artifact.current_version;
  const raw = contentSrc(id, shown, origin);

  return (
    <Shell title={artifact.title} right={
      <>
        <select value={shown} onChange={e => { const n = Number((e.target as HTMLSelectElement).value); location.assign(n === latest ? `/a/${id}` : `/a/${id}/v/${n}`); }}>
          {versions.map(v => <option value={v.n} key={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>)}
        </select>
        <a class="hide-sm" href={raw} target="_blank" rel="noopener">open raw</a>
        <button onClick={() => navigator.clipboard?.writeText(location.origin + `/a/${id}`)}>copy link</button>
      </>
    }>
      <div class="viewer">
        {deleted ? <p class="empty">This artifact was deleted.</p> : <Frame id={id} n={shown} origin={origin} />}
        {newer && !deleted && (
          <div class="banner"><span>v{newer} published</span><button class="primary" onClick={() => location.assign(`/a/${id}`)}>Reload</button></div>
        )}
        {shown < latest && !newer && <div class="banner"><span class="muted">viewing v{shown}; latest is v{latest}</span><a href={`/a/${id}`}>latest</a></div>}
      </div>
    </Shell>
  );
}

function Shell({ title, right, children }: { title: string; right?: ComponentChildren; children: ComponentChildren }) {
  return (
    <div class="page">
      <header class="topbar">
        <a href="/" title="Gallery">←</a>
        <h1>{title}</h1>
        <span style="flex:1" />
        {right}
      </header>
      {children}
    </div>
  );
}
