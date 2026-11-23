// The artifact view: the page skeleton (created, or adopted from the daemon's
// HTML), one ArtifactController, the frame host, and the three islands.
import { islands } from "./islands/svelte";
import { type ArtifactProps, ArtifactController, type ViewState } from "./view/artifact-controller";
import type { Boot } from "./view/boot";
import { FrameHost } from "./view/frame-host";
import { setBy } from "./view/publisher";
import { type Skeleton, skeleton } from "./view/skeleton";
import { fitTitleTags, reachesGallery, setTitleTags, tagsOf } from "./view/title-tags";

export { MOVE_TO_CLICK, MOVE_TO_PICK, pageWait } from "./view/artifact-controller";
export type { ArtifactProps } from "./view/artifact-controller";

/** `controller()`: the view's controller now (another artifact's `update`
 * replaces it). */
export type ArtifactMount = { root: HTMLElement; update(props: ArtifactProps): void; unmount(): void; controller(): ArtifactController };

/** Sets `el`'s text, leaving the DOM alone when it is already that. */
function setText(el: HTMLElement, text: string): void {
  if (el.textContent !== text) el.textContent = text;
}

/** Keeps the page's own parts in step with the view: the title and its
 * by-line once the artifact is known and the top bar shows its controls
 * (`shown`: so they come together, where the daemon did not write the
 * title into the page; empty until then, a placeholder bar each; "Clax"
 * when it could not be loaded), and the top bar's
 * comment-mode rule. The viewer stays in the page from the first frame on,
 * the stage and the drawer's box where they will be: until the view is
 * ready the stage says it is loading (`.stage-wait`, the static frame's),
 * and when the artifact could not be loaded the stage says why (`status`),
 * across its whole width. Touches the DOM only where something changed. */
function pageFollows(sk: Skeleton, s: ViewState, status: HTMLElement, shown: boolean): void {
  const ready = !s.error && !!s.data && s.origin !== undefined;
  const known = !s.error && !!s.data && shown;
  setText(sk.title, known ? s.data!.artifact.title : s.error ? "Clax" : "");
  setBy(sk.by, known ? s.data!.artifact : null);
  sk.title.parentElement!.classList.toggle("pending", !s.error && !known);
  if (sk.topbar.classList.contains("commenting") !== s.commenting) sk.topbar.classList.toggle("commenting", s.commenting);
  sk.topbar.classList.toggle("working", s.working.length > 0);
  if (ready) {
    if (status.isConnected) status.remove();
    if (sk.viewer.classList.contains("with-sidebar") !== s.panel) sk.viewer.classList.toggle("with-sidebar", s.panel);
    return;
  }
  if (!s.error) return;
  sk.viewer.querySelectorAll(":scope > :is(.drawer.held, .rail-held)").forEach(e => e.remove());
  if (status.className !== "empty") status.className = "empty";
  status.setAttribute("role", "alert");
  setText(status, s.error);
  if (status.parentElement !== sk.stage) sk.stage.append(status);
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
    // The tags beside the by-line, once the artifact is known: links once
    // the gallery answers this viewer (`reach`), asked when it first has
    // tags; their buttons load the popovers' chunk on the first press, and
    // the chips fit their line again as the bar's width changes.
    const ttl = sk.title.parentElement!;
    let reach: boolean | null = null;
    const pops = () => import("./ui/tag-editor");
    const editTags = (button: HTMLButtonElement) => void pops().then(m => m.toggleTagEditor(button, ctl), () => {});
    const listTags = (button: HTMLButtonElement) => void pops().then(m => m.toggleTagList(button, ctl, !!reach), () => {});
    const tagsFollow = (s: ViewState) => {
      const t = tagsOf(s);
      const shown = !s.error && !!s.data && !s.deleted;
      if (shown && t.tags.length && reach === null) {
        reach = false;
        void reachesGallery().then(ok => { reach = ok; if (ok) tagsFollow(ctl.state.get()); });
      }
      setTitleTags(ttl, shown ? t.tags : [], shown && t.may, !!reach, listTags, editTags);
      watchTags();
    };
    const refit = () => { const box = ttl.querySelector<HTMLElement>(":scope > .ttags"); if (box) fitTitleTags(box); };
    const resized = typeof ResizeObserver === "function" ? new ResizeObserver(refit) : null;
    resized?.observe(ttl);
    // The tags are laid out once the top bar shows its controls: they fit their line then, in the same task, before that frame paints.
    // The title written by the daemon shows from the first paint; else it is
    // written as the top bar shows its controls, in the same frame.
    let shown = !!sk.title.textContent;
    const barShown = () => { shown = true; pageFollows(sk, ctl.state.get(), status, shown); refit(); };
    sk.topbar.addEventListener("clax-bar-shown", barShown);
    // The tags' box too: its column narrows when the by-line's text grows, the title's box keeping its width.
    let watched: Element | null = null;
    const watchTags = () => { const box = ttl.querySelector(":scope > .ttags"); if (box && box !== watched) { watched = box; resized?.observe(box); } };
    const offPage = ctl.state.subscribe(s => {
      pageFollows(sk, s, status, shown);
      tagsFollow(s);
    });
    const offs = [islands.topbar(sk.topbarIsland, ctl), islands.stage(sk.stageIsland, ctl), islands.sidebar(sk.sidebarIsland, ctl)];
    // Without one, it starts once the islands are mounted: its requests
    // (the artifact and its threads, together) go out at once.
    if (!b) begin();
    return {
      ctl,
      stop: () => {
        for (const off of offs) off();
        offPage();
        resized?.disconnect();
        sk.topbar.removeEventListener("clax-bar-shown", barShown);
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
    controller: () => run.ctl,
  };
}
