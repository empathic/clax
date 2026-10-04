import { mountArtifactView } from "./artifact";
import { parseShellPath } from "./route";
import { loadMoreMenu } from "./ui/more-menu.svelte";
import { allowSidebarPrefetch } from "./ui/sidebar-chunk";
import { afterPaint } from "./view/after-paint";
import { readBoot, takeEarly } from "./view/boot";

const r = parseShellPath(location.pathname);
// The daemon serves this entry only for artifact paths.
if (r.kind === "artifact") {
  // The thread list's chunk is fetched after the first paint even while the
  // threads are hidden (not for a deleted artifact), so the first Threads tap
  // shows them at once.
  allowSidebarPrefetch();
  mountArtifactView(document.getElementById("app")!, { id: r.id, pinnedVersion: r.version, file: r.file }, { boot: readBoot(), early: () => takeEarly() });
  afterPaint(loadMoreMenu);
  // The view ended as the page was hidden (`ArtifactController`): a page the
  // back/forward cache restores loads again, live.
  addEventListener("pageshow", e => { if (e.persisted) location.reload(); });
}
