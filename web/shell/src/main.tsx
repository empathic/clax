import { render } from "preact";
import Gallery from "./gallery";
import ArtifactView from "./artifact";

function route() {
  const m = location.pathname.match(/^\/a\/([0-9a-hj-km-np-tv-z]{12})(?:\/v\/(\d+))?\/?$/);
  if (m) return <ArtifactView id={m[1]} pinnedVersion={m[2] ? Number(m[2]) : null} />;
  return <Gallery />;
}

render(route(), document.getElementById("app")!);
