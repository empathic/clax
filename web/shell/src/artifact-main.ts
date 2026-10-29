import { mountArtifactView } from "./artifact";
import { bootFeedback } from "./feedback/boot";
import { appQuery, inspectQuery, parseShellPath } from "./route";
import { loadMoreMenu } from "./ui/more-menu.svelte";
import { allowSidebarPrefetch } from "./ui/sidebar-chunk";
import { afterPaint } from "./view/after-paint";
import { readBoot, takeEarly } from "./view/boot";

const r = parseShellPath(location.pathname);
// The daemon serves this entry only for artifact paths.
if (r.kind === "artifact") {
  // The app's inspect view (`?app=1&inspect=1`, `app/host.ts`) has no
  // threads or menu to show, so it fetches neither.
  const inspecting = inspectQuery(location.search);
  // The thread list's chunk is fetched after the first paint even while the
  // threads are hidden (not for a deleted artifact), so the first Threads tap
  // shows them at once.
  if (!inspecting) allowSidebarPrefetch();
  // Reporting a problem with Clax (`feedback/`), asked first so the
  // overlay's recorder starts as early as it can; not in the inspect view.
  if (!inspecting) bootFeedback();
  const mount = mountArtifactView(document.getElementById("app")!, { id: r.id, pinnedVersion: r.version, file: r.file }, { boot: readBoot(), early: () => takeEarly() });
  // The Clax app's view (`?app=1`, and its inspect view): its host module,
  // with its styles, loads only here, so a browser's view carries none of it.
  if (appQuery(location.search)) void import("./app/host").then(m => m.installHost(window, mount.controller()));
  if (!inspecting) afterPaint(loadMoreMenu);
  // The view ended as the page was hidden (`ArtifactController`): a page the
  // back/forward cache restores loads again, live.
  addEventListener("pageshow", e => { if (e.persisted) location.reload(); });
}
