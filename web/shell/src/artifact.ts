// The artifact view: the page skeleton (created, or adopted from the daemon's
// HTML), one ArtifactController, the frame host, and the three islands.
import { ISLANDS, type Islands } from "./islands";
import { type ArtifactProps, ArtifactController, type ViewState } from "./view/artifact-controller";
import { afterPaint } from "./view/after-paint";
import { FrameHost } from "./view/frame-host";
import { type Skeleton, skeleton } from "./view/skeleton";

export { MOVE_TO_CLICK, MOVE_TO_PICK, pageWait } from "./view/artifact-controller";
export type { ArtifactProps } from "./view/artifact-controller";

export type ArtifactMount = { root: HTMLElement; update(props: ArtifactProps): void; unmount(): void };

/** Sets `el`'s text, leaving the DOM alone when it is already that. */
function setText(el: HTMLElement, text: string): void {
  if (el.textContent !== text) el.textContent = text;
}

/** Keeps the page's own parts in step with the view: the title, and, until
 * the artifact and the frame mode are known (or when the artifact could not
 * be loaded), a message in the page in place of the viewer. Touches the DOM
 * only where something changed. */
function pageFollows(sk: Skeleton, s: ViewState, status: HTMLElement): void {
  const ready = !s.error && !!s.data && s.origin !== undefined;
  setText(sk.title, ready ? s.data!.artifact.title : "Clax");
  if (ready) {
    if (status.isConnected) status.remove();
    if (sk.viewer.parentElement !== sk.page) sk.page.append(sk.viewer);
    if (sk.viewer.classList.contains("with-sidebar") !== s.panel) sk.viewer.classList.toggle("with-sidebar", s.panel);
    return;
  }
  if (sk.viewer.isConnected) sk.viewer.remove();
  const cls = s.error ? "empty" : "empty muted";
  if (status.className !== cls) status.className = cls;
  setText(status, s.error ?? "Loading…");
  if (status.parentElement !== sk.page) sk.page.append(status);
}

/** Mounts the artifact view into `root`. `update` shows other props: for the
 * same artifact the controller keeps its state (`ArtifactController.update`)
 * and the islands render again; another artifact starts the view over. */
export function mountArtifactView(root: HTMLElement, props: ArtifactProps, islands: Islands = ISLANDS): ArtifactMount {
  type Running = { ctl: ArtifactController; sk: Skeleton; offs: (() => void)[]; stop(): void };
  const mountIslands = (sk: Skeleton, ctl: ArtifactController) => [islands.topbar(sk.topbarIsland, ctl), islands.stage(sk.stageIsland, ctl), islands.sidebar(sk.sidebarIsland, ctl)];
  const start = (p: ArtifactProps): Running => {
    const sk = skeleton(root);
    const status = root.ownerDocument.createElement("p");
    const ctl = new ArtifactController(p);
    ctl.frame = new FrameHost(sk.stage, () => ctl.frameLoaded());
    // Subscribed first, so the viewer is back in the page before the
    // controller puts the frame in it.
    const offPage = ctl.state.subscribe(s => pageFollows(sk, s, status));
    // The controller starts (its requests, listeners and stream) once the
    // loading page has painted, when the view's effects started before.
    const cancelStart = afterPaint(() => ctl.start());
    const run: Running = {
      ctl, sk, offs: mountIslands(sk, ctl),
      stop: () => {
        cancelStart();
        for (const off of run.offs) off();
        offPage();
        ctl.dispose();
        ctl.frame?.remove();
        sk.page.remove();
      },
    };
    return run;
  };
  let run = start(props);
  return {
    root,
    update: next => {
      if (next.id !== run.ctl.id) { run.stop(); run = start(next); return; }
      run.ctl.update(next);
      // The islands read the props from the controller, not from its state.
      for (const off of run.offs.splice(0)) off();
      run.offs.push(...mountIslands(run.sk, run.ctl));
    },
    unmount: () => run.stop(),
  };
}
