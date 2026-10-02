import { mountArtifactView } from "./artifact";
import { parseShellPath } from "./route";
import { loadMoreMenu } from "./ui/more-menu.svelte";
import { afterPaint } from "./view/after-paint";
import { readBoot, takeEarly } from "./view/boot";

const r = parseShellPath(location.pathname);
// The daemon serves this entry only for artifact paths.
if (r.kind === "artifact") {
  mountArtifactView(document.getElementById("app")!, { id: r.id, pinnedVersion: r.version, file: r.file }, { boot: readBoot(), early: () => takeEarly() });
  afterPaint(loadMoreMenu);
}
