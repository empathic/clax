// The director (spec 2026-10-27-sizzle-reel-design §6.2): the only way a
// vignette touches time. It moves the camera's cursor along eased paths
// while real mouse events follow it, clicks, types at a human rhythm, holds
// a shot, waits for a state (never for a time) and marks beats. The camera
// decides what time is: in fast mode real time passes as the video records;
// in deterministic mode only `roll` advances the page's clock, a frame at a
// time.
import type { Browser, BrowserContextOptions, Locator, Page } from "@playwright/test";
import type { hostedDaemon } from "../e2e/hosted-helpers.ts";
import type { startDaemon } from "../e2e/fixtures.ts";
import { cursorPress, cursorTo } from "./cursor.ts";
import { CURVES, ease, travelMs, typingDelays } from "./pace.ts";

/** What films the camera's page. `frame()` is the frame the next roll
 * starts on; `beats` collects the director's beats with their first frames. */
export interface Camera {
  frameMs: number;
  roll(ms: number): Promise<void>;
  still(): Promise<void>;
  frame(): number;
  beats: { name: string; frame: number }[];
}

/** What a vignette's setup is handed: the browser, the camera's page (made
 * once, with the reel's viewport, scale, colour scheme and fixed wall clock;
 * `storageState` signs it in), the e2e harness's daemons, and `onStop` for
 * whatever must be stopped when the vignette ends, however it ends. */
export type Stage = {
  browser: Browser;
  newCamera(opts?: { storageState?: BrowserContextOptions["storageState"] }): Promise<Page>;
  startDaemon: typeof startDaemon;
  hostedDaemon: typeof hostedDaemon;
  onStop(fn: () => Promise<void> | void): void;
};

/** One vignette: `setup` makes its daemons, people and pages (off camera,
 * but for the camera's own page) and returns what `play` needs. */
export type Vignette<Cast = unknown> = {
  id: string;
  setup(stage: Stage): Promise<Cast>;
  play(d: Director, cast: Cast): Promise<void>;
};

/** How `until` reads and waits on real time; injected by the tests. */
export type PollClock = { now(): number; wait(ms: number): Promise<void> };

const realClock: PollClock = { now: () => Date.now(), wait: ms => new Promise(r => setTimeout(r, ms)) };

/** `until`'s waits between checks: as the e2e tests' `expect.poll` (web/e2e/fixtures.ts). */
export const POLL_INTERVALS = [10, 20, 50, 100];
/** How long `until` waits for its state before the vignette fails. */
export const UNTIL_MS = 30_000;
/** How long a control the director is given may take to show. */
export const VISIBLE_MS = 5_000;

/** How long a press is held, and the pause after it, in ms. */
const PRESS_HOLD_MS = 70;
const AFTER_CLICK_MS = 150;
const AFTER_KEY_MS = 120;

// Node runs the recorder by stripping its types, which leaves no room for
// TypeScript's parameter properties: the fields are spelled out.
export class Director {
  private readonly page: Page;
  private readonly camera: Camera;
  private readonly seed: string;
  private readonly clock: PollClock;
  private at: { x: number; y: number } | null = null;

  /** `seed` seeds the typing rhythm (the vignette's ID); `clock` is the real one but in tests. */
  constructor(page: Page, camera: Camera, seed: string, clock: PollClock = realClock) {
    this.page = page;
    this.camera = camera;
    this.seed = seed;
    this.clock = clock;
  }

  /** Waits for `target` to show; a control that never does fails the
   * vignette naming it, rather than filming the wrong screen. */
  private async visible(target: Locator): Promise<void> {
    try {
      await target.waitFor({ state: "visible", timeout: VISIBLE_MS });
    } catch (e) {
      throw new Error(`${String(target)} is not visible: ${e instanceof Error ? e.message.split("\n")[0] : String(e)}`, { cause: e });
    }
  }

  /** The cursor travels to `target`'s centre (moved by `opts.offset`) along
   * the standard curve, in `travelMs` of its distance, as real mouse moves
   * follow it, so hover states happen. */
  async move(target: Locator, opts: { offset?: { x: number; y: number } } = {}): Promise<void> {
    await this.visible(target);
    const box = await target.boundingBox();
    if (!box) throw new Error(`${String(target)} has no box on screen`);
    const to = { x: box.x + box.width / 2 + (opts.offset?.x ?? 0), y: box.y + box.height / 2 + (opts.offset?.y ?? 0) };
    await this.travel(to);
  }

  /** The cursor travels to (`to.x`, `to.y`) in viewport pixels. */
  private async travel(to: { x: number; y: number }): Promise<void> {
    const from = this.at ?? this.start();
    const steps = Math.max(1, Math.round(travelMs(Math.hypot(to.x - from.x, to.y - from.y)) / this.camera.frameMs));
    for (let i = 1; i <= steps; i++) {
      const e = ease(i / steps, CURVES.standard);
      const x = Math.round((from.x + (to.x - from.x) * e) * 10) / 10;
      const y = Math.round((from.y + (to.y - from.y) * e) * 10) / 10;
      await cursorTo(this.page, x, y);
      await this.page.mouse.move(x, y);
      await this.camera.roll(this.camera.frameMs);
    }
    this.at = to;
  }

  /** Where the cursor enters from: low in the viewport, right of centre. */
  private start(): { x: number; y: number } {
    const v = this.page.viewportSize() ?? { width: 1280, height: 800 };
    return { x: Math.round(v.width * 0.62), y: Math.round(v.height * 0.86) };
  }

  /** Moves to `target` and clicks it: the press ring, a real press and release. */
  async click(target: Locator): Promise<void> {
    await this.move(target);
    await cursorPress(this.page);
    await this.page.mouse.down();
    await this.camera.roll(PRESS_HOLD_MS);
    await this.page.mouse.up();
    await this.camera.roll(AFTER_CLICK_MS);
  }

  /** Clicks `target` and types `text` into it a key at a time, at about 14
   * characters a second with a jitter seeded by the vignette. */
  async type(target: Locator, text: string): Promise<void> {
    await this.click(target);
    const delays = typingDelays(text, this.seed);
    for (const [i, ch] of Array.from(text).entries()) {
      await this.page.keyboard.type(ch);
      await this.camera.roll(delays[i]!);
    }
  }

  /** Presses `key` (as Playwright names keys), then holds a moment. */
  async press(key: string): Promise<void> {
    await this.page.keyboard.press(key);
    await this.camera.roll(AFTER_KEY_MS);
  }

  /** Holds the shot for `ms`: the camera keeps rolling. */
  async hold(ms: number): Promise<void> {
    await this.camera.roll(ms);
  }

  /**
   * Waits until `cond` holds, checking after 10, 20 and 50 ms and then every
   * 100 ms, with the camera stilled (in deterministic mode no frame passes
   * while it waits). Fails after 30 s with `until: <what>`.
   */
  async until(cond: () => Promise<boolean>, what: string): Promise<void> {
    await this.camera.still();
    const deadline = this.clock.now() + UNTIL_MS;
    for (let i = 0; ; i++) {
      if (await cond()) return;
      if (this.clock.now() >= deadline) throw new Error(`until: ${what} (not within ${UNTIL_MS / 1000} s)`);
      await this.clock.wait(POLL_INTERVALS[Math.min(i, POLL_INTERVALS.length - 1)]!);
    }
  }

  /** Marks beat `name` at the camera's next frame, for beats.json. */
  beat(name: string): void {
    this.camera.beats.push({ name, frame: this.camera.frame() });
  }

  /** Runs `fn`: API calls and other people's actions, off camera. The
   * camera keeps its time: in deterministic mode no frame passes. */
  async offstage<T>(fn: () => Promise<T>): Promise<T> {
    await this.camera.still();
    return fn();
  }
}
