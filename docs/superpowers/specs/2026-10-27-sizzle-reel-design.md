# The sizzle reel: Clax's real UI in about 70 seconds

Status: draft; the owner's decisions of 2026-10-27 on the five open
questions (§14) are folded in.

## 1. Intent

A short video of Clax's real UI: a cold open, then six to eight vignettes
of six to eight seconds, each behind a one-line title card ("Realtime
collaboration", …), then an end card. About 70 seconds in full, and a
cut of about 45 seconds. The README shows a short animated GIF of it,
linked to the landing page; the landing page, the Clax Quickstart, plays
the full video once that is published (§11).

The reel is a **build artifact**. Its inputs live in the repository: the
vignette scripts, `reel.toml`, the card templates and the composer. Its
frames, clips, MP4, GIF and poster are build outputs, never committed.
Uploading is a separate, explicit, confirming step.

It is built and shipped in three phases, in this order:

1. **The README GIF.** The recorder, the deterministic mode, the three
   vignettes the GIF draws on (`point`, `version`, `realtime`), the GIF
   render (§9.4), its upload to the landing artifact and the README block.
2. **The full video, as a build output.** The remaining vignettes, the
   cards, the composition: `reel.mp4`, `reel-45.mp4` and the poster, built
   by `bazel build //web/reel:reel` and copied to `target/reel/`. Nothing
   is uploaded.
3. **Publishing the video (later).** Byte ranges on assets, the player on
   the landing page, and the upload of `reel.mp4` (the landing page's
   player and the GitHub release asset).

Phases 2 and 3 do not block phase 1, and phase 1 needs no MP4.

## 2. Owner decisions (2026-10-27, binding)

- D1. A script against the **real UI** drives each vignette: a scratch
  daemon and real browsers, reusing `web/e2e`'s harness (`startDaemon`
  and the API helpers in `web/e2e/fixtures.ts`, the people helpers in
  `web/e2e/people-helpers.ts` and `web/e2e/hosted-helpers.ts`, and the
  shell's test clock `claxTestClock`, `web/shell/src/clock.ts`). Each
  vignette is one script. The agent side is scripted through the API
  (register a session, publish the next version, reply on cue). No model
  runs.
- D2. Deterministic: a fixed viewport, scale, theme and fonts, seeded
  content, an injected visible cursor for the local person (remote
  cursors are Clax's own live layers), and eased human pacing.
- D3. Two recording modes: a **fast** mode (Playwright `recordVideo`) for
  iteration, and a **deterministic** mode (step the clock, one screenshot
  per frame) for final renders.
- D4. Composition with fframes from Clax's design tokens: title cards,
  transitions and the end card. `reel.toml` lists vignettes, titles and
  durations.
- D5. `bazel build //web/reel:reel` (tagged `manual`, so never in `//...`
  or `just ci`); `just reel` copies the outputs to the git-ignored
  `target/reel/`; `just reel <vignette>` renders one. Bazel's cache
  re-renders only what an input changed.
- D6. `just reel-upload` shows size, duration and source commit, asks,
  then uploads, and writes a manifest (commit, date, per-vignette
  hashes). Destinations: the landing artifact on clax.dev, updated in
  place so its URL is stable (primary), and a GitHub release asset at
  release time. What it uploads is phased (§10): the GIF first; the MP4
  (the landing page's player and the release asset) later.
- D7. The README shows a linked animated GIF (D12); the landing page
  plays the full video once it is published (phase 3).

Decisions on the open questions (2026-10-27, binding; §14):

- D8. **The landing page is the Clax Quickstart.** The repository keeps
  one clean, canonical version of it, `web/quickstart/`: it already ships
  in the web build, is seeded into every new gallery, and the clax.dev
  landing artifact is published from it. `reel-upload` attaches the reel
  to that artifact. Which clax.dev artifact it is gets fixed when it is
  first published (§9.3, `[upload]`).
  *Amended 2026-10-28 (owner):* the landing page is not a published
  artifact. It is the quickstart the release carries, which a hosted Clax
  serves at `/start/` and sends a signed-out visitor to from `/`
  (`clax_http::hosted::LANDING`, `clax_server::hosted`'s `landing`).
  Where the reel goes on that page, now that no clax.dev artifact holds
  it, is open (the record on `web/reel/reel.toml`'s `[upload]`).
- D9. **The encoder is x264**, through fframes' `h264` and
  `libav-agree-gpl` features. The GPL is acceptable because the composer
  is a developer tool that is never shipped; it does not reach Clax's
  binaries or the video (R2).
- D10. **Silent.** No audio track in the first version; a music bed may
  come later.
- D11. **Light theme only** in the first version; a dark render comes
  later.
- D12. **The README shows a linked animated GIF, rendered independently
  of the video** (§9.4): its own short loop (6 to 10 seconds) of the
  strongest beats, made from the recorded frames, not transcoded from
  `reel.mp4`, with its own size, frame rate and palette budget.
- D13. **The full video is a build output first.** `reel.mp4` is built
  and kept in `target/reel/`; publishing it (landing-page player,
  release asset) is a later phase, after the GIF and the README.
- D14. **The Clax app is out of scope** (unreleased): no vignette shows a
  phone or the app.

## 3. The vignettes

Each vignette is one script, `web/reel/vignettes/<id>.ts`, exporting a
`Vignette` (§6.2). "Camera" is the local person's browser, the one
recorded; the other browsers act but are not filmed. All copy on the
seeded pages is invented for the reel (a fictional team, "Northwind",
planning a launch); no real person or company appears. The Clax app is
unreleased and out of scope (D14): no vignette shows it.

| # | ID | Title card | In the 45 s cut | Seconds |
|---|---|---|---|---|
| 0 | `cold-open` | none | yes (5 s) | 6 |
| 1 | `point` | Point at anything | yes | 6.5 |
| 2 | `version` | A new version, in seconds | yes | 6.5 |
| 3 | `realtime` | Realtime collaboration | yes | 7 |
| 3a | `agent` | Ask your agent | yes | 12 |
| 4 | `history` | Every version kept | yes | 6 |
| 5 | `devserver` | Comment on your dev server | no | 6.5 |
| 6 | `more` | Pages that do more | no | 6.5 |
| 7 | `claxbot` | ClaxBot builds a page | no | 7 |
| 8 | `share` | Share to clax.dev | yes | 7 |

Full reel: 6 + 8 × (1.2 s card + vignette) + a 4 s end card ≈ 70 s. The
45 s cut: the cold open trimmed to 5 s, five vignettes behind 1.0 s
cards, the same end card ≈ 46 s. Durations are `reel.toml`'s; the table
is the starting point.

- **`cold-open`** (local daemon). Seed: an empty gallery but for two
  pinned pages. Beat: an agent session (`registerSession`, harness
  `claude`) publishes "Launch plan" (`publishAs`); its card arrives in
  the gallery live (the gallery's stream), the cursor opens it, the page
  fills the frame. Payoff: an agent's page has a home, seconds after it
  was made.
- **`point`** (local). Seed: "Launch plan" v3 (a chart and a table).
  Beat: the cursor enters comment mode, hovers the chart's March bar
  (the comment area outlines it), types "Make March stand out" in the
  composer, chooses Send to agent. Payoff: a comment pinned to exactly
  what was pointed at, handed to the agent.
- **`version`** (local). Continues `point`'s seed with its thread sent.
  Beat: the agent sets its working record (`setWorking`, the working
  pill shows "working on 1 thread"), publishes v4 addressing the thread
  (`publishNext` with `addresses`), replies "Highlighted March in the
  accent colour" (`replyAsAgent`). The page swaps to v4 in place; the
  thread shows the reply. Payoff: the round trip, with no page reload.
- **`realtime`** (a hosted daemon, as `web/e2e/live-layers.spec.ts`
  runs it: on a local daemon every browser is the owner). People: Alex
  (camera, the owner), Mia and Sam (`memberOf` with `rightsOn(aid)`).
  Seed: a shared to-do page ("Launch checklist") declaring the `db`
  capability. Beat: Mia's and Sam's pointers move over the list (their
  own `page.mouse` moves; Alex sees the shell's arrows and name tags);
  Mia starts a comment and her draft shows typing in Alex's view; Sam
  ticks "Book the venue", and the tick appears in Alex's list. Payoff:
  several people on one page, live.
- **`agent`** (hosted, as `realtime`, so a teammate can watch). People:
  Alex (camera, the owner) and Mia (a member, teal). Seed: an agent
  session (`registerSession`, harness `claude`) publishes "Northwind Ops"
  (`seed/ops-v1.html`), a launch-ops console declaring `db`: a sidebar,
  a metrics strip with a small SVG chart, and a board of the launch's
  tasks kept in the page's documents. Beat: Alex turns on comment mode,
  draws an area over the board and writes "@claude Timeline with
  dependencies + burndown"; the agent takes the thread (the working
  strip and the card's "working on it"), publishes v2
  (`seed/ops-v2.html`: the same tasks as a timeline with their
  dependencies drawn, a burndown, the critical path and a new theme)
  addressing the thread, and replies; Alex clicks Reload and v2 lands,
  the version button reading v2; Mia, on the same page, loads it too,
  points at the timeline and replies, which the agent sees. Payoff: an
  agent makes a major change to a real application while people watch
  and answer. Beats `ask`, `working`, `reveal`, `touch`: the composer
  runs fast through `working` and at real speed from `reveal`.
- **`history`** (local). Seed: "Launch plan" with six versions, each with
  a label and a note. Beat: the cursor opens Versions, scrubs back
  through v6 → v2 (the page changes under the scrubber), restores v2 as
  v7. Payoff: nothing an agent did is lost.
- **`devserver`** (local, the extension). Uses
  `web/e2e/extension-fixtures.ts`: a real Vite dev server on a copy of
  `web/e2e/live-site/` and Chromium with the unpacked test extension
  (`launchPersistentContext`, `channel: "chromium"`). Beat: the overlay
  opens on `localhost`, the cursor points at a button, comments
  "Rounder corners", sends it; the agent's reply arrives in the overlay.
  Payoff: comments on any page, including your own app. The extension
  runs in Playwright's Chromium (the `chrome-overlay` project does it
  today), but only in new headless (`channel: "chromium"`), which bears
  on the deterministic mode (§7.2).
- **`more`** (local). Seed: a "Launch metrics" page declaring `net` for
  a loopback data server the script starts (`net.spec.ts`'s pattern) and
  `db`. Beat: the approval bar asks; the cursor approves; the page's
  numbers fill in and one ticks up as the data server's value changes.
  Payoff: pages are apps, with permissions the person grants.
- **`claxbot`** (local). A daemon whose `[bot] base_url` names a scripted
  Messages API (`web/e2e/fake-messages.ts`, `fakeMessages` with
  `scriptTurn` and `finishTurn`), as `claxbot-daemon.spec.ts` starts it:
  no route is stubbed and no model runs. Beat: the cursor sends a thread
  on a bare "Launch day" page to ClaxBot ("Add a countdown to launch
  day"); the run panel follows its steps; the new version appears.
  Payoff: no agent installed, still a change.
- **`share`** (a local daemon and a hosted daemon standing in for
  clax.dev, signed in by device code as `web/e2e/linked.spec.ts`
  connects them). Beat: the cursor chooses Publish and pushes; offstage
  the hosted owner shares the public copy by link (`shareByLink`); a
  second browser (Priya, a guest, `guestOn`) opens it and comments; the
  comment arrives in the camera's thread list through the link. Payoff: share a page with
  anyone, and their comments come home. The composer's window frame
  shows the page's title, never a URL: the hosted daemon runs on
  `localhost`, and the frame must not claim it is clax.dev.

The vignette scripts reach the UI by role and label, as the e2e specs
do, so a renamed control fails a render loudly rather than filming the
wrong thing.

## 4. Where things live

```
web/reel/                      the recorder (TypeScript, the web pnpm project)
  reel.toml                    vignettes, titles, durations, cuts, the GIF, the landing artifact
  config.ts                    reads and checks reel.toml
  director.ts                  the beat API a vignette uses (§6.2)
  cursor.ts                    the injected cursor (§6.3)
  pace.ts                      easing and human timings
  camera.ts                    fast and deterministic capture (§7)
  record.ts                    entry point: one vignette → clip or frames
  vignettes/<id>.ts            one per vignette
  seed/                        seeded pages (HTML, data) the vignettes publish
  BUILD.bazel, reel.bzl        the Bazel targets (§8)
  *.test.ts                    vitest units for config, pace, director
tools/reel-compose/            the composer (Rust, a Cargo workspace of its own)
  Cargo.toml, Cargo.lock       fframes 1.3.0 and nothing from Clax
  src/main.rs                  subcommands: gif, clip, compose, poster, probe
  src/tokens.rs                design/tokens.json → card colours, type, motion
  src/gif.rs                   the README GIF: segments from frames (§9.4)
  src/cards.rs                 title card, end card, transitions (fframes Video)
  src/reel.rs                  the composition: scenes from reel.toml
  BUILD.bazel                  //tools/reel-compose:reel-compose
scripts/reel-upload.sh         the upload (§10), and its test
```

- `web/reel/` is part of the web pnpm project, so it imports
  `../e2e/fixtures.ts` and the helpers directly and uses the same
  `@playwright/test` and Chromium (`@chromium//:browsers`). It is its
  own Bazel package, so nothing in it is an input of `//web:dist`: an
  edit to a vignette rebuilds no web UI and no Rust. `web/package.json`'s
  `lint` and `web/tsconfig.json`'s `include` gain `reel`.
- `tools/reel-compose/` is a Cargo workspace of its own (its own
  `[workspace]` table, and listed in the root `Cargo.toml`'s `exclude`,
  as `crates/clax-worker` is), depending on no Clax crate. It is not a
  member of the root workspace, so `scripts/check-crate-layers.py`
  (which reads the root's `cargo metadata`) never sees it, `cargo
  nextest --workspace` and clippy over the workspace never build it, and
  the layering in `docs/usage.md` is unchanged. Its manifest forbids
  `unsafe_code` itself (`[lints.rust]`), and `scripts/check-no-unsafe.sh`
  is extended to scan `tools/` too: today it scans only `crates/`.
- Its outputs and the recorder's are Bazel outputs; `just reel` copies
  them into `target/reel/` (git-ignored already: `target/`).

## 5. The look

- **Canvas:** 1920 × 1080, 30 fps, light theme only (`prefers-color-scheme:
  light` forced in every browser context; a dark render is a later
  addition, D11). The camera's browser
  viewport is 1280 × 800 CSS pixels at device scale 1.25, so a capture is
  1600 × 1000 pixels; the composer places it in a window frame on the
  canvas (§9.2) and never scales it up.
- **Fonts:** the web UI uses the system font stack
  (`design/tokens.json` `font.web`), so the camera renders with the
  platform's fonts. Final renders are made on macOS; the manifest records
  the macOS and Chromium versions. A render on Linux (CI) is for review
  only and looks different. Chromium runs with
  `--font-render-hinting=none` and `--disable-lcd-text`. The cards use
  Inter (SIL OFL 1.1), fetched by Bazel by checksum (`http_archive` in
  `MODULE.bazel`), never committed and never a system font.
- **Colours and motion** come from `design/tokens.json`: `bg`, `fg`,
  `muted`, `agent` (the accent), `you`, `card`, `border`, the
  `radius` and `elevation` scales, and `motion.curves` (`standard`,
  `decelerate`, `accelerate`) with `t-enter` (300 ms) and `t-exit`
  (200 ms). The Clax mark is drawn from `design/mark.json`. A token
  change re-renders the cards and nothing else.
- **Seeded content:** fixed names (Alex, Mia, Sam, Priya; agent sessions
  named `planner` and `builder`), fixed page copy under `web/reel/seed/`,
  and a fixed wall clock (§7.1) so "2 min ago" reads the same each run.

## 6. The recorder

### 6.1 One vignette, one process

`node reel/record.ts --vignette <id> --mode fast|deterministic --out
<dir>` starts what the vignette asks for (a local daemon by
`startDaemon`, or a hosted one by `hostedDaemon`, each on a scratch
`CLAX_HOME` under the action's temporary directory and a port the system
picks, never `~/.clax` or 7480), launches Chromium from
`PLAYWRIGHT_BROWSERS_PATH`, runs the vignette, writes its output, and
stops every daemon it started by the handle `startDaemon` returned. It
needs `CLAX_E2E_BIN` and `CLAX_E2E_WEB_DIST`, as the e2e tests do; the web
UI is `//web:dist_e2e`, the build with the test clock, whose production
bundle differs from `//web:dist` only by that clock
(`web/scripts/bundle-size.mjs` checks the production one carries none).

Output: in fast mode `clip.webm` and `beats.json`; in deterministic mode
`frames/000000.png …` and `beats.json`. `beats.json` names each beat
with its first frame, so the composer can trim a vignette to a beat.

### 6.2 The director

A vignette is

```ts
export type Vignette = {
  id: string;
  setup(stage: Stage): Promise<Cast>;     // daemons, people, seeded pages
  play(d: Director, cast: Cast): Promise<void>;
};
```

and the `Director` is the only way a vignette touches time:

- `d.move(target: Locator, opts?)`: the injected cursor travels to the
  target's centre along an eased path while real `mouse.move` events
  follow it (hover states happen). Duration `pace.travel(px)`: 250 ms +
  0.6 ms per pixel, at most 900 ms, `motion.curves.standard`.
- `d.click(target)`, `d.type(target, text)` (about 14 characters a
  second with a deterministic jitter from the vignette ID),
  `d.press(key)`.
- `d.hold(ms)`: the camera keeps rolling.
- `d.until(cond: () => Promise<boolean>)`: waits for a state (as
  `expect.poll` does), never for time. In fast mode real time passes; in
  deterministic mode no frame is taken while it waits (§7.2).
- `d.beat(name)`: marks a beat in `beats.json`.
- `d.offstage(fn)`: runs API calls and other people's actions (the
  agent publishing, Mia typing); the camera keeps its time.

No vignette calls `setTimeout`, `page.waitForTimeout` or a sleep: AGENTS.md
forbids sleeps standing in for a condition, and pacing is the director's.

### 6.3 The cursor

`cursor.ts` is added to the camera page by `page.addInitScript`: a
closed shadow root on the top document holding an arrow (the system's
look, 20 px at device scale) with `pointer-events: none` and the highest
z-index, a soft press ring on click, hidden until the first move. The
director moves it; the page never does. It is the camera's alone: other
people's pointers are Clax's live layers, which the shell draws.

## 7. Recording modes

### 7.1 Both modes

- One Chromium, the pinned `@chromium//:browsers`; the camera's context
  has the viewport, scale and colour scheme of §5, `reducedMotion:
  "no-preference"`, locale `en-US`, timezone `UTC`.
- Wall time in the camera's pages starts at `REEL_EPOCH =
  2025-11-03T09:41:00Z`: Playwright's `clock.install({ time: REEL_EPOCH })`
  on the camera's context. It flows while the vignette sets up; a
  deterministic camera then pauses it on the next whole 2 s of that time
  (a jump of under 2.25 s, well inside every staleness rule of the
  shell's, such as the stream's 35 s), so dates on screen read the same
  each run (to within the setup's few seconds). Times the daemon stamps
  are real ones, not the page's; the epoch is in the past so a page
  behind the daemons reads what they stamp as "just now" and a working
  clock as 0:00 on any day.
- The shell's own timers (consent arming, the hint, presence reports)
  run on its test clock; the director advances it with `advance()` from
  `web/e2e/time.ts` where an e2e test would, never for pacing.

### 7.2 Fast and deterministic

- **Fast:** `recordVideo` at 1600 × 1000 on the camera's context.
  Real time; frames are whatever Chromium painted. For iterating on a
  script: seconds per vignette, not identical twice.
- **Deterministic** (`web/reel/camera.ts`): the camera's page runs on
  its own clock, which only the camera moves. Playwright's clock (timers,
  `Date`, `performance.now()`, `requestAnimationFrame`, in every frame of
  the page) is paused at the roll; the DevTools Animation domain's
  playback rate is 0 on every renderer of the page, so no animation moves
  on real time; and every animation (shadow trees included, found through
  `DOM.getDocument` with `pierce`) is paused when first seen and set to its
  age on the camera's clock, or finished once its age reaches its end, so
  its finish event fires. `Math.random` is seeded in the camera's
  documents. A frame is: run the clock 1000/30 ms (in whole ms), wait out
  the page's requests (but the event stream) and drain every frame's task
  queue, set every animation to its age, `Page.captureScreenshot` (PNG, the
  viewport at its device pixels). Network answers, the daemon's events and
  Chromium's rendering run on real time, so between frames a `d.until`
  waits for real state while no frame passes; what another person does is
  waited for in the camera's page before the film moves on. The result is
  one PNG per frame, identical on the same machine and inputs (§12, R1).
- The `devserver` vignette needs the extension and so new headless; if
  the deterministic camera does not drive its frames, that vignette renders in fast
  mode in the final reel too, and the manifest says so. Task 2 of the
  plan finds out.

## 8. Under Bazel

| Target (`//web/reel:…`) | What it is |
|---|---|
| `<id>_fast` | the vignette in fast mode: `clip.webm`, `beats.json` |
| `<id>_frames` | the vignette in deterministic mode: `frames/`, `beats.json` |
| `<id>_clip` | `<id>.mp4`: the frames encoded by `reel-compose clip` |
| `reel_gif` | `reel.gif` from the `<id>_frames` of the vignettes `[gif]` names and `reel.toml`; independent of every `<id>_clip` and of `reel` (§9.4) |
| `reel` | `reel.mp4`, `reel-45.mp4`, `reel-poster.png` from every `<id>_clip` and `reel.toml` |
| `reel_fast` | the same as `reel` from every `<id>_fast` |

- Every target is tagged `manual`, so `//...`, `just ci` and `just
  test-affected` never build one. The recorder targets are also
  `no-sandbox` (as the browser tests are, `docs/bazel.md` "Browser
  tests"), `no-remote` and `cpu:4`.
- Inputs of `<id>_frames`: the vignette, `web/reel/*.ts`, `web/reel/seed/`,
  `reel.toml`, the `web/e2e` helpers it imports, `//crates/clax-cli:clax`
  and `:clax_warm`, `//web:dist_e2e`, `//web:dist_extension_test`
  (the `devserver` vignette), `:node_modules` and `@chromium//:browsers`.
  So a Rust or web UI change re-renders every vignette, which is right
  (the UI is the subject); a vignette edit re-renders that vignette and
  the composition; a card or token edit re-composes without
  re-recording.
- `bazel/js/defs.bzl`'s `npm_script_build` runs a script of the package
  it is called in; it gains `project` (default: the calling package) and
  `skips`, so `//web/reel` targets run `web/package.json`'s
  `reel:record` with the binary and the browsers read in place, as
  `npm_script_test` does.
- The composer builds with a second crate_universe hub,
  `@reel_crates`, from `tools/reel-compose/Cargo.toml` and its own
  `Cargo.lock`. **This is not hermetic**, and says so: fframes reaches
  FFmpeg through `ffmpeg-sys-fframes` 9.0.0, whose build script
  downloads a prebuilt static FFmpeg from that crate's GitHub releases
  (no checksum) or compiles FFmpeg from a git clone (nasm and clang
  needed), runs bindgen against libclang, and with the `h264` feature
  links the system's x264 (Homebrew `x264`; `libx264-dev` on Linux). x264
  was chosen over macOS's VideoToolbox encoder (D9): it is a software
  encoder with `crf`, `preset` and `tune` controls, it runs the same on
  macOS and on the Linux CI runner, and the GPL it carries stays in a
  developer tool that is never shipped. Its
  build-script action is tagged `requires-network`, `no-sandbox`,
  `no-remote`, and gets `LIBCLANG_PATH` and `FFMPEG_BINARIES_CACHE`
  (under `~/.cache/clax-reel/ffmpeg`, outside every worktree). Once
  built, the composer is cached like any output, in the cache every
  worktree shares.
- **fframes can composite video clips.** fframes 1.3.0
  (<https://crates.io/crates/fframes/1.3.0>, API at
  <https://docs.rs/fframes/1.3.0/fframes/>) renders each frame from an
  SVG tree (`Video::render_frame` → `svgr!`), places scenes back to back
  (`Video::define_scenes`), and embeds a video file's frame in a scene
  through `Frame::get_synced_video_frame(ctx, name,
  &SyncVideoFrameInput)` → `FFramesSyncedVideoFrame::into_image`, with the
  file registered by `DynamicMediaProvider::with_video`. So one fframes
  `Video` is the whole reel: cards, transitions, the window frame and the
  clips, encoded once (`fframes::render`, `EncoderOptions` with
  `codec_params` `crf`, `preset`, `tune`). No ffmpeg command line is
  needed. Its `Previewer` renders single frames to PNG (the poster) and
  `Previewer::inspect` reports missing fonts and clipped text (a test).
- `just reel --gif` builds `//web/reel:reel_gif` and copies `reel.gif` and
  `build.json` into `target/reel/` (phase 1). `just reel` builds `//web/reel:reel` and copies its files into
  `target/reel/` with `build.json` (the commit `git rev-parse HEAD`, and
  whether the reel's inputs were dirty); `just reel <id>` builds
  `<id>_clip` (and `--fast`, `<id>_fast`) and copies it; `just reel
  --fast` builds `reel_fast`. All go through `scripts/run.sh`; without
  Bazel the recipe refuses and says the reel needs Bazel (no native
  twin: the recorder's native run would need `pnpm install` in the
  worktree).

### 8.1 The composer

`reel-compose` subcommands, each a pure function of its inputs:

- `clip <frames-dir> --fps 30 -o <id>.mp4` (phase 2): a fframes `Video` whose
  frame `n` is the vignette's PNG `n`, x264 at `crf 12`, `preset slow`,
  `tune animation`: near-lossless, so composition re-encodes once.
- `compose --config reel.toml --cut full|45 --clips <dir> --tokens
  design/tokens.json --mark design/mark.json -o reel.mp4`: the reel.
  Final encode H.264 High, yuv420p, BT.709, `crf 20`, `preset slow`,
  `tune animation`, faststart, no audio track. Budget: under 18 MiB
  (the asset limit is 20 MiB, `asset_too_large`).
- `gif --config reel.toml --frames <dir> --tokens design/tokens.json --font-dir <dir> -o reel.gif`:
  the README GIF, from the vignettes' PNG frames and never from an MP4
  (§9.4). Budget: under 2 MiB.
- `poster --config reel.toml --clips <dir> -o reel-poster.png`: the
  frame `reel.toml` names (`[poster] at`).
- `probe <file>`: JSON `{duration_s, width, height, bytes}`, for
  `reel-upload` and tests.

### 8.2 CI (phase 3)

A workflow of its own, `.github/workflows/reel.yml`: weekly
(`schedule`) and on demand (`workflow_dispatch`), on a Linux runner,
`apt-get install libx264-dev nasm clang`, then `bazelisk build
//web/reel:reel` and `actions/upload-artifact` with `retention-days: 7`.
It uploads nothing anywhere else and never gates a merge. Its fonts are
Linux's (§5): it is a check that the reel still renders, not a release.

## 9. The composition

### 9.1 Structure

`[cold open] → ([card] → [vignette])× → [end card]`. Transitions: a card
enters over the outgoing vignette's last 300 ms (`t-enter`,
`decelerate`) and leaves by a 200 ms cross-fade into the next vignette
(`t-exit`, `accelerate`). No other effects.

### 9.2 The cards

- **Title card:** the `bg` colour, the Clax mark at 64 px in the top
  left corner, the title in Inter 600 at 88 px in `fg`, centred, one
  line (the composer fails a title wider than 1600 px rather than
  wrapping it), and an `agent`-coloured underline that draws across in
  `t-enter`.
- **Window frame** around each vignette: a `card` rectangle with
  `radius.lg`, `elevation.2`'s shadow, a 44 px title bar with three dots
  and the page title in `muted` (no URL; §3 `share`), centred on `bg`.
- **End card:** the mark at 128 px, "Your agents build pages. Clax gives
  them a home." (the quickstart's headline) in Inter 600 at 64 px, and
  "clax.dev" in `agent` at 40 px.

### 9.3 `reel.toml`

```toml
[reel]
fps = 30
size = [1920, 1080]
capture = { viewport = [1280, 800], scale = 1.25, theme = "light" }
card_seconds = { full = 1.2, short = 1.0 }

[[vignette]]
id = "cold-open"
seconds = 6
short_seconds = 5
# no title: the cold open has no card

[[vignette]]
id = "realtime"
title = "Realtime collaboration"
seconds = 7
in_short = true
# … one table per vignette, in reel order (§3)

[end]
seconds = 4
headline = "Your agents build pages. Clax gives them a home."
footer = "clax.dev"

[poster]
vignette = "realtime"
at = 3.5

[gif]                       # §9.4: its own render, not the video's
fps = 15
size = [720, 450]           # the capture's 16:10, scaled; the caption bar is added below it
caption_px = 36
max_mib = 2

[[gif.segment]]
vignette = "point"
beat = "pointed"
offset = -0.3               # seconds from the beat
seconds = 2.5
caption = "Point at anything"

[[gif.segment]]
vignette = "version"
beat = "v4"
offset = -0.3
seconds = 2.5
caption = "A new version, in seconds"

[[gif.segment]]
vignette = "realtime"
beat = "tick"
offset = -1.0
seconds = 3
caption = "Realtime collaboration"

[upload]
landing_artifact = ""       # the landing artifact's ID on clax.dev (D8)
landing = ""                # its public URL
```

`[upload]` is empty until the Quickstart is first published to clax.dev
from `web/quickstart/`; the commit that follows that first publish sets
both keys (the ID is the last path segment of the URL, and `config.ts`
checks that it is). `reel-upload` and the README block refuse and wait
while they are empty. The local artifact the landing artifact is linked
to is per machine, in `~/.config/clax/reel/upload.toml` (§10), never in
the repository.

`config.ts` and the composer read the same file and refuse: an unknown
key, a vignette ID with no script, a title in a vignette marked
`cold-open`, a duration outside 3–12 s, a full reel outside 60–80 s, a
short cut outside 40–50 s, a `[gif]` of fewer than two or more than four
segments or outside 6–10 s in all, a `[gif]` segment naming a vignette or
beat that does not exist, and a `landing` that does not end in
`landing_artifact`.

### 9.4 The README GIF

The GIF is **its own short render**, not a transcode of `reel.mp4`. It
reads the same deterministic frames the video's clips are made from
(`<id>_frames`, `beats.json`) and nothing else of the video: no cards, no
window frame, no transitions, no H.264 generation loss before the
palette. That lets it be sized and paced for a README, and be built in
phase 1 with no MP4 at all.

- **Cut list.** `[gif.segment]` tables (§9.3) name a vignette, a beat in
  its `beats.json`, an offset and a length: the strongest beats, 6 to 10
  seconds in all, three segments in the draft (a comment pinned to a
  chart, the new version swapping in, three people on one page, 8 s).
  Segments join with hard cuts; the loop restarts at the first.
- **Size and pace.** 720 px wide (a README column, shown at `width=720`),
  the capture's 16:10 scaled to 720 × 450 plus a 36 px caption bar in
  `card` with the segment's caption in Inter at `fg`; 15 fps, from every
  second source frame, with delays of 7, 7 and 6 centiseconds in turn
  (the GIF format counts in hundredths of a second).
- **Palette and encoding.** One global palette of 256 colours, quantised
  by the `gif` crate's NeuQuant over a sample of every segment's frames,
  no dithering (the UI is flat colour); each frame after a segment's
  first stores only the rectangle that changed, unchanged pixels
  transparent. Budget: under 2 MiB (`max_mib`); the composer fails above
  it and prints the frame that grew most, so the cut list or the size is
  tuned, never the palette quietly cut.
- **Deterministic.** The same frames and `reel.toml` give the same GIF,
  byte for byte, on one machine.

## 10. Uploading

`just reel-upload [--video] [--release vX.Y.Z]` runs
`scripts/reel-upload.sh`. Phase 1 builds the GIF part; the `--video` part
and the release's MP4 are phase 3 and do not exist before it.

1. Reads `target/reel/build.json`. Refuses when the build's inputs were
   dirty, or `HEAD` is not the build's commit (rebuild with `just reel
   --gif`, or `just reel` for the video).
2. Prints what it will upload with its size in MiB (`reel.gif`; with
   `--video`, `reel.mp4` too, and its duration by `reel-compose probe`),
   the source commit and its subject, and each destination; asks
   `Upload? [y/N]`; anything but `y` stops with nothing changed. There is
   no flag that skips the question.
3. **clax.dev (always).** Through the owner's installed Clax: the `clax`
   on `PATH`, refused when it resolves into a worktree or `bazel-bin`
   (AGENTS.md: no unmerged build on `~/.clax`). The local artifact is the
   one in `~/.config/clax/reel/upload.toml` (`artifact = "<ID>"`); the
   script checks with `GET /api/artifacts/<ID>/remotes` that its link's
   `url` is `reel.toml`'s `[upload] landing`, and refuses otherwise.
   It stages a scratch directory (copies, never links) holding the
   Quickstart's files exactly as `web/quickstart/` publishes them
   (`clax publish index.html --dir web/quickstart`) plus `reel.gif` and
   `reel-manifest.json`, and publishes that as a version labelled `Reel
   <short commit>`. The landing artifact is published only this way,
   because a publish carries exactly the files it names and the push
   removes any file the release lacks: publishing `web/quickstart/` alone
   would take `reel.gif` out from under the README (R8). Then `clax push
   <ID>`. The public artifact, and so its URL, stays the same.
   With `--video` (phase 3): `clax asset upload <ID> reel.mp4
   reel-poster.png --json` first, and the version also carries
   `reel.json` (below), so `push` uploads the assets the version names
   to the public copy once and rewrites their addresses
   (`docs/contract.md`, "Pushing").
4. **The release (with `--release`).** `gh release upload vX.Y.Z
   clax-reel.gif clax-reel-manifest.json`, and with `--video`
   `clax-reel.mp4`, after asking again; refuses a release that already
   has those assets (no `--clobber`). `scripts/release.sh`'s last step
   ("what is left by hand") names this command.
5. Writes `target/reel/manifest.json` and prints the landing URL.

`reel.json` (phase 3), the page's pointer to its video:

```json
{"video": "/_clax/blob/<asset ID>", "poster": "/_clax/blob/<asset ID>",
 "duration_s": 71.2, "commit": "<sha>", "date": "2026-11-03"}
```

The manifest: `{commit, date, macos, chromium, mode: {<id>:
"deterministic"|"fast"}, clips: {<id>: "<sha256>"}, outputs: {"reel.gif":
"<sha256>", …}}`; `clips` holds the hash of each vignette's frame set the
GIF (and, later, the video) was made from.

## 11. Where it is shown

### 11.1 The landing page (phase 3: the video)

`web/quickstart/` is the canonical Quickstart (D8): one clean version in
the repository, which the web build embeds in the binary, which is seeded
into new galleries, and from which the clax.dev landing artifact is
published (§10). Phase 1 changes nothing on the page itself: the reel
reaches it as a file, `reel.gif`, that the README links to. The player
below is phase 3.

The page contract keeps a page to its own files and assets
(`docs/contract.md`, "Page contract"; the site's lockdown policy,
`crates/clax-http/src/content_policy.rs`, `default-src 'self' data:
blob:`), so the video is an **asset of the landing artifact**, served at
`/_clax/blob/<asset ID>` on its own site, never a URL elsewhere.

- `web/quickstart/index.html` gains, after the lede, a hidden
  `<figure class="reel">`. On load the page fetches its own
  `reel.json`. When it parses and names `video`, the figure shows a
  `<video controls playsinline preload="metadata">` with that `src` and
  `poster`, and the duration as a caption ("Clax in 70 seconds"). No
  autoplay.
- **Seeded copies** (the binary's quickstart, `clax_core::quickstart`,
  copied into every new gallery) never carry the video: `reel.json` is
  not in `web/scripts/build-quickstart.mjs`'s `SHIPPED`, assets are per
  artifact, and `Release::request` removes any path the release lacks.
  Their `reel.json` fetch is a 404 (harmless, as `ideas-data.js`'s
  already is), and the figure shows instead a link card, "Watch Clax in
  70 seconds on clax.dev", to `reel.toml`'s landing URL, written into
  the page as a constant beside `CLAXDEV`. On clax.dev itself before the
  first upload, the figure stays hidden.
- **Byte ranges.** Safari plays a `<video>` only from a server that
  answers `Range` with 206, and seeking needs it everywhere.
  `serve_blob` (`crates/clax-server/src/routes/assets.rs`) answers every
  request 200 with the whole asset. It gains single-range support:
  `Accept-Ranges: bytes` on every answer; `Range: bytes=a-b`, `a-` and
  `-n` → 206 with `Content-Range`; an unsatisfiable range → 416 with
  `Content-Range: bytes */<size>`; a multi-range or malformed header → the
  whole asset, 200. The hosted Worker serves assets through the same
  route. Each range still reads the whole asset into memory first, an
  existing concern (record `a7ee35dc`), which a video's many range
  requests make more frequent.

### 11.2 The README (phase 1)

GitHub plays inline video only for attachments uploaded through its own
editor; an `.mp4` linked from anywhere else is a link. The README shows
a **linked animated GIF** (D12), the independent render of §9.4:

```html
<p align="center">
  <a href="<landing URL>"><img src="<landing site>/reel.gif" width="720"
     alt="Clax: a person points at a chart and comments, the agent ships
     a new version, and three people work on one page at once"></a>
</p>
```

below the opening paragraph. `reel.gif` is a **published file** of the
landing artifact, not an asset, so its address
(`https://<landing artifact ID>.<site domain>/reel.gif`, the current
version's file) never changes: each `reel-upload` replaces the GIF behind
the same URL, and the README is edited once, when `[upload]` is set,
never per upload. GitHub's image proxy caches it; a new GIF can take
hours to show there. The release's `clax-reel.gif` is the copy that lasts
with a version. The full video is reached from the GIF's link, the
landing page, once phase 3 publishes it.

## 12. Risks

- R1. **A deterministic clock and Clax's realtime UI.** Chromium's
  virtual time was the first design and could not film `realtime`: its
  screenshots stopped returning once pointers moved over the artifact
  frame, and the bridge's 25 ms snapshot timer never ran while it was
  paused. The camera as built (§7.2) steps the page's own clock and
  animations instead; what crosses from the network or another renderer
  is waited for (requests per frame, other people's steps in the
  vignette), and `//web/reel:realtime_determinism` checks two recordings
  frame for frame.
- R2. **The composer's toolchain.** FFmpeg arrives unpinned (§8);
  x264 is GPL (`libav-agree-gpl`); a first build compiles FFmpeg when no
  prebuilt archive matches. The composer is a developer tool never
  shipped with Clax, so the GPL does not reach Clax's binaries or the
  video; the owner accepted this (D9, Q2). The GIF step needs fframes
  only for its caption, so phase 1 still builds FFmpeg.
- R3. **Every UI change re-renders every vignette** under Bazel. A final
  render is minutes of Chromium per vignette; only `just reel` pays it,
  on demand.
- R4. **Fonts differ by machine** (§5). Final renders on macOS only.
- R5. **The landing page's assets grow** by one video per upload on
  clax.dev and locally: `push` uploads new assets and deletes none.
- R6. **A hosted daemon in `realtime` and `share`** shows the hosted
  shell's chrome where it differs from a local one (its sign-in name in
  the top bar).
- R7. **The image proxy and the site domain.** Camo must fetch
  `reel.gif` from the landing artifact's site without a cookie; a public
  page's files are served to anyone, but this is checked on the first
  upload, before the README links it.
- R8. **Republishing the Quickstart drops the reel's files.** A publish
  of `web/quickstart/` alone carries neither `reel.gif` nor `reel.json`,
  and the push removes what the release lacks. The landing artifact is
  therefore published only by `reel-upload`, which stages the
  Quickstart's files with the reel's (§10); a publish by hand breaks the
  README's image until the next `reel-upload`.

## 13. Review Focus

- A vignette script that drifts from the UI (a renamed control) fails the
  render naming the locator, rather than filming a wrong screen.
- Two renders of one vignette in deterministic mode on one machine give
  byte-identical frames; the first divergence is the bug to find.
- The GIF is made from frames, never from `reel.mp4`: it builds, and is
  under 2 MiB, with no MP4 present; it loops through its 6 to 10 seconds
  of segments; two builds from the same frames are byte-identical.
- The landing artifact is never published without `reel.gif`: a
  `reel-upload` stages the Quickstart's files and the reel's together.
- `reel-upload` never reaches clax.dev without the person's `y`, never
  runs a worktree's `clax`, and never pushes to an artifact whose link
  is not the landing URL. Without `--video` it uploads no MP4.
- (Phase 3.) A seeded quickstart never shows a broken video: no `reel.json`, a
  `reel.json` naming a missing asset, and a `reel.json` that does not
  parse all end in the link card or nothing.
- (Phase 3.) A Safari viewer of the landing page can play and seek the
  video (ranges, §11.1).

## 14. Open questions for the owner

Decided by the owner on 2026-10-27; the decisions are D8 to D14 (§2).

- Q1. The landing page: **decided 2026-10-27.** The Clax Quickstart is
  the landing page; `web/quickstart/` is its canonical, clean version and
  the clax.dev landing artifact is published from it. `reel-upload`
  attaches the reel to that artifact. The clax.dev artifact's ID is fixed
  when the Quickstart is first published and then set in `web/reel/reel.toml`
  `[upload] landing_artifact` and `landing` (§9.3); the per-machine local
  artifact stays in `~/.config/clax/reel/upload.toml` (§10).
- Q2. x264 or VideoToolbox: **decided 2026-10-27, x264.** The GPL is
  acceptable: the composer is a developer tool that never ships (R2). It
  also keeps `crf`, `preset` and `tune` control and the same output on
  macOS and the Linux CI runner.
- Q3. Silent or a music bed: **decided 2026-10-27, silent for now**; a
  music bed may come later (it needs a licence for the landing page and
  the README).
- Q4. Light only or dark too: **decided 2026-10-27, light only** in the
  first version; dark later (a second render of every vignette and a
  second asset chosen by `prefers-color-scheme`).
- Q5. README GIF or user-attachment MP4: **decided 2026-10-27, a linked
  animated GIF, rendered independently of the video** (§9.4, §11.2). The
  full video is a build output now, and its publication is phase 3.
- Also decided 2026-10-27: the Clax app is out of scope (D14).

Proposed here, for the owner to confirm or change:

- Q6. The GIF's cut list (§9.3 `[gif]`): `point`, `version`, `realtime`,
  8 s in all, 720 px wide, 15 fps, under 2 MiB.
- Q7. The landing artifact is published only by `reel-upload`, which
  stages the Quickstart's files with the reel's (§10, R8), so the
  Quickstart has one publishing path.
