import { mount } from "svelte";
import { mountArtifactView } from "./artifact";
import { parseShellPath } from "./route";
import Gallery from "./ui/Gallery.svelte";

const app = document.getElementById("app")!;
const r = parseShellPath(location.pathname);
if (r.kind === "artifact") mountArtifactView(app, { id: r.id, pinnedVersion: r.version, file: r.file });
else mount(Gallery, { target: app });
