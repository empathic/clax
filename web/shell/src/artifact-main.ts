import "./owner-proof-install";
import { mountArtifactView } from "./artifact";
import { appQuery, inspectQuery, parseShellPath } from "./route";
import { loadMoreMenu } from "./ui/more-menu.svelte";
import { allowSidebarPrefetch } from "./ui/sidebar-chunk";
import { readBoot, takeEarly } from "./view/boot";
import { startDeciders } from "./view/deciders.svelte";

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
  const boot = readBoot();
  // What decides the top bar's optional controls and the thread list's
  // questions (the feedback menu first, so the overlay's recorder starts as
  // early as it can): the block's, else asked now, beside the view's data;
  // none in the inspect view.
  startDeciders(r.id, boot?.deciders ?? null, inspecting);
  // The top bar's controls' code, asked at once beside the view's data (and
  // modulepreloaded by the page): the bar shows its controls once it is in.
  if (!inspecting) loadMoreMenu();
  const mount = mountArtifactView(document.getElementById("app")!, { id: r.id, pinnedVersion: r.version, file: r.file }, { boot, early: () => takeEarly() });
  // The Clax app's view (`?app=1`, and its inspect view): its host module,
  // with its styles, loads only here, so a browser's view carries none of it.
  if (appQuery(location.search)) void import("./app/host").then(m => m.installHost(window, mount.controller()));
  // The view ended as the page was hidden (`ArtifactController`): a page the
  // back/forward cache restores loads again, live.
  addEventListener("pageshow", e => { if (e.persisted) location.reload(); });
}
