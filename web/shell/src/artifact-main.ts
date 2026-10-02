import { mountArtifactView } from "./artifact";
import { parseShellPath } from "./route";
import { loadMoreMenu } from "./ui/more-menu.svelte";
import { loadSidebar } from "./ui/sidebar-chunk";
import { afterPaint } from "./view/after-paint";
import { readBoot, takeEarly } from "./view/boot";

const r = parseShellPath(location.pathname);
// The daemon serves this entry only for artifact paths.
if (r.kind === "artifact") {
  mountArtifactView(document.getElementById("app")!, { id: r.id, pinnedVersion: r.version, file: r.file }, { boot: readBoot(), early: () => takeEarly() });
  // After the first paint: the more menu, and the thread list's chunk even
  // while the threads are hidden, so the first Threads tap shows them at once.
  afterPaint(() => { loadMoreMenu(); loadSidebar().catch(() => {}); });
}
