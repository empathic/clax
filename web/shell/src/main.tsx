import { render } from "preact";
import Gallery from "./gallery";
import ArtifactView from "./artifact";
import { parseShellPath } from "./route";

function route() {
  const r = parseShellPath(location.pathname);
  if (r.kind === "artifact") return <ArtifactView id={r.id} pinnedVersion={r.version} file={r.file} />;
  return <Gallery />;
}

render(route(), document.getElementById("app")!);
