// The artifact view: the page skeleton (created, or adopted from the daemon's
// HTML), one ArtifactController, the frame host, and the three islands.
import { islands } from "./islands/svelte";
import { type ArtifactProps, ArtifactController, type ViewState } from "./view/artifact-controller";
import { afterPaint } from "./view/after-paint";
import type { Boot } from "./view/boot";
import { FrameHost } from "./view/frame-host";
import { publisherText } from "./view/gallery-model";
import { type Skeleton, skeleton } from "./view/skeleton";

export { MOVE_TO_CLICK, MOVE_TO_PICK, pageWait } from "./view/artifact-controller";
export type { ArtifactProps } from "./view/artifact-controller";

export type ArtifactMount = { root: HTMLElement; update(props: ArtifactProps): void; unmount(): void };

/** Sets `el`'s text, leaving the DOM alone when it is already that. */
function setText(el: HTMLElement, text: string): void {
  if (el.textContent !== text) el.textContent = text;
}

/** Keeps the page's own parts in step with the view: the title and its
 * by-line once the artifact is known, the top bar's comment-mode rule, and, until the frame mode is known too (or when the
 * artifact could not be loaded), a message in the page in place of the
 * viewer. Touches the DOM only where something changed. A frame the daemon
 * served is never in a viewer this takes out: it comes with the bootstrap,
 * which makes the view ready before this first runs. */
function pageFollows(sk: Skeleton, s: ViewState, status: HTMLElement): void {
  const ready = !s.error && !!s.data && s.origin !== undefined;
  setText(sk.title, !s.error && s.data ? s.data.artifact.title : "Clax");
  setText(sk.by, !s.error && s.data ? publisherText(s.data.artifact) : "");
  if (sk.topbar.classList.contains("commenting") !== s.commenting) sk.topbar.classList.toggle("commenting", s.commenting);
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

/** `boot`: the daemon's bootstrap (`readBoot`), for the first view only;
 * `early`: what the page heard before the shell listened (`takeEarly`),
 * replayed to the controller once it listens. */
export type MountOptions = { boot?: Boot | null; early?: () => Event[] };

/** Mounts the artifact view into `root`. `update` shows other props: for the
 * same artifact the controller keeps its state (`ArtifactController.update`),
 * which holds the props, so the islands redraw as on any change; another
 * artifact starts the view over. */
export function mountArtifactView(root: HTMLElement, props: ArtifactProps, opts: MountOptions = {}): ArtifactMount {
  // The bootstrap describes the first view only.
  let boot = opts.boot ?? null;
  type Running = { ctl: ArtifactController; stop(): void };
  const start = (p: ArtifactProps): Running => {
    const sk = skeleton(root);
    const status = root.ownerDocument.createElement("p");
    const b = boot?.artifact.artifact.id === p.id ? boot : null;
    boot = null;
    // A frame in the HTML is used only with its bootstrap.
    if (!b) sk.stage.querySelector(":scope > iframe.frame")?.remove();
    const ctl = new ArtifactController(p, { boot: b });
    ctl.frame = new FrameHost(sk.stage, () => ctl.frameLoaded());
    // The controller starts (its listeners and stream, and the requests the
    // bootstrap does not answer), then hears what the page heard before it
    // listened, in order.
    const begin = () => {
      ctl.start();
      for (const e of opts.early?.() ?? []) ctl.replay(e);
    };
    // With a bootstrap it starts at once: the view is ready before the page
    // follows it, so a served frame stays in the page and is adopted.
    if (b) begin();
    // Subscribed before a start without a bootstrap, so the viewer is back in
    // the page before the controller puts the frame in it.
    const offPage = ctl.state.subscribe(s => pageFollows(sk, s, status));
    // Without one, it starts once the loading page has painted, when the
    // view's effects started before.
    const cancelStart = b ? () => {} : afterPaint(begin);
    const offs = [islands.topbar(sk.topbarIsland, ctl), islands.stage(sk.stageIsland, ctl), islands.sidebar(sk.sidebarIsland, ctl)];
    return {
      ctl,
      stop: () => {
        cancelStart();
        for (const off of offs) off();
        offPage();
        ctl.dispose();
        ctl.frame?.remove();
        sk.page.remove();
      },
    };
  };
  let run = start(props);
  return {
    root,
    update: next => {
      if (next.id !== run.ctl.id) { run.stop(); run = start(next); return; }
      run.ctl.update(next);
    },
    unmount: () => run.stop(),
  };
}
