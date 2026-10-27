// The reel's cameras (spec 2026-10-27-sizzle-reel-design §7): what turns
// the camera's page into a clip. The fast camera lets Playwright record the
// page's video (`recordVideo` on the camera's context, record.ts) while real
// time passes; it is for iterating on a vignette, seconds per run, never
// the same twice, and at the viewport's CSS size (Playwright scales a video
// down to fit, never up). The deterministic camera runs the page's clocks
// and animations on time of its own, which only it moves, and films the
// page a frame at a time at its device pixels: the same frames each run on
// one machine.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { CDPSession, Frame, Page, Request } from "@playwright/test";
import type { Camera } from "./director.ts";

/** The reel's frame rate: beats.json counts frames at it. */
export const FPS = 30;

/** The wall clock every camera page starts at, so relative times and dates
 * on screen read the same each run (spec §7.1). */
export const REEL_EPOCH = new Date("2026-11-03T09:41:00Z");

/** The fast camera for a video that began recording at `startedAt` (the
 * camera page's creation, `Date.now()`): its frame is the real time since. */
export function fastCamera(startedAt: number, fps = FPS): Camera & { elapsedMs(): number } {
  const frameMs = 1000 / fps;
  const elapsedMs = () => Date.now() - startedAt;
  return {
    frameMs,
    beats: [],
    elapsedMs,
    frame: () => Math.round(elapsedMs() / frameMs),
    // The one wait on real time in the recorder: the length of the shot,
    // while the video records it, not a stand-in for a condition.
    roll: ms => new Promise(resolve => setTimeout(resolve, ms)),
    // Real time passes regardless: there is nothing to pause.
    still: async () => {},
  };
}

/** The file of frame `n` (from 0) under a recording's directory. */
export const framePath = (out: string, n: number) => join(out, "frames", `${String(n).padStart(6, "0")}.png`);

/** A deterministic camera starts rolling at the first whole multiple of
 * this many ms of its page's clock (since REEL_EPOCH) at least
 * ROLL_MARGIN_MS ahead: a jump of at most 2.25 s, well under every
 * staleness rule of the shell's (its stream counts as dead after 35 s
 * unheard, DEAD_MS in web/shell/src/stream.ts), and a multiple of the
 * clock's 16 ms animation frame, so frames fall on the same phase each run. */
export const ROLL_GRID_MS = 2_000;
export const ROLL_MARGIN_MS = 250;
/** The most the camera lets its page's clock jump when it starts rolling. */
export const MAX_ROLL_JUMP_MS = 5_000;

// Pauses every animation of `this` (a document or a shadow root) at its
// first sight, on the camera's time `t` (ms), and sets each to its age in
// that time: CSS transitions and animations and the Web Animations API
// alike. `Animation.id` (shared by every world) keeps when it was first
// seen. A document's getAnimations() leaves out its shadow trees' (and
// brings its style up to date, which makes its new transitions).
// One whose age has reached its end is finished instead, so its finish
// event fires as it would for a person (Svelte's transitions and
// element.animate() callers wait for it).
const FREEZE_IN = `function (t) {
  for (const a of this.getAnimations()) {
    if (!a.id.startsWith("reel@")) { a.id = "reel@" + t; a.pause(); }
    const age = t - Number(a.id.slice(5));
    const end = a.effect ? a.effect.getComputedTiming().endTime : Infinity;
    if (Number.isFinite(end) && age >= end) a.finish();
    else a.currentTime = age;
  }
}`;
const FREEZE = (t: number) => `(${FREEZE_IN}).call(document, ${t})`;

// Resolves once every task the frame had queued has run: a message posted
// to itself (no clock involved) comes back after them.
const DRAINED = `new Promise(r => { const c = new MessageChannel(); c.port1.onmessage = () => r(0); c.port2.postMessage(0); })`;

/** How long a frame waits for the page's requests to finish before the
 * recording fails, naming them: a guard against a request that never ends,
 * not a pace. */
export const QUIET_MS = 10_000;

type Reach = { session: CDPSession; contextId?: number };
const sessions = new WeakMap<Page, { cdp: Promise<CDPSession>; frames: Map<Frame, Promise<Reach>> }>();

/** How `quietIn` reaches `frame` of `page`. */
async function reach(page: Page, frame: Frame, cdp: CDPSession): Promise<Reach> {
  if (frame === page.mainFrame()) return { session: cdp };
  const own = await page.context().newCDPSession(frame).catch(() => null);
  if (own) return { session: own };
  const { frameTree } = await cdp.send("Page.getFrameTree");
  type Tree = typeof frameTree;
  const find = (t: Tree): string | undefined => (t !== frameTree && t.frame.url === frame.url() && (t.frame.name ?? "") === frame.name() ? t.frame.id : (t.childFrames ?? []).map(find).find(Boolean));
  const frameId = find(frameTree);
  if (!frameId) throw new Error(`no frame ${frame.url()} in ${page.url()}`);
  const { executionContextId } = await cdp.send("Page.createIsolatedWorld", { frameId, worldName: "clax-reel" });
  return { session: cdp, contextId: executionContextId };
}

function cached(page: Page) {
  let s = sessions.get(page);
  if (!s) {
    s = { cdp: page.context().newCDPSession(page), frames: new Map() };
    sessions.set(page, s);
  }
  return s;
}

/** How `quietIn` reaches `frame` of `page`, made once per frame document. */
async function reachOf(page: Page, frame: Frame): Promise<Reach> {
  const s = cached(page);
  if (!s.frames.has(frame)) s.frames.set(frame, s.cdp.then(cdp => reach(page, frame, cdp)));
  return s.frames.get(frame)!;
}

/**
 * Evaluates `expression` in `frame` of `page` (awaiting a promise it
 * returns) through the DevTools protocol, granting the page no user
 * activation (Playwright's own evaluate would, as a gesture). A frame in a
 * process of its own is reached through its own session, in its own
 * globals; one in the page's process, in an isolated world of the
 * recorder's (the same document, none of the page's globals), the same one
 * each call while the frame keeps its document.
 */
export async function quietIn<T = unknown>(page: Page, frame: Frame, expression: string): Promise<T> {
  const s = cached(page);
  for (let attempt = 0; ; attempt++) {
    const { session, contextId } = await reachOf(page, frame);
    try {
      const r = await session.send("Runtime.evaluate", { expression, contextId, awaitPromise: true, userGesture: false, returnByValue: true });
      if (r.exceptionDetails) throw new Error(`${frame.url()}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
      return r.result.value as T;
    } catch (e) {
      // The frame's document was replaced, and its world with it: once more in a new one.
      if (contextId === undefined || attempt > 0 || !/context/i.test(String(e))) throw e;
      s.frames.delete(frame);
    }
  }
}

/**
 * The deterministic camera on `page`, writing `<out>/frames/NNNNNN.png`
 * (from 000000, so a beat's frame names its file) of the page's device
 * pixels. Its context must have Playwright's clock installed
 * (`context.clock.install`): the camera pauses it on the next ROLL_GRID_MS
 * step of its time (a jump of a second or two, which no staleness rule of
 * the shell's notices), and from then on the page's time moves only when
 * `roll` moves it. Timers, `Date`, `performance.now()` and animation frames run on
 * that clock in every frame of the page; its CSS and Web animations are
 * paused when first seen and set to their age on it. Network answers, the
 * daemon's events and the browser's own rendering run on real time, so a
 * `d.until` between frames waits for real state while no frame passes.
 * `roll(ms)` films the frames `ms` makes, counted on the total rolled so
 * far (so a run of short holds keeps to their sum); each is 1/`fps` s of
 * the clock (in whole ms); then every request the page made in it is
 * waited out and its answer handled (so what a timer fetched shows on the
 * frame the timer ran in, however long the network took), its animations
 * are set to their ages, and a PNG screenshot is taken of the viewport.
 * `still()` does nothing: between rolls the clock stands.
 */
export async function deterministicCamera(page: Page, out: string, fps = FPS): Promise<Camera> {
  const frameMs = 1000 / fps;
  const cdp = await page.context().newCDPSession(page);
  // The page's clock has run since REEL_EPOCH while the vignette set up.
  const ran = (await quietIn<number>(page, page.mainFrame(), "Date.now()")) - REEL_EPOCH.getTime();
  const rollAt = Math.ceil((ran + ROLL_MARGIN_MS) / ROLL_GRID_MS) * ROLL_GRID_MS;
  if (rollAt - ran > MAX_ROLL_JUMP_MS) throw new Error(`the camera's clock would jump ${rollAt - ran} ms to roll, over ${MAX_ROLL_JUMP_MS}`);
  await page.clock.pauseAt(REEL_EPOCH.getTime() + rollAt);
  mkdirSync(join(out, "frames"), { recursive: true });
  // The viewport at its device pixels (a screenshot is otherwise at its CSS size).
  const [width, height, scale] = await quietIn<[number, number, number]>(page, page.mainFrame(), "[innerWidth, innerHeight, devicePixelRatio]");
  const clip = { x: 0, y: 0, width, height, scale };
  let frames = 0;
  // The camera's time (ms since it started rolling): the end of the last frame.
  let now = 0;
  // Time rolled so far, in ms: the frames due are those it covers.
  let rolled = 0;

  // The page's requests in flight, but for the streams that never end.
  const pending = new Set<Request>();
  let settled: (() => void) | null = null;
  const endless = (r: Request) =>
    r.resourceType() === "eventsource" || r.resourceType() === "websocket" ||
    // The shell's event stream fetched as a stream (without a shared worker).
    new URL(r.url()).pathname === "/api/stream" || (r.headers()["accept"] ?? "").includes("text/event-stream");
  page.on("request", r => {
    if (!endless(r)) pending.add(r);
  });
  const done = (r: Request) => {
    if (pending.delete(r)) settled?.();
  };
  page.on("requestfinished", done);
  page.on("requestfailed", done);

  /** Pauses and sets the animations of every shadow tree `session` reaches
   * (closed ones too, which no script of the page can reach), as FREEZE does
   * a document's. */
  async function freezeShadows(session: CDPSession, t: number): Promise<void> {
    const { root } = await session.send("DOM.getDocument", { depth: -1, pierce: true });
    type Node = typeof root;
    const shadows: number[] = [];
    const walk = (n: Node): void => {
      for (const r of n.shadowRoots ?? []) {
        shadows.push(r.backendNodeId);
        walk(r);
      }
      for (const c of n.children ?? []) walk(c);
      if (n.contentDocument) walk(n.contentDocument);
    };
    walk(root);
    for (const backendNodeId of shadows) {
      const { object } = await session.send("DOM.resolveNode", { backendNodeId });
      if (!object.objectId) continue;
      await session.send("Runtime.callFunctionOn", { objectId: object.objectId, functionDeclaration: FREEZE_IN, arguments: [{ value: t }], userGesture: false, returnByValue: true });
      await session.send("Runtime.releaseObject", { objectId: object.objectId });
    }
  }

  // The renderers whose animation timelines are stopped: nothing the page
  // starts between two freezes moves on real time, however long the frame
  // takes (a 120 ms transition would otherwise finish unfilmed on a loaded
  // machine). Each animation is then set to its age on the camera's clock.
  const stopped = new Set<CDPSession>();
  async function stop(session: CDPSession): Promise<void> {
    if (stopped.has(session)) return;
    stopped.add(session);
    await session.send("Animation.enable");
    await session.send("Animation.setPlaybackRate", { playbackRate: 0 });
  }

  /** Pauses every animation of the page at its first sight, at `t`, and sets each to its age. */
  async function freeze(t: number): Promise<void> {
    const reached = new Set<CDPSession>();
    for (const frame of page.frames()) {
      const { session } = await reachOf(page, frame);
      await stop(session);
      await quietIn(page, frame, FREEZE(t));
      reached.add(session);
    }
    for (const session of reached) await freezeShadows(session, t);
  }

  /** Waits until the page has no request in flight and has handled every answer. */
  async function quiet(): Promise<void> {
    for (;;) {
      // Whatever the page queued runs, and the requests it made are heard:
      // the frames inside first, then the page, twice over, so what one
      // posted to another (in another process) has arrived and run.
      for (let pass = 0; pass < 2; pass++) for (const frame of page.frames().reverse()) await quietIn(page, frame, DRAINED);
      if (!pending.size) return;
      const finished = new Promise<boolean>(resolve => {
        settled = () => resolve(true);
      });
      let guard: ReturnType<typeof setTimeout> | undefined;
      const late = new Promise<boolean>(resolve => {
        guard = setTimeout(() => resolve(false), QUIET_MS);
      });
      const ok = await Promise.race([finished, late]);
      clearTimeout(guard);
      settled = null;
      if (!ok) throw new Error(`a frame waited ${QUIET_MS / 1000} s on ${[...pending].map(r => r.url()).join(", ")}`);
    }
  }

  return {
    frameMs,
    beats: [],
    frame: () => frames,
    async roll(ms: number) {
      rolled += ms;
      while (frames < Math.round(rolled / frameMs)) {
        const to = Math.round((frames + 1) * frameMs);
        // What started since the last frame started at its time.
        await freeze(now);
        await page.clock.runFor(to - now);
        now = to;
        await quiet();
        await freeze(now);
        const shot = await cdp.send("Page.captureScreenshot", { format: "png", clip, captureBeyondViewport: false });
        writeFileSync(framePath(out, frames), Buffer.from(shot.data, "base64"));
        frames += 1;
      }
    },
    still: async () => {},
  };
}
