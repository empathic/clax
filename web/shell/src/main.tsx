import { render } from "preact";
import { mountArtifactView } from "./artifact";
import Gallery from "./gallery";
import { parseShellPath } from "./route";

const app = document.getElementById("app")!;
const r = parseShellPath(location.pathname);
if (r.kind === "artifact") mountArtifactView(app, { id: r.id, pinnedVersion: r.version, file: r.file });
else render(<Gallery />, app);
