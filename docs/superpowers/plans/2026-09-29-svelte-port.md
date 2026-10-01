# Svelte Shell Port and Time to Usable Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Preact shell with a Svelte 5 (runes) + Vite SPA without changing any behaviour the e2e suite pins, then cut the time from opening a shell link to a readable artifact with comment mode ready.

**Architecture:** First, while the shell is still Preact, the artifact view's logic moves out of `artifact.tsx` into a framework-free `ArtifactController` (plain TypeScript with a store that follows the Svelte store contract). A plain-DOM skeleton and `FrameHost` own the page layout and the content `<iframe>`, and three small "islands" (topbar, stage overlays, sidebar) render the interactive parts. Each island is its own mount root, so Preact and Svelte islands can share one page during the port. The islands are then ported one at a time, with the unchanged Playwright suite as the parity oracle, and Preact is removed last. Finally, the daemon injects a bootstrap block and the content `<iframe>` into the shell HTML, the bridge loads its comment, clip and capability parts lazily, and the shell ships as two small entries with inlined CSS. Timing and size gates hold all of this in place.

**Tech Stack:** Svelte 5.57 (runes mode), `@sveltejs/vite-plugin-svelte` 6.2 (the line that supports Vite 6; 7.x needs Vite 8), Vite 6.4, Vitest 3.2 + jsdom + `@testing-library/svelte` 5.4, `svelte-check` 4.7, oxlint 1.86, Playwright, Rust 2024 (axum 0.8, rust-embed 8).

**Spec:** `docs/superpowers/specs/2026-09-28-clax-design.md`. That is the path after the rename plan `docs/superpowers/plans/2026-09-29-clax-rename.md`, which must be merged first. §4 Repository layout, §6 HTTP API, §8 Shell UI and viewer, §9 Runtime bridge and capabilities, §14 Security model and §16 Testing are amended in Task 1.

**Precondition:** the rename plan is merged. Every name below is the Clax name: `clax:` message types, `/_clax/` routes, `CLAX_HOME`, `clax.origin-ok`, crates `clax-*`, `window.__clax`. If `git grep -il artifax -- web crates` returns anything outside the rename plan's approved exceptions, stop: the rename is not done.

## Global Constraints

- Rust edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` pass.
- `oxlint --deny-warnings` passes over `shell bridge e2e perf` and the config files. It lints the `<script>` blocks of `.svelte` files (verified with oxlint 1.86.0: a `debugger` in a `.svelte` script is reported, and names used only in the template are not reported as unused). Template expressions are checked by `svelte-check --fail-on-warnings`, which also fails on Svelte compiler warnings (a11y included).
- Every e2e and perf spec that opens an artifact runs in both frame modes, `subdomain` and `sandbox`.
- Commit with `git commit --no-gpg-sign`, staging with `git add` and explicit paths only.
- Never bind or connect to port 7480 or 7481 (the agents' daemon and the dev daemon). Tests start daemons with `--port 0` and a temporary `CLAX_HOME`. Never read, write or delete the real `~/.artifax`, `~/.clax`, `~/.claude` or `~/.codex`.
- In prose, comments and commit messages, write "ID", never "id", except as a literal symbol in code.
- Svelte runs in runes mode only (`compilerOptions.runes: true`): no SvelteKit, no SSR, no `hydrate`, no `svelte/legacy`, no `export let`.
- `web/bridge/**` stays framework-free TypeScript. `web/shell/src/caps/**` and `web/shell/src/view/**` import neither `preact` nor `svelte`. Check with `grep -rlE "from \"(preact|svelte)" web/bridge web/shell/src/caps web/shell/src/view`, which must print nothing.
- Tasks 2–10 must leave the e2e specs and `web/e2e/fixtures.ts` byte-for-byte unchanged: `git diff --exit-code $(cat .svelte-port-base) -- web/e2e` exits 0. Task 1 writes `.svelte-port-base` (untracked, in `.git/info/exclude`). Tasks 11–13 may add e2e specs but must not change an existing assertion.
- Pinned versions: `svelte@^5.57.1`, `@sveltejs/vite-plugin-svelte@^6.2.4`, `svelte-check@^4.7.6`, `@testing-library/svelte@^5.4.2`. Vite stays on `^6`.
- Every task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `exit=0`. Check the status itself, not only the last line of output.
- **Time to usable** is measured in Chromium by `web/perf/usable.perf.ts`, for a warm browser (the shell's files cached, cookies set, a fresh tab):
  - *link → first paint* is the artifact frame's first contentful paint minus the shell document's `performance.timeOrigin`;
  - *link → comment ready* is the moment the bridge turns comment mode on in the frame (`documentElement.style.cursor === "crosshair"`), after the harness pressed **Comment** as soon as it could, minus the same origin.
  - Each is the median of 9 samples after 2 discarded warm-ups, per frame mode.
- **Budgets** (in `web/perf/budget.json`) scale under load: `limit = budget × clamp(controlMedian / control, 1, 3)`, where the control is a direct navigation to the same content URL measured in the same run. The perf project retries a failed test twice. Budgets only ever go down: recording a budget refuses to raise one.
- **Targets**, checked from Task 13 on: link → first paint median ≤ 50% of the Task 1 baseline, and link → comment ready ≤ 70% of it, in both modes. The eager bridge is ≤ 30% of its gzip size before the split (`bridgeBaseline`, recorded in Task 11; the bridge does not change in Tasks 1–10). If a target is missed, stop and report the numbers. Do not loosen it.
- The daemon token never appears in any HTML the daemon serves.

## Review Focus

1. **The server-rendered frame greets before the shell's JS runs.** Its `clax:hello` and early `clax:use` must still be welcomed and answered, in order. Task 12 buffers them with an inline listener and replays them (test: "replays a hello and a capability request that arrived before the shell mounted").
2. **The frame mode is guessed wrong.** A `clax_frame=subdomain` cookie meets `clax.origin-ok=0`, or the probe fails. The `<iframe>` must be replaced once, and nothing the stale frame posts may be answered (test: "replaces a server frame whose mode the shell does not confirm, and ignores the stale frame").
3. **Hostile strings in the bootstrap.** A thread body or title containing `</script>`, `<!--`, U+2028 or `<img onerror>` must neither break out of the JSON block nor inject markup (Rust tests in Task 12 plus the e2e title check).
4. **A page whose own CSP blocks the lazy bridge parts** (`script-src 'unsafe-inline'`). Comment mode must say it could not load, never fail silently (e2e "says comment mode could not load when the page's CSP blocks it", Task 13).
5. **A link with a fragment on first load while the server rendered the frame.** The frame must open at the fragment, and the address bar must keep it (test in Task 12).

---

## File Structure

Shell (`web/shell/`), in the final state:

| Path | Responsibility |
|---|---|
| `index.html`, `artifact.html` | The two entries: gallery, and artifact view (`artifact.html` holds the skeleton markup and the daemon's injection markers) |
| `src/gallery-main.ts`, `src/artifact-main.ts` | Entry scripts: mount the gallery; read the bootstrap, drain early messages, mount the artifact view |
| `src/artifact.ts` | `mountArtifactView(root, props, opts)`: skeleton + controller + frame host + islands; re-exports `pageWait`, `MOVE_TO_PICK`, `MOVE_TO_CLICK` |
| `src/view/store.ts` | `Store<S>`: an immutable-snapshot store that follows the Svelte store contract |
| `src/view/artifact-controller.ts` | `ArtifactController`: all artifact-view state and behaviour formerly in `artifact.tsx` |
| `src/view/frame-gate.ts`, `anchor-handles.ts`, `thread-sync.ts`, `url.ts` | Pieces of the controller, each unit-tested |
| `src/view/frame-host.ts`, `skeleton.ts` | Plain DOM: the content `<iframe>` (created or adopted) and the page skeleton |
| `src/view/sidebar-model.ts`, `pins-model.ts`, `composer-model.ts`, `gallery-model.ts`, `viewer-name-model.ts`, `prompt-queue.ts` | Logic pulled out of the leaf components |
| `src/view/boot.ts` | The bootstrap block's type and reader, and the early-message buffer |
| `src/islands/index.ts` | `Islands` registry (which framework mounts each island) |
| `src/ui/*.svelte`, `src/ui/ticker.svelte.ts` | The Svelte components |
| `src/test/svelte.ts`, `src/test/Probe.svelte`, `src/test/probe.test.ts` | Framework-neutral test mount helper and its self-test |
| `src/caps/**` | Unchanged capability host |

Bridge (`web/bridge/src/`):

| Path | Responsibility |
|---|---|
| `bridge.ts` | The eager bridge: `window.claude`, hello, channel, link handover, the part loader |
| `parts/comment.ts`, `parts/clip.ts`, `parts/caps.ts` | Lazy entries: comment mode + anchors + areas; clip rendering (modern-screenshot); page-side capability members |
| `parts-url.ts`, `parts-static.ts` | Loader implementations: by URL (production), by static import (unit tests), chosen by the `clax-bridge-parts` alias |
| `block.ts` | `blockAncestor`, moved out of `clip.ts` so comment mode does not pull in the clip library |
| `comments-context.ts` | The `CommentsContext` type (the object itself is created by `bridge.ts` and passed to parts) |

Daemon: `crates/clax-server/src/routes/shell.rs` (entries, injection), `crates/clax-server/src/boot.rs` (bootstrap assembly, escaping, shell-path parsing), `crates/clax-server/tests/shell_boot.rs`.

Perf and size: `web/perf/playwright.config.ts`, `web/perf/usable.perf.ts`, `web/perf/budget.json`, `web/perf/bundle-budget.json`, `web/scripts/bundle-size.mjs`.

## Port strategy: why per island, leaf-first inside each

Porting leaf components first across the whole tree would mean a Preact parent rendering Svelte children, which needs wrapper components in both directions and tests that know about both. Porting per route would leave the artifact view, 735 lines and 40 unit tests, as one big bang. The island split (Task 5) gives three independent mount roots under the artifact route: topbar, stage overlays and sidebar. Each reads the same `ArtifactController` store and calls its methods. A page can therefore mix Preact and Svelte islands with no wrappers. Each island port (Tasks 7–9) is small enough to review, and ends with the unchanged e2e suite green in both frame modes. Inside each island the leaves go first (Card before Sidebar, Pins and Composer before the stage). The gallery is its own route and goes last with the topbar.

## How the Preact logic maps onto runes

| Preact in `artifact.tsx` | Where it goes |
|---|---|
| `useState` for view data | `ViewState` fields in `ArtifactController.state` (a `Store`); components read it with `fromStore(ctl.state)` + `$derived` |
| `useRef` mirrors (`commentingRef`, `draftRef`, `threadsRef`, `fileRef`, …) | Gone: the controller reads its own current state (`this.s`), which is always the latest |
| `useRef` for timers, handles and maps (`startedPicks`, `anchorIds`, `pendingScroll`, `hashFrame`, `ownPublish`) | Private controller fields |
| "Reset while rendering, not in an effect" (the hello gate) and "made while rendering" (the capability host) | `viewChanged()`: runs synchronously when the data or origin arrives, *before* `FrameHost` inserts the `<iframe>`, so a hello can never beat it |
| `useMemo(promptQueue)`, `useMemo(commentsUi)` | Created once in the controller constructor |
| `useEffect(…, [deps])` reactions (comment-mode message, resume-after, focus, capture timer, `host.uiChanged`, resolve anchors) | `react(prev, next)`: one pass per microtask over the diff of two snapshots, batched like Preact's render |
| `useEffect` listeners (`message`, `popstate`, `keydown`/`keyup`, `mouseover`, media query, shield press) | `start()` adds them and `dispose()` removes them |
| `useEffect(() => () => host?.dispose(), [host])` | `viewChanged()` disposes the replaced host; `dispose()` disposes the last one. Unmount calls `dispose()` |
| Keyed `<Frame key={n-o}>` | `FrameHost.show(src, sandboxed, key)` replaces the `<iframe>` only when the key changes |
| Component-local `useState` (composer body, reply text, armed) | `$state` in the `.svelte` component |
| DOM-local `useEffect` (Pins ResizeObserver, composer focus, clip object URL, PromptDialog timer and Escape, Sidebar clock) | `$effect` / `{@attach}` in the component; the clock is `ui/ticker.svelte.ts` |
| `ref={registerShield}` | `{@attach shield}` returning `() => registerShield(null)` |

---

### Task 1: Spec amendments, the time-to-usable harness, and the Preact baseline

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md` (§4, §6, §8, §9, §14, §16)
- Create: `web/perf/playwright.config.ts`, `web/perf/usable.perf.ts`, `web/perf/budget.json`
- Modify: `web/package.json` (script `perf`, lint path), `web/tsconfig.json` (include `perf`), `scripts/quality_gates.sh`, `.gitignore`

**Interfaces:**
- Produces: `npm run perf` (in `web/`). It exits 0 when every median is within budget. With `CLAX_PERF_RECORD=baseline` it writes the baseline and first budgets; with `CLAX_PERF_RECORD=budget` it lowers the budgets to the measured medians × 1.25, refusing to raise any. It writes `web/perf/results.json` (ignored by git) on every run.
- Produces: `web/perf/budget.json` with the shape `{ "baseline": Modes<Metrics & {control}>, "budget": Modes<Metrics>, "control": Modes<number>, "enforceTargets": boolean }`, where `Modes<T> = { subdomain: T; sandbox: T }` and `Metrics = { firstPaint: number; commentReady: number }` (milliseconds, integers).

- [ ] **Step 1: Record the port's base commit**

```bash
git rev-parse HEAD > .svelte-port-base
grep -qx '.svelte-port-base' .git/info/exclude || echo '.svelte-port-base' >> .git/info/exclude
```

- [ ] **Step 2: Amend the spec, §4 Repository layout**

In the layout block, replace the line
`  shell/                           Preact + TypeScript: gallery, artifact shell, comment sidebar`
with
`  shell/                           Svelte 5 (runes) + TypeScript, Vite SPA: gallery, artifact shell, comment sidebar`.
In the paragraph after the block, replace `Web: Preact, Vite,` with `Web: Svelte 5, Vite,`.

- [ ] **Step 3: Amend the spec, §8 Shell UI and viewer**

Replace the sentence that begins `The shell is Preact + TypeScript with CSS tokens on `:root`` so that it begins `The shell is Svelte 5 (runes mode, no SvelteKit, no SSR) + TypeScript with CSS tokens on `:root``. Keep the rest of that sentence. Then append this subsection at the end of §8:

```markdown
### Time to usable

Opening a shell link must be fast. *Link → first paint* runs from the shell
document's navigation start to the artifact frame's first contentful paint.
*Link → comment ready* runs to the moment the bridge turns comment mode on in
the frame, for a viewer who presses **Comment** as soon as they can. Both are
measured in Chromium for a warm browser (the shell's files cached, cookies set,
a fresh tab), in both frame modes, by `web/perf/usable.perf.ts`, a quality gate
with budgets in `web/perf/budget.json`.

What makes it fast:

- The shell has two entries. `/` serves `index.html` (the gallery); `/a/…`
  serves `artifact.html`, whose body already holds the page skeleton. Each
  entry loads only its own view's code. The shell's CSS is inlined in both.
- For `/a/…` the daemon injects a bootstrap block into `artifact.html`:
  `<script type="application/json" id="clax-boot">`, holding the artifact with
  its versions, every thread, and the viewer when the request's viewer cookie
  names one. The shell reads it instead of making its first API calls.
- When the request carries a `clax_frame` cookie (`subdomain` or `sandbox`,
  set by the shell once it has decided the frame mode), or comes from a
  non-loopback host (always `sandbox`), the daemon also injects the content
  `<iframe>` itself. The artifact then loads in parallel with the shell's
  JavaScript. The shell adopts that frame when its own decision agrees, and
  replaces it otherwise. Messages the frame posts before the shell has mounted
  are buffered by an inline listener and replayed in order.
- The bridge loads eagerly only what every page needs (`window.claude`, the
  hello, the channel, link handover). Comment mode with anchors and areas,
  clip rendering, and the page-side capability members are separate parts
  under `/_clax/bridge/`, loaded on first use: comment mode right after the
  welcome, clip rendering when comment mode turns on, capability members on
  the first `claude.use`. A part that cannot load (the page's own CSP forbids
  it) makes the bridge post `clax:degraded`, and the shell says so.
```

- [ ] **Step 4: Amend the spec, §6 HTTP API**

In the route table or list, next to the shell routes, add:

```markdown
- `GET /` → `index.html`; `GET /a/<id>[/v/<n>][/<file>]` → `artifact.html` with
  the bootstrap block and, when the frame mode is known, the content `<iframe>`
  (§8 Time to usable). Both are HTML responses: `Cache-Control: no-cache`, an
  `ETag` over the exact bytes sent, and `Vary: Cookie`. The bootstrap never
  holds the daemon token and is escaped for a `<script>` element (`<`, `>`,
  `&`, U+2028 and U+2029 as `\u` escapes).
- `GET /_clax/bridge/<part>-<hash>.js`: the bridge's lazy parts, ES modules
  with content-hashed names, `Cache-Control: public, max-age=31536000,
  immutable` and `Access-Control-Allow-Origin: *` (a sandboxed frame imports
  them from an opaque origin). The eager `bridge.js` names them, so its `?v=`
  hash changes whenever a part does.
```

- [ ] **Step 5: Amend the spec, §9, §14 and §16**

In §9, after the paragraph describing where the daemon injects the bridge tag, add: `The bridge's comment mode, clip rendering and page-side capability members are lazy parts (§8 Time to usable); the protocol gains `clax:degraded` (bridge → shell: `{ part: "comment" | "clip" | "caps", message }`).`

In §14 add a bullet: `The shell HTML for `/a/…` embeds only what `GET /api/artifacts/<id>`, `GET /api/artifacts/<id>/threads` (without the token) and `GET /api/viewers/me` (for the cookie's own viewer, never creating one) would answer the same browser, is never served on an artifact origin, and never holds the daemon token.`

In §16 add: `Shell unit tests use Vitest with jsdom and `@testing-library/svelte`. `svelte-check --fail-on-warnings` runs with the typecheck. Two gates hold time to usable: `web/perf` (Playwright timing, budgets in `web/perf/budget.json`) and `web/scripts/bundle-size.mjs` (gzip sizes of each entry's critical JavaScript and of the eager bridge, budgets in `web/perf/bundle-budget.json`).`

- [ ] **Step 6: Write the Playwright config for the perf project**

`web/perf/playwright.config.ts`:

```ts
import { defineConfig } from "@playwright/test";
// One worker, never alongside the e2e run (quality_gates.sh runs it after).
// Two retries absorb a sample run that a burst of machine load spoiled; the
// budgets also scale with the in-run control (usable.perf.ts).
export default defineConfig({ testDir: ".", testMatch: "*.perf.ts", timeout: 600_000, retries: 2, workers: 1, reporter: "list", use: { browserName: "chromium" } });
```

- [ ] **Step 7: Write the harness**

`web/perf/usable.perf.ts`:

```ts
import { test, expect, type Browser } from "@playwright/test";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { type FrameMode, contentFrame, publish, startDaemon } from "../e2e/fixtures";

type Metrics = { firstPaint: number; commentReady: number };
type Modes<T> = { subdomain: T; sandbox: T };
type Budget = { baseline: Modes<Metrics & { control: number }>; budget: Modes<Metrics>; control: Modes<number>; enforceTargets: boolean };
type Sample = Metrics & { control: number };

const BUDGET = fileURLToPath(new URL("./budget.json", import.meta.url));
const RESULTS = fileURLToPath(new URL("./results.json", import.meta.url));
const WARMUPS = 2;
const SAMPLES = 9;
const MODES: FrameMode[] = ["subdomain", "sandbox"];
/** What `CLAX_PERF_RECORD` asks for: nothing, the first baseline, or lower budgets. */
const RECORD = process.env.CLAX_PERF_RECORD ?? "";

/** A readable page of about 60 KB: headings, paragraphs, a table. */
const PAGE = "<!doctype html><title>Perf</title><style>body{font:16px/1.5 system-ui;margin:2rem}</style>"
  + Array.from({ length: 40 }, (_, i) => `<h2>Section ${i + 1}</h2><p>${"Clax keeps the conversation next to the page it is about. ".repeat(24)}</p>`).join("")
  + `<table>${Array.from({ length: 30 }, (_, r) => `<tr>${Array.from({ length: 6 }, (_, c) => `<td>r${r}c${c}</td>`).join("")}</tr>`).join("")}</table>`;

/** Runs in every frame before its scripts: records the document's time
 * origin, its first contentful paint (or, where paint timing is missing, two
 * animation frames after DOMContentLoaded), and when comment mode's crosshair
 * first shows, all in epoch milliseconds. */
function recorder() {
  const rec = { origin: performance.timeOrigin, paint: null as number | null, raf: null as number | null, crosshair: null as number | null };
  Object.defineProperty(window, "claxPerf", { value: rec });
  const at = () => performance.timeOrigin + performance.now();
  try {
    new PerformanceObserver(list => {
      for (const e of list.getEntries()) if (e.name === "first-contentful-paint" && rec.paint === null) rec.paint = performance.timeOrigin + e.startTime;
    }).observe({ type: "paint", buffered: true });
  } catch { /* no paint timing here */ }
  const check = () => { if (rec.crosshair === null && document.documentElement?.style.cursor === "crosshair") rec.crosshair = at(); };
  new MutationObserver(check).observe(document, { subtree: true, attributes: true, attributeFilter: ["style"] });
  const painted = () => requestAnimationFrame(() => requestAnimationFrame(() => { rec.raf ??= at(); }));
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", painted, { once: true });
  else painted();
}

type Rec = { origin: number; paint: number | null; raf: number | null; crosshair: number | null };
const median = (xs: number[]) => { const s = [...xs].sort((a, b) => a - b); return s[Math.floor(s.length / 2)]; };

async function poll<T>(f: () => Promise<T | null>, what: string): Promise<T> {
  const deadline = Date.now() + 60_000;
  for (;;) {
    const v = await f();
    if (v !== null) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 20));
  }
}

/** One sample: a context that has seen the artifact once (a first tab that
 * loaded it and pressed nothing), then a fresh tab opening the link. */
async function sample(browser: Browser, base: string, port: string, id: string, mode: FrameMode): Promise<Sample> {
  const ctx = await browser.newContext();
  try {
    await ctx.addInitScript(recorder);
    if (mode === "sandbox") await ctx.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    const first = await ctx.newPage();
    await first.goto(`${base}/a/${id}`);
    await contentFrame(first, id, 1);
    await first.getByRole("button", { name: "Comment" }).waitFor();
    await first.close();

    const page = await ctx.newPage();
    await page.goto(`${base}/a/${id}`, { waitUntil: "commit" });
    await page.getByRole("button", { name: "Comment" }).click();
    const frame = await contentFrame(page, id, 1);
    const shell = await poll(() => page.evaluate(() => (window as unknown as { claxPerf?: Rec }).claxPerf?.origin ?? null), "the shell's time origin");
    const rec = await poll(async () => {
      const r = await frame.evaluate(() => (window as unknown as { claxPerf?: Rec }).claxPerf ?? null);
      return r && (r.paint ?? r.raf) !== null && r.crosshair !== null ? r : null;
    }, "first paint and comment mode in the frame");

    const direct = mode === "subdomain" ? `http://${id}.localhost:${port}/v/1/` : `${base}/c/${id}/v/1/`;
    await page.goto(direct);
    const ctl = await poll(() => page.evaluate(() => {
      const r = (window as unknown as { claxPerf?: Rec }).claxPerf;
      const p = r ? r.paint ?? r.raf : null;
      return r && p !== null ? p - r.origin : null;
    }), "the control page's first paint");
    return { firstPaint: (rec.paint ?? rec.raf)! - shell, commentReady: rec.crosshair! - shell, control: ctl };
  } finally {
    await ctx.close();
  }
}

function load(): Budget | null {
  return existsSync(BUDGET) ? JSON.parse(readFileSync(BUDGET, "utf8")) as Budget : null;
}

function judge(mode: FrameMode, m: Sample): void {
  const round = (x: number) => Math.ceil(x);
  const b = load();
  if (RECORD === "baseline") {
    if (b?.baseline[mode].firstPaint) throw new Error("budget.json already holds a baseline; it is recorded once, in Task 1");
    const next: Budget = b ?? { baseline: { subdomain: { firstPaint: 0, commentReady: 0, control: 0 }, sandbox: { firstPaint: 0, commentReady: 0, control: 0 } }, budget: { subdomain: { firstPaint: 0, commentReady: 0 }, sandbox: { firstPaint: 0, commentReady: 0 } }, control: { subdomain: 0, sandbox: 0 }, enforceTargets: false };
    next.baseline[mode] = { firstPaint: round(m.firstPaint), commentReady: round(m.commentReady), control: round(m.control) };
    next.budget[mode] = { firstPaint: round(m.firstPaint * 1.25), commentReady: round(m.commentReady * 1.25) };
    next.control[mode] = round(m.control);
    writeFileSync(BUDGET, JSON.stringify(next, null, 2) + "\n");
    return;
  }
  if (!b) throw new Error("web/perf/budget.json is missing; record a baseline with CLAX_PERF_RECORD=baseline");
  const scale = Math.min(3, Math.max(1, m.control / b.control[mode]));
  const limit = (x: number) => x * scale;
  const report = `${mode}: first paint ${m.firstPaint.toFixed(0)} ms (budget ${b.budget[mode].firstPaint}, limit ${limit(b.budget[mode].firstPaint).toFixed(0)}), comment ready ${m.commentReady.toFixed(0)} ms (budget ${b.budget[mode].commentReady}, limit ${limit(b.budget[mode].commentReady).toFixed(0)}), control ${m.control.toFixed(0)} ms (scale ${scale.toFixed(2)})`;
  console.log(report);
  expect(m.firstPaint, report).toBeLessThanOrEqual(limit(b.budget[mode].firstPaint));
  expect(m.commentReady, report).toBeLessThanOrEqual(limit(b.budget[mode].commentReady));
  if (b.enforceTargets) {
    expect(m.firstPaint, `${report}; target ≤ 50% of the baseline ${b.baseline[mode].firstPaint}`).toBeLessThanOrEqual(limit(b.baseline[mode].firstPaint * 0.5));
    expect(m.commentReady, `${report}; target ≤ 70% of the baseline ${b.baseline[mode].commentReady}`).toBeLessThanOrEqual(limit(b.baseline[mode].commentReady * 0.7));
  }
  if (RECORD === "budget") {
    const next = { firstPaint: round(m.firstPaint * 1.25), commentReady: round(m.commentReady * 1.25) };
    b.budget[mode] = { firstPaint: Math.min(next.firstPaint, b.budget[mode].firstPaint), commentReady: Math.min(next.commentReady, b.budget[mode].commentReady) };
    b.control[mode] = round(m.control);
    writeFileSync(BUDGET, JSON.stringify(b, null, 2) + "\n");
  }
}

let d: Awaited<ReturnType<typeof startDaemon>>;
let id = "";
test.beforeAll(async () => {
  test.setTimeout(300_000);
  d = await startDaemon();
  id = (await publish(d.base, d.token, "Perf", { "index.html": PAGE })).artifact.id;
});
test.afterAll(async () => { await d?.stop(); });

for (const mode of MODES) {
  test(`time to usable (${mode})`, async ({ browser }) => {
    const port = new URL(d.base).port;
    for (let i = 0; i < WARMUPS; i++) await sample(browser, d.base, port, id, mode);
    const xs: Sample[] = [];
    for (let i = 0; i < SAMPLES; i++) xs.push(await sample(browser, d.base, port, id, mode));
    const m: Sample = { firstPaint: median(xs.map(x => x.firstPaint)), commentReady: median(xs.map(x => x.commentReady)), control: median(xs.map(x => x.control)) };
    const results = existsSync(RESULTS) ? JSON.parse(readFileSync(RESULTS, "utf8")) : {};
    results[mode] = { median: m, samples: xs };
    writeFileSync(RESULTS, JSON.stringify(results, null, 2) + "\n");
    judge(mode, m);
  });
}
```

- [ ] **Step 8: Wire the script, typecheck, lint and ignore**

In `web/package.json`, add `"perf": "playwright test -c perf/playwright.config.ts"` to `scripts`, and put `perf` after `e2e` in the `lint` script's path list. In `web/tsconfig.json`, set `"include": ["shell/src", "bridge/src", "bridge/test", "e2e", "perf"]`. Append `web/perf/results.json` to `.gitignore`.

- [ ] **Step 9: Run it with no budget and watch it fail**

Run: `cd web && npm run build && npm run perf; echo "exit=$?"`
Expected: both tests FAIL with `web/perf/budget.json is missing`, and `exit=1`.

- [ ] **Step 10: Record the baseline on the Preact shell**

Run: `cd web && CLAX_PERF_RECORD=baseline npm run perf; echo "exit=$?"`
Expected: `exit=0`, and `web/perf/budget.json` holds non-zero `baseline`, `budget` and `control` for both modes. If `results.json` shows a sample's `firstPaint` that came from `raf` (paint timing missing in a frame), that is acceptable. Note it in the commit message.

- [ ] **Step 11: Run it again as a gate**

Run: `cd web && npm run perf; echo "exit=$?"`
Expected: two `time to usable (…)` lines, each within its limit, and `exit=0`.

- [ ] **Step 12: Add the gate**

In `scripts/quality_gates.sh`, after the `web e2e` line, add:

```bash
run "time to usable"        bash -c 'cd web && npm run perf'
```

- [ ] **Step 13: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `all gates passed` and `exit=0`.

- [ ] **Step 14: Commit**

```bash
git add docs/superpowers/specs/2026-09-28-clax-design.md web/perf/playwright.config.ts web/perf/usable.perf.ts web/perf/budget.json web/package.json web/tsconfig.json scripts/quality_gates.sh .gitignore
git commit --no-gpg-sign -m "Measure time to usable in both frame modes and record the Preact baseline

The spec now names Svelte 5 for the shell and states how links become
usable fast: two entries, a bootstrap block, a server-rendered frame and
lazy bridge parts. web/perf times link to first paint and link to comment
ready over 9 warm samples per frame mode, scales its budgets by an in-run
control, and gates quality_gates.sh."
```

---

### Task 2: A framework-neutral test seam

The six Preact unit-test files render JSX. After this task no test contains JSX. Every mount goes through one helper, whose Svelte twin (Task 6) has the same signature, so porting a component changes one import line in its test.

**Files:**
- Create: `web/shell/src/test/preact.ts`
- Rename and modify (`git mv` then edit): `web/shell/src/{artifact,comments,gallery,prompt,sidebar,viewer-name}.test.tsx` → `….test.ts`
- Modify: `web/vitest.config.ts` (the include pattern keeps `.tsx` until Task 10; nothing to change now)

**Interfaces:**
- Produces, from `web/shell/src/test/preact.ts`:
  ```ts
  export type Mounted<P> = { root: HTMLElement; update(props: P): void; unmount(): void };
  export function mount<P extends object>(C: ComponentType<P>, props: P, root?: HTMLElement): Mounted<P>;
  export function flush(fn?: () => void): void;
  ```
  Task 6 creates `web/shell/src/test/svelte.ts` with the same three exports, typed over Svelte's `Component<P>`.

- [ ] **Step 1: Capture the current test list**

Run: `cd web && npx vitest list shell 2>/dev/null | sed -E 's/\.test\.tsx?//' | sort > /tmp/clax-tests-before.txt; wc -l /tmp/clax-tests-before.txt`
Expected: one line per test (at least 150). Keep the file.

- [ ] **Step 2: Write the helper**

`web/shell/src/test/preact.ts`:

```ts
// Mounts a Preact component for a unit test; test/svelte.ts has the same
// exports, so a test changes one import when its component is ported.
import { type ComponentType, h, render } from "preact";
import { act } from "preact/test-utils";

export type Mounted<P> = { root: HTMLElement; update(props: P): void; unmount(): void };

function attach(): HTMLElement {
  const root = document.createElement("div");
  document.body.appendChild(root);
  return root;
}

/** Renders `C` with `props` into `root` (a new div in the body by default),
 * running its effects before returning. */
export function mount<P extends object>(C: ComponentType<P>, props: P, root: HTMLElement = attach()): Mounted<P> {
  act(() => { render(h(C, props), root); });
  return {
    root,
    update: next => act(() => { render(h(C, next), root); }),
    unmount: () => act(() => { render(null, root); }),
  };
}

/** Runs `fn` (a timer advance, a click) and applies every update it caused. */
export function flush(fn?: () => void): void {
  act(() => { fn?.(); });
}
```

- [ ] **Step 3: Convert each test file, with no other edits**

For each of the six files, run `git mv web/shell/src/X.test.tsx web/shell/src/X.test.ts` and then apply these rules. Do not touch any assertion, `waitFor`, stub or test name.

1. Remove `import { render } from "preact";` and `import { act } from "preact/test-utils";`. Add `import { flush, mount } from "./test/preact";`.
2. `render(<C a={x} b="y" />, root)` becomes `mount(C, { a: x, b: "y" }, root)`. A JSX boolean attribute `c` becomes `c: true`. A spread `{...p}` becomes `...p`.
3. `render(null, root)` becomes a call to the `unmount()` of the `Mounted` value returned where that root was mounted. Keep that value in a `const view = mount(…)` at the mount site.
4. A second `render(<C … />, root)` into the same root becomes `view.update({ … })`.
5. `act(() => { … })` becomes `flush(() => { … })`.

For example, in `artifact.test.ts` the local `mount` helper becomes:

```ts
async function mountView(fetchImpl: (url: string, init?: RequestInit) => Promise<Response>, comments?: (url: string, init?: RequestInit) => Promise<Response>, file?: string, pinned: number | null = null) {
  // …the fetch and EventSource stubs, unchanged…
  sessionStorage.setItem("clax.origin-ok", "0");
  const { default: ArtifactView } = await import("./artifact");
  gesture = await import("./caps/gesture");
  return mount(ArtifactView, { id: ID, pinnedVersion: pinned, file });
}
```

Its call sites change from `const root = await mount(…)` to `const view = await mountView(…); const root = view.root;`. The host-disposal test uses `view.update({ id: ID, pinnedVersion: 1, file: undefined })` and then `view.unmount()`. In `comments.test.ts` the local `mount(node)` helper becomes `mountIt<P extends object>(C: ComponentType<P>, props: P)`, returning `{ root, done: () => { view.unmount(); root.remove(); } }`, with `import type { ComponentType } from "preact";`. That type-only import is the one Preact reference left in a test file. Task 8 replaces it.

- [ ] **Step 4: Prove nothing but the extension changed in the test list**

Run: `cd web && npx vitest list shell 2>/dev/null | sed -E 's/\.test\.tsx?//' | sort > /tmp/clax-tests-after.txt; diff /tmp/clax-tests-before.txt /tmp/clax-tests-after.txt && echo same`
Expected: `same`.

- [ ] **Step 5: Prove no test holds JSX**

Run: `ls web/shell/src/*.test.tsx 2>/dev/null | wc -l; grep -lE '<[A-Z][A-Za-z]* ' web/shell/src/*.test.ts || echo none`
Expected: `0` and `none`.

- [ ] **Step 6: Run the unit tests and every gate**

Run: `cd web && npm test -- --reporter=dot; cd .. && bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: all unit tests PASS; `exit=0`.

- [ ] **Step 7: Commit**

```bash
git add web/shell/src/test/preact.ts web/shell/src/artifact.test.ts web/shell/src/comments.test.ts web/shell/src/gallery.test.ts web/shell/src/prompt.test.ts web/shell/src/sidebar.test.ts web/shell/src/viewer-name.test.ts web/shell/src/artifact.test.tsx web/shell/src/comments.test.tsx web/shell/src/gallery.test.tsx web/shell/src/prompt.test.tsx web/shell/src/sidebar.test.tsx web/shell/src/viewer-name.test.tsx
git commit --no-gpg-sign -m "Mount shell components in unit tests through one helper, without JSX

test/preact.ts exports mount(C, props, root) and flush(fn); a Svelte twin
with the same exports lets each test follow its component by changing one
import. Assertions and test names are unchanged."
```

---

### Task 3: Pull the leaf components' logic into plain TypeScript

**Files:**
- Create: `web/shell/src/view/composer-model.ts`, `view/pins-model.ts`, `view/sidebar-model.ts`, `view/gallery-model.ts`, `view/viewer-name-model.ts`, `view/prompt-queue.ts`
- Create: `web/shell/src/view/composer-model.test.ts`, `view/pins-model.test.ts`, `view/sidebar-model.test.ts`, `view/gallery-model.test.ts`, `view/viewer-name-model.test.ts`, `view/prompt-queue.test.ts`
- Modify: `web/shell/src/comments.tsx`, `sidebar.tsx`, `gallery.tsx`, `viewer-name.tsx`, `prompt.tsx`, `artifact.tsx` (imports only), `comments.test.ts`, `prompt.test.ts`, `artifact.test.ts` (the `captureWait` import path only)

**Interfaces (Produces):**
- `view/composer-model.ts`: `type Draft` (moved verbatim from `comments.tsx`), `captureWait`, `CAPTURE_LATE`, `MAX_CLIP_BYTES`, `nextDraft`, `takePick`, `withClip` (moved verbatim), and new `composerQuote(draft: Draft): string`.
- `view/pins-model.ts`: `PIN_RIGHT_ROOM` (moved), `type PinPlace = { thread: Thread; n: number; left: number; top: number }`, `pinPlaces(threads: Thread[], resolved: Record<string, AnchorResult>, file: string | null, stage: number): PinPlace[]`.
- `view/sidebar-model.ts`: `type SidebarSections = { open: Thread[]; detached: Thread[]; resolved: Thread[]; numbers: Map<string, number>; file: string | null }`, `sidebarSections(threads: Thread[], resolved: Record<string, AnchorResult>, file: string | null | undefined, holds?: (f: string) => boolean): SidebarSections`, `needsTicking(threads: Thread[]): boolean`, `authorLabel(c: Comment): string`.
- `view/gallery-model.ts`: `filterArtifacts(list: Artifact[], query: string): Artifact[]`, `publisherText(a: Artifact): string | null` (null means "published from the command line").
- `view/viewer-name-model.ts`: `type SetNotice = (u: string | null | ((prev: string | null) => string | null)) => void`, `class NameSaver { constructor(setNotice: SetNotice, onViewer?: (v: Viewer) => void); load(show: (name: string) => void): void; edit(): void; save(name: string): void }`.
- `view/prompt-queue.ts`: `type Ask`, `ALLOW_DELAY_MS`, `promptQueue` (moved verbatim from `prompt.tsx`).

- [ ] **Step 1: Write the failing model tests**

`web/shell/src/view/pins-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";
import { PIN_RIGHT_ROOM, pinPlaces } from "./pins-model";

const t = (id: string, file = "index.html", status: "open" | "resolved" = "open") => ({ id, status, anchor: { file } }) as unknown as Thread;
const at = (x: number, y: number, w = 100, h = 20): AnchorResult => ({ id: "x", found: true, method: "selector", rect: { x, y, w, h } }) as AnchorResult;

describe("pinPlaces", () => {
  it("numbers attached open threads on the page and skips unmeasured and scrolled-away ones without renumbering", () => {
    const threads = [t("a"), t("b"), t("c"), t("d", "other.html"), t("e", "index.html", "resolved")];
    const places = pinPlaces(threads, { a: at(10, 50), b: { id: "b", found: true, method: "selector", rect: null } as AnchorResult, c: at(10, -40, 100, 30) }, "index.html", 0);
    expect(places.map(p => [p.thread.id, p.n])).toEqual([["a", 1]]);
    expect(places[0]).toMatchObject({ left: 98, top: 38 });
  });
  it("keeps a pin clear of the stage's right edge", () => {
    const [p] = pinPlaces([t("a")], { a: at(0, 50, 1000) }, "index.html", 800);
    expect(p.left).toBe(800 - PIN_RIGHT_ROOM);
  });
  it("draws no pins over a document that did not greet", () => {
    expect(pinPlaces([t("a")], { a: at(0, 50) }, null, 0)).toEqual([]);
  });
});
```

`web/shell/src/view/sidebar-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";
import { needsTicking, sidebarSections } from "./sidebar-model";

const t = (id: string, file = "index.html", extra: Partial<Thread> = {}) => ({ id, status: "open", anchor: { file }, sent_to_agent: false, feedback_state: null, ...extra }) as unknown as Thread;
const lost = { found: false, method: null, rect: null } as unknown as AnchorResult;

describe("sidebarSections", () => {
  it("orders attached then elsewhere, sends lost and unheld threads to Detached, and numbers only the attached", () => {
    const s = sidebarSections([t("a"), t("b"), t("c", "about.html"), t("d", "gone.html"), t("e", "index.html", { status: "resolved" } as Partial<Thread>)], { b: lost }, "index.html", f => f !== "gone.html");
    expect(s.open.map(x => x.id)).toEqual(["a", "c"]);
    expect(s.detached.map(x => x.id)).toEqual(["b", "d"]);
    expect(s.resolved.map(x => x.id)).toEqual(["e"]);
    expect([...s.numbers]).toEqual([["a", 1]]);
  });
  it("defaults the page to the index when none is given", () => {
    expect(sidebarSections([t("a")], {}, undefined).file).toBe("index.html");
  });
  it("ticks only while an open thread sent to the agent shows elapsed time", () => {
    expect(needsTicking([t("a")])).toBe(false);
    expect(needsTicking([t("a", "index.html", { sent_to_agent: true, feedback_state: { thread_id: "a", state: "sent", tier: null, since: "2026-09-29T00:00:00Z", resends: 0, exhausted: false } } as Partial<Thread>)])).toBe(true);
  });
});
```

`web/shell/src/view/composer-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { type Draft, composerQuote } from "./composer-model";

const d = (anchor: Record<string, unknown>, label?: string) => ({ pickId: "p", version: 1, clip: null, label, anchor: { file: "index.html", ...anchor } }) as unknown as Draft;

describe("composerQuote", () => {
  it("prefers the page's label, then the quote in guillemets cut at 160 characters, then the custom name, area label or selector", () => {
    expect(composerQuote(d({ quote: "x" }, "Chart"))).toBe("Chart");
    expect(composerQuote(d({ quote: "  a\n  b " }))).toBe("«a b»");
    expect(composerQuote(d({ quote: "q".repeat(200) }))).toBe(`«${"q".repeat(160)}…»`);
    expect(composerQuote(d({ kind: "custom", custom_name: "Row 3" }))).toBe("Row 3");
    expect(composerQuote(d({ kind: "element", selector: "body > h2" }))).toBe("body > h2");
  });
});
```

`web/shell/src/view/gallery-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Artifact } from "../api";
import { filterArtifacts, publisherText } from "./gallery-model";

const a = (title: string, description: string | null, extra: Partial<Artifact> = {}) => ({ id: title, title, description, icon: null, updated_at: "x", current_version: 1, pinned: false, ...extra }) as Artifact;

describe("gallery model", () => {
  it("matches title or description, case-insensitively, ignoring surrounding spaces", () => {
    const list = [a("Quarterly Review", null), a("Notes", "the REVIEW draft"), a("Other", null)];
    expect(filterArtifacts(list, "  review ").map(x => x.title)).toEqual(["Quarterly Review", "Notes"]);
    expect(filterArtifacts(list, "")).toBe(list);
  });
  it("names the harness session, an agent session, or nothing for the command line", () => {
    expect(publisherText(a("x", null, { owner_session_id: "s", owner_harness: "codex" }))).toBe("published by codex session");
    expect(publisherText(a("x", null, { owner_session_id: "s", owner_harness: null }))).toBe("published by an agent session");
    expect(publisherText(a("x", null))).toBeNull();
  });
});
```

`web/shell/src/view/viewer-name-model.test.ts`:

```ts
import { afterEach, describe, expect, it, vi } from "vitest";

describe("NameSaver", () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });
  it("saves only after the initial lookup answered, skips an unchanged name, and keeps a name the viewer typed first", async () => {
    const calls: string[] = [];
    let answer!: (r: Response) => void;
    vi.stubGlobal("fetch", vi.fn((url: string, init?: RequestInit) => {
      calls.push(`${init?.method ?? "GET"} ${url}`);
      if (!init?.method) return new Promise<Response>(r => { answer = r; });
      return Promise.resolve(new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: "Ada", created_at: "x" } })));
    }));
    const { NameSaver } = await import("./viewer-name-model");
    const shown: string[] = [];
    const s = new NameSaver(() => {});
    s.load(n => shown.push(n));
    s.edit();
    s.save("Ada");
    await new Promise(r => setTimeout(r, 10));
    expect(calls).toEqual(["GET /api/viewers/me"]);
    answer(new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: "Bo", created_at: "x" } })));
    await new Promise(r => setTimeout(r, 10));
    expect(shown).toEqual([]);
    expect(calls).toEqual(["GET /api/viewers/me", "PUT /api/viewers/me"]);
    s.save("Ada");
    await new Promise(r => setTimeout(r, 10));
    expect(calls).toHaveLength(2);
  });
});
```

`web/shell/src/view/prompt-queue.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { type Ask, promptQueue } from "./prompt-queue";

describe("promptQueue", () => {
  it("shows one prompt at a time, in order", async () => {
    const shown: (Ask | null)[] = [];
    const ask = promptQueue(a => shown.push(a));
    const p = { title: "t", body: "b", allow: "Allow", deny: "Deny" };
    const first = ask(p);
    const second = ask({ ...p, title: "t2" });
    await Promise.resolve();
    expect(shown.filter(Boolean)).toHaveLength(1);
    shown.at(-1)!.answer("allow");
    await expect(first).resolves.toBe("allow");
    await new Promise(r => setTimeout(r, 0));
    expect(shown.at(-1)!.prompt.title).toBe("t2");
    shown.at(-1)!.answer("deny");
    await expect(second).resolves.toBe("deny");
  });
});
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cd web && npx vitest run shell/src/view`
Expected: FAIL, every file with `Failed to resolve import`.

- [ ] **Step 3: Write the models**

`web/shell/src/view/composer-model.ts`: move `Draft`, `captureWait`, `CAPTURE_LATE`, `MAX_CLIP_BYTES`, `nextDraft`, `takePick` and `withClip` from `comments.tsx` with their doc comments, unchanged. Then add:

```ts
import { areaLabel } from "../threads";

/** What the composer shows for its target: the page's label, else the quote
 * (whitespace collapsed, cut at 160 characters), else the custom anchor's
 * name, an area's label, or the element's selector. */
export function composerQuote(draft: Draft): string {
  const quote = draft.anchor.quote?.replace(/\s+/g, " ").trim();
  if (draft.label !== undefined) return draft.label;
  if (quote) return `«${quote.length > 160 ? `${quote.slice(0, 160)}…` : quote}»`;
  if (draft.anchor.kind === "custom") return draft.anchor.custom_name ?? "";
  if (draft.anchor.kind === "area") return areaLabel(draft.anchor);
  return draft.anchor.selector ?? "";
}
```

`web/shell/src/view/pins-model.ts`:

```ts
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";

/** Room a pin keeps from the stage's right edge: its own 22 px plus 16 px for a
 * classic scrollbar in the frame. */
export const PIN_RIGHT_ROOM = 38;

export type PinPlace = { thread: Thread; n: number; left: number; top: number };

/** Where each pin goes over the frame: the open threads on `file` (none when
 * it is null, a document that did not greet) that are not detached, numbered
 * like the sidebar's Open section; only those found with a rectangle not
 * wholly above the frame get a pin, at the rectangle's top right, never past
 * `stage` (its width; 0 while unmeasured) minus the scrollbar's room. */
export function pinPlaces(threads: Thread[], resolved: Record<string, AnchorResult>, file: string | null, stage: number): PinPlace[] {
  const attached = threads.filter(t => t.status === "open" && t.anchor.file === file && !(resolved[t.id] && !resolved[t.id].found));
  const out: PinPlace[] = [];
  attached.forEach((t, i) => {
    const r = resolved[t.id]?.rect;
    if (!r || r.y + r.h <= 0) return;
    let left = r.x + r.w - 12;
    if (stage > 0) left = Math.min(left, stage - PIN_RIGHT_ROOM);
    out.push({ thread: t, n: i + 1, left: Math.max(0, left), top: Math.max(0, r.y - 12) });
  });
  return out;
}
```

`web/shell/src/view/sidebar-model.ts`:

```ts
import { type AnchorResult, INDEX_FILE } from "../../../bridge/src/protocol";
import type { Comment, Thread } from "../threads";
import { hasElapsedLabel } from "../waiting";

export type SidebarSections = { open: Thread[]; detached: Thread[]; resolved: Thread[]; numbers: Map<string, number>; file: string | null };

/** Open threads: those on the page shown and found (numbered like the pins),
 * then those on other pages the version holds; Detached: open threads on the
 * page shown and not found, then those on a page the version does not hold;
 * then resolved threads. `file` undefined means the index; null, a document
 * that did not greet. */
export function sidebarSections(threads: Thread[], resolved: Record<string, AnchorResult>, file: string | null | undefined, holds: (f: string) => boolean = () => true): SidebarSections {
  const page = file === undefined ? INDEX_FILE : file;
  const open = threads.filter(t => t.status === "open");
  const here = open.filter(t => t.anchor.file === page);
  const gone = open.filter(t => !holds(t.anchor.file));
  const detached = [...here.filter(t => resolved[t.id] && !resolved[t.id].found), ...gone.filter(t => !here.includes(t))];
  const attached = here.filter(t => !detached.includes(t));
  const elsewhere = open.filter(t => t.anchor.file !== page && !gone.includes(t));
  return {
    open: [...attached, ...elsewhere],
    detached,
    resolved: threads.filter(t => t.status === "resolved"),
    numbers: new Map(attached.map((t, i) => [t.id, i + 1])),
    file: page,
  };
}

/** Whether a label shows elapsed time, so the sidebar's clock must tick. */
export function needsTicking(threads: Thread[]): boolean {
  return threads.some(t => t.status === "open" && t.sent_to_agent && hasElapsedLabel(t.feedback_state));
}

/** A comment's author line: the agent with its harness, else the viewer's name. */
export function authorLabel(c: Comment): string {
  return c.author_kind === "agent" ? `Agent · via ${c.via_harness ?? c.author_name}` : c.author_name;
}
```

`web/shell/src/view/gallery-model.ts`:

```ts
import type { Artifact } from "../api";

/** The artifacts whose title or description contains `query` (trimmed, any case); all of them for an empty query. */
export function filterArtifacts(list: Artifact[], query: string): Artifact[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter(a => a.title.toLowerCase().includes(q) || (a.description ?? "").toLowerCase().includes(q));
}

/** Who published the card: a harness session, an agent session, or null for the command line. */
export function publisherText(a: Artifact): string | null {
  if (!a.owner_session_id) return null;
  return `published by ${a.owner_harness ? `${a.owner_harness} session` : "an agent session"}`;
}
```

`web/shell/src/view/viewer-name-model.ts`:

```ts
import { NAME_FAILED, NAME_LOAD_FAILED, report, scopedNotice } from "../failure";
import { type Viewer, getViewer, setViewerName } from "../threads";

export type SetNotice = (u: string | null | ((prev: string | null) => string | null)) => void;

/** The "Your name" field's behaviour. A failed load or save shows in the
 * notice; a successful save clears either. A save waits for the initial
 * lookup, which sets the viewer cookie, so the two never create two viewers. */
export class NameSaver {
  private saved = "";
  private loaded: Promise<unknown> = Promise.resolve();
  private edited = false;

  constructor(private readonly setNotice: SetNotice, private readonly onViewer?: (v: Viewer) => void) {}

  /** Starts the lookup; `show` gets the stored name unless the viewer typed first. */
  load(show: (name: string) => void): void {
    this.loaded = report(getViewer(), NAME_LOAD_FAILED, scopedNotice(this.setNotice, NAME_LOAD_FAILED)).then(v => {
      if (!v) return;
      this.onViewer?.(v);
      this.saved = v.display_name ?? "";
      if (!this.edited) show(v.display_name ?? "");
    });
  }

  /** The viewer typed in the field. */
  edit(): void {
    this.edited = true;
  }

  /** Saves `name` (trimmed) once the lookup answered, unless it is the stored name. */
  save(name: string): void {
    const next = name.trim();
    void this.loaded.then(() => {
      if (next === this.saved) return;
      void report(setViewerName(next), NAME_FAILED, scopedNotice(this.setNotice, NAME_FAILED, NAME_LOAD_FAILED)).then(v => {
        if (v) { this.onViewer?.(v); this.saved = v.display_name ?? ""; }
      });
    });
  }
}
```

`web/shell/src/view/prompt-queue.ts`: move `Ask`, `ALLOW_DELAY_MS` and `promptQueue` from `prompt.tsx`, with their doc comments, unchanged. It imports `Prompt` and `PromptAnswer` types from `../caps/grants`.

- [ ] **Step 4: Run the model tests**

Run: `cd web && npx vitest run shell/src/view`
Expected: PASS.

- [ ] **Step 5: Make the Preact components use the models**

- `comments.tsx`: import from `./view/composer-model` and `./view/pins-model`. `Pins` renders `pinPlaces(threads, resolved, file, stage).map(p => <button … key={p.thread.id} title={p.thread.comments[0]?.body ?? ""} aria-label={`Thread ${p.n}`} style={{ left: `${p.left}px`, top: `${p.top}px` }} …>{p.n}</button>)`. `Composer`'s quote paragraph renders `{composerQuote(draft)}`. Delete the moved declarations.
- `sidebar.tsx`: `const s = sidebarSections(p.threads, p.resolved, p.file, p.holds)` replaces the inline filters; `ticking` is `p.now === undefined && needsTicking(p.threads)`; the author line uses `authorLabel(c)`.
- `gallery.tsx`: `const shown = artifacts && filterArtifacts(artifacts, query)`; the publisher span text is `publisherText(a)`, and a null result renders `published from the command line` as before.
- `viewer-name.tsx`: `const saver = useMemo(() => new NameSaver(setNotice, onViewer), [])`; `useEffect(() => saver.load(setName), [])`; `onInput` calls `saver.edit()` before `setName`; `save` becomes `() => saver.save(name)`.
- `prompt.tsx`: keep only `PromptDialog`, importing `Ask` and `ALLOW_DELAY_MS` from `./view/prompt-queue`.
- `artifact.tsx`: import `Draft`, `CAPTURE_LATE`, `MAX_CLIP_BYTES`, `captureWait`, `nextDraft`, `takePick` and `withClip` from `./view/composer-model`, and `promptQueue` and `Ask` from `./view/prompt-queue`.
- Tests: in `comments.test.ts`, import `type Draft, nextDraft, takePick, withClip` from `./view/composer-model` and `PIN_RIGHT_ROOM` from `./view/pins-model`. In `prompt.test.ts`, import `ALLOW_DELAY_MS, type Ask` from `./view/prompt-queue`. In `artifact.test.ts`, `(await import("./view/composer-model")).captureWait.ms = 50`.

- [ ] **Step 6: Run the unit tests, the typecheck and every gate**

Run: `cd web && npm run typecheck && npm test -- --reporter=dot; cd .. && bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: PASS and `exit=0`.

- [ ] **Step 7: Commit**

```bash
git add web/shell/src/view/composer-model.ts web/shell/src/view/pins-model.ts web/shell/src/view/sidebar-model.ts web/shell/src/view/gallery-model.ts web/shell/src/view/viewer-name-model.ts web/shell/src/view/prompt-queue.ts web/shell/src/view/composer-model.test.ts web/shell/src/view/pins-model.test.ts web/shell/src/view/sidebar-model.test.ts web/shell/src/view/gallery-model.test.ts web/shell/src/view/viewer-name-model.test.ts web/shell/src/view/prompt-queue.test.ts web/shell/src/comments.tsx web/shell/src/sidebar.tsx web/shell/src/gallery.tsx web/shell/src/viewer-name.tsx web/shell/src/prompt.tsx web/shell/src/artifact.tsx web/shell/src/comments.test.ts web/shell/src/prompt.test.ts web/shell/src/artifact.test.ts
git commit --no-gpg-sign -m "Move the leaf components' decisions into framework-free view models

Pin placement, sidebar sections, the composer's quote, gallery search,
the name saver and the prompt queue are plain TypeScript with their own
tests; the Preact components only render what they return."
```

---

### Task 4: The artifact view's building blocks, framework-free

These are the pieces `ArtifactController` (Task 5) is assembled from. Each carries one rule that `artifact.tsx` held in refs, and each gets its own tests. Nothing uses them yet.

**Files:**
- Create: `web/shell/src/view/store.ts`, `view/url.ts`, `view/frame-gate.ts`, `view/anchor-handles.ts`, `view/thread-sync.ts`, `view/frame-host.ts`, `view/skeleton.ts`
- Create: `web/shell/src/view/store.test.ts`, `view/url.test.ts`, `view/frame-gate.test.ts`, `view/anchor-handles.test.ts`, `view/thread-sync.test.ts`, `view/frame-host.test.ts`, `view/skeleton.test.ts`

**Interfaces (Produces):**
- `class Store<S extends object> { constructor(initial: S); get(): S; set(patch: Partial<S> | ((s: S) => Partial<S>)): void; subscribe(fn: (s: S) => void): () => void }`. `subscribe` follows the Svelte store contract (`fn` runs at once, then after every change). `set` replaces the snapshot only when some field changed (`Object.is`).
- `setUrl(url: string, push?: boolean): boolean`, `validHash(h: unknown): h is string`
- `class FrameGate { open: boolean; reset(): void; hello(ok: boolean): void; load(): boolean; close(): void }`
- `class AnchorHandles { handle(threadId: string): string; known(threadId: string): string | null; thread(handle: string): string | undefined; forget(): void }`
- `type ThreadChange = (ts: Thread[]) => Thread[]`, `class ThreadSync { constructor(apply: (f: ThreadChange) => void); change(f: ThreadChange): void; begin(): (answer: Thread[] | undefined) => void }`
- `FRAME_SANDBOX: string`, `class FrameHost { el: HTMLIFrameElement | null; constructor(stage: HTMLElement, onLoad: () => void); show(src: string, sandboxed: boolean, key: string): HTMLIFrameElement; remove(): void }`
- `SKELETON_HTML: string`, `type Skeleton = { page: HTMLElement; title: HTMLElement; viewer: HTMLElement; stage: HTMLElement; topbarIsland: HTMLElement; stageIsland: HTMLElement; sidebarIsland: HTMLElement }`, `skeleton(root: HTMLElement): Skeleton`

- [ ] **Step 1: Write the failing tests**

`web/shell/src/view/store.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { Store } from "./store";

describe("Store", () => {
  it("calls a subscriber at once and after each real change, never for an equal patch", () => {
    const s = new Store({ a: 1, b: [1] });
    const seen = vi.fn();
    const off = s.subscribe(seen);
    expect(seen).toHaveBeenCalledTimes(1);
    s.set({ a: 1 });
    expect(seen).toHaveBeenCalledTimes(1);
    const before = s.get();
    s.set(x => ({ a: x.a + 1 }));
    expect(seen).toHaveBeenCalledTimes(2);
    expect(s.get()).not.toBe(before);
    expect(s.get()).toEqual({ a: 2, b: [1] });
    off();
    s.set({ a: 3 });
    expect(seen).toHaveBeenCalledTimes(2);
  });
});
```

`web/shell/src/view/url.test.ts`:

```ts
import { afterEach, describe, expect, it, vi } from "vitest";
import { setUrl, validHash } from "./url";

describe("url", () => {
  afterEach(() => { vi.restoreAllMocks(); history.replaceState(null, "", "/"); });
  it("accepts an empty or #-led fragment of at most 512 characters", () => {
    expect(validHash("")).toBe(true);
    expect(validHash("#a")).toBe(true);
    expect(validHash("a")).toBe(false);
    expect(validHash(`#${"x".repeat(512)}`)).toBe(false);
    expect(validHash(3)).toBe(false);
  });
  it("pushes or replaces, and reports a refusal instead of throwing", () => {
    const n = history.length;
    expect(setUrl("/a/x", true)).toBe(true);
    expect(history.length).toBe(n + 1);
    expect(setUrl("/a/y")).toBe(true);
    expect(location.pathname).toBe("/a/y");
    vi.spyOn(history, "pushState").mockImplementation(() => { throw new DOMException("too many", "SecurityError"); });
    expect(setUrl("/a/z", true)).toBe(false);
    expect(location.pathname).toBe("/a/y");
  });
});
```

`web/shell/src/view/frame-gate.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { FrameGate } from "./frame-gate";

describe("FrameGate", () => {
  it("opens on a matching hello, closes on a foreign one", () => {
    const g = new FrameGate();
    g.hello(true);
    expect(g.open).toBe(true);
    g.hello(false);
    expect(g.open).toBe(false);
  });
  it("closes on a load no matching hello preceded, and keeps a page that greeted before its load", () => {
    const g = new FrameGate();
    g.hello(true);
    expect(g.load()).toBe(false);
    expect(g.open).toBe(true);
    expect(g.load()).toBe(true);
    expect(g.open).toBe(false);
    g.hello(true);
    expect(g.open).toBe(true);
  });
  it("closes on reset and on close", () => {
    const g = new FrameGate();
    g.hello(true);
    g.close();
    expect(g.open).toBe(false);
    g.hello(true);
    g.reset();
    expect(g.open).toBe(false);
    expect(g.load()).toBe(true);
  });
});
```

`web/shell/src/view/anchor-handles.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { AnchorHandles } from "./anchor-handles";

describe("AnchorHandles", () => {
  it("gives each thread one opaque handle until forgotten, and maps it back", () => {
    const h = new AnchorHandles();
    const a = h.handle("t1");
    expect(a).toMatch(/^a[0-9a-f]{24}$/);
    expect(h.handle("t1")).toBe(a);
    expect(h.known("t1")).toBe(a);
    expect(h.thread(a)).toBe("t1");
    expect(h.known("t2")).toBeNull();
    h.forget();
    expect(h.thread(a)).toBeUndefined();
    expect(h.handle("t1")).not.toBe(a);
  });
});
```

`web/shell/src/view/thread-sync.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Thread } from "../threads";
import { ThreadSync } from "./thread-sync";

const t = (id: string) => ({ id }) as Thread;

describe("ThreadSync", () => {
  it("replays changes made while a load was in flight on top of its answer", () => {
    let threads: Thread[] = [];
    const sync = new ThreadSync(f => { threads = f(threads); });
    const done = sync.begin();
    sync.change(ts => [...ts, t("event")]);
    done([t("loaded")]);
    expect(threads.map(x => x.id)).toEqual(["loaded", "event"]);
  });
  it("applies only the latest load's answer, and keeps nothing once it answered or failed", () => {
    let threads: Thread[] = [];
    const sync = new ThreadSync(f => { threads = f(threads); });
    const older = sync.begin();
    const newer = sync.begin();
    older([t("old")]);
    expect(threads).toEqual([]);
    newer(undefined);
    sync.change(ts => [...ts, t("later")]);
    expect(threads.map(x => x.id)).toEqual(["later"]);
    const again = sync.begin();
    again([t("fresh")]);
    expect(threads.map(x => x.id)).toEqual(["fresh"]);
  });
});
```

`web/shell/src/view/frame-host.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { FRAME_SANDBOX, FrameHost } from "./frame-host";

function stage() {
  const s = document.createElement("div");
  s.innerHTML = `<div class="island"></div>`;
  document.body.append(s);
  return s;
}

describe("FrameHost", () => {
  it("creates the frame first in the stage with the shell's attributes, and keeps it for the same key", () => {
    const s = stage();
    const host = new FrameHost(s, () => {});
    const el = host.show("/c/x/v/1/", true, "1-s");
    expect(s.firstElementChild).toBe(el);
    expect(el.className).toBe("frame");
    expect(el.title).toBe("artifact content");
    expect(el.getAttribute("allow")).toBe("clipboard-write; fullscreen");
    expect(el.getAttribute("sandbox")).toBe(FRAME_SANDBOX);
    expect(host.show("/c/x/v/1/#moved", true, "1-s")).toBe(el);
    const next = host.show("http://x.localhost:1/v/2/", false, "2-o");
    expect(next).not.toBe(el);
    expect(el.isConnected).toBe(false);
    expect(next.hasAttribute("sandbox")).toBe(false);
  });
  it("adopts a frame already in the stage when its src and sandboxing match, and replaces it otherwise", () => {
    const s = stage();
    s.insertAdjacentHTML("afterbegin", `<iframe class="frame" src="/c/x/v/1/" sandbox="${FRAME_SANDBOX}"></iframe>`);
    const served = s.querySelector("iframe")!;
    const onLoad = vi.fn();
    expect(new FrameHost(s, onLoad).show("/c/x/v/1/", true, "1-s")).toBe(served);
    served.dispatchEvent(new Event("load"));
    expect(onLoad).toHaveBeenCalledTimes(1);
    const other = stage();
    other.insertAdjacentHTML("afterbegin", `<iframe class="frame" src="http://x.localhost:1/v/1/"></iframe>`);
    const stale = other.querySelector("iframe")!;
    const made = new FrameHost(other, () => {}).show("/c/x/v/1/", true, "1-s");
    expect(made).not.toBe(stale);
    expect(stale.isConnected).toBe(false);
  });
  it("removes the frame and stops hearing its loads", () => {
    const s = stage();
    const onLoad = vi.fn();
    const host = new FrameHost(s, onLoad);
    const el = host.show("/c/x/v/1/", true, "k");
    host.remove();
    el.dispatchEvent(new Event("load"));
    expect(onLoad).not.toHaveBeenCalled();
    expect(host.el).toBeNull();
    expect(s.querySelector("iframe")).toBeNull();
  });
});
```

`web/shell/src/view/skeleton.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { SKELETON_HTML, skeleton } from "./skeleton";

describe("skeleton", () => {
  it("creates the page once and finds the same elements again", () => {
    const root = document.createElement("div");
    const a = skeleton(root);
    const b = skeleton(root);
    expect(root.querySelectorAll(".page")).toHaveLength(1);
    expect(b.stage).toBe(a.stage);
    expect(a.title.textContent).toBe("Clax");
    expect(a.stage.parentElement).toBe(a.viewer);
    expect(a.stageIsland.parentElement).toBe(a.stage);
    expect(a.sidebarIsland.parentElement).toBe(a.viewer);
    expect(a.topbarIsland.parentElement?.classList.contains("topbar")).toBe(true);
  });
  it("adopts a page the server sent", () => {
    const root = document.createElement("div");
    root.innerHTML = `<div class="page">${SKELETON_HTML}</div>`;
    const page = root.firstElementChild;
    expect(skeleton(root).page).toBe(page);
  });
});
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cd web && npx vitest run shell/src/view`
Expected: the seven new files FAIL with `Failed to resolve import`; the Task 3 files PASS.

- [ ] **Step 3: Write the modules**

`web/shell/src/view/store.ts`:

```ts
/** An immutable-snapshot store. `subscribe` follows the Svelte store
 * contract, so a Svelte component reads it with `fromStore`; a Preact
 * component, with `useState` plus `subscribe`. */
export class Store<S extends object> {
  private value: S;
  private readonly subs = new Set<(s: S) => void>();

  constructor(initial: S) {
    this.value = initial;
  }

  get(): S {
    return this.value;
  }

  /** Merges `patch` into a new snapshot and tells every subscriber; a patch
   * whose every field is already equal (`Object.is`) changes nothing. */
  set(patch: Partial<S> | ((s: S) => Partial<S>)): void {
    const p = typeof patch === "function" ? patch(this.value) : patch;
    const keys = Object.keys(p) as (keyof S)[];
    if (keys.every(k => Object.is(p[k], this.value[k]))) return;
    this.value = { ...this.value, ...p };
    for (const fn of [...this.subs]) fn(this.value);
  }

  subscribe(fn: (s: S) => void): () => void {
    this.subs.add(fn);
    fn(this.value);
    return () => { this.subs.delete(fn); };
  }
}
```

`web/shell/src/view/url.ts`: move `validHash` and `setUrl` from `artifact.tsx` verbatim, with their doc comments, and export both. `artifact.tsx` imports them from here.

`web/shell/src/view/frame-gate.ts`:

```ts
/** Whether the frame's latest hello named the shown artifact and version (and
 * a page it holds): only then are its capability requests answered and events
 * pushed to it, so a document the frame navigated to gets nothing. A frame
 * load with no matching hello since the previous load closes the gate as
 * well, and a later hello reopens it: a wrapped page's hello may arrive
 * before or after its load event, so this never shuts out a wrapped page, and
 * it shuts out a document without the bridge whenever the page before it
 * greeted before its own load. */
export class FrameGate {
  open = false;
  private helloSinceLoad = false;

  /** Another artifact, version or origin is shown: closed until it greets. */
  reset(): void {
    this.open = false;
    this.helloSinceLoad = false;
  }

  hello(ok: boolean): void {
    this.open = ok;
    if (ok) this.helloSinceLoad = true;
  }

  /** The frame loaded a document. True when no matching hello came since the
   * previous load: the gate closed, and the page and its pins are forgotten. */
  load(): boolean {
    const stale = !this.helloSinceLoad;
    if (stale) this.open = false;
    this.helloSinceLoad = false;
    return stale;
  }

  /** The shell sent the frame to another page: closed until that page greets. */
  close(): void {
    this.open = false;
  }
}
```

`web/shell/src/view/anchor-handles.ts`:

```ts
/** Thread anchors go to the frame under opaque handles, new for every page
 * that greets, so a page never learns a thread's store ID; the frame's
 * results are mapped back through `thread`. */
export class AnchorHandles {
  private byHandle = new Map<string, string>();
  private byThread = new Map<string, string>();

  handle(threadId: string): string {
    let h = this.byThread.get(threadId);
    if (!h) {
      const b = new Uint8Array(12);
      crypto.getRandomValues(b);
      h = `a${Array.from(b, x => x.toString(16).padStart(2, "0")).join("")}`;
      this.byThread.set(threadId, h);
      this.byHandle.set(h, threadId);
    }
    return h;
  }

  /** The handle already given to the thread on this page, if any. */
  known(threadId: string): string | null {
    return this.byThread.get(threadId) ?? null;
  }

  thread(handle: string): string | undefined {
    return this.byHandle.get(handle);
  }

  forget(): void {
    this.byHandle = new Map();
    this.byThread = new Map();
  }
}
```

`web/shell/src/view/thread-sync.ts`:

```ts
import type { Thread } from "../threads";

export type ThreadChange = (ts: Thread[]) => Thread[];

/** Keeps a thread list loaded from the daemon consistent with changes that
 * happen while the load is in flight (events, this shell's own writes): its
 * answer may predate them, so they are replayed on top of it. Only the latest
 * load's answer is applied; once it answers or fails, nothing more is kept. */
export class ThreadSync {
  private n = 0;
  private since: ThreadChange[] | null = null;

  constructor(private readonly apply: (f: ThreadChange) => void) {}

  change(f: ThreadChange): void {
    this.since?.push(f);
    this.apply(f);
  }

  /** Starts a load; call the result with its answer, or undefined when it failed. */
  begin(): (answer: Thread[] | undefined) => void {
    const n = ++this.n;
    const since: ThreadChange[] = [];
    this.since = since;
    return answer => {
      if (n !== this.n) return;
      this.since = null;
      if (answer) this.apply(() => since.reduce((acc, f) => f(acc), answer));
    };
  }
}
```

`web/shell/src/view/frame-host.ts`:

```ts
/** The sandbox a frame gets when artifacts have no origin of their own. */
export const FRAME_SANDBOX = "allow-scripts allow-forms allow-modals allow-popups allow-downloads";

/** The content `<iframe>`, first in the stage. The shell never re-creates it
 * on a render: it is replaced only when `key` (version and frame mode)
 * changes, and one the daemon already put in the stage is adopted when its
 * `src` and sandboxing are what the shell would have made. */
export class FrameHost {
  el: HTMLIFrameElement | null = null;
  private key = "";
  private readonly loaded = () => this.onLoad();

  constructor(private readonly stage: HTMLElement, private readonly onLoad: () => void) {}

  show(src: string, sandboxed: boolean, key: string): HTMLIFrameElement {
    if (this.el && this.key === key) return this.el;
    const served = this.el ? null : this.stage.querySelector<HTMLIFrameElement>(":scope > iframe.frame");
    if (served && served.getAttribute("src") === src && served.hasAttribute("sandbox") === sandboxed) {
      this.take(served, key);
      return served;
    }
    served?.remove();
    this.remove();
    const el = this.stage.ownerDocument.createElement("iframe");
    el.className = "frame";
    el.title = "artifact content";
    el.setAttribute("allow", "clipboard-write; fullscreen");
    if (sandboxed) el.setAttribute("sandbox", FRAME_SANDBOX);
    el.src = src;
    this.take(el, key);
    this.stage.prepend(el);
    return el;
  }

  remove(): void {
    this.el?.removeEventListener("load", this.loaded);
    this.el?.remove();
    this.el = null;
    this.key = "";
  }

  private take(el: HTMLIFrameElement, key: string): void {
    el.addEventListener("load", this.loaded);
    this.el = el;
    this.key = key;
  }
}
```

`web/shell/src/view/skeleton.ts`:

```ts
/** The artifact view's static layout. `.island` elements are `display:
 * contents` mount points for the topbar controls, the stage's overlays and the
 * sidebar; `<!--clax:frame-->` marks where the daemon may put the content
 * frame (Task 12). `artifact.html` carries the same markup. */
export const SKELETON_HTML = `<header class="topbar"><a href="/" title="Gallery">←</a><h1>Clax</h1><div class="island"></div></header><div class="viewer"><div class="stage"><!--clax:frame--><div class="island"></div></div><div class="island"></div></div>`;

export type Skeleton = { page: HTMLElement; title: HTMLElement; viewer: HTMLElement; stage: HTMLElement; topbarIsland: HTMLElement; stageIsland: HTMLElement; sidebarIsland: HTMLElement };

/** The skeleton in `root`, adopted when present (sent by the daemon), else created. */
export function skeleton(root: HTMLElement): Skeleton {
  let page = root.querySelector<HTMLElement>(":scope > .page");
  if (!page) {
    page = root.ownerDocument.createElement("div");
    page.className = "page";
    page.innerHTML = SKELETON_HTML;
    root.append(page);
  }
  const p: HTMLElement = page;
  const q = (sel: string) => p.querySelector<HTMLElement>(sel)!;
  return { page: p, title: q(".topbar > h1"), viewer: q(".viewer"), stage: q(".stage"), topbarIsland: q(".topbar > .island"), stageIsland: q(".stage > .island"), sidebarIsland: q(".viewer > .island") };
}
```

In `web/shell/src/theme.css`, append: `.island { display: contents; }`

- [ ] **Step 4: Run the tests**

Run: `cd web && npx vitest run shell/src/view`
Expected: PASS.

- [ ] **Step 5: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 6: Commit**

```bash
git add web/shell/src/view/store.ts web/shell/src/view/url.ts web/shell/src/view/frame-gate.ts web/shell/src/view/anchor-handles.ts web/shell/src/view/thread-sync.ts web/shell/src/view/frame-host.ts web/shell/src/view/skeleton.ts web/shell/src/view/store.test.ts web/shell/src/view/url.test.ts web/shell/src/view/frame-gate.test.ts web/shell/src/view/anchor-handles.test.ts web/shell/src/view/thread-sync.test.ts web/shell/src/view/frame-host.test.ts web/shell/src/view/skeleton.test.ts web/shell/src/artifact.tsx web/shell/src/theme.css
git commit --no-gpg-sign -m "Add the artifact view's framework-free building blocks

A snapshot store with the Svelte store contract, the hello gate, anchor
handles, the thread-load replay, the frame host that creates or adopts
the content iframe, and the page skeleton with display: contents island
mount points."
```

---

### Task 5: ArtifactController and three Preact islands replace `artifact.tsx`

This is the one large move. Every line of behaviour in `artifact.tsx` moves into `ArtifactController`, and `artifact.tsx` is deleted. The 40 `ArtifactView` unit tests and the e2e suite pin the result. Only the local mount helper of `artifact.test.ts` changes.

**Files:**
- Create: `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`
- Create: `web/shell/src/artifact.ts` (mount + re-exports), `web/shell/src/islands/index.ts`, `web/shell/src/islands/preact.tsx`
- Delete: `web/shell/src/artifact.tsx`, `web/shell/src/frame.tsx`
- Modify: `web/shell/src/main.tsx`, `web/shell/src/artifact.test.ts` (the `mountView` helper only)

**Interfaces:**
- Consumes: everything Task 3 and Task 4 produce.
- Produces, from `view/artifact-controller.ts`:
  ```ts
  export type ArtifactProps = { id: string; pinnedVersion: number | null; file?: string };
  export type Loaded = { artifact: Artifact; versions: Version[] };
  export type ViewState = { data: Loaded | null; error: string | null; origin: string | null | undefined; newer: number | null; deleted: boolean; commenting: boolean; panel: boolean; narrow: boolean; threads: Thread[]; resolved: Record<string, AnchorResult>; draft: Draft | null; selected: string | null; hovered: string | null; busy: number; notice: string | null; hint: string | null; me: Viewer | null; ask: Ask | null; file: string | null };
  export const pageWait: { ms: number };
  export const MOVE_TO_PICK: string; export const MOVE_TO_CLICK: string;
  export class ArtifactController {
    readonly state: Store<ViewState>; readonly id: string; readonly pinnedVersion: number | null; readonly startFile: string;
    frame: FrameHost | null;
    constructor(props: ArtifactProps);
    start(): void; dispose(): void; frameLoaded(): void;
    shown(s?: ViewState): number; latest(s?: ViewState): number; missing(s?: ViewState): string | null;
    holds(f: string, s?: ViewState): boolean; here(version: number | null, s?: ViewState): string; rawHref(s?: ViewState): string; openCount(s?: ViewState): number;
    toggleComment(): void; togglePanel(): void; chooseVersion(n: number): void; copyLink(): void; reloadLatest(): void;
    dismissNotice(): void; readonly setNotice: SetNotice; setMe(v: Viewer): void;
    selectThread(t: Thread): void; openPin(t: Thread): void; hover(t: Thread | null): void;
    sendThread(t: Thread): void; resolveThread(t: Thread): void; reply(t: Thread, body: string): void;
    composerInput(text: string): void; cancelDraft(): void; submitDraft(body: string): Promise<void>;
  }
  ```
- Produces, from `islands/index.ts`: `type MountIsland = (target: HTMLElement, ctl: ArtifactController) => () => void; type Islands = { topbar: MountIsland; stage: MountIsland; sidebar: MountIsland }; const ISLANDS: Islands`.
- Produces, from `artifact.ts`: `mountArtifactView(root: HTMLElement, props: ArtifactProps, islands?: Islands): { root: HTMLElement; update(props: ArtifactProps): void; unmount(): void }`, plus re-exports of `pageWait`, `MOVE_TO_PICK`, `MOVE_TO_CLICK`.

- [ ] **Step 1: Write the controller's own failing tests**

`web/shell/src/view/artifact-controller.test.ts`:

```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ID = "7q3k9mzx2b4t";
const loaded = { artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "x", current_version: 2, pinned: false }, versions: [{ artifact_id: ID, n: 2, label: null, created_at: "x", files: {} }] };

class FakeES { addEventListener() {} close() {} }

async function started() {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(
    url.includes("/threads") ? { threads: [], next_cursor: null } : url.startsWith("/api/viewers") ? { viewer: { public_id: "u_1", display_name: null, created_at: "x" } } : url === "/api/token" ? { token: "tk" } : loaded))));
  sessionStorage.setItem("clax.origin-ok", "0");
  const { ArtifactController } = await import("./artifact-controller");
  const { FrameHost } = await import("./frame-host");
  const stage = document.createElement("div");
  document.body.append(stage);
  const ctl = new ArtifactController({ id: ID, pinnedVersion: null });
  ctl.frame = new FrameHost(stage, () => ctl.frameLoaded());
  ctl.start();
  const deadline = Date.now() + 2000;
  while (!stage.querySelector("iframe") && Date.now() < deadline) await new Promise(r => setTimeout(r, 5));
  return { ctl, frame: stage.querySelector("iframe")! };
}

const hello = (win: Window, version = 2) => window.dispatchEvent(new MessageEvent("message", { data: { type: "clax:hello", artifact: ID, version, file: "index.html" }, origin: "null", source: win }));

describe("ArtifactController", () => {
  beforeEach(() => { vi.resetModules(); history.replaceState(null, "", `/a/${ID}`); });
  afterEach(() => { vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); });

  it("inserts the frame only once the artifact and the frame mode are known, with the gate already reset", async () => {
    const { ctl, frame } = await started();
    expect(ctl.state.get().data?.artifact.title).toBe("T");
    expect(ctl.state.get().origin).toBeNull();
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
    const posted: { type: string }[] = [];
    frame.contentWindow!.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
    hello(frame.contentWindow!);
    expect(posted.map(m => m.type)).toContain("clax:welcome");
    ctl.dispose();
  });

  it("tells the capability host about UI changes once per turn, however many fields changed", async () => {
    const { ctl, frame } = await started();
    hello(frame.contentWindow!);
    const { CapabilityHost } = await import("../caps/host");
    const ui = vi.spyOn(CapabilityHost.prototype, "uiChanged");
    ctl.toggleComment();
    ctl.togglePanel();
    ctl.hover(null);
    ctl.toggleComment();
    ctl.toggleComment();
    await Promise.resolve();
    await Promise.resolve();
    expect(ui).toHaveBeenCalledTimes(1);
    ctl.dispose();
  });

  it("after dispose, answers no hello and changes no state", async () => {
    const { ctl, frame } = await started();
    const posted: unknown[] = [];
    frame.contentWindow!.postMessage = ((m: unknown) => { posted.push(m); }) as Window["postMessage"];
    ctl.dispose();
    hello(frame.contentWindow!);
    const before = ctl.state.get();
    ctl.toggleComment();
    expect(posted).toEqual([]);
    expect(ctl.state.get()).toBe(before);
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd web && npx vitest run shell/src/view/artifact-controller.test.ts`
Expected: FAIL with `Failed to resolve import "./artifact-controller"`.

- [ ] **Step 3: Write the controller**

`web/shell/src/view/artifact-controller.ts`:

```ts
// The artifact view's state and behaviour, framework-free: what artifact.tsx
// held in hooks. Components render `state` and call the intent methods; the
// frame is a FrameHost the mount gives it.
import { type AnchorResult, INDEX_FILE, type ShellToBridge } from "../../../bridge/src/protocol";
import { ApiError, type Artifact, type Version, getArtifact, getToken } from "../api";
import { acceptFromFrame, helloMatches, sendToFrame } from "../bridge-link";
import type { Declared } from "../caps/availability";
import { frameGesture, onShieldPress, setForwardedKeys } from "../caps/gesture";
import { CapabilityHost, type CommentsUi } from "../caps/host";
import { type ArtifactEvent, subscribe } from "../events";
import { LOAD_FAILED, OPEN_FAILED, POST_FAILED, RESOLVE_FAILED, SEND_FAILED, report, scopedNotice } from "../failure";
import { nav } from "../nav";
import { artifactOrigin, pageSrc, probeOrigin } from "../origin";
import { parseShellPath, shellPath } from "../route";
import { type Thread, type Viewer, addComment, createThread, currentViewer, getViewer, listThreads, onViewer, resolveThread, sendToAgent, upsert } from "../threads";
import { AnchorHandles } from "./anchor-handles";
import { CAPTURE_LATE, type Draft, MAX_CLIP_BYTES, captureWait, nextDraft, takePick, withClip } from "./composer-model";
import { FrameGate } from "./frame-gate";
import type { FrameHost } from "./frame-host";
import { type Ask, promptQueue } from "./prompt-queue";
import { Store } from "./store";
import { type ThreadChange, ThreadSync } from "./thread-sync";
import { setUrl, validHash } from "./url";
import type { SetNotice } from "./viewer-name-model";

export type ArtifactProps = { id: string; pinnedVersion: number | null; file?: string };
export type Loaded = { artifact: Artifact; versions: Version[] };
export type ViewState = {
  data: Loaded | null;
  error: string | null;
  /** The artifact origin, null in sandbox mode, undefined until decided. */
  origin: string | null | undefined;
  newer: number | null;
  deleted: boolean;
  commenting: boolean;
  panel: boolean;
  narrow: boolean;
  threads: Thread[];
  resolved: Record<string, AnchorResult>;
  draft: Draft | null;
  selected: string | null;
  /** The thread whose card or pin the pointer is over; it, else the selected
   * thread, is the frame's focus (a drawn area is outlined dashed). */
  hovered: string | null;
  /** Posts and sends to the agent in flight. */
  busy: number;
  notice: string | null;
  /** A brief hint when the viewer's press did not count as their gesture in the page. */
  hint: string | null;
  me: Viewer | null;
  ask: Ask | null;
  /** The published file of the page in the frame, from its latest matching
   * hello (the URL's file until then); null while the frame shows a document
   * that did not greet. */
  file: string | null;
};

/** How long a pick's start stays valid for its pick (longer than the longest clip render, an area's 12 s). */
const PICK_WAIT_MS = 20_000;
/** The notice kind for a thread the daemon kept without its screenshot. */
const CLIP_DROPPED = "Posted without its screenshot";
/** How long the gesture hint stays. */
const HINT_MS = 2_500;
/** How long a page the shell sent the frame to may take to greet before the jump is given up (settable for tests). */
export const pageWait = { ms: 5000 };
export const MOVE_TO_PICK = "Move the pointer to pick";
export const MOVE_TO_CLICK = "Move the pointer, then click again";

const media = (q: string) => typeof matchMedia === "function" && matchMedia(q).matches;
const ids = (ts: Thread[]) => ts.map(t => t.id).join(",");

export class ArtifactController {
  readonly state: Store<ViewState>;
  readonly id: string;
  readonly pinnedVersion: number | null;
  readonly startFile: string;
  /** The URL fragment the frame opens at. */
  readonly startHash: string;
  frame: FrameHost | null = null;

  private disposed = false;
  private live = false;
  private latestKnown = 0;
  private viewKey = "";
  private readonly gate = new FrameGate();
  private readonly handles = new AnchorHandles();
  private readonly sync: ThreadSync;
  /** Picks whose start arrived with the viewer's gesture in the frame, by pick ID, with when. */
  private readonly startedPicks = new Map<string, number>();
  /** The pick ID of the open composer when a pick made in comment mode opened
   * it: comment mode, off while that composer is open, comes back when it closes. */
  private resumeAfter: string | null = null;
  /** What the open composer holds, so a page's open never replaces typed text. */
  private composerText = "";
  /** A page anchors threads itself (comments.customAnchors). */
  private customLive = false;
  private host: CapabilityHost | null = null;
  /** This view's own page publishes in flight; see CapEnv.ownPublish. */
  private readonly ownPublish: { active: number; settled?(): void } = { active: 0 };
  private deferredPublish: number | null = null;
  private pendingScroll: { thread: Thread; timer: ReturnType<typeof setTimeout> } | null = null;
  /** The frame's latest known fragment. */
  private frameHash: string;
  private hashFrame = 0;
  private hintTimer: ReturnType<typeof setTimeout> | undefined;
  private captureTimer: ReturnType<typeof setTimeout> | undefined;
  private overFrame = false;
  private stream: (() => void) | null = null;
  private readonly offs: (() => void)[] = [];
  /** The snapshot before this turn's first change, while reactions are queued. */
  private before: ViewState | null = null;
  private readonly prompt = promptQueue(ask => this.set({ ask }));
  readonly commentsUi: CommentsUi;

  constructor(props: ArtifactProps) {
    this.id = props.id;
    this.pinnedVersion = props.pinnedVersion;
    this.startFile = props.file ?? INDEX_FILE;
    this.startHash = location.hash;
    this.frameHash = this.startHash;
    this.state = new Store<ViewState>({
      data: null, error: null, origin: undefined, newer: null, deleted: false, commenting: false,
      panel: media("(min-width: 900px)"), narrow: media("(max-width: 480px)"),
      threads: [], resolved: {}, draft: null, selected: null, hovered: null, busy: 0,
      notice: null, hint: null, me: null, ask: null, file: this.startFile,
    });
    this.sync = new ThreadSync(f => this.set(s => ({ threads: f(s.threads) })));
    this.ownPublish.settled = () => {
      const n = this.deferredPublish;
      this.deferredPublish = null;
      if (n === null) return;
      if (this.pinnedVersion === null) nav.assign(this.here(null));
      else if (n > this.latestKnown) { this.latestKnown = n; this.set({ newer: n }); }
    };
    this.commentsUi = {
      openComposer: (d, opts) => {
        const next = nextDraft(this.s.draft, this.composerText, d, opts);
        if (!next) return false;
        this.set({ draft: next });
        return true;
      },
      attachClip: (token, clip, clipError) => this.set(s => ({ draft: withClip(s.draft, token, clip, clipError) })),
      upsert: t => this.changeThreads(ts => upsert(ts, t)),
      remove: tid => { this.changeThreads(ts => ts.filter(t => t.id !== tid)); this.set(s => ({ selected: s.selected === tid ? null : s.selected })); },
      setCustom: live => {
        this.customLive = live;
        if (live) this.set({ resolved: {} });
        else this.resolveAll();
      },
      place: rects => this.set({ resolved: Object.fromEntries(Object.entries(rects).map(([tid, rect]) => [tid, { id: tid, found: true, method: "custom" as const, rect }])) }),
      select: tid => this.set({ panel: true, selected: tid }),
      exitMode: () => { if (!this.composerText.trim()) this.set({ commenting: false }); },
      dismiss: () => {
        if (this.s.draft) {
          if (this.composerText.trim()) return false;
          this.set({ draft: null });
          return true;
        }
        if (this.s.selected === null) return false;
        this.set({ selected: null });
        return true;
      },
      enterMode: () => this.set({ commenting: true }),
      state: () => ({ mode: this.s.commenting, composing: this.s.draft !== null, threads: this.s.threads, selected: this.s.selected, busy: this.s.busy > 0 }),
    };
  }

  private get s(): ViewState {
    return this.state.get();
  }

  // ---- derived values (pure over a snapshot, so components can derive them) ----

  shown(s: ViewState = this.s): number {
    return this.pinnedVersion ?? s.data?.artifact.current_version ?? 0;
  }

  latest(s: ViewState = this.s): number {
    return s.data?.artifact.current_version ?? 0;
  }

  private version(s: ViewState = this.s): Version | undefined {
    return s.data?.versions.find(v => v.n === this.shown(s));
  }

  /** A page the URL names that the shown version does not hold (the index always is). */
  missing(s: ViewState = this.s): string | null {
    const v = this.version(s);
    return this.startFile !== INDEX_FILE && v !== undefined && !Object.hasOwn(v.files, this.startFile) ? this.startFile : null;
  }

  /** Whether the shown version holds `f` (the index always; any file while the versions are unknown). */
  holds(f: string, s: ViewState = this.s): boolean {
    const v = this.version(s);
    return f === INDEX_FILE || !v || Object.hasOwn(v.files, f);
  }

  private isPage(f: string): boolean {
    const v = this.version();
    if (f === INDEX_FILE || !v) return true;
    return Object.hasOwn(v.files, f) && v.files[f].content_type.split(";")[0].trim().toLowerCase() === "text/html";
  }

  private pageNow(s: ViewState): string {
    const r = parseShellPath(location.pathname);
    return s.file ?? (r.kind === "artifact" ? r.file : INDEX_FILE);
  }

  /** The shell URL of the current page in `version` (null: the latest). */
  here(version: number | null, s: ViewState = this.s): string {
    return shellPath(this.id, version, this.pageNow(s), this.shown(s));
  }

  rawHref(s: ViewState = this.s): string {
    return pageSrc(this.id, this.shown(s), s.origin ?? null, this.pageNow(s));
  }

  openCount(s: ViewState = this.s): number {
    return s.threads.filter(t => t.status === "open").length;
  }

  // ---- state and reactions ----

  private set(patch: Partial<ViewState> | ((s: ViewState) => Partial<ViewState>)): void {
    if (this.disposed) return;
    const prev = this.s;
    this.state.set(patch);
    if (this.before === null && this.s !== prev) {
      this.before = prev;
      queueMicrotask(() => this.react());
    }
  }

  /** What Preact ran as effects after a render: once per turn, over the
   * difference between the snapshot before the turn's first change and now. */
  private react(): void {
    const prev = this.before;
    this.before = null;
    if (!prev || this.disposed) return;
    const s = this.s;
    if (ids(prev.threads) !== ids(s.threads) || prev.origin !== s.origin || prev.data !== s.data) this.resolveAll();
    if (prev.commenting !== s.commenting) {
      this.send({ type: "clax:comment-mode", on: s.commenting });
      // A pick still in flight when comment mode ends is moot.
      if (!s.commenting) this.startedPicks.clear();
    }
    if (prev.draft !== s.draft) this.draftChanged(s.draft);
    if (prev.hovered !== s.hovered || prev.selected !== s.selected || prev.threads !== s.threads) this.sendFocus();
    if (prev.draft?.clipToken !== s.draft?.clipToken || prev.draft?.capturing !== s.draft?.capturing) this.armCapture(s.draft);
    if (prev.deleted !== s.deleted) this.showFrame();
    if (prev.commenting !== s.commenting || prev.draft !== s.draft || prev.selected !== s.selected || prev.threads !== s.threads || prev.file !== s.file || prev.busy !== s.busy) this.host?.uiChanged();
  }

  /** The pick's composer closed (posted, cancelled or dismissed): comment mode
   * comes back on. One replaced by another composer, or closed after the
   * viewer pressed Comment or the artifact was deleted, does not. */
  private draftChanged(draft: Draft | null): void {
    const pick = this.resumeAfter;
    if (pick === null || draft?.pickId === pick) return;
    this.resumeAfter = null;
    if (!draft && !this.s.deleted) this.set({ commenting: true });
  }

  /** A screenshot still being taken that never arrives: the composer says so. */
  private armCapture(draft: Draft | null): void {
    clearTimeout(this.captureTimer);
    const token = draft?.capturing ? draft.clipToken : undefined;
    if (!token) return;
    this.captureTimer = setTimeout(() => this.set(s => ({ draft: withClip(s.draft, token, null, CAPTURE_LATE) })), captureWait.ms);
  }

  private changeThreads(f: ThreadChange): void {
    this.sync.change(f);
  }

  private noticeFor(prefix: string): (text: string | null) => void {
    return scopedNotice(this.setNotice, prefix);
  }

  readonly setNotice: SetNotice = u => this.set(s => ({ notice: typeof u === "function" ? u(s.notice) : u }));

  private showHint(text: string): void {
    this.set({ hint: text });
    clearTimeout(this.hintTimer);
    this.hintTimer = setTimeout(() => this.set({ hint: null }), HINT_MS);
  }

  private whileBusy<T>(p: Promise<T>): Promise<T> {
    this.set(s => ({ busy: s.busy + 1 }));
    return p.finally(() => this.set(s => ({ busy: s.busy - 1 })));
  }

  // ---- the frame ----

  private frameWin(): Window | null {
    return this.frame?.el?.contentWindow ?? null;
  }

  private send(m: ShellToBridge): void {
    sendToFrame(this.frameWin(), this.s.origin ?? null, m);
  }

  /** Runs when the artifact or the frame mode becomes known, before the frame
   * for them exists: the gate is reset and the capability host made first, so
   * a hello or a request that arrives as soon as the frame is inserted is judged
   * against the shown artifact and version. */
  private viewChanged(): void {
    const s = this.s;
    if (!s.data || s.origin === undefined) return;
    const key = `${this.id}/${this.shown()}/${s.origin}`;
    if (key === this.viewKey) return;
    this.viewKey = key;
    this.gate.reset();
    this.host?.dispose();
    const data = s.data;
    this.host = new CapabilityHost(getToken().then(token => ({
      aid: this.id,
      version: this.shown(),
      pinned: this.pinnedVersion !== null,
      token,
      viewer: currentViewer,
      declared: (data.artifact.capabilities ?? {}) as Declared,
      prompt: this.prompt,
      post: m => { if (this.gate.open) this.send(m); },
      reload: () => nav.assign(this.here(null)),
      ownPublish: this.ownPublish,
      page: () => this.s.file,
      comments: this.commentsUi,
      files: data.versions.find(v => v.n === this.shown())?.files,
    })));
    const host = this.host;
    queueMicrotask(() => { if (this.host === host) host.uiChanged(); });
    this.showFrame();
  }

  private showFrame(): void {
    const s = this.s;
    if (!this.frame || !s.data || s.origin === undefined) return;
    if (s.deleted || this.missing()) { this.frame.remove(); return; }
    this.frame.show(pageSrc(this.id, this.shown(), s.origin, this.startFile) + this.startHash, s.origin === null, `${this.shown()}-${s.origin ? "o" : "s"}`);
  }

  /** The frame loaded a document; one that never greeted loses its page and pins. */
  frameLoaded(): void {
    if (this.gate.load()) this.set({ file: null, resolved: {} });
  }

  private resolveAll(): void {
    if (this.customLive) return;
    const file = this.s.file;
    this.send({ type: "clax:resolve-anchors", requestId: `r${Date.now()}`, anchors: this.s.threads.filter(t => t.anchor.file === file).map(t => ({ id: this.handles.handle(t.id), anchor: t.anchor, sameVersion: t.version_n === this.shown() })) });
  }

  /** Tells the frame which thread's drawn area to outline dashed. */
  private sendFocus(): void {
    const tid = this.s.hovered ?? this.s.selected;
    this.send({ type: "clax:focus", id: tid === null ? null : this.handles.known(tid) });
  }

  private clearPending(): void {
    if (this.pendingScroll) clearTimeout(this.pendingScroll.timer);
    this.pendingScroll = null;
  }

  /** Sends the frame to `target` at `hash`; `replace` keeps the frame's history
   * entry. The outgoing document is done: the gate closes and its page and
   * pins are forgotten until the next page greets. */
  private navigateFrame(target: string, replace: boolean, hash = ""): void {
    const el = this.frame?.el;
    if (!el) return;
    this.frameHash = hash;
    this.gate.close();
    this.set({ file: null, resolved: {} });
    const url = pageSrc(this.id, this.shown(), this.s.origin ?? null, target) + hash;
    if (replace && el.contentWindow) {
      try { el.contentWindow.location.replace(url); return; } catch { /* fall back to src */ }
    }
    el.src = url;
  }

  /** Moves the frame, showing `page`, to `hash` in place (the gate stays open). */
  private moveFragment(page: string, hash: string): void {
    this.frameHash = hash;
    try { this.frameWin()?.location.replace(pageSrc(this.id, this.shown(), this.s.origin ?? null, page) + (hash || "#")); } catch { /* the frame is gone */ }
  }

  /** Shows `target` as one history entry: the shell URL is pushed and the frame
   * moves without an entry of its own; when the push is refused the frame
   * still moves, with its own entry. */
  private openPage(target: string, hash = ""): void {
    const pushed = setUrl(shellPath(this.id, this.pinnedVersion, target, this.shown()) + hash, true);
    this.navigateFrame(target, pushed, hash);
  }

  // ---- lifecycle ----

  start(): void {
    this.live = true;
    getArtifact(this.id).then(d => {
      if (this.disposed) return;
      this.latestKnown = Math.max(this.latestKnown, d.artifact.current_version);
      this.set(s => ({ data: d, newer: s.newer !== null && s.newer <= d.artifact.current_version ? null : s.newer }));
      this.viewChanged();
    }, e => this.set({ error: e instanceof ApiError && e.status === 404 ? "Artifact not found" : String(e) }));
    const o = artifactOrigin(this.id);
    const decided = (origin: string | null) => { if (this.disposed) return; this.set({ origin }); this.viewChanged(); };
    if (!o) decided(null);
    else void probeOrigin(o).then(ok => decided(ok ? o : null));
    this.offs.push(onShieldPress(() => this.showHint(this.s.commenting ? MOVE_TO_PICK : MOVE_TO_CLICK)));
    this.listen();
    this.loadThreads();
    this.openStream();
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.live = false;
    for (const off of this.offs.splice(0)) off();
    this.stream?.();
    this.stream = null;
    this.host?.dispose();
    this.host = null;
    this.clearPending();
    clearTimeout(this.hintTimer);
    clearTimeout(this.captureTimer);
    if (this.hashFrame) cancelAnimationFrame(this.hashFrame);
  }

  private listen(): void {
    const onMessage = (e: MessageEvent) => this.onMessage(e);
    // Whether the pointer is over the content frame.
    const onOver = (e: MouseEvent) => { this.overFrame = e.target === this.frame?.el; };
    // While comment mode is on and the pointer is over the frame, Option and,
    // with it held, Up and Down go to the page (not from a text field), and so
    // does Escape; they are the page's keys, not input to the shell.
    const forwards = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.localName === "input" || t.localName === "textarea" || t.isContentEditable);
      if (!this.s.commenting || !this.overFrame || typing) return false;
      return e.key === "Alt" || e.key === "Escape" || ((e.key === "ArrowUp" || e.key === "ArrowDown") && e.altKey);
    };
    const onKey = (e: KeyboardEvent) => {
      if (forwards(e)) {
        this.send({ type: "clax:key", key: e.key as "Alt" | "ArrowUp" | "ArrowDown" | "Escape", down: e.type === "keydown" });
        if (e.key === "ArrowUp" || e.key === "ArrowDown") e.preventDefault();
      } else if (e.key === "Escape" && e.type === "keydown") {
        this.set({ commenting: false });
      }
    };
    // Back and forward move the shell URL between pages; the frame follows.
    const onPop = () => {
      this.clearPending();
      const r = parseShellPath(location.pathname);
      if (r.kind !== "artifact" || r.id !== this.id) return;
      if (r.file !== this.s.file) this.navigateFrame(r.file, true, location.hash);
      else if (location.hash !== this.frameHash) this.moveFragment(r.file, location.hash);
    };
    const unforward = setForwardedKeys(forwards);
    addEventListener("message", onMessage);
    addEventListener("mouseover", onOver);
    addEventListener("keydown", onKey);
    addEventListener("keyup", onKey);
    addEventListener("popstate", onPop);
    const mq = typeof matchMedia === "function" ? matchMedia("(max-width: 480px)") : null;
    const onNarrow = () => this.set({ narrow: !!mq?.matches });
    mq?.addEventListener?.("change", onNarrow);
    this.offs.push(() => {
      unforward();
      removeEventListener("message", onMessage);
      removeEventListener("mouseover", onOver);
      removeEventListener("keydown", onKey);
      removeEventListener("keyup", onKey);
      removeEventListener("popstate", onPop);
      mq?.removeEventListener?.("change", onNarrow);
    });
  }

  private onMessage(e: MessageEvent): void {
    const m = acceptFromFrame(e, this.frameWin(), this.s.origin ?? null);
    if (!m || this.disposed) return;
    switch (m.type) {
      case "clax:hello": {
        // A stale or foreign document, or one naming a page this version does
        // not hold, gets no welcome, no anchors and no pins.
        const greeted = typeof m.file === "string" && m.file ? m.file : INDEX_FILE;
        this.gate.hello(helloMatches(m, this.id, this.shown()) && this.holds(greeted));
        this.startedPicks.clear();
        this.handles.forget();
        if (!this.gate.open) { this.set({ resolved: {}, file: null }); break; }
        this.host?.reset();
        this.set({ resolved: {}, file: greeted });
        // The address bar follows the frame to another page.
        const r = parseShellPath(location.pathname);
        if (r.kind !== "artifact" || r.file !== greeted) setUrl(this.here(this.pinnedVersion) + location.hash);
        this.send({ type: "clax:welcome", mode: this.s.commenting ? "comment" : "view" });
        this.resolveAll();
        this.sendFocus();
        const p = this.pendingScroll;
        if (p) {
          // The jump's page greeted: scroll there. Another page greeted: the viewer moved on.
          this.clearPending();
          if (p.thread.anchor.file === greeted) this.send({ type: "clax:scroll-to", anchor: p.thread.anchor, sameVersion: p.thread.version_n === this.shown() });
        }
        break;
      }
      case "clax:pick-start":
        // The viewer's pick itself: taken only in comment mode and while the
        // viewer's latest input went to the frame (frameGesture); a refused
        // start shows the hint. The bridge never has two picks in flight, so a
        // start arriving while another is pending means one was forged: both
        // are refused. (The full rationale is in caps/gesture.ts.)
        if (this.gate.open && this.s.commenting && typeof m.pickId === "string" && m.pickId.length <= 64) {
          if (!frameGesture()) { this.showHint(MOVE_TO_PICK); break; }
          const now = Date.now();
          for (const [pid, at] of this.startedPicks) if (now - at > PICK_WAIT_MS) this.startedPicks.delete(pid);
          if (this.startedPicks.size) this.startedPicks.clear();
          else this.startedPicks.set(m.pickId, now);
        }
        break;
      case "clax:pick": {
        // A pick counts only after its gesture-checked start, in comment mode;
        // a clip the daemon would not keep is dropped here, with the reason.
        if (!takePick(this.startedPicks, m.pickId, this.s.commenting)) break;
        this.set({ commenting: false });
        this.resumeAfter = m.pickId;
        const png = m.clipPng instanceof ArrayBuffer && m.clipPng.byteLength > 0 ? m.clipPng : null;
        const tooBig = !!png && png.byteLength > MAX_CLIP_BYTES;
        this.set({ draft: { pickId: m.pickId, anchor: m.anchor, version: m.version, clip: png && !tooBig ? new Blob([png], { type: "image/png" }) : null, clipError: tooBig ? "the screenshot was too large to keep" : m.clipError } });
        break;
      }
      case "clax:anchors": {
        if (this.customLive) break;
        this.set(s => {
          const next: Record<string, AnchorResult> = m.requestId ? {} : { ...s.resolved };
          for (const r of m.results) {
            const tid = this.handles.thread(r.id);
            if (tid) next[tid] = { ...r, id: tid };
          }
          return { resolved: next };
        });
        break;
      }
      case "clax:cancel": this.set({ commenting: false }); break;
      case "clax:hash":
        // The page's fragment moved: the address bar follows in place, once
        // per animation frame with the latest fragment.
        if (!this.gate.open || !validHash(m.hash)) break;
        this.frameHash = m.hash;
        if (!this.hashFrame) {
          this.hashFrame = requestAnimationFrame(() => {
            this.hashFrame = 0;
            if (location.hash !== this.frameHash) setUrl(location.pathname + location.search + this.frameHash);
          });
        }
        break;
      case "clax:navigate": {
        // A link the page handed over: an HTML page of this version is one
        // history entry; another file of it loads in the frame; anything else is ignored.
        if (!this.gate.open || typeof m.file !== "string" || !this.holds(m.file)) break;
        const hash = validHash(m.hash) ? m.hash : "";
        this.clearPending();
        if (this.isPage(m.file)) this.openPage(m.file, hash);
        else this.navigateFrame(m.file, false, hash);
        break;
      }
      case "clax:hover": break;
      case "clax:use": case "clax:call": if (this.gate.open) void this.host?.handle(m); break;
    }
  }

  private loadThreads(): void {
    const done = this.sync.begin();
    void report(listThreads(this.id), LOAD_FAILED, this.noticeFor(LOAD_FAILED)).then(done);
  }

  private openStream(): void {
    const open = async (resync: boolean) => {
      // The owner shell passes its token (null on a LAN view).
      const token = await getToken();
      if (!this.live) return;
      this.stream?.();
      this.stream = subscribe(this.id, e => this.onEvent(e), token);
      if (resync) this.onEvent({ type: "resync", dropped: 0 });
    };
    // The daemon reads the viewer cookie when the stream opens, so it opens
    // once the lookup has set the cookie, and reopens when the viewer changes.
    const first = () => { if (this.live && !this.stream) void open(false); };
    void getViewer().then(first, first);
    this.offs.push(onViewer(() => { if (this.live && this.stream) void open(true); }));
  }

  private onEvent(e: ArtifactEvent): void {
    if (this.disposed) return;
    this.host?.onEvent(e);
    if (e.type === "version" && e.by_page && e.n > this.shown()) {
      // The page republished itself: every unpinned view follows at once; a
      // pinned view gets the banner; while this view's own publish is in
      // flight, another view's publish waits for it to settle.
      if (this.ownPublish.active > 0) { this.deferredPublish = Math.max(this.deferredPublish ?? 0, e.n); return; }
      if (this.pinnedVersion === null) {
        this.latestKnown = Math.max(this.latestKnown, e.n);
        nav.assign(this.here(null));
        return;
      }
    }
    if (e.type === "version" && e.n > this.latestKnown) { this.latestKnown = e.n; this.set({ newer: e.n }); }
    if (e.type === "artifact_deleted") this.set({ deleted: true });
    if (e.type === "thread") this.changeThreads(ts => upsert(ts, e.thread));
    if (e.type === "thread_deleted") { this.changeThreads(ts => ts.filter(t => t.id !== e.thread_id)); this.set(s => ({ selected: s.selected === e.thread_id ? null : s.selected })); }
    if (e.type === "feedback_state") this.changeThreads(ts => ts.map(t => t.id === e.thread_id ? { ...t, feedback_state: { thread_id: e.thread_id, state: e.state, tier: e.tier, since: e.since, resends: e.resends, exhausted: e.exhausted } } : t));
    // A (re)connect may follow a daemon restart that dropped events: reload like a resync.
    if (e.type === "resync" || e.type === "ready") {
      this.loadThreads();
      getArtifact(this.id).then(d => {
        const n = d.artifact.current_version;
        if (n > this.latestKnown) { this.latestKnown = n; this.set({ newer: n }); }
      }, err => { if (err instanceof ApiError && err.status === 404) this.set({ deleted: true }); });
    }
  }

  private saveThread(p: Promise<Thread>, prefix: string): void {
    void report(this.whileBusy(p), prefix, this.noticeFor(prefix)).then(t => { if (t) this.changeThreads(ts => upsert(ts, t)); });
  }

  // ---- intents ----

  toggleComment(): void { this.resumeAfter = null; this.set(s => ({ commenting: !s.commenting })); }
  togglePanel(): void { this.set(s => ({ panel: !s.panel })); }
  chooseVersion(n: number): void { nav.assign(this.here(n === this.latest() ? null : n)); }
  copyLink(): void { void navigator.clipboard?.writeText(location.origin + this.here(this.pinnedVersion) + location.hash).catch(() => {}); }
  reloadLatest(): void { nav.assign(this.here(null)); }
  dismissNotice(): void { this.set({ notice: null }); }
  setMe(v: Viewer): void { this.set({ me: v }); }
  hover(t: Thread | null): void { this.set({ hovered: t?.id ?? null }); }
  /** A pin's click: the sidebar opens on its thread. */
  openPin(t: Thread): void { this.set({ panel: true }); this.selectThread(t); }

  /** Selects `t` and brings it into view: in this page, or on the page it is on
   * (given up with a notice when that page does not greet in `pageWait.ms`). */
  selectThread(t: Thread): void {
    this.set({ selected: t.id });
    this.clearPending();
    if (t.anchor.file === this.s.file) {
      // A custom-anchors page brings its own threads into view.
      if (!this.host?.reveal(t.id)) this.send({ type: "clax:scroll-to", anchor: t.anchor, sameVersion: t.version_n === this.shown() });
      return;
    }
    // A thread on a page this version does not hold is detached: nothing to open.
    if (!this.holds(t.anchor.file)) return;
    const timer = setTimeout(() => {
      if (this.pendingScroll?.thread !== t) return;
      this.pendingScroll = null;
      this.noticeFor(OPEN_FAILED)(`${OPEN_FAILED} ${t.anchor.file}: the page did not load`);
    }, pageWait.ms);
    this.pendingScroll = { thread: t, timer };
    this.openPage(t.anchor.file);
  }

  sendThread(t: Thread): void { this.saveThread(sendToAgent(this.id, t.id), SEND_FAILED); }
  resolveThread(t: Thread): void { this.saveThread(resolveThread(this.id, t.id), RESOLVE_FAILED); }
  reply(t: Thread, body: string): void { this.saveThread(addComment(this.id, t.id, body), POST_FAILED); }
  composerInput(text: string): void { this.composerText = text; }
  cancelDraft(): void { this.set({ draft: null }); }

  /** Posts the open composer's comment; a failure shows in the notice and is rethrown so the composer stays. */
  async submitDraft(body: string): Promise<void> {
    const draft = this.s.draft;
    if (!draft) return;
    try {
      const { thread, clip_error: clipError } = await this.whileBusy(createThread(this.id, { anchor: draft.anchor, body, version: draft.version, clip: draft.clip }));
      this.noticeFor(POST_FAILED)(null);
      this.noticeFor(CLIP_DROPPED)(clipError ? `${CLIP_DROPPED}: ${clipError}` : null);
      this.changeThreads(ts => upsert(ts, thread));
      this.set({ selected: thread.id, draft: null, panel: true });
    } catch (e) {
      void report(Promise.reject(e), POST_FAILED, this.noticeFor(POST_FAILED));
      throw e;
    }
  }
}
```

A note on `react()` versus the Preact effects it replaces: Preact ran them after each render, so several `setState` calls in one handler produced one effect pass. `react()` keeps that batching per microtask, and the second test in Step 1 pins it for `host.uiChanged`. The work the Preact code did "while rendering" (the gate reset and the host) runs synchronously in `viewChanged()`.

- [ ] **Step 4: Run the controller tests**

Run: `cd web && npx vitest run shell/src/view/artifact-controller.test.ts`
Expected: PASS.

- [ ] **Step 5: Write the island registry and the Preact islands**

`web/shell/src/islands/index.ts`:

```ts
// Which framework mounts each island of the artifact view. Each island is its
// own mount root over one ArtifactController, so the page may mix frameworks
// while the shell is ported.
import type { ArtifactController } from "../view/artifact-controller";
import { preactIslands } from "./preact";

export type MountIsland = (target: HTMLElement, ctl: ArtifactController) => () => void;
export type Islands = { topbar: MountIsland; stage: MountIsland; sidebar: MountIsland };

export const ISLANDS: Islands = { topbar: preactIslands.topbar, stage: preactIslands.stage, sidebar: preactIslands.sidebar };
```

`web/shell/src/islands/preact.tsx`:

```tsx
import { type ComponentType, h, render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { INDEX_FILE } from "../../../bridge/src/protocol";
import { registerShield } from "../caps/gesture";
import { Composer, Pins } from "../comments";
import { PromptDialog } from "../prompt";
import { shellPath } from "../route";
import { Sidebar } from "../sidebar";
import { ViewerName } from "../viewer-name";
import type { ArtifactController, ViewState } from "../view/artifact-controller";
import type { Islands } from "./index";

function useView(ctl: ArtifactController): ViewState {
  const [s, setS] = useState(ctl.state.get());
  useEffect(() => ctl.state.subscribe(setS), [ctl]);
  return s;
}

function Topbar({ ctl }: { ctl: ArtifactController }) {
  const s = useView(ctl);
  if (s.error || !s.data) return null;
  const shown = ctl.shown(s);
  const latest = ctl.latest(s);
  return (
    <>
      <button aria-pressed={s.commenting} class={s.commenting ? "primary" : ""} disabled={s.deleted} onClick={() => ctl.toggleComment()}>Comment</button>
      <button aria-pressed={s.panel} onClick={() => ctl.togglePanel()}>Threads ({ctl.openCount(s)})</button>
      {!s.narrow && <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />}
      <select value={shown} disabled={s.deleted} onChange={e => ctl.chooseVersion(Number((e.target as HTMLSelectElement).value))}>
        {s.data.versions.map(v => <option value={v.n} key={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>)}
      </select>
      {s.deleted
        ? <span class="hide-sm muted">open raw</span>
        : <a class="hide-sm" href={ctl.rawHref(s)} target="_blank" rel="noopener">open raw</a>}
      {navigator.clipboard && <button disabled={s.deleted} onClick={() => ctl.copyLink()}>copy link</button>}
    </>
  );
}

function Stage({ ctl }: { ctl: ArtifactController }) {
  const s = useView(ctl);
  if (s.error) return <p class="empty">{s.error}</p>;
  if (!s.data || s.origin === undefined) return <p class="empty muted">Loading…</p>;
  const shown = ctl.shown(s);
  const latest = ctl.latest(s);
  const missing = ctl.missing(s);
  const draft = s.draft;
  return (
    <>
      {s.deleted
        ? <p class="empty">This artifact was deleted.</p>
        : missing && <p class="empty">v{shown} has no page {missing}. <a href={shellPath(ctl.id, ctl.pinnedVersion, INDEX_FILE)}>Open the index</a></p>}
      {!s.deleted && !missing && <div class="frame-shield" aria-hidden="true" ref={registerShield}><div /><div /><div /><div /></div>}
      {s.hint && <p class="gesture-hint" role="status">{s.hint}</p>}
      {!s.deleted && !missing && <Pins threads={s.threads} resolved={s.resolved} file={s.file} onSelect={t => ctl.openPin(t)} onHover={t => ctl.hover(t)} />}
      {draft && <Composer key={draft.pickId} draft={draft} onText={v => ctl.composerInput(v)} onCancel={() => ctl.cancelDraft()} onSubmit={body => ctl.submitDraft(body)} />}
      {s.newer && !s.deleted && (
        <div class="banner"><span>v{s.newer} published</span><button class="primary" onClick={() => ctl.reloadLatest()}>Reload</button></div>
      )}
      {shown < latest && !s.newer && !s.deleted && <div class="banner"><span class="muted">viewing v{shown}; latest is v{latest}</span><a href={ctl.here(null, s)}>latest</a></div>}
      {s.notice && (
        <div class="banner notice" role="alert"><span>{s.notice}</span><button onClick={() => ctl.dismissNotice()}>Dismiss</button></div>
      )}
      {s.ask && <PromptDialog ask={s.ask} />}
    </>
  );
}

function SidebarPanel({ ctl }: { ctl: ArtifactController }) {
  const s = useView(ctl);
  if (s.error || !s.data || !s.panel) return null;
  return <Sidebar threads={s.threads} resolved={s.resolved} selected={s.selected} file={s.file} holds={f => ctl.holds(f, s)}
    me={s.me} header={s.narrow ? <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} /> : undefined}
    onSelect={t => ctl.selectThread(t)}
    onHover={t => ctl.hover(t)}
    onSend={t => ctl.sendThread(t)}
    onResolve={t => ctl.resolveThread(t)}
    onReply={(t, body) => ctl.reply(t, body)} />;
}

const island = (C: ComponentType<{ ctl: ArtifactController }>) => (target: HTMLElement, ctl: ArtifactController) => {
  render(h(C, { ctl }), target);
  return () => render(null, target);
};

export const preactIslands: Islands = { topbar: island(Topbar), stage: island(Stage), sidebar: island(SidebarPanel) };
```

`islands/index.ts` imports `preact.tsx` at run time, while `preact.tsx` imports only a type from `index.ts`, so the two modules form no runtime cycle.

- [ ] **Step 6: Write the mount and delete the old view**

`web/shell/src/artifact.ts`:

```ts
// The artifact view: the page skeleton (created, or adopted from the daemon's
// HTML), one ArtifactController, the frame host, and the three islands.
import { ISLANDS, type Islands } from "./islands";
import { type ArtifactProps, ArtifactController } from "./view/artifact-controller";
import { FrameHost } from "./view/frame-host";
import { skeleton } from "./view/skeleton";

export { MOVE_TO_CLICK, MOVE_TO_PICK, pageWait } from "./view/artifact-controller";

export type ArtifactMount = { root: HTMLElement; update(props: ArtifactProps): void; unmount(): void };

export function mountArtifactView(root: HTMLElement, props: ArtifactProps, islands: Islands = ISLANDS): ArtifactMount {
  const start = (p: ArtifactProps) => {
    const sk = skeleton(root);
    const ctl = new ArtifactController(p);
    ctl.frame = new FrameHost(sk.stage, () => ctl.frameLoaded());
    const offState = ctl.state.subscribe(s => {
      sk.title.textContent = s.error || !s.data ? "Clax" : s.data.artifact.title;
      sk.viewer.classList.toggle("with-sidebar", s.panel && !!s.data && !s.error);
    });
    const offs = [islands.topbar(sk.topbarIsland, ctl), islands.stage(sk.stageIsland, ctl), islands.sidebar(sk.sidebarIsland, ctl)];
    ctl.start();
    return () => {
      for (const off of offs) off();
      offState();
      ctl.dispose();
      ctl.frame?.remove();
      sk.page.remove();
    };
  };
  let stop = start(props);
  return {
    root,
    update: next => { stop(); stop = start(next); },
    unmount: () => stop(),
  };
}
```

Run: `git rm web/shell/src/artifact.tsx web/shell/src/frame.tsx`

`web/shell/src/main.tsx`:

```tsx
import { render } from "preact";
import { mountArtifactView } from "./artifact";
import Gallery from "./gallery";
import { parseShellPath } from "./route";

const app = document.getElementById("app")!;
const r = parseShellPath(location.pathname);
if (r.kind === "artifact") mountArtifactView(app, { id: r.id, pinnedVersion: r.version, file: r.file });
else render(<Gallery />, app);
```

- [ ] **Step 7: Point the artifact tests at the mount**

In `web/shell/src/artifact.test.ts`, change only `mountView`: replace `const { default: ArtifactView } = await import("./artifact"); … return mount(ArtifactView, { id: ID, pinnedVersion: pinned, file });` with

```ts
  const { mountArtifactView } = await import("./artifact");
  gesture = await import("./caps/gesture");
  const root = document.createElement("div");
  document.body.appendChild(root);
  return mountArtifactView(root, { id: ID, pinnedVersion: pinned, file });
```

and remove the now-unused `mount` import from `./test/preact` if nothing else in the file uses it. The host-disposal test keeps calling `view.update({ id: ID, pinnedVersion: 1, file: undefined })` and `view.unmount()`. `ArtifactMount` has both.

- [ ] **Step 8: Run the unit tests**

Run: `cd web && npm test -- --reporter=dot`
Expected: PASS, with the same test count as `/tmp/clax-tests-after.txt` from Task 2 plus the controller's 3 tests and the Task 3 and 4 model tests. If an `ArtifactView` test fails, the controller differs from `artifact.tsx`. Compare the handler in question line by line against the deleted file (`git show HEAD:web/shell/src/artifact.tsx`). Do not edit the test.

- [ ] **Step 9: Confirm the rendered DOM is the same**

Run: `cd web && npm run build && npm run e2e; echo "exit=$?"`
Expected: every spec PASSES in both frame modes, `exit=0`. Then check that the e2e files are untouched:
Run: `git diff --exit-code $(cat .svelte-port-base) -- web/e2e; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 10: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`, including `time to usable` within the Task 1 budgets.

- [ ] **Step 11: Commit**

```bash
git add web/shell/src/view/artifact-controller.ts web/shell/src/view/artifact-controller.test.ts web/shell/src/artifact.ts web/shell/src/islands/index.ts web/shell/src/islands/preact.tsx web/shell/src/main.tsx web/shell/src/artifact.test.ts web/shell/src/artifact.tsx web/shell/src/frame.tsx
git commit --no-gpg-sign -m "Move the artifact view into ArtifactController and three islands

The controller holds every piece of state and behaviour artifact.tsx kept
in hooks and runs its effects once per microtask over snapshot diffs; the
frame is a FrameHost it owns, and the topbar, stage overlays and sidebar
are separate Preact mount roots over the controller's store."
```

---

### Task 6: Svelte toolchain beside Preact

**Files:**
- Modify: `web/package.json`, `web/package-lock.json` (by `npm install`), `web/vite.shell.config.ts`, `web/vitest.config.ts`
- Create: `web/svelte.config.js`, `web/shell/src/svelte-env.d.ts`, `web/shell/src/test/svelte.ts`, `web/shell/src/test/Probe.svelte`, `web/shell/src/test/probe.test.ts`

**Interfaces:**
- Produces, from `web/shell/src/test/svelte.ts`: `type Mounted<P>`, `mount<P extends Record<string, unknown>>(C: Component<P>, props: P, root?: HTMLElement): Mounted<P>`, `flush(fn?: () => void): void`. The signatures are the same as in `test/preact.ts`.
- Produces: `npm run typecheck` = `tsc --noEmit && svelte-check --tsconfig ./tsconfig.json --fail-on-warnings`; `npm run lint` covers `.svelte` files.

- [ ] **Step 1: Install**

Run: `cd web && npm install --save svelte@^5.57.1 && npm install --save-dev @sveltejs/vite-plugin-svelte@^6.2.4 svelte-check@^4.7.6 @testing-library/svelte@^5.4.2`
Expected: no peer-dependency error. `@sveltejs/vite-plugin-svelte@6.2.x` declares `vite: ^6.3.0 || ^7.0.0`, and the tree has Vite 6.4.3.

- [ ] **Step 2: Configure Svelte and Vite**

`web/svelte.config.js`:

```js
// Runes mode everywhere; TypeScript in <script lang="ts"> is handled by Svelte itself.
export default { compilerOptions: { runes: true } };
```

`web/vite.shell.config.ts`: add `import { svelte } from "@sveltejs/vite-plugin-svelte";` and set `plugins: [preact(), svelte()]`.

`web/vitest.config.ts`:

```ts
import { defineConfig } from "vitest/config";
import preact from "@preact/preset-vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
export default defineConfig({
  // svelteTesting() resolves Svelte's browser build under jsdom and unmounts after each test.
  plugins: [preact(), svelte(), svelteTesting()],
  test: { environment: "jsdom", include: ["bridge/test/**/*.test.ts", "shell/src/**/*.test.{ts,tsx}"] },
});
```

`web/shell/src/svelte-env.d.ts`:

```ts
/// <reference types="svelte" />
```

This brings in Svelte's ambient `declare module "*.svelte"`, so `tsc` accepts `.svelte` imports from `.ts` files. `svelte-check` checks the components themselves.

In `.svelte` files, a type-only import must say `import type` (or `import { type X }`), because Svelte strips types without a type checker. `svelte-check` reports any that do not.

- [ ] **Step 3: Write the Svelte test helper and its failing self-test**

`web/shell/src/test/Probe.svelte`:

```svelte
<script lang="ts">
  let { label, onGone }: { label: string; onGone?: () => void } = $props();
  let clicks = $state(0);
  $effect(() => () => onGone?.());
</script>

<button onclick={() => clicks++}>{label} {clicks}</button>
```

`web/shell/src/test/probe.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import Probe from "./Probe.svelte";
import { flush, mount } from "./svelte";

describe("the Svelte test helper", () => {
  it("mounts, applies a click and new props, and unmounts with the component's teardown", () => {
    const gone = vi.fn();
    const view = mount(Probe, { label: "n", onGone: gone });
    const button = view.root.querySelector("button")!;
    expect(button.textContent).toBe("n 0");
    flush(() => button.click());
    expect(button.textContent).toBe("n 1");
    view.update({ label: "m", onGone: gone });
    expect(view.root.querySelector("button")!.textContent).toBe("m 1");
    view.unmount();
    expect(view.root.querySelector("button")).toBeNull();
    expect(gone).toHaveBeenCalledTimes(1);
  });
});
```

Run: `cd web && npx vitest run shell/src/test/probe.test.ts`
Expected: FAIL with `Failed to resolve import "./svelte"`.

- [ ] **Step 4: Write the helper**

`web/shell/src/test/svelte.ts`:

```ts
// Mounts a Svelte component for a unit test, with the same exports as
// test/preact.ts. Updates are applied synchronously (flushSync), so a test
// reads the DOM right after an action, as it did under Preact's act().
import { render } from "@testing-library/svelte";
import { type Component, flushSync } from "svelte";

export type Mounted<P> = { root: HTMLElement; update(props: P): void; unmount(): void };

function attach(): HTMLElement {
  const root = document.createElement("div");
  document.body.appendChild(root);
  return root;
}

export function mount<P extends Record<string, unknown>>(C: Component<P>, props: P, root: HTMLElement = attach()): Mounted<P> {
  const r = render(C, { props, target: root });
  flushSync();
  return {
    root,
    update: next => { void r.rerender(next); flushSync(); },
    unmount: () => { r.unmount(); flushSync(); },
  };
}

export function flush(fn?: () => void): void {
  fn?.();
  flushSync();
}
```

Run: `cd web && npx vitest run shell/src/test/probe.test.ts`
Expected: PASS. If `update` does not show `m 1`, check that `rerender` came from `@testing-library/svelte` 5.x. Its Svelte 5 path updates props in place and keeps state. The `1` proves that no remount happened.

- [ ] **Step 5: Wire svelte-check into the typecheck**

In `web/package.json`, set `"typecheck": "tsc --noEmit && svelte-check --tsconfig ./tsconfig.json --fail-on-warnings"`.

Run: `cd web && npm run typecheck; echo "exit=$?"`
Expected: `svelte-check found 0 errors and 0 warnings`, `exit=0`.

Prove that it checks components. Put `let n: number = "x";` in `Probe.svelte`'s script and run `npm run typecheck; echo "exit=$?"`. Expect an error naming `Probe.svelte` and `exit=1`. Then restore it with `git checkout -- web/shell/src/test/Probe.svelte` (or remove the line again if the file is not yet committed).

- [ ] **Step 6: Prove oxlint covers `.svelte` files**

The `lint` script already lists `shell`. Add a `debugger;` line to `Probe.svelte`'s script, then:
Run: `cd web && npm run lint; echo "exit=$?"`
Expected: `eslint(no-debugger)` reported at `shell/src/test/Probe.svelte`, `exit=1`. Remove the line and rerun, expecting `exit=0`.

- [ ] **Step 7: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`. The bundle is unchanged, since no `.svelte` file is imported by the app yet.

- [ ] **Step 8: Commit**

```bash
git add web/package.json web/package-lock.json web/svelte.config.js web/vite.shell.config.ts web/vitest.config.ts web/shell/src/svelte-env.d.ts web/shell/src/test/svelte.ts web/shell/src/test/Probe.svelte web/shell/src/test/probe.test.ts
git commit --no-gpg-sign -m "Add Svelte 5 beside Preact: build plugin, tests, svelte-check

vite-plugin-svelte 6.2 (the Vite 6 line), runes mode, @testing-library/svelte
under jsdom, and svelte-check with --fail-on-warnings in the typecheck.
test/svelte.ts mounts components with the same helper API as test/preact.ts."
```

---

### Task 7: Port the sidebar island (ThreadCard, Sidebar, ViewerName)

**Files:**
- Create: `web/shell/src/ui/ticker.svelte.ts`, `ui/ThreadCard.svelte`, `ui/Sidebar.svelte`, `ui/ViewerName.svelte`, `ui/SidebarIsland.svelte`, `web/shell/src/islands/svelte.ts`
- Modify: `web/shell/src/islands/index.ts` (the `sidebar` entry), `web/shell/src/sidebar.test.ts`, `web/shell/src/viewer-name.test.ts` (imports only)

**Interfaces:**
- Consumes: `sidebarSections`, `needsTicking`, `authorLabel` (Task 3); `NameSaver`, `SetNotice` (Task 3); `ArtifactController` (Task 5); `mount`/`flush` from `test/svelte.ts` (Task 6).
- Produces: `ticker(active: () => boolean, fixed: () => Date | undefined): { readonly now: Date }`; `svelteIslands: Partial<Islands>` in `islands/svelte.ts`; Svelte components whose props match the Preact ones by name. The exception is `Sidebar`'s `header`, which becomes a Svelte `Snippet`.

- [ ] **Step 1: Point the two tests at Svelte components that do not exist yet**

In `web/shell/src/sidebar.test.ts`, replace the two imports `import { flush, mount } from "./test/preact";` and `import { Sidebar } from "./sidebar";` with `import { flush, mount } from "./test/svelte";` and `import Sidebar from "./ui/Sidebar.svelte";`. In `web/shell/src/viewer-name.test.ts`, do the same with `ViewerName` from `./ui/ViewerName.svelte`. Change nothing else. (If `flush` is not used in a file, import only `mount`.)

Run: `cd web && npx vitest run shell/src/sidebar.test.ts shell/src/viewer-name.test.ts`
Expected: FAIL with `Failed to resolve import "./ui/Sidebar.svelte"` and `"./ui/ViewerName.svelte"`.

- [ ] **Step 2: Write the components**

`web/shell/src/ui/ticker.svelte.ts`:

```ts
/** A clock for elapsed-time labels: it ticks each second while `active()`
 * holds, unless `fixed()` pins it (tests). Call during component setup. */
export function ticker(active: () => boolean, fixed: () => Date | undefined): { readonly now: Date } {
  let tick = $state(new Date());
  $effect(() => {
    if (fixed() !== undefined || !active()) return;
    tick = new Date();
    const timer = setInterval(() => { tick = new Date(); }, 1000);
    return () => clearInterval(timer);
  });
  return { get now() { return fixed() ?? tick; } };
}
```

`web/shell/src/ui/ThreadCard.svelte`:

```svelte
<script lang="ts">
  import { type Thread, type Viewer, anchorLabel, resolvedByLabel } from "../threads";
  import { waitingLabel } from "../waiting";
  import { authorLabel } from "../view/sidebar-model";

  type Props = {
    t: Thread; n?: number; now: Date; me?: Viewer | null; selected: string | null; file: string | null;
    onSelect(t: Thread): void; onSend(t: Thread): void; onResolve(t: Thread): void; onReply(t: Thread, body: string): void; onHover?(t: Thread | null): void;
  };
  let { t, n, now, me, selected, file, onSelect, onSend, onResolve, onReply, onHover }: Props = $props();
  let reply = $state("");
  const label = $derived(t.status === "open" && t.sent_to_agent ? waitingLabel(t.feedback_state, now) : null);
</script>

<!-- The card's header button is its keyboard path; a click anywhere else on the card is a pointer shortcut to the same action. -->
<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
<article class={`thread-card${selected === t.id ? " selected" : ""}`} data-thread={t.id} onclick={() => onSelect(t)}
  onmouseenter={() => onHover?.(t)} onmouseleave={() => onHover?.(null)}>
  <header>
    <button type="button" class="card-head" aria-pressed={selected === t.id} onclick={e => { e.stopPropagation(); onSelect(t); }}>
      {#if n !== undefined}<span class="thread-num">{n}</span>{/if}
      <span class="anchor-label">{anchorLabel(t.anchor)}</span>
      {#if t.anchor.file !== file}<span class="file-label muted small">on {t.anchor.file}</span>{/if}
      <span class="muted small">v{t.version_n}</span>
    </button>
  </header>
  {#if t.clip_url}<img class="thumb" src={t.clip_url} alt="Screenshot of the commented region" loading="lazy" />{/if}
  {#each t.comments as c (c.id)}
    <div class={`comment ${c.author_kind === "agent" ? "agent" : "from-viewer"}`}>
      <div class="author">{authorLabel(c)}{#if c.via_page}<span class="via-page muted small"> · via the page</span>{/if}</div>
      <div class="body">{c.body}</div>
    </div>
  {/each}
  {#if label}<p class="waiting">{label}</p>{/if}
  {#if t.status === "resolved" && t.resolved_by}<p class="resolved-by muted small">Resolved by {resolvedByLabel(t.resolved_by, me)}</p>{/if}
  {#if t.status === "open"}
    <!-- Only stops a click on these controls from also selecting the card. -->
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="actions" onclick={e => e.stopPropagation()}>
      {#if !t.sent_to_agent}<button class="primary" onclick={() => onSend(t)}>Send to agent</button>{/if}
      <button onclick={() => onResolve(t)}>Resolve</button>
    </div>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
  <form class="reply" onclick={e => e.stopPropagation()} onsubmit={e => { e.preventDefault(); if (reply.trim()) { onReply(t, reply); reply = ""; } }}>
    <input aria-label="Reply" placeholder="Reply…" bind:value={reply} />
    <button type="submit">Reply</button>
  </form>
</article>
```

If `svelte-check` reports another `a11y_*` code on one of these three elements, add that code to the `svelte-ignore` comment directly above the element. The Preact markup had the same pointer-only handlers. Do not change the markup instead: the e2e suite pins it.

`web/shell/src/ui/Sidebar.svelte`:

```svelte
<script lang="ts">
  import type { Snippet } from "svelte";
  import type { AnchorResult } from "../../../bridge/src/protocol";
  import type { Thread, Viewer } from "../threads";
  import { needsTicking, sidebarSections } from "../view/sidebar-model";
  import ThreadCard from "./ThreadCard.svelte";
  import { ticker } from "./ticker.svelte";

  type Props = {
    threads: Thread[];
    resolved: Record<string, AnchorResult>;
    /** Fixed clock for tests; without it the sidebar ticks each second while a label shows elapsed time. */
    now?: Date;
    selected: string | null;
    onSelect(t: Thread): void;
    onSend(t: Thread): void;
    onResolve(t: Thread): void;
    onReply(t: Thread, body: string): void;
    onHover?(t: Thread | null): void;
    me?: Viewer | null;
    /** Rendered above the sections (the "Your name" field on narrow screens). */
    header?: Snippet;
    file?: string | null;
    holds?: (file: string) => boolean;
  };
  let p: Props = $props();
  const clock = ticker(() => needsTicking(p.threads), () => p.now);
  const s = $derived(sidebarSections(p.threads, p.resolved, p.file, p.holds));
</script>

{#snippet section(cls: string, title: string, list: Thread[])}
  <section class={cls}>
    <h2>{title} <span class="muted">{list.length}</span></h2>
    {#if list.length === 0}
      <p class="muted small">None.</p>
    {:else}
      {#each list as t (t.id)}
        <ThreadCard {t} n={s.numbers.get(t.id)} now={clock.now} me={p.me} selected={p.selected} file={s.file}
          onSelect={p.onSelect} onSend={p.onSend} onResolve={p.onResolve} onReply={p.onReply} onHover={p.onHover} />
      {/each}
    {/if}
  </section>
{/snippet}

<aside class="sidebar" aria-label="Comment threads">
  {@render p.header?.()}
  {@render section("section-open", "Open", s.open)}
  {@render section("section-detached", "Detached", s.detached)}
  {@render section("section-resolved", "Resolved", s.resolved)}
</aside>
```

`web/shell/src/ui/ViewerName.svelte`:

```svelte
<script lang="ts">
  import { onMount } from "svelte";
  import type { Viewer } from "../threads";
  import { NameSaver, type SetNotice } from "../view/viewer-name-model";

  let { setNotice, onViewer }: { setNotice: SetNotice; onViewer?: (v: Viewer) => void } = $props();
  let name = $state("");
  const saver = new NameSaver(u => setNotice(u), v => onViewer?.(v));
  onMount(() => saver.load(n => { name = n; }));
</script>

<input class="viewer-name" aria-label="Your name" placeholder="Your name" maxlength="60" bind:value={name}
  oninput={() => saver.edit()} onblur={() => saver.save(name)}
  onkeydown={e => { if (e.key === "Enter") { e.preventDefault(); saver.save(name); } }} />
```

`web/shell/src/ui/SidebarIsland.svelte`:

```svelte
<script lang="ts">
  import { fromStore } from "svelte/store";
  import type { ArtifactController } from "../view/artifact-controller";
  import Sidebar from "./Sidebar.svelte";
  import ViewerName from "./ViewerName.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
</script>

{#snippet nameField()}
  <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />
{/snippet}

{#if !s.error && s.data && s.panel}
  <Sidebar threads={s.threads} resolved={s.resolved} selected={s.selected} file={s.file} holds={f => ctl.holds(f, s)} me={s.me}
    header={s.narrow ? nameField : undefined}
    onSelect={t => ctl.selectThread(t)} onHover={t => ctl.hover(t)} onSend={t => ctl.sendThread(t)}
    onResolve={t => ctl.resolveThread(t)} onReply={(t, body) => ctl.reply(t, body)} />
{/if}
```

If `svelte-check` reports `state_referenced_locally` for `ctl` in `fromStore(ctl.state)`, put `// svelte-ignore state_referenced_locally` on the line above. The comment above the props already states why it is safe. Do the same in every island in Tasks 8 and 9.

`web/shell/src/islands/svelte.ts`:

```ts
import { type Component, mount, unmount } from "svelte";
import SidebarIsland from "../ui/SidebarIsland.svelte";
import type { ArtifactController } from "../view/artifact-controller";
import type { Islands, MountIsland } from "./index";

const island = (C: Component<{ ctl: ArtifactController }>): MountIsland => (target, ctl) => {
  const c = mount(C, { target, props: { ctl } });
  return () => { void unmount(c); };
};

export const svelteIslands: Partial<Islands> = { sidebar: island(SidebarIsland) };
```

In `web/shell/src/islands/index.ts`, import `svelteIslands` from `./svelte` and set `sidebar: svelteIslands.sidebar!`.

- [ ] **Step 3: Run the ported tests**

Run: `cd web && npx vitest run shell/src/sidebar.test.ts shell/src/viewer-name.test.ts`
Expected: PASS, the same tests as before.

- [ ] **Step 4: Run the artifact view's tests, which now render a Svelte sidebar**

Run: `cd web && npx vitest run shell/src/artifact.test.ts`
Expected: PASS. "Your name" is found in the header (still a Preact island) when wide and in the Svelte sidebar when narrow.

- [ ] **Step 5: Check types, lint and the e2e oracle**

Run: `cd web && npm run lint && npm run typecheck && npm run build && npm run e2e; echo "exit=$?"; cd .. && git diff --exit-code $(cat .svelte-port-base) -- web/e2e; echo "e2e-unchanged=$?"`
Expected: `exit=0` and `e2e-unchanged=0`.

- [ ] **Step 6: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 7: Commit**

```bash
git add web/shell/src/ui/ticker.svelte.ts web/shell/src/ui/ThreadCard.svelte web/shell/src/ui/Sidebar.svelte web/shell/src/ui/ViewerName.svelte web/shell/src/ui/SidebarIsland.svelte web/shell/src/islands/svelte.ts web/shell/src/islands/index.ts web/shell/src/sidebar.test.ts web/shell/src/viewer-name.test.ts
git commit --no-gpg-sign -m "Render the comment sidebar with Svelte

ThreadCard, Sidebar and ViewerName are Svelte 5 components over the same
view models; the sidebar island mounts them beside the Preact topbar and
stage. The sidebar and name tests now mount the Svelte components."
```

---

### Task 8: Port the stage island (Pins, Composer, PromptDialog, banners)

**Files:**
- Create: `web/shell/src/ui/Pins.svelte`, `ui/Composer.svelte`, `ui/PromptDialog.svelte`, `ui/StageIsland.svelte`
- Modify: `web/shell/src/islands/svelte.ts`, `islands/index.ts` (the `stage` entry), `web/shell/src/comments.test.ts`, `web/shell/src/prompt.test.ts` (imports and the local mount helper only)

**Interfaces:**
- Consumes: `pinPlaces`, `composerQuote`, `Draft`, `ALLOW_DELAY_MS`, `Ask`, `ArtifactController`, `registerShield`.
- Produces: `svelteIslands.stage`.

- [ ] **Step 1: Point the two tests at the Svelte components**

In `web/shell/src/comments.test.ts`:
- replace `import { flush, mount } from "./test/preact";` with `import { flush, mount } from "./test/svelte";`;
- replace the Preact `Pins` and `Sidebar` imports with `import Pins from "./ui/Pins.svelte";` and `import Sidebar from "./ui/Sidebar.svelte";`;
- in "say a screenshot is being taken", replace `const { Composer } = await import("./comments");` with `const { default: Composer } = await import("./ui/Composer.svelte");`;
- in `mountIt`, replace `ComponentType<P>` (from `preact`) with `Component<P>` (`import type { Component } from "svelte";`), and replace `P extends object` with `P extends Record<string, unknown>`.

In `web/shell/src/prompt.test.ts`:
- replace `./test/preact` with `./test/svelte`;
- replace the `PromptDialog` import with `import PromptDialog from "./ui/PromptDialog.svelte";`.

Run: `cd web && npx vitest run shell/src/comments.test.ts shell/src/prompt.test.ts`
Expected: FAIL with `Failed to resolve import` for the missing `.svelte` files.

- [ ] **Step 2: Write the components**

`web/shell/src/ui/Pins.svelte`:

```svelte
<script lang="ts">
  import { type AnchorResult, INDEX_FILE } from "../../../bridge/src/protocol";
  import type { Thread } from "../threads";
  import { pinPlaces } from "../view/pins-model";

  type Props = { threads: Thread[]; resolved: Record<string, AnchorResult>; onSelect(t: Thread): void; onHover?(t: Thread | null): void; width?: number; file?: string | null };
  let { threads, resolved, onSelect, onHover, width, file = INDEX_FILE }: Props = $props();
  let measured = $state(0);
  // The stage's width, followed while no fixed `width` is given.
  const measure = (el: HTMLElement) => {
    if (width !== undefined) return;
    const read = () => { measured = el.clientWidth; };
    read();
    if (typeof ResizeObserver === "function") {
      const ro = new ResizeObserver(read);
      ro.observe(el);
      return () => ro.disconnect();
    }
    addEventListener("resize", read);
    return () => removeEventListener("resize", read);
  };
  const places = $derived(pinPlaces(threads, resolved, file, width ?? measured));
</script>

<div class="pins" {@attach measure}>
  {#each places as p (p.thread.id)}
    <button class="thread-pin" title={p.thread.comments[0]?.body ?? ""} aria-label={`Thread ${p.n}`} style:left={`${p.left}px`} style:top={`${p.top}px`}
      onclick={() => onSelect(p.thread)} onmouseenter={() => onHover?.(p.thread)} onmouseleave={() => onHover?.(null)}>{p.n}</button>
  {/each}
</div>
```

`web/shell/src/ui/Composer.svelte`:

```svelte
<script lang="ts">
  import { onDestroy } from "svelte";
  import { INDEX_FILE } from "../../../bridge/src/protocol";
  import { type Draft, composerQuote } from "../view/composer-model";

  type Props = { draft: Draft; onCancel(): void; onSubmit(body: string): Promise<void>; onText?(text: string): void };
  let { draft, onCancel, onSubmit, onText }: Props = $props();
  let body = $state("");
  let busy = $state(false);
  let clipUrl = $state<string | null>(null);
  $effect(() => {
    const clip = draft.clip;
    if (!clip) { clipUrl = null; return; }
    const u = URL.createObjectURL(clip);
    clipUrl = u;
    return () => URL.revokeObjectURL(u);
  });
  // The composer is keyed by its pick: closing it tells the owner the text is gone.
  onDestroy(() => onText?.(""));
  // The viewer types at once: focus moves from the page to the textarea.
  const focus = (el: HTMLTextAreaElement) => { el.focus(); };

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!body.trim() || busy) return;
    busy = true;
    // `onSubmit` reports a failure in the notice banner and rethrows; the draft stays for a retry.
    try { await onSubmit(body); } catch { busy = false; }
  }
</script>

<form class="composer" onsubmit={submit}>
  <p class="composer-quote">{composerQuote(draft)}</p>
  {#if draft.anchor.file !== INDEX_FILE}<p class="file-label muted small">on {draft.anchor.file}</p>{/if}
  {#if clipUrl}
    <img class="clip" src={clipUrl} alt="Screenshot of the selected region" />
  {:else if draft.capturing}
    <p class="muted small">Taking the screenshot…</p>
  {:else}
    <p class="muted small">No screenshot{draft.clipError ? `: ${draft.clipError}` : ""}</p>
  {/if}
  <textarea {@attach focus} rows="3" placeholder="Comment… (@agent sends it to the agent)" bind:value={body}
    oninput={e => onText?.(e.currentTarget.value)} onkeydown={e => { if (e.key === "Escape") onCancel(); }}></textarea>
  <div class="actions">
    <button type="button" onclick={onCancel}>Cancel</button>
    <button type="submit" class="primary" disabled={busy || !body.trim() || !!draft.capturing}>Post comment</button>
  </div>
</form>
```

`web/shell/src/ui/PromptDialog.svelte`:

```svelte
<script lang="ts">
  import { ALLOW_DELAY_MS, type Ask } from "../view/prompt-queue";

  // The one modal the shell shows for a page. It opens with focus on the
  // refusing button, and "Allow" stays disabled for ALLOW_DELAY_MS, so a
  // keystroke meant for the page cannot grant consent. Escape dismisses it.
  let { ask }: { ask: Ask } = $props();
  let armed = $state(false);
  $effect(() => {
    const a = ask;
    armed = false;
    const timer = setTimeout(() => { armed = true; }, ALLOW_DELAY_MS);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") a.answer("dismiss"); };
    addEventListener("keydown", onKey);
    return () => { clearTimeout(timer); removeEventListener("keydown", onKey); };
  });
  const focus = (el: HTMLButtonElement) => { el.focus(); };
</script>

<div class="prompt-backdrop">
  <div class="prompt" role="dialog" aria-modal="true" aria-labelledby="prompt-title" aria-describedby="prompt-body">
    <h2 id="prompt-title">{ask.prompt.title}</h2>
    <p id="prompt-body">{ask.prompt.body}</p>
    <div class="actions">
      <button type="button" {@attach focus} onclick={() => ask.answer("deny")}>{ask.prompt.deny}</button>
      <button type="button" class="primary" disabled={!armed} onclick={() => { if (armed) ask.answer("allow"); }}>{ask.prompt.allow}</button>
    </div>
  </div>
</div>
```

`web/shell/src/ui/StageIsland.svelte`:

```svelte
<script lang="ts">
  import { fromStore } from "svelte/store";
  import { INDEX_FILE } from "../../../bridge/src/protocol";
  import { registerShield } from "../caps/gesture";
  import { shellPath } from "../route";
  import type { ArtifactController } from "../view/artifact-controller";
  import Composer from "./Composer.svelte";
  import Pins from "./Pins.svelte";
  import PromptDialog from "./PromptDialog.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  const shown = $derived(ctl.shown(s));
  const latest = $derived(ctl.latest(s));
  const missing = $derived(ctl.missing(s));
  const shield = (el: HTMLElement) => { registerShield(el); return () => registerShield(null); };
</script>

{#if s.error}
  <p class="empty">{s.error}</p>
{:else if !s.data || s.origin === undefined}
  <p class="empty muted">Loading…</p>
{:else}
  {#if s.deleted}
    <p class="empty">This artifact was deleted.</p>
  {:else if missing}
    <p class="empty">v{shown} has no page {missing}. <a href={shellPath(ctl.id, ctl.pinnedVersion, INDEX_FILE)}>Open the index</a></p>
  {/if}
  {#if !s.deleted && !missing}
    <div class="frame-shield" aria-hidden="true" {@attach shield}><div></div><div></div><div></div><div></div></div>
  {/if}
  {#if s.hint}<p class="gesture-hint" role="status">{s.hint}</p>{/if}
  {#if !s.deleted && !missing}
    <Pins threads={s.threads} resolved={s.resolved} file={s.file} onSelect={t => ctl.openPin(t)} onHover={t => ctl.hover(t)} />
  {/if}
  {#if s.draft}
    {#key s.draft.pickId}
      <Composer draft={s.draft} onText={v => ctl.composerInput(v)} onCancel={() => ctl.cancelDraft()} onSubmit={body => ctl.submitDraft(body)} />
    {/key}
  {/if}
  {#if s.newer && !s.deleted}
    <div class="banner"><span>v{s.newer} published</span><button class="primary" onclick={() => ctl.reloadLatest()}>Reload</button></div>
  {/if}
  {#if shown < latest && !s.newer && !s.deleted}
    <div class="banner"><span class="muted">viewing v{shown}; latest is v{latest}</span><a href={ctl.here(null, s)}>latest</a></div>
  {/if}
  {#if s.notice}
    <div class="banner notice" role="alert"><span>{s.notice}</span><button onclick={() => ctl.dismissNotice()}>Dismiss</button></div>
  {/if}
  {#if s.ask}
    {#key s.ask}<PromptDialog ask={s.ask} />{/key}
  {/if}
{/if}
```

In `islands/svelte.ts`, add `import StageIsland from "../ui/StageIsland.svelte";` and `stage: island(StageIsland)`. In `islands/index.ts`, set `stage: svelteIslands.stage!`.

- [ ] **Step 3: Run the ported tests**

Run: `cd web && npx vitest run shell/src/comments.test.ts shell/src/prompt.test.ts`
Expected: PASS. The PromptDialog timing test depends on `flush` running `flushSync()` after `vi.advanceTimersByTime`. Under fake timers the `$effect`'s `setTimeout` is faked, so `ALLOW_DELAY_MS - 1` keeps "Allow" disabled and one more millisecond enables it.

- [ ] **Step 4: Run the artifact view's tests, which now render a Svelte stage**

Run: `cd web && npx vitest run shell/src/artifact.test.ts`
Expected: PASS. These pin the composer's keying per pick, Escape in the textarea (a delegated `keydown` that bubbles from the textarea to the island root), Post disabled while a screenshot is taken, and the hint.

- [ ] **Step 5: Check types, lint and the e2e oracle**

Run: `cd web && npm run lint && npm run typecheck && npm run build && npm run e2e; echo "exit=$?"; cd .. && git diff --exit-code $(cat .svelte-port-base) -- web/e2e; echo "e2e-unchanged=$?"`
Expected: `exit=0` and `e2e-unchanged=0`. The gesture and area specs matter most here, because the shield is now registered through an attachment.

- [ ] **Step 6: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 7: Commit**

```bash
git add web/shell/src/ui/Pins.svelte web/shell/src/ui/Composer.svelte web/shell/src/ui/PromptDialog.svelte web/shell/src/ui/StageIsland.svelte web/shell/src/islands/svelte.ts web/shell/src/islands/index.ts web/shell/src/comments.test.ts web/shell/src/prompt.test.ts
git commit --no-gpg-sign -m "Render the stage overlays with Svelte

Pins, the composer, the consent dialog, the banners and the gesture
shield are the stage island's Svelte components; the shield registers
through an attachment that unregisters it on removal."
```

---

### Task 9: Port the topbar island and the gallery

**Files:**
- Create: `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/ui/Gallery.svelte`, `web/shell/src/main.ts`
- Delete: `web/shell/src/main.tsx`
- Modify: `web/shell/src/islands/svelte.ts`, `islands/index.ts` (the `topbar` entry), `web/shell/index.html` (script `src`), `web/shell/src/gallery.test.ts` (imports only)

**Interfaces:**
- Consumes: `filterArtifacts`, `publisherText` (Task 3); `ViewerName.svelte` (Task 7).
- Produces: `svelteIslands.topbar`; `Gallery.svelte` (no props).

- [ ] **Step 1: Point the gallery test at the Svelte gallery**

In `web/shell/src/gallery.test.ts`:
- replace `./test/preact` with `./test/svelte`;
- replace `const { default: Gallery } = await import("./gallery");` with `const { default: Gallery } = await import("./ui/Gallery.svelte");`.

Run: `cd web && npx vitest run shell/src/gallery.test.ts`
Expected: FAIL with `Failed to resolve import "./ui/Gallery.svelte"`.

- [ ] **Step 2: Write the components and the entry**

`web/shell/src/ui/TopbarIsland.svelte`:

```svelte
<script lang="ts">
  import { fromStore } from "svelte/store";
  import type { ArtifactController } from "../view/artifact-controller";
  import ViewerName from "./ViewerName.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  const shown = $derived(ctl.shown(s));
  const latest = $derived(ctl.latest(s));
  const canCopy = typeof navigator !== "undefined" && !!navigator.clipboard;
</script>

{#if !s.error && s.data}
  <button aria-pressed={s.commenting} class={s.commenting ? "primary" : ""} disabled={s.deleted} onclick={() => ctl.toggleComment()}>Comment</button>
  <button aria-pressed={s.panel} onclick={() => ctl.togglePanel()}>Threads ({ctl.openCount(s)})</button>
  {#if !s.narrow}<ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />{/if}
  <select value={shown} disabled={s.deleted} onchange={e => ctl.chooseVersion(Number(e.currentTarget.value))}>
    {#each s.data.versions as v (v.n)}
      <option value={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>
    {/each}
  </select>
  {#if s.deleted}
    <span class="hide-sm muted">open raw</span>
  {:else}
    <a class="hide-sm" href={ctl.rawHref(s)} target="_blank" rel="noopener">open raw</a>
  {/if}
  {#if canCopy}<button disabled={s.deleted} onclick={() => ctl.copyLink()}>copy link</button>{/if}
{/if}
```

`web/shell/src/ui/Gallery.svelte`:

```svelte
<script lang="ts">
  import { onMount } from "svelte";
  import { type Artifact, deleteArtifact, getToken, listArtifacts, patchArtifact } from "../api";
  import { relativeTime } from "../format";
  import { filterArtifacts, publisherText } from "../view/gallery-model";

  let artifacts = $state<Artifact[] | null>(null);
  let error = $state<string | null>(null);
  let token = $state<string | null>(null);
  let query = $state("");
  const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const refresh = () => listArtifacts().then(a => { error = null; artifacts = a; }, e => { error = describe(e); });
  const act = (op: () => Promise<unknown>) => op().then(refresh, e => { error = describe(e); });
  const shown = $derived(artifacts && filterArtifacts(artifacts, query));
  onMount(() => { void refresh(); void getToken().then(t => { token = t; }); });
</script>

<header class="topbar">
  <h1>Clax</h1><span class="muted hide-sm">local artifacts</span>
  <input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" bind:value={query} />
</header>
<main class="wrap">
  {#if error}<p class="empty">Could not load artifacts: {error}</p>{/if}
  {#if artifacts && artifacts.length === 0}
    <p class="empty">No artifacts yet. Publish one with <code>clax publish index.html</code>.</p>
  {/if}
  {#if artifacts && artifacts.length > 0 && shown && shown.length === 0}
    <p class="empty">No artifacts match your search.</p>
  {/if}
  {#if shown && shown.length > 0}
    <div class="grid">
      {#each shown as a (a.id)}
        {@const by = publisherText(a)}
        <div class="card-wrap">
          <a class="card" href={`/a/${a.id}`}>
            {#if a.pinned}<span class="pin" title="Pinned">★</span>{/if}
            <h2>{a.title}</h2>
            {#if a.description}<p>{a.description}</p>{/if}
            <div class="meta">
              <span>v{a.current_version}</span>
              <span>{relativeTime(a.updated_at)}</span>
              {#if by}
                <span class="publisher">{#if a.owner_live}<span class="live-dot" role="img" aria-label="session is live" title="Session is live"></span>{/if}{by}</span>
              {:else}
                <span>published from the command line</span>
              {/if}
            </div>
          </a>
          {#if token}
            {@const tk = token}
            <div class="card-tools">
              <button type="button" title={a.pinned ? "Unpin" : "Pin"} aria-label={a.pinned ? `Unpin ${a.title}` : `Pin ${a.title}`}
                onclick={() => act(() => patchArtifact(a.id, { pinned: !a.pinned }, tk))}>{a.pinned ? "★" : "☆"}</button>
              <button type="button" title="Delete"
                onclick={() => { if (confirm(`Delete "${a.title}"? This removes every version.`)) act(() => deleteArtifact(a.id, tk)); }}>Delete</button>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</main>
```

`web/shell/src/main.ts`:

```ts
import { mount } from "svelte";
import { mountArtifactView } from "./artifact";
import { parseShellPath } from "./route";
import Gallery from "./ui/Gallery.svelte";

const app = document.getElementById("app")!;
const r = parseShellPath(location.pathname);
if (r.kind === "artifact") mountArtifactView(app, { id: r.id, pinnedVersion: r.version, file: r.file });
else mount(Gallery, { target: app });
```

Run: `git rm web/shell/src/main.tsx`. In `web/shell/index.html`, change `src="./src/main.tsx"` to `src="./src/main.ts"`. In `islands/svelte.ts`, add `import TopbarIsland from "../ui/TopbarIsland.svelte";` and `topbar: island(TopbarIsland)`. In `islands/index.ts`, set all three entries from `svelteIslands` and remove the `preactIslands` import.

- [ ] **Step 3: Run the unit tests**

Run: `cd web && npm test -- --reporter=dot`
Expected: PASS, including `gallery.test.ts` and all `artifact.test.ts` tests, now with no Preact island mounted.

- [ ] **Step 4: Check types, lint and the e2e oracle**

Run: `cd web && npm run lint && npm run typecheck && npm run build && npm run e2e; echo "exit=$?"; cd .. && git diff --exit-code $(cat .svelte-port-base) -- web/e2e; echo "e2e-unchanged=$?"`
Expected: `exit=0` and `e2e-unchanged=0`.

- [ ] **Step 5: Look at it in a browser, next to the base commit**

Serve this tree and the base commit side by side. Use scratch homes and ports the kernel picks; never `just watch` or `just dev`, which bind 7481 and serve `~/.clax-dev`:

```bash
git worktree add /tmp/clax-base "$(cat .svelte-port-base)"
(cd /tmp/clax-base/web && npm ci && npm run build) && (cd /tmp/clax-base && cargo build -q -p clax-cli)
(cd web && npm run build) && cargo build -q -p clax-cli
for tree in "$PWD" /tmp/clax-base; do
  home="$(mktemp -d)"
  CLAX_HOME="$home" CLAX_NO_OPEN=1 "$tree/target/debug/clax" serve --foreground --bind 127.0.0.1 --port 0 &
  until [ -f "$home/daemon.json" ]; do sleep 0.2; done
  printf '<h1 id="top">Two</h1><p>First section.</p><a href="more.html#b">more</a>' > "$home/index.html"
  printf '<h2 id="b">More</h2><p>Second page.</p>' > "$home/more.html"
  (cd "$home" && CLAX_HOME="$home" CLAX_NO_OPEN=1 "$tree/target/debug/clax" publish index.html --file more.html --title "Look")
  echo "$tree: http://localhost:$(python3 -c "import json;print(json.load(open('$home/daemon.json'))['port'])")"
done
```

Open both printed URLs in Chromium (at desktop width and at 390 px width, in light and dark color schemes) and do the same things in each:
- open `/`;
- open the artifact;
- turn comment mode on and pick the heading;
- post a comment;
- open the sidebar and check where "Your name" sits;
- follow the "more" link and press Back.

The two must look and behave the same. When done: `kill %1 %2; git worktree remove --force /tmp/clax-base`.

- [ ] **Step 6: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 7: Commit**

```bash
git add web/shell/src/ui/TopbarIsland.svelte web/shell/src/ui/Gallery.svelte web/shell/src/main.ts web/shell/src/main.tsx web/shell/index.html web/shell/src/islands/svelte.ts web/shell/src/islands/index.ts web/shell/src/gallery.test.ts
git commit --no-gpg-sign -m "Render the topbar and the gallery with Svelte

Every island and the gallery route are Svelte; Preact now renders
nothing in the shell."
```

---

### Task 10: Remove Preact

**Files:**
- Delete: `web/shell/src/comments.tsx`, `sidebar.tsx`, `gallery.tsx`, `prompt.tsx`, `viewer-name.tsx`, `web/shell/src/islands/preact.tsx`, `web/shell/src/test/preact.ts`
- Modify: `web/package.json`, `web/package-lock.json`, `web/vite.shell.config.ts`, `web/vitest.config.ts`, `web/tsconfig.json`, `web/shell/src/islands/index.ts`, `web/shell/src/islands/svelte.ts`

- [ ] **Step 1: Delete the Preact sources**

Run: `git rm web/shell/src/comments.tsx web/shell/src/sidebar.tsx web/shell/src/gallery.tsx web/shell/src/prompt.tsx web/shell/src/viewer-name.tsx web/shell/src/islands/preact.tsx web/shell/src/test/preact.ts`

In `islands/svelte.ts`, export `svelteIslands: Islands` (no longer `Partial`). In `islands/index.ts`, set `export const ISLANDS: Islands = svelteIslands;` and drop the non-null assertions.

- [ ] **Step 2: Remove the packages and settings**

Run: `cd web && npm uninstall preact @preact/preset-vite`
- `vite.shell.config.ts`: remove the `@preact/preset-vite` import and `preact()` from `plugins`.
- `vitest.config.ts`: remove the same; set `include: ["bridge/test/**/*.test.ts", "shell/src/**/*.test.ts"]`.
- `tsconfig.json`: remove `"jsx": "react-jsx"` and `"jsxImportSource": "preact"`.

- [ ] **Step 3: Prove Preact is gone**

Run: `git grep -il preact -- web ':!web/package-lock.json'; grep -c '"node_modules/preact"' web/package-lock.json; ls web/shell/src/*.tsx web/shell/src/**/*.tsx 2>/dev/null | wc -l`
Expected: no file names, `0`, `0`.

Run: `grep -rlE "from \"(preact|svelte)" web/bridge web/shell/src/caps web/shell/src/view || echo clean`
Expected: `clean`.

- [ ] **Step 4: Run the unit tests, the typecheck and the e2e oracle**

Run: `cd web && npm ci && npm run lint && npm run typecheck && npm test -- --reporter=dot && npm run build && npm run e2e; echo "exit=$?"; cd .. && git diff --exit-code $(cat .svelte-port-base) -- web/e2e; echo "e2e-unchanged=$?"`
Expected: `exit=0` and `e2e-unchanged=0`.

- [ ] **Step 5: Run every gate, including time to usable**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`. The port must stay within the Task 1 budgets (baseline × 1.25, scaled by the control). Svelte's runtime is not expected to be smaller than Preact's; the gains come in Tasks 11–13. If `time to usable` fails, run `cd web && npm run perf` twice more. A median that stays over its limit is a regression to find (compare `web/perf/results.json` with the baseline), not a budget to raise.

- [ ] **Step 6: Commit**

```bash
git add web/package.json web/package-lock.json web/vite.shell.config.ts web/vitest.config.ts web/tsconfig.json web/shell/src/islands/index.ts web/shell/src/islands/svelte.ts web/shell/src/comments.tsx web/shell/src/sidebar.tsx web/shell/src/gallery.tsx web/shell/src/prompt.tsx web/shell/src/viewer-name.tsx web/shell/src/islands/preact.tsx web/shell/src/test/preact.ts
git commit --no-gpg-sign -m "Remove Preact from the shell

The shell is Svelte 5 in runes mode; the Preact components, islands, test
helper, Vite preset and JSX settings are gone."
```

---

### Task 11: Two shell entries, inlined CSS, and a bundle-size gate

Before this task one `index.html` loads the gallery and the artifact view together. After it, `/` serves `index.html` (gallery code only) and `/a/…` serves `artifact.html`. `artifact.html` already holds the skeleton, so the page has its layout before any script runs. Both inline the CSS, so no stylesheet request blocks rendering.

**Files:**
- Create: `web/shell/artifact.html`, `web/shell/src/gallery-main.ts`, `web/shell/src/artifact-main.ts`, `web/shell/src/route-cases.json`, `web/scripts/clean-dist.mjs`, `web/scripts/bundle-size.mjs`, `web/perf/bundle-budget.json`, `crates/clax-server/src/shell_route.rs`, `crates/clax-server/tests/shell_entries.rs`
- Delete: `web/shell/src/main.ts`
- Modify: `web/shell/index.html`, `web/vite.shell.config.ts`, `web/package.json`, `web/shell/src/route.test.ts`, `web/shell/src/view/skeleton.test.ts`, `crates/clax-server/src/routes/shell.rs`, `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/lib.rs`, `scripts/quality_gates.sh`, `justfile`, `web/perf/budget.json` (lowered by recording)

**Interfaces:**
- Produces: `clax_server::shell_route::{ShellRoute, parse_shell_path(path: &str) -> ShellRoute, decode_component(s: &str) -> Option<String>}`, with `enum ShellRoute { Gallery, Artifact { id: ArtifactId, version: Option<u64>, file: String } }`. It follows `parseShellPath` in `web/shell/src/route.ts`, and `route-cases.json` pins that both agree.
- Produces: `clax_server::routes::shell::{gallery_page, artifact_page}` handlers; `artifact_page` serves `artifact.html` for a path that `parse_shell_path` reads as an artifact, else `index.html`.
- Produces: `node scripts/bundle-size.mjs [--record]` in `web/`, with budgets `{ "gallery": number, "artifact": number, "bridge": number, "bridgeBaseline": number }` (gzip bytes) in `web/perf/bundle-budget.json`.

- [ ] **Step 1: Write the shared route cases and make the TypeScript side read them**

`web/shell/src/route-cases.json`:

```json
[
  { "path": "/", "route": { "kind": "gallery" } },
  { "path": "/a/7q3k9mzx2b4t", "route": { "kind": "artifact", "id": "7q3k9mzx2b4t", "version": null, "file": "index.html" } },
  { "path": "/a/7q3k9mzx2b4t/", "route": { "kind": "artifact", "id": "7q3k9mzx2b4t", "version": null, "file": "index.html" } },
  { "path": "/a/7q3k9mzx2b4t/v/3", "route": { "kind": "artifact", "id": "7q3k9mzx2b4t", "version": 3, "file": "index.html" } },
  { "path": "/a/7q3k9mzx2b4t/v/3/docs/about.html", "route": { "kind": "artifact", "id": "7q3k9mzx2b4t", "version": 3, "file": "docs/about.html" } },
  { "path": "/a/7q3k9mzx2b4t/docs/a%20b.html", "route": { "kind": "artifact", "id": "7q3k9mzx2b4t", "version": null, "file": "docs/a b.html" } },
  { "path": "/a/7q3k9mzx2b4t/v/x/a.html", "route": { "kind": "artifact", "id": "7q3k9mzx2b4t", "version": null, "file": "v/x/a.html" } },
  { "path": "/a/7q3k9mzx2b4t/%E2%9C%93.html", "route": { "kind": "artifact", "id": "7q3k9mzx2b4t", "version": null, "file": "✓.html" } },
  { "path": "/a/7q3k9mzx2b4t/%E0%A4%A.html", "route": { "kind": "gallery" } },
  { "path": "/a/7q3k9mzx2b4t/%ED%A0%80.html", "route": { "kind": "gallery" } },
  { "path": "/a/7q3k9mzx2b4t/%+1.html", "route": { "kind": "gallery" } },
  { "path": "/a/7q3k9mzx2b4u", "route": { "kind": "gallery" } },
  { "path": "/a/7Q3K9MZX2B4T", "route": { "kind": "gallery" } },
  { "path": "/b/7q3k9mzx2b4t", "route": { "kind": "gallery" } }
]
```

Append to `web/shell/src/route.test.ts`:

```ts
import cases from "./route-cases.json";

describe("parseShellPath and the daemon agree", () => {
  it.each(cases)("$path", ({ path, route }) => {
    expect(parseShellPath(path)).toEqual(route);
  });
});
```

(`parseShellPath` is already imported in that file. Add `"resolveJsonModule": true` to `web/tsconfig.json` if `tsc` rejects the JSON import.)

Run: `cd web && npx vitest run shell/src/route.test.ts`
Expected: PASS. The TypeScript side is the reference. If a case fails here, the case is wrong: fix the JSON, not `route.ts`.

- [ ] **Step 2: Write the failing Rust side**

`crates/clax-server/src/shell_route.rs` starts with only the test module:

```rust
//! The shell's URL scheme, read on the daemon as `web/shell/src/route.ts`
//! reads it in the browser (`route-cases.json` pins that they agree).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agrees_with_the_shell_on_every_shared_case() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/shell/src/route-cases.json"
        )))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let path = case["path"].as_str().unwrap();
            let want = &case["route"];
            let got = match parse_shell_path(path) {
                ShellRoute::Gallery => serde_json::json!({"kind": "gallery"}),
                ShellRoute::Artifact { id, version, file } => serde_json::json!({
                    "kind": "artifact", "id": id.as_str(), "version": version, "file": file
                }),
            };
            assert_eq!(&got, want, "{path}");
        }
    }

    #[test]
    fn decodes_like_decode_uri_component() {
        assert_eq!(decode_component("a%20b").as_deref(), Some("a b"));
        assert_eq!(decode_component("%2F").as_deref(), Some("/"));
        assert_eq!(decode_component("%zz"), None);
        assert_eq!(decode_component("%+1"), None);
        assert_eq!(decode_component("%"), None);
        assert_eq!(decode_component("%C3"), None);
    }
}
```

Add `pub mod shell_route;` to `crates/clax-server/src/lib.rs`.

Run: `cargo test -p clax-server --lib shell_route`
Expected: FAIL to compile: `cannot find function parse_shell_path`.

- [ ] **Step 3: Implement it**

Above the test module in `shell_route.rs`:

```rust
use clax_core::ArtifactId;

/// What a shell path names.
#[derive(Debug, PartialEq)]
pub enum ShellRoute {
    Gallery,
    /// `/a/<id>[/v/<n>][/<file>]`; `file` is `index.html` when the path names none.
    Artifact {
        id: ArtifactId,
        version: Option<u64>,
        file: String,
    },
}

/// `path` (still percent-encoded) as the shell reads it: after `/a/<id>`, a
/// `v` segment followed by an all-digit one is a version and the rest is the
/// file, each segment decoded; anything malformed is the gallery.
pub fn parse_shell_path(path: &str) -> ShellRoute {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() < 3 || !segs[0].is_empty() || segs[1] != "a" {
        return ShellRoute::Gallery;
    }
    let Ok(id) = ArtifactId::parse(segs[2]) else {
        return ShellRoute::Gallery;
    };
    let mut rest = &segs[3..];
    let mut version = None;
    if rest.len() >= 2
        && rest[0] == "v"
        && !rest[1].is_empty()
        && rest[1].bytes().all(|b| b.is_ascii_digit())
    {
        // Too many digits for u64 reads as u64::MAX, a version no artifact has.
        version = Some(rest[1].parse::<u64>().unwrap_or(u64::MAX));
        rest = &rest[2..];
    }
    if rest.last() == Some(&"") {
        rest = &rest[..rest.len() - 1];
    }
    let mut parts = Vec::with_capacity(rest.len());
    for s in rest {
        let Some(d) = decode_component(s) else {
            return ShellRoute::Gallery;
        };
        parts.push(d);
    }
    let file = parts.join("/");
    ShellRoute::Artifact {
        id,
        version,
        file: if file.is_empty() { "index.html".into() } else { file },
    }
}

/// JavaScript's `decodeURIComponent`: `%XX` escapes (two hex digits each)
/// decoded to bytes that must form UTF-8; `None` where it would throw.
pub fn decode_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = b.get(i + 1..i + 3)?;
            if !h.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
```

The shell's `Number(rest[1])` turns a very long digit string into an imprecise float. The daemon reads one as `u64::MAX`. Both then name a version no artifact has, and both serve the artifact entry, so behaviour is unchanged. `route-cases.json` pins exact agreement on every other input.

Run: `cargo test -p clax-server --lib shell_route`
Expected: PASS.

- [ ] **Step 4: Write the failing entry test**

`crates/clax-server/tests/shell_entries.rs`:

```rust
//! `/` serves the gallery entry and `/a/…` the artifact entry. Its own test
//! binary: the web UI override is process-wide.
#![cfg(debug_assertions)]

mod common;
use clax_server::routes::shell::set_web_dist;
use common::TestServer;

#[tokio::test]
async fn each_route_gets_its_own_entry() {
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(dist.path().join("index.html"), "<p>gallery</p>").unwrap();
    std::fs::write(dist.path().join("artifact.html"), "<p>artifact</p>").unwrap();
    set_web_dist(dist.path().to_path_buf());
    let ts = TestServer::spawn().await;
    let body = |p: &'static str| {
        let ts = &ts;
        async move { ts.get(p).await.text().await.unwrap() }
    };
    assert_eq!(body("/").await, "<p>gallery</p>");
    assert_eq!(body("/a/7q3k9mzx2b4t").await, "<p>artifact</p>");
    assert_eq!(body("/a/7q3k9mzx2b4t/v/2/docs/x.html").await, "<p>artifact</p>");
    assert_eq!(body("/a/not-an-id").await, "<p>gallery</p>");
    let res = ts.get("/a/7q3k9mzx2b4t").await;
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert!(res.headers().contains_key("etag"));
}
```

Run: `cargo test -p clax-server --test shell_entries`
Expected: FAIL: `/a/7q3k9mzx2b4t` returns `<p>gallery</p>`.

- [ ] **Step 5: Serve the entries**

In `crates/clax-server/src/routes/shell.rs`, replace `pub async fn shell(req: HeaderMap)` with:

```rust
/// An entry of the shell (`index.html` or `artifact.html`) as an HTML page.
fn entry(req: &HeaderMap, name: &str) -> Result<Response, ApiError> {
    match asset(name) {
        Some(f) => Ok(http_cache::html(req, &String::from_utf8_lossy(&f.data))),
        None => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ui_not_built",
            "the web UI has not been built; run `just web`",
        )),
    }
}

/// `/`: the gallery.
pub async fn gallery_page(req: HeaderMap) -> Result<Response, ApiError> {
    entry(&req, "index.html")
}

/// `/a/…`: the artifact view, or the gallery for a path that names no artifact
/// (the shell shows the gallery for it too).
pub async fn artifact_page(uri: axum::http::Uri, req: HeaderMap) -> Result<Response, ApiError> {
    match crate::shell_route::parse_shell_path(uri.path()) {
        crate::shell_route::ShellRoute::Artifact { .. } => entry(&req, "artifact.html"),
        crate::shell_route::ShellRoute::Gallery => entry(&req, "index.html"),
    }
}
```

In `crates/clax-server/src/routes/mod.rs`, route `"/"` to `shell::gallery_page`, and the three `/a/…` routes to `shell::artifact_page`.

Run: `cargo test -p clax-server --test shell_entries && cargo test -p clax-server`
Expected: PASS.

- [ ] **Step 6: Split the shell into two entries**

`web/shell/index.html`: keep it as is, but load `./src/gallery-main.ts`.

`web/shell/artifact.html`:

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>Clax</title>
<link rel="stylesheet" href="./src/theme.css">
<!--clax:boot-->
</head>
<body>
<div id="app"><div class="page"><header class="topbar"><a href="/" title="Gallery">←</a><h1>Clax</h1><div class="island"></div></header><div class="viewer"><div class="stage"><!--clax:frame--><div class="island"></div></div><div class="island"></div></div></div></div>
<script type="module" src="./src/artifact-main.ts"></script>
</body>
</html>
```

`web/shell/src/gallery-main.ts`:

```ts
import { mount } from "svelte";
import Gallery from "./ui/Gallery.svelte";

mount(Gallery, { target: document.getElementById("app")! });
```

`web/shell/src/artifact-main.ts`:

```ts
import { mountArtifactView } from "./artifact";
import { parseShellPath } from "./route";

const r = parseShellPath(location.pathname);
// The daemon serves this entry only for artifact paths.
if (r.kind === "artifact") mountArtifactView(document.getElementById("app")!, { id: r.id, pinnedVersion: r.version, file: r.file });
```

Run: `git rm web/shell/src/main.ts`

Append to `web/shell/src/view/skeleton.test.ts`:

```ts
import { readFileSync } from "node:fs";

describe("artifact.html", () => {
  it("carries the skeleton markup exactly", () => {
    const html = readFileSync(new URL("../../artifact.html", import.meta.url), "utf8");
    expect(html).toContain(`<div id="app"><div class="page">${SKELETON_HTML}</div></div>`);
    expect(html).toContain("<!--clax:boot-->");
  });
});
```

- [ ] **Step 7: Build both entries with the CSS inlined, from a clean output directory**

`web/scripts/clean-dist.mjs`:

```js
// Removes the previous build's shell and bridge files (their names change
// with their content, and every file in web/dist is embedded in the binary),
// keeping web/dist/.gitkeep.
import { rmSync } from "node:fs";
const dist = new URL("../dist/", import.meta.url);
rmSync(new URL("_clax/", dist), { recursive: true, force: true });
rmSync(new URL(".vite/", dist), { recursive: true, force: true });
for (const f of ["index.html", "artifact.html"]) rmSync(new URL(f, dist), { force: true });
```

In `web/package.json`: `"build": "node scripts/clean-dist.mjs && vite build -c vite.bridge.config.ts && vite build -c vite.shell.config.ts"`.

`web/vite.shell.config.ts`:

```ts
import { defineConfig, type Plugin } from "vite";
import type { OutputAsset } from "rollup";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Replaces each entry's stylesheet link with the stylesheet itself, so no
 * request blocks the first render, and drops the separate CSS files. */
function inlineCss(): Plugin {
  return {
    name: "clax-inline-css",
    apply: "build",
    enforce: "post",
    generateBundle(_options, bundle) {
      const sheets = Object.values(bundle).filter((f): f is OutputAsset => f.type === "asset" && f.fileName.endsWith(".css"));
      for (const c of Object.values(bundle)) {
        if (c.type === "chunk" && c.viteMetadata?.importedCss.size) throw new Error(`${c.fileName} imports CSS; the shell's CSS must come from its HTML entries`);
      }
      for (const f of Object.values(bundle)) {
        if (f.type !== "asset" || !f.fileName.endsWith(".html")) continue;
        let html = String(f.source);
        for (const css of sheets) {
          const text = String(css.source);
          if (text.includes("</style")) throw new Error(`${css.fileName} contains "</style" and cannot be inlined`);
          html = html.replace(new RegExp(`<link rel="stylesheet"[^>]*href="/${escape(css.fileName)}"[^>]*>`), () => `<style>${text}</style>`);
        }
        if (html.includes(`<link rel="stylesheet"`)) throw new Error(`${f.fileName} still links a stylesheet`);
        f.source = html;
      }
      for (const css of sheets) delete bundle[css.fileName];
    },
  };
}

export default defineConfig({
  root: "shell", base: "/", plugins: [svelte(), inlineCss()],
  build: {
    outDir: "../dist", emptyOutDir: false, assetsDir: "_clax/shell", manifest: true,
    rollupOptions: { input: { index: "shell/index.html", artifact: "shell/artifact.html" } },
  },
  server: { proxy: { "/api": "http://127.0.0.1:7481", "/c": "http://127.0.0.1:7481", "/_blob": "http://127.0.0.1:7481", "/healthz": "http://127.0.0.1:7481" } },
});
```

(The `server.proxy` block is the dev server's existing configuration (as the stable-install plan left it) and is unchanged. It is never started by any gate.)

In the `justfile`, change the `web` recipe's `rm -rf …` line to `cd web && node scripts/clean-dist.mjs`. Change the `clean` recipe's `rm -rf web/dist/_clax web/dist/index.html web/node_modules` to `rm -rf web/dist/_clax web/dist/.vite web/dist/index.html web/dist/artifact.html web/node_modules`.

Run: `cd web && npm run build && ls dist dist/_clax/shell && grep -c '<link rel="stylesheet"' dist/index.html dist/artifact.html; grep -c '<!--clax:boot-->' dist/artifact.html; grep -c '<!--clax:frame-->' dist/artifact.html`
Expected: `index.html`, `artifact.html`, `.vite`, `_clax`; only `.js` files under `_clax/shell`; `0` stylesheet links in each; `1` and `1` markers. Vite does not minify HTML, so the comments survive. If a marker is missing, stop and report: the daemon matches the comment text exactly (Task 12).

- [ ] **Step 8: Write the bundle-size gate**

`web/scripts/bundle-size.mjs`:

```js
// Gzip sizes of what must load before each shell entry can render (the HTML
// and its module script with that script's static imports, per the Vite
// manifest) and of the eager bridge, against web/perf/bundle-budget.json.
// --record lowers the budgets to the measured sizes plus 10%, never raising one.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const dist = new URL("../dist/", import.meta.url);
const budgetFile = new URL("../perf/bundle-budget.json", import.meta.url);
const read = p => readFileSync(new URL(p, dist));
const gz = p => gzipSync(read(p), { level: 9 }).length;
const manifest = JSON.parse(read(".vite/manifest.json"));

function closure(key, seen = new Set()) {
  if (seen.has(key)) return seen;
  seen.add(key);
  for (const k of manifest[key].imports ?? []) closure(k, seen);
  return seen;
}
const entry = html => {
  const files = [...closure(html)].map(k => manifest[k].file);
  return gz(html) + files.reduce((n, f) => n + gz(f), 0);
};

for (const [html, markers] of [["index.html", []], ["artifact.html", ["<!--clax:boot-->", "<!--clax:frame-->"]]]) {
  const text = read(html).toString();
  if (text.includes(`<link rel="stylesheet"`)) throw new Error(`dist/${html} links a stylesheet; the CSS must be inlined`);
  for (const m of markers) if (!text.includes(m)) throw new Error(`dist/${html} lost the ${m} marker the daemon injects at`);
}

const sizes = { gallery: entry("index.html"), artifact: entry("artifact.html"), bridge: gz("_clax/bridge.js") };
console.log(`gzip bytes: gallery ${sizes.gallery}, artifact ${sizes.artifact}, eager bridge ${sizes.bridge}`);

const budget = existsSync(budgetFile) ? JSON.parse(readFileSync(budgetFile, "utf8")) : null;
if (process.argv.includes("--record")) {
  const up = n => Math.ceil((n * 1.1) / 256) * 256;
  const next = { ...(budget ?? { bridgeBaseline: sizes.bridge }) };
  for (const k of ["gallery", "artifact", "bridge"]) next[k] = Math.min(up(sizes[k]), budget?.[k] ?? Infinity);
  writeFileSync(budgetFile, JSON.stringify(next, null, 2) + "\n");
  process.exit(0);
}
if (!budget) throw new Error("web/perf/bundle-budget.json is missing; run node scripts/bundle-size.mjs --record");
let failed = false;
for (const k of ["gallery", "artifact", "bridge"]) {
  if (sizes[k] > budget[k]) { console.error(`${k}: ${sizes[k]} gzip bytes, over its budget of ${budget[k]}`); failed = true; }
}
if (failed) process.exit(1);
```

Run: `cd web && node scripts/bundle-size.mjs; echo "exit=$?"`
Expected: an error saying the budget file is missing, `exit=1`.

Run: `cd web && node scripts/bundle-size.mjs --record && cat perf/bundle-budget.json && node scripts/bundle-size.mjs; echo "exit=$?"`
Expected: the budget file with all four keys (`bridgeBaseline` equals the measured bridge size), then the sizes line and `exit=0`. Check that `gallery` holds no artifact-view code: `grep -l "clax:hello" dist/_clax/shell/*.js` must list only chunks outside the gallery's closure (compare with `manifest["index.html"].imports`).

In `scripts/quality_gates.sh`, after the `web build` line, add:

```bash
run "web bundle size"       bash -c 'cd web && node scripts/bundle-size.mjs'
```

- [ ] **Step 9: Check the e2e suite and lower the time budgets**

Run: `cd web && npm run e2e; echo "exit=$?"`
Expected: `exit=0` in both frame modes. The e2e specs that load `/a/<ID>` now get `artifact.html` with its skeleton.

Run: `cd web && CLAX_PERF_RECORD=budget npm run perf; echo "exit=$?"; git diff --stat perf/budget.json`
Expected: `exit=0`; the budgets fell or stayed put, and none rose (the harness refuses to raise one).

- [ ] **Step 10: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 11: Commit**

```bash
git add web/shell/artifact.html web/shell/index.html web/shell/src/gallery-main.ts web/shell/src/artifact-main.ts web/shell/src/main.ts web/shell/src/route-cases.json web/shell/src/route.test.ts web/shell/src/view/skeleton.test.ts web/scripts/clean-dist.mjs web/scripts/bundle-size.mjs web/perf/bundle-budget.json web/perf/budget.json web/vite.shell.config.ts web/package.json web/tsconfig.json crates/clax-server/src/shell_route.rs crates/clax-server/src/lib.rs crates/clax-server/src/routes/shell.rs crates/clax-server/src/routes/mod.rs crates/clax-server/tests/shell_entries.rs scripts/quality_gates.sh justfile
git commit --no-gpg-sign -m "Serve the gallery and the artifact view as separate entries with inlined CSS

/ loads only the gallery's code and /a/... gets artifact.html, whose body
already holds the page skeleton; the daemon reads shell paths exactly as
the shell does (route-cases.json). Each build starts from a clean
web/dist, and a gate holds each entry's critical JavaScript and the eager
bridge to gzip budgets."
```

---

### Task 12: The daemon embeds the first-load data and the content frame

Before this task the shell spends its first round trips on `GET /api/artifacts/<ID>`, the thread list and the viewer lookup. It also decides the frame mode, possibly probing `<ID>.localhost`, and only then inserts the `<iframe>`. After it, `artifact.html` arrives with a bootstrap block, and, for a browser that has used Clax before, with the `<iframe>` already in the stage. The artifact then loads in parallel with the shell's JavaScript.

**Files:**
- Create: `crates/clax-server/src/boot.rs`, `crates/clax-server/tests/shell_boot.rs`, `web/shell/src/view/boot.ts`, `web/shell/src/boot.test.ts`, `web/shell/src/frame-src-cases.json`, `web/e2e/boot.spec.ts`
- Modify: `crates/clax-server/src/lib.rs`, `crates/clax-server/src/routes/shell.rs`, `crates/clax-server/src/routes/artifacts.rs` (`with_owner` becomes `pub(crate)`), `crates/clax-server/src/viewer.rs` (`read` is already `pub(crate)`; no change unless the compiler asks), `web/shell/artifact.html`, `web/shell/src/artifact.ts`, `web/shell/src/artifact-main.ts`, `web/shell/src/view/artifact-controller.ts`, `web/shell/src/origin.ts`, `web/shell/src/origin.test.ts`, `web/shell/src/threads.ts`, `web/.oxlintrc.json`, `web/perf/usable.perf.ts` (the priming visit), `web/perf/budget.json` (lowered by recording)

**Interfaces:**
- Produces (Rust): `clax_server::boot::{FRAME_SANDBOX, FRAME_COOKIE, Injected, assemble(s: &AppState, route: ShellRoute, headers: &HeaderMap) -> Result<Option<Injected>, ApiError>, inject(template: &str, i: Option<&Injected>) -> String, script_json(v: &Value) -> String, html_escape(s: &str) -> String, encode_component(s: &str) -> String, page_src(id: &str, n: u64, origin: Option<&str>, file: &str) -> String}`.
- Produces (TS), from `view/boot.ts`: `type Boot = { v: 1; artifact: Loaded; threads: Thread[]; viewer: Viewer | null; frame: { mode: "subdomain" | "sandbox"; src: string } | null }`, `readBoot(doc?: Document): Boot | null`, `takeEarly(win?: Window): MessageEvent[]`, `FRAME_COOKIE = "clax_frame"`, `rememberFrameMode(origin: string | null, doc?: Document): void`.
- Produces (TS): `cachedOriginOk(): boolean | null` from `origin.ts`; `seedViewer(v: Viewer | null): void` from `threads.ts`; `ArtifactController` gains a second constructor argument `{ boot?: Boot | null }` and a public `replay(e: MessageEvent): void`; `mountArtifactView(root, props, opts?: { islands?: Islands; boot?: Boot | null; early?: () => MessageEvent[] })`. The third argument changes from `islands` to an options object; no caller passes it yet.

- [ ] **Step 1: Pin the frame src format on both sides**

`web/shell/src/frame-src-cases.json`:

```json
[
  { "id": "7q3k9mzx2b4t", "n": 1, "origin": null, "file": "index.html", "src": "/c/7q3k9mzx2b4t/v/1/" },
  { "id": "7q3k9mzx2b4t", "n": 12, "origin": "http://7q3k9mzx2b4t.localhost:7481", "file": "index.html", "src": "http://7q3k9mzx2b4t.localhost:7481/v/12/" },
  { "id": "7q3k9mzx2b4t", "n": 1, "origin": null, "file": "docs/a b.html", "src": "/c/7q3k9mzx2b4t/v/1/docs/a%20b.html" },
  { "id": "7q3k9mzx2b4t", "n": 1, "origin": null, "file": "✓/é (1)!*~'.html", "src": "/c/7q3k9mzx2b4t/v/1/%E2%9C%93/%C3%A9%20(1)!*~'.html" },
  { "id": "7q3k9mzx2b4t", "n": 1, "origin": null, "file": "a#b?c&d.html", "src": "/c/7q3k9mzx2b4t/v/1/a%23b%3Fc%26d.html" }
]
```

Append to `web/shell/src/origin.test.ts`:

```ts
import srcCases from "./frame-src-cases.json";

describe("pageSrc and the daemon agree", () => {
  it.each(srcCases)("$file", ({ id, n, origin, file, src }) => {
    expect(pageSrc(id, n, origin, file)).toBe(src);
  });
});
```

Run: `cd web && npx vitest run shell/src/origin.test.ts`
Expected: PASS. TypeScript is the reference; fix the JSON if a case is wrong.

- [ ] **Step 2: Write the failing Rust unit tests for the helpers**

`crates/clax-server/src/boot.rs`, starting with only its tests:

```rust
//! The first-load data the daemon puts into `artifact.html` for `/a/…`
//! (spec §8 Time to usable): the bootstrap block, the page title and, when
//! the frame mode is known, the content frame. String injection at comment
//! markers the shell build keeps; no template engine.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn page_src_agrees_with_the_shell() {
        let cases: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/shell/src/frame-src-cases.json"
        )))
        .unwrap();
        for c in cases.as_array().unwrap() {
            let got = page_src(
                c["id"].as_str().unwrap(),
                c["n"].as_u64().unwrap(),
                c["origin"].as_str(),
                c["file"].as_str().unwrap(),
            );
            assert_eq!(got, c["src"].as_str().unwrap(), "{c}");
        }
    }

    #[test]
    fn script_json_cannot_close_its_element_or_break_a_line() {
        let v = json!({"t": "</script><!--<script>\u{2028}\u{2029}&>"});
        let s = script_json(&v);
        for bad in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
            assert!(!s.contains(bad), "{s}");
        }
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap(), v);
    }

    #[test]
    fn html_escape_covers_text_and_attributes() {
        assert_eq!(html_escape(r#"<a href="x" title='y'>&</a>"#), "&lt;a href=&quot;x&quot; title=&#39;y&#39;&gt;&amp;&lt;/a&gt;");
    }

    #[test]
    fn inject_without_data_only_removes_the_markers() {
        let t = "<head><!--clax:boot--></head><div class=\"stage\"><!--clax:frame--></div><h1>Clax</h1>";
        assert_eq!(inject(t, None), "<head></head><div class=\"stage\"></div><h1>Clax</h1>");
        let i = Injected { boot: "{\"v\":1}".into(), frame: Some("<iframe></iframe>".into()), title: "A <b>".into() };
        assert_eq!(
            inject(t, Some(&i)),
            "<head><script type=\"application/json\" id=\"clax-boot\">{\"v\":1}</script></head><div class=\"stage\"><iframe></iframe></div><h1>A &lt;b&gt;</h1>"
        );
    }
}
```

Add `pub mod boot;` to `crates/clax-server/src/lib.rs`.

Run: `cargo test -p clax-server --lib boot::`
Expected: FAIL to compile (`cannot find function page_src`, …).

- [ ] **Step 3: Implement the helpers and the assembly**

Above the tests in `boot.rs`:

```rust
use crate::error::ApiError;
use crate::feedback::thread_view;
use crate::routes::artifacts::with_owner;
use crate::shell_route::ShellRoute;
use crate::state::AppState;
use axum::http::{HeaderMap, header};
use serde_json::{Value, json};

/// The frame's sandbox, as `FRAME_SANDBOX` in `web/shell/src/view/frame-host.ts`.
pub const FRAME_SANDBOX: &str = "allow-scripts allow-forms allow-modals allow-popups allow-downloads";
/// Set by the shell once it has decided the frame mode: `subdomain` or `sandbox`.
pub const FRAME_COOKIE: &str = "clax_frame";
const BOOT_MARK: &str = "<!--clax:boot-->";
const FRAME_MARK: &str = "<!--clax:frame-->";
const TITLE_MARK: &str = "<h1>Clax</h1>";

/// What goes into `artifact.html`: the bootstrap JSON (already escaped by
/// [`script_json`]), the frame's markup when the mode is known, the title.
pub struct Injected {
    pub boot: String,
    pub frame: Option<String>,
    pub title: String,
}

/// `v` as JSON safe inside a `<script>` element: `<`, `>`, `&`, U+2028 and
/// U+2029 (which occur only inside JSON strings) become `\u` escapes.
pub fn script_json(v: &Value) -> String {
    let s = serde_json::to_string(v).expect("a JSON value serialises");
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// Escapes text for an HTML element or a double- or single-quoted attribute.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// JavaScript's `encodeURIComponent`.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Where the frame shows `file` of version `n`, as `pageSrc` in `web/shell/src/origin.ts`.
pub fn page_src(id: &str, n: u64, origin: Option<&str>, file: &str) -> String {
    let root = match origin {
        Some(o) => format!("{o}/v/{n}/"),
        None => format!("/c/{id}/v/{n}/"),
    };
    if file == "index.html" {
        return root;
    }
    root + &file.split('/').map(encode_component).collect::<Vec<_>>().join("/")
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// The frame mode the shell will choose, when the daemon can know it: `Some(None)`
/// for sandbox (always on a non-loopback host, where the shell has no artifact
/// origins), `Some(Some(origin))` for the subdomain the cookie names; `None`
/// when a loopback browser has not decided yet.
fn frame_origin(headers: &HeaderMap, id: &str) -> Option<Option<String>> {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let (name, port) = match host.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (h, Some(p)),
        _ => (host, None),
    };
    if name != "localhost" && name != "127.0.0.1" {
        return Some(None);
    }
    match cookie(headers, FRAME_COOKIE).as_deref() {
        Some("subdomain") => Some(Some(format!("http://{id}.localhost{}", port.map(|p| format!(":{p}")).unwrap_or_default()))),
        Some("sandbox") => Some(None),
        _ => None,
    }
}

/// Everything `artifact.html` gets for `route`, or `None` when it names no
/// artifact the store has. The data is what `GET /api/artifacts/<id>`,
/// `GET /api/artifacts/<id>/threads?include_resolved=true` without the token,
/// and `GET /api/viewers/me` for the cookie's existing viewer would answer:
/// never the token, and no viewer is created.
pub async fn assemble(s: &AppState, route: ShellRoute, headers: &HeaderMap) -> Result<Option<Injected>, ApiError> {
    let ShellRoute::Artifact { id, version, file } = route else {
        return Ok(None);
    };
    let viewer_id = crate::viewer::read(headers);
    let codex = s.feedback_ctx().codex_push();
    let lookup = id.clone();
    let found = s
        .store_call(move |st| {
            let Some(a) = st.get_artifact(&lookup)? else {
                return Ok(None);
            };
            let versions = st.list_versions(&lookup)?;
            let owner = match &a.owner_session_id {
                Some(sid) => st.get_session(sid)?,
                None => None,
            };
            let mut threads = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let (page, next) = st.list_threads(&lookup, true, cursor.as_deref(), 200)?;
                for t in &page {
                    threads.push(thread_view(st, t, codex, false)?);
                }
                match next {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
            let viewer = match viewer_id {
                Some(v) => st.get_viewer(&v)?,
                None => None,
            };
            Ok(Some((a, versions, owner, threads, viewer)))
        })
        .await?;
    let Some((a, versions, owner, threads, viewer)) = found else {
        return Ok(None);
    };
    let n = version.unwrap_or(u64::from(a.current_version));
    let holds = versions
        .iter()
        .find(|v| u64::from(v.n) == n)
        .is_some_and(|v| file == "index.html" || v.files.contains_key(&file));
    let (frame, frame_json) = match frame_origin(headers, id.as_str()) {
        Some(origin) if holds => {
            let src = page_src(id.as_str(), n, origin.as_deref(), &file);
            let sandbox = if origin.is_none() { format!(" sandbox=\"{FRAME_SANDBOX}\"") } else { String::new() };
            (
                Some(format!(
                    "<iframe class=\"frame\" title=\"artifact content\" src=\"{}\" allow=\"clipboard-write; fullscreen\"{sandbox}></iframe>",
                    html_escape(&src)
                )),
                json!({"mode": if origin.is_some() { "subdomain" } else { "sandbox" }, "src": src}),
            )
        }
        _ => (None, Value::Null),
    };
    let boot = json!({
        "v": 1,
        "artifact": {"artifact": with_owner(&a, owner.as_ref()), "versions": versions},
        "threads": threads,
        "viewer": viewer,
        "frame": frame_json,
    });
    Ok(Some(Injected { boot: script_json(&boot), frame, title: a.title.clone() }))
}

/// `template` (the built `artifact.html`) with `i` injected at its markers,
/// or with the markers removed when there is nothing to inject.
pub fn inject(template: &str, i: Option<&Injected>) -> String {
    let Some(i) = i else {
        return template.replacen(BOOT_MARK, "", 1).replacen(FRAME_MARK, "", 1);
    };
    template
        .replacen(TITLE_MARK, &format!("<h1>{}</h1>", html_escape(&i.title)), 1)
        .replacen(FRAME_MARK, i.frame.as_deref().unwrap_or(""), 1)
        .replacen(BOOT_MARK, &format!("<script type=\"application/json\" id=\"clax-boot\">{}</script>", i.boot), 1)
}
```

In `crates/clax-server/src/routes/artifacts.rs`, change `fn with_owner` to `pub(crate) fn with_owner`.

Run: `cargo test -p clax-server --lib boot:: && cargo clippy -p clax-server --all-targets -- -D warnings`
Expected: PASS and no warnings.

- [ ] **Step 4: Write the failing integration tests**

`crates/clax-server/tests/shell_boot.rs`:

```rust
//! `/a/…` with the bootstrap block and the server-rendered frame. Its own test
//! binary: the web UI override is process-wide.
#![cfg(debug_assertions)]

mod common;
use clax_server::boot::FRAME_SANDBOX;
use clax_server::routes::shell::set_web_dist;
use common::TestServer;
use serde_json::{Value, json};

const ENTRY: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/shell/artifact.html"));
const OPEN: &str = r#"<script type="application/json" id="clax-boot">"#;

fn dist() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("clax-shell-boot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("artifact.html"), ENTRY).unwrap();
        std::fs::write(dir.join("index.html"), "<p>gallery</p>").unwrap();
        set_web_dist(dir);
    });
}

/// The bootstrap block's text and its parsed value.
fn boot(html: &str) -> (String, Value) {
    let start = html.find(OPEN).expect("a bootstrap block") + OPEN.len();
    let end = start + html[start..].find("</script>").unwrap();
    let text = html[start..end].to_string();
    let v = serde_json::from_str(&text).unwrap();
    (text, v)
}

async fn page(ts: &TestServer, path: &str, headers: &[(&str, String)]) -> reqwest::Response {
    let mut req = ts.client.get(format!("{}{path}", ts.base));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    req.send().await.unwrap()
}

async fn publish_version(ts: &TestServer, id: &str, html: &str) {
    let res = ts
        .authed(ts.client.post(format!("{}/api/artifacts/{id}/versions", ts.base)))
        .json(&json!({"files": {"index.html": {"content": html, "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.text().await.unwrap());
}

#[tokio::test]
async fn embeds_what_the_api_answers_and_never_the_token() {
    dist();
    let ts = TestServer::spawn().await;
    let created = ts.publish("Report", &[("index.html", "<p>x</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    ts.thread(&id, 1, "first").await;
    let res = page(&ts, &format!("/a/{id}"), &[]).await;
    assert!(res.headers().get("set-cookie").is_none(), "no viewer is created");
    let html = res.text().await.unwrap();
    let (_, b) = boot(&html);
    let api: Value = ts.get(&format!("/api/artifacts/{id}")).await.json().await.unwrap();
    let threads: Value = ts.get(&format!("/api/artifacts/{id}/threads?include_resolved=true&limit=200")).await.json().await.unwrap();
    assert_eq!(b["v"], 1);
    assert_eq!(b["artifact"], api);
    assert_eq!(b["threads"], threads["threads"]);
    assert_eq!(b["viewer"], Value::Null);
    assert_eq!(b["frame"], Value::Null);
    assert!(!html.contains(&ts.token));
    assert!(html.contains("<h1>Report</h1>"));
    assert!(!html.contains("<!--clax:boot-->") && !html.contains("<!--clax:frame-->"));
}

#[tokio::test]
async fn names_the_cookies_viewer_only() {
    dist();
    let ts = TestServer::spawn().await;
    let id = ts.publish("V", &[("index.html", "<p>x</p>")]).await["artifact"]["id"].as_str().unwrap().to_string();
    let v = ts.viewer(Some("Ada")).await;
    let cookie = format!("clax_viewer={}", v.cookie);
    let html = page(&ts, &format!("/a/{id}"), &[("cookie", cookie.clone())]).await.text().await.unwrap();
    let me: Value = page(&ts, "/api/viewers/me", &[("cookie", cookie)]).await.json().await.unwrap();
    assert_eq!(boot(&html).1["viewer"], me["viewer"]);
    let unknown = page(&ts, &format!("/a/{id}"), &[("cookie", "clax_viewer=01J00000000000000000000000".into())]).await.text().await.unwrap();
    assert_eq!(boot(&unknown).1["viewer"], Value::Null);
}

#[tokio::test]
async fn hostile_titles_and_comments_stay_data() {
    dist();
    let ts = TestServer::spawn().await;
    let id = ts.publish("</script><b>x</b>", &[("index.html", "<p>x</p>")]).await["artifact"]["id"].as_str().unwrap().to_string();
    let body = "</script><!--<script>\u{2028}\u{2029}&";
    ts.thread(&id, 1, body).await;
    let html = page(&ts, &format!("/a/{id}"), &[]).await.text().await.unwrap();
    let (text, b) = boot(&html);
    for bad in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
        assert!(!text.contains(bad), "{bad:?} in {text}");
    }
    assert_eq!(b["threads"][0]["comments"][0]["body"], body);
    assert_eq!(b["artifact"]["artifact"]["title"], "</script><b>x</b>");
    assert!(html.contains("<h1>&lt;/script&gt;&lt;b&gt;x&lt;/b&gt;</h1>"));
}

#[tokio::test]
async fn renders_the_frame_only_when_the_mode_is_known() {
    dist();
    let ts = TestServer::spawn().await;
    let created = ts.publish("F", &[("index.html", "<p>x</p>"), ("docs/a b.html", "<p>y</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    publish_version(&ts, &id, "<p>v2</p>").await;
    let port = ts.addr.port();
    let get = |path: String, headers: Vec<(&'static str, String)>| {
        let ts = &ts;
        async move { page(ts, &path, &headers).await.text().await.unwrap() }
    };
    assert!(!get(format!("/a/{id}"), vec![]).await.contains("<iframe"));
    let sandbox = get(format!("/a/{id}"), vec![("cookie", "clax_frame=sandbox".into())]).await;
    assert!(sandbox.contains(&format!(r#"<iframe class="frame" title="artifact content" src="/c/{id}/v/2/" allow="clipboard-write; fullscreen" sandbox="{FRAME_SANDBOX}"></iframe>"#)), "{sandbox}");
    assert_eq!(boot(&sandbox).1["frame"], json!({"mode": "sandbox", "src": format!("/c/{id}/v/2/")}));
    let sub = get(format!("/a/{id}"), vec![("cookie", "clax_frame=subdomain".into())]).await;
    assert!(sub.contains(&format!(r#"src="http://{id}.localhost:{port}/v/2/" allow="clipboard-write; fullscreen"></iframe>"#)), "{sub}");
    let lan = get(format!("/a/{id}"), vec![("host", format!("192.168.1.5:{port}"))]).await;
    assert!(lan.contains(&format!(r#"src="/c/{id}/v/2/""#)) && lan.contains("sandbox="), "{lan}");
    let pinned = get(format!("/a/{id}/v/1/docs/a%20b.html"), vec![("cookie", "clax_frame=sandbox".into())]).await;
    assert!(pinned.contains(&format!(r#"src="/c/{id}/v/1/docs/a%20b.html""#)), "{pinned}");
    let missing = get(format!("/a/{id}/nope.html"), vec![("cookie", "clax_frame=sandbox".into())]).await;
    assert!(!missing.contains("<iframe"));
    let res = page(&ts, &format!("/a/{id}"), &[("host", format!("{id}.localhost:{port}"))]).await;
    assert_eq!(res.status(), 404, "never served on an artifact origin");
}

#[tokio::test]
async fn the_etag_covers_the_injected_bytes() {
    dist();
    let ts = TestServer::spawn().await;
    let id = ts.publish("E", &[("index.html", "<p>x</p>")]).await["artifact"]["id"].as_str().unwrap().to_string();
    let first = page(&ts, &format!("/a/{id}"), &[]).await;
    assert_eq!(first.headers()["cache-control"], "no-cache");
    assert_eq!(first.headers()["vary"], "Cookie");
    let tag = first.headers()["etag"].to_str().unwrap().to_string();
    let again = page(&ts, &format!("/a/{id}"), &[("if-none-match", tag.clone())]).await;
    assert_eq!(again.status(), 304);
    assert_eq!(again.headers()["vary"], "Cookie");
    ts.thread(&id, 1, "new").await;
    let after = page(&ts, &format!("/a/{id}"), &[("if-none-match", tag.clone())]).await;
    assert_eq!(after.status(), 200);
    assert_ne!(after.headers()["etag"].to_str().unwrap(), tag);
    let v = ts.viewer(None).await;
    let as_viewer = page(&ts, &format!("/a/{id}"), &[("cookie", format!("clax_viewer={}", v.cookie))]).await;
    assert_ne!(as_viewer.headers()["etag"], after.headers()["etag"]);
}

#[tokio::test]
async fn an_unknown_artifact_gets_the_bare_entry() {
    dist();
    let ts = TestServer::spawn().await;
    let res = page(&ts, "/a/7q3k9mzx2b4t", &[]).await;
    assert_eq!(res.status(), 200);
    let html = res.text().await.unwrap();
    assert!(!html.contains(OPEN) && !html.contains("<!--clax:boot-->"));
}
```

Run: `cargo test -p clax-server --test shell_boot`
Expected: FAIL: `a bootstrap block` panics (the route does not inject yet).

- [ ] **Step 5: Inject in the route**

In `crates/clax-server/src/routes/shell.rs`, replace `artifact_page` with:

```rust
/// `/a/…`: `artifact.html` with the first-load data (crate::boot), or the
/// gallery for a path that names no artifact. `Vary: Cookie`: the viewer and
/// the frame mode come from cookies.
pub async fn artifact_page(
    axum::extract::State(s): axum::extract::State<crate::state::AppState>,
    uri: axum::http::Uri,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let route = crate::shell_route::parse_shell_path(uri.path());
    if matches!(route, crate::shell_route::ShellRoute::Gallery) {
        return entry(&req, "index.html");
    }
    let Some(f) = asset("artifact.html") else {
        return entry(&req, "artifact.html");
    };
    let template = String::from_utf8_lossy(&f.data).into_owned();
    let injected = crate::boot::assemble(&s, route, &req).await?;
    let mut res = http_cache::html(&req, &crate::boot::inject(&template, injected.as_ref()));
    res.headers_mut().insert(header::VARY, axum::http::HeaderValue::from_static("Cookie"));
    Ok(res)
}
```

Run: `cargo test -p clax-server --test shell_boot && cargo test -p clax-server --test shell_entries`
Expected: PASS.

- [ ] **Step 6: Buffer the frame's early messages in the entry**

In `web/shell/artifact.html`, put this line directly before `<!--clax:boot-->`:

```html
<script id="clax-early">(function(){var q=[];function keep(e){q.push(e)}addEventListener("message",keep);window.__claxEarly={take:function(){removeEventListener("message",keep);var r=q;q=[];return r}}})();</script>
```

In `web/.oxlintrc.json`, set the rule to `["warn", { "allow": ["__clax", "__claxEarly"] }]`.

- [ ] **Step 7: Write the failing shell tests**

`web/shell/src/boot.test.ts`:

```ts
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FRAME_SANDBOX } from "./view/frame-host";
import { SKELETON_HTML } from "./view/skeleton";

const ID = "7q3k9mzx2b4t";
const loaded = { artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "x", current_version: 2, pinned: false }, versions: [{ artifact_id: ID, n: 2, label: null, created_at: "x", files: {} }] };
const viewer = { public_id: "u_0123456789abcdef012345", display_name: null, created_at: "x" };
const boot = (frame: { mode: "subdomain" | "sandbox"; src: string } | null) => ({ v: 1 as const, artifact: loaded, threads: [], viewer, frame });
const EARLY = /<script id="clax-early">([\s\S]*?)<\/script>/.exec(readFileSync(new URL("../artifact.html", import.meta.url), "utf8"))![1];
const SUB = `http://${ID}.localhost:3000/v/2/`;

class FakeES { addEventListener() {} close() {} }

async function waitFor<T>(check: () => T | null | undefined | false, what: string): Promise<T> {
  const deadline = Date.now() + 2000;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}

const fromFrame = (win: Window, data: unknown, origin = "null") => window.dispatchEvent(new MessageEvent("message", { data, origin, source: win }));

/** The page as the daemon sends it, with `frame` in the stage. */
function served(frame: string): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = `<div class="page">${SKELETON_HTML.replace("<!--clax:frame-->", frame)}</div>`;
  document.body.append(root);
  return root;
}

function stubFetch(probe: () => Promise<Response>) {
  const f = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.endsWith("/healthz")) return probe();
    if (url === "/api/token") return new Response(JSON.stringify({ token: "tk" }));
    if (url.includes("/threads")) return new Response(JSON.stringify({ threads: [], next_cursor: null }));
    return new Response(JSON.stringify(loaded));
  });
  vi.stubGlobal("fetch", f);
  vi.stubGlobal("EventSource", FakeES);
  return f;
}

async function mountServed(root: HTMLElement, b: ReturnType<typeof boot>) {
  const { mountArtifactView } = await import("./artifact");
  const { takeEarly } = await import("./view/boot");
  return mountArtifactView(root, { id: ID, pinnedVersion: null }, { boot: b, early: () => takeEarly() });
}

describe("the first load from the daemon's HTML", () => {
  beforeEach(async () => { vi.resetModules(); (await import("./threads")).forgetViewer(); history.replaceState(null, "", `/a/${ID}`); });
  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
    document.cookie = "clax_frame=; Max-Age=0; Path=/";
    delete (window as { __claxEarly?: unknown }).__claxEarly;
    document.body.replaceChildren();
    history.replaceState(null, "", "/");
  });

  it("replays a hello and a capability request that arrived before the shell mounted, and never fetches the artifact", async () => {
    const fetched = stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    new Function(EARLY)();
    const root = served(`<iframe class="frame" src="/c/${ID}/v/2/" sandbox="${FRAME_SANDBOX}"></iframe>`);
    const frame = root.querySelector("iframe")!;
    const posted: { type: string; id?: string }[] = [];
    frame.contentWindow!.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
    fromFrame(frame.contentWindow!, { type: "clax:hello", artifact: ID, version: 2, file: "index.html" });
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "early", name: "permissions" });
    await mountServed(root, boot({ mode: "sandbox", src: `/c/${ID}/v/2/` }));
    expect(root.querySelector("iframe")).toBe(frame);
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
    await waitFor(() => posted.some(m => m.type === "clax:use-result" && m.id === "early"), "the early request's answer");
    expect(fetched.mock.calls.map(c => String(c[0]))).not.toContain(`/api/artifacts/${ID}`);
    expect(document.cookie).toContain("clax_frame=sandbox");
  });

  it("replaces a server frame whose mode the shell does not confirm, and ignores the stale frame", async () => {
    stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    new Function(EARLY)();
    const root = served(`<iframe class="frame" src="${SUB}"></iframe>`);
    const stale = root.querySelector("iframe")!;
    const posted: unknown[] = [];
    stale.contentWindow!.postMessage = ((m: unknown) => { posted.push(m); }) as Window["postMessage"];
    fromFrame(stale.contentWindow!, { type: "clax:hello", artifact: ID, version: 2, file: "index.html" }, `http://${ID}.localhost:3000`);
    await mountServed(root, boot({ mode: "subdomain", src: SUB }));
    const now = root.querySelector("iframe")!;
    expect(now).not.toBe(stale);
    expect(stale.isConnected).toBe(false);
    expect(now.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
    expect(now.getAttribute("sandbox")).toBe(FRAME_SANDBOX);
    await new Promise(r => setTimeout(r, 30));
    expect(posted).toEqual([]);
  });

  it("keeps the served frame when the probe confirms the guess, and replaces it when the probe fails", async () => {
    stubFetch(async () => new Response("{}"));
    const kept = served(`<iframe class="frame" src="${SUB}"></iframe>`);
    const first = kept.querySelector("iframe")!;
    const view = await mountServed(kept, boot({ mode: "subdomain", src: SUB }));
    await waitFor(() => sessionStorage.getItem("clax.origin-ok") === "1", "the probe");
    expect(kept.querySelector("iframe")).toBe(first);
    view.unmount();
    sessionStorage.clear();
    vi.resetModules();
    stubFetch(async () => { throw new TypeError("no such host"); });
    const replaced = served(`<iframe class="frame" src="${SUB}"></iframe>`);
    const guess = replaced.querySelector("iframe")!;
    await mountServed(replaced, boot({ mode: "subdomain", src: SUB }));
    await waitFor(() => replaced.querySelector("iframe") !== guess, "the replacement");
    expect(replaced.querySelector("iframe")!.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
  });

  it("opens the served frame at the URL's fragment and keeps it in the address bar", async () => {
    stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    history.replaceState(null, "", `/a/${ID}#part-2`);
    const root = served(`<iframe class="frame" src="/c/${ID}/v/2/" sandbox="${FRAME_SANDBOX}"></iframe>`);
    await mountServed(root, boot({ mode: "sandbox", src: `/c/${ID}/v/2/` }));
    expect(root.querySelector("iframe")!.getAttribute("src")).toBe(`/c/${ID}/v/2/#part-2`);
    expect(location.hash).toBe("#part-2");
  });
});
```

Run: `cd web && npx vitest run shell/src/boot.test.ts`
Expected: FAIL: `./view/boot` cannot be resolved.

- [ ] **Step 8: Implement the shell side**

`web/shell/src/view/boot.ts`:

```ts
import type { Thread, Viewer } from "../threads";
import type { Loaded } from "./artifact-controller";

/** The daemon's first-load data for `/a/…` (spec §8 Time to usable). */
export type Boot = { v: 1; artifact: Loaded; threads: Thread[]; viewer: Viewer | null; frame: { mode: "subdomain" | "sandbox"; src: string } | null };

/** The bootstrap block in `doc`, or null when there is none or it is not version 1. */
export function readBoot(doc: Document = document): Boot | null {
  const text = doc.getElementById("clax-boot")?.textContent;
  if (!text) return null;
  try {
    const b = JSON.parse(text) as Boot;
    return b && b.v === 1 ? b : null;
  } catch {
    return null;
  }
}

/** The messages `artifact.html`'s inline listener kept before the shell mounted, oldest first; the listener stops. */
export function takeEarly(win: Window = window): MessageEvent[] {
  const early = (win as unknown as { __claxEarly?: { take(): MessageEvent[] } }).__claxEarly;
  return early ? early.take() : [];
}

export const FRAME_COOKIE = "clax_frame";

/** Tells the daemon which frame the next load of this browser can get in its HTML. */
export function rememberFrameMode(origin: string | null, doc: Document = document): void {
  doc.cookie = `${FRAME_COOKIE}=${origin ? "subdomain" : "sandbox"}; Path=/; Max-Age=2592000; SameSite=Lax`;
}
```

The cookie only decides which `<iframe>`, if any, the next HTML carries. The shell still decides the frame mode itself and replaces a frame that disagrees. A cookie set by anyone else, for example a page on a sibling `*.localhost` origin, therefore costs at most one extra frame load.

In `web/shell/src/origin.ts`, export the cache read: rename `readCache` to `export function cachedOriginOk(): boolean | null` (same body) and use it in `probeOrigin`.

In `web/shell/src/threads.ts`, add below `getViewer`:

```ts
/** The viewer the daemon found for the page's cookie (its bootstrap): the
 * lookup is answered without a request. No-op for null or once looked up. */
export function seedViewer(v: Viewer | null): void {
  if (!v || viewerMemo) return;
  viewerMemo = Promise.resolve(v);
  announceViewer(v);
}
```

In `web/shell/src/view/artifact-controller.ts`:
- import `{ type Boot, rememberFrameMode } from "./boot"`, `cachedOriginOk` from `../origin`, and `seedViewer` from `../threads`;
- change the constructor signature to `constructor(props: ArtifactProps, private readonly opts: { boot?: Boot | null } = {})`;
- replace the body of `start()` down to `this.offs.push(onShieldPress(…))` with:

```ts
    this.live = true;
    const boot = this.opts.boot ?? null;
    if (boot) {
      seedViewer(boot.viewer);
      this.loaded(boot.artifact);
      this.set({ threads: boot.threads });
    } else {
      getArtifact(this.id).then(d => this.loaded(d), e => this.set({ error: e instanceof ApiError && e.status === 404 ? "Artifact not found" : String(e) }));
    }
    this.decideOrigin(boot);
```

- replace the `this.loadThreads();` line in `start()` with `if (!boot) this.loadThreads();` (the stream's `ready` event reloads the threads in both cases);
- add:

```ts
  private loaded(d: Loaded): void {
    if (this.disposed) return;
    this.latestKnown = Math.max(this.latestKnown, d.artifact.current_version);
    this.set(s => ({ data: d, newer: s.newer !== null && s.newer <= d.artifact.current_version ? null : s.newer }));
    this.viewChanged();
  }

  /** The frame mode: this tab's cached probe; else the daemon's guess (from
   * the cookie), confirmed or corrected by a probe; else a probe. */
  private decideOrigin(boot: Boot | null): void {
    const o = artifactOrigin(this.id);
    const decided = (origin: string | null) => {
      if (this.disposed) return;
      rememberFrameMode(origin);
      this.set({ origin });
      this.viewChanged();
    };
    if (!o) { decided(null); return; }
    const cached = cachedOriginOk();
    if (cached !== null) { decided(cached ? o : null); return; }
    if (boot?.frame) {
      const guess = boot.frame.mode === "subdomain" ? o : null;
      decided(guess);
      void probeOrigin(o).then(ok => { if ((ok ? o : null) !== guess) decided(ok ? o : null); });
      return;
    }
    void probeOrigin(o).then(ok => decided(ok ? o : null));
  }

  /** A message the frame posted before the shell mounted. */
  replay(e: MessageEvent): void {
    this.onMessage(e);
  }
```

- delete the inline `getArtifact(...).then(...)` data handling and the `decided`/`probeOrigin` lines that `loaded` and `decideOrigin` now hold.

In `web/shell/src/artifact.ts`, change the signature and the start of the mount to:

```ts
export type MountOptions = { islands?: Islands; boot?: Boot | null; early?: () => MessageEvent[] };

export function mountArtifactView(root: HTMLElement, props: ArtifactProps, opts: MountOptions = {}): ArtifactMount {
  const islands = opts.islands ?? ISLANDS;
  // The bootstrap describes the first view only.
  let boot = opts.boot ?? null;
  const start = (p: ArtifactProps) => {
    const sk = skeleton(root);
    const ctl = new ArtifactController(p, { boot: boot?.artifact.artifact.id === p.id ? boot : null });
    boot = null;
```

After `ctl.start();` add `for (const e of opts.early?.() ?? []) ctl.replay(e);`. Import `type Boot` from `./view/boot`.

`web/shell/src/artifact-main.ts`:

```ts
import { mountArtifactView } from "./artifact";
import { parseShellPath } from "./route";
import { readBoot, takeEarly } from "./view/boot";

const r = parseShellPath(location.pathname);
// The daemon serves this entry only for artifact paths.
if (r.kind === "artifact") mountArtifactView(document.getElementById("app")!, { id: r.id, pinnedVersion: r.version, file: r.file }, { boot: readBoot(), early: () => takeEarly() });
```

- [ ] **Step 9: Run the shell tests**

Run: `cd web && npx vitest run shell/src/boot.test.ts && npm test -- --reporter=dot`
Expected: PASS, including every `artifact.test.ts` test (they mount without a bootstrap, the old path).

- [ ] **Step 10: Add the end-to-end checks**

`web/e2e/boot.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { type FrameMode, contentFrame, publish, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = `<h1>Top</h1><p style="height:3000px">long</p><h2 id="part-2">Part 2</h2>`;

for (const mode of ["subdomain", "sandbox"] as FrameMode[]) {
  test(`a second visit gets the frame in the HTML, opens at the link's fragment, and comments at once (${mode})`, async ({ browser }) => {
    const { artifact } = await publish(d.base, d.token, "Boot", { "index.html": PAGE });
    const ctx = await browser.newContext();
    if (mode === "sandbox") await ctx.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    const first = await ctx.newPage();
    await first.goto(`${d.base}/a/${artifact.id}`);
    await contentFrame(first, artifact.id, 1);
    await first.close();
    const html = await (await ctx.request.get(`${d.base}/a/${artifact.id}`)).text();
    expect(html).toContain(`<iframe class="frame"`);
    expect(html).not.toContain(d.token);
    const page = await ctx.newPage();
    await page.goto(`${d.base}/a/${artifact.id}#part-2`);
    const frame = await contentFrame(page, artifact.id, 1);
    await expect.poll(() => frame.evaluate(() => location.hash)).toBe("#part-2");
    expect(new URL(page.url()).hash).toBe("#part-2");
    await page.getByRole("button", { name: "Comment" }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    await ctx.close();
  });

  test(`a hostile title is shown as text (${mode})`, async ({ page }) => {
    if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    const title = `</script><img src=x onerror="window.pwned=1">`;
    const { artifact } = await publish(d.base, d.token, title, { "index.html": "<p>x</p>" });
    await page.goto(`${d.base}/a/${artifact.id}`);
    await expect(page.locator(".topbar h1")).toHaveText(title);
    expect(await page.evaluate(() => (window as { pwned?: number }).pwned)).toBeUndefined();
  });
}
```

Run: `cd web && npm run build && npm run e2e; echo "exit=$?"`
Expected: `exit=0`. All earlier specs PASS unchanged, in both modes.

- [ ] **Step 11: Make the perf harness's priming visit set the cookie, then lower the budgets**

The harness's first tab already loads the artifact, so the `clax_frame` cookie is set before the measured tab opens, and no harness change is needed. Confirm with `grep -n "first.goto" web/perf/usable.perf.ts`.

Run: `cd web && CLAX_PERF_RECORD=budget npm run perf; echo "exit=$?"; git diff perf/budget.json`
Expected: `exit=0`. The `firstPaint` budgets fall clearly, since the frame no longer waits for the shell's JavaScript.

- [ ] **Step 12: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 13: Commit**

```bash
git add crates/clax-server/src/boot.rs crates/clax-server/src/lib.rs crates/clax-server/src/routes/shell.rs crates/clax-server/src/routes/artifacts.rs crates/clax-server/tests/shell_boot.rs web/shell/artifact.html web/shell/src/view/boot.ts web/shell/src/boot.test.ts web/shell/src/frame-src-cases.json web/shell/src/origin.ts web/shell/src/origin.test.ts web/shell/src/threads.ts web/shell/src/view/artifact-controller.ts web/shell/src/artifact.ts web/shell/src/artifact-main.ts web/.oxlintrc.json web/e2e/boot.spec.ts web/perf/budget.json
git commit --no-gpg-sign -m "Embed the first-load data and the content frame in the artifact entry

The daemon injects a script-safe bootstrap block (artifact, versions,
threads, the cookie's viewer) and, once the shell has recorded the frame
mode in the clax_frame cookie, the content iframe itself, so the page
loads in parallel with the shell. An inline listener keeps the frame's
early messages for the shell to replay; a frame whose mode the shell does
not confirm is replaced. The ETag covers the injected bytes, with Vary:
Cookie, and the token is never embedded."
```

---

### Task 13: Lazy bridge parts, and the time-to-usable targets

The bridge is a render-blocking classic script at the top of every artifact page, and today it carries everything: comment mode, anchoring, areas and the screenshot library (`modern-screenshot`, about 24 KB minified). After this task the eager `bridge.js` holds only what every page needs before its own scripts run: `window.claude`, the hello, the channel and link handover. Three ES-module parts load on first use.

**Files:**
- Create: `web/bridge/src/block.ts`, `web/bridge/src/comments-context.ts`, `web/bridge/src/parts/comment.ts`, `parts/clip.ts`, `parts/caps.ts`, `parts/types.ts`, `web/bridge/src/parts-url.ts`, `web/bridge/src/parts-static.ts`, `web/vite.bridge-parts.config.ts`, `web/bridge/test/split.test.ts`, `web/e2e/bridge-parts.spec.ts`
- Modify: `scripts/dev.sh`, `web/bridge/src/bridge.ts`, `web/bridge/src/clip.ts`, `web/bridge/src/area.ts`, `web/bridge/src/use.ts`, `web/bridge/src/caps/index.ts`, `web/bridge/src/caps/comments.ts`, `web/bridge/src/protocol.ts`, `web/vite.bridge.config.ts`, `web/vitest.config.ts`, `web/tsconfig.json`, `web/package.json`, the bridge tests named in Step 7, `web/shell/src/failure.ts`, `web/shell/src/view/artifact-controller.ts`, `crates/clax-server/src/routes/shell.rs`, `web/perf/budget.json`, `web/perf/bundle-budget.json`

**Interfaces:**
- Produces: the module alias `clax-bridge-parts` exporting `loadParts(bridgeSrc: string): Parts`, with `type Parts = { comment(): Promise<CommentPart>; clip(): Promise<ClipPart>; caps(): Promise<CapsPart> }`. Each loader is memoised. `parts-url.ts` imports by URL next to the bridge script (production); `parts-static.ts` imports the sources (unit tests) and also exports `settle(): Promise<void>`.
- Produces: `type CommentsContext` and the eager singleton `commentsContext` in `comments-context.ts`. Parts receive it as an argument and never import it, because a part is a separate build with its own module instances.
- Produces: `localsFor(name, rpc, config, env: CapsEnv)` and `commentsLocals(rpc, config, env: CapsEnv)`, with `type CapsEnv = { ctx: CommentsContext; clip: () => Promise<ClipPart> }`. Also `makeUse({ framed, rpc, locals })`, where `locals: (name: CapabilityName, rpc: Rpc, config: unknown) => Promise<Local>` is now required.
- Produces: the protocol message `{ type: "clax:degraded"; part: "comment" | "clip" | "caps"; message: string }` (bridge → shell); `PART_FAILED: Record<"comment" | "clip" | "caps", string>` in `failure.ts`.

- [ ] **Step 1: Write the split guard (failing)**

`web/bridge/test/split.test.ts`:

```ts
// The eager bridge must not pull in what the lazy parts carry, and a part must
// not import state the eager bridge owns (a part is a separate build).
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), "../src");
const ALIAS: Record<string, string> = { "clax-bridge-parts": resolve(SRC, "parts-url.ts") };

/** Modules `file` imports at run time (type-only imports excluded), resolved to files or bare names. */
function runtimeImports(file: string): string[] {
  const text = readFileSync(file, "utf8");
  const out: string[] = [];
  const re = /^\s*(?:import|export)\s+(?!type\b)(?:[^"';]*?\sfrom\s+)?"([^"]+)"/gm;
  for (const m of text.matchAll(re)) {
    const spec = m[1];
    if (ALIAS[spec]) out.push(ALIAS[spec]);
    else if (spec.startsWith(".")) out.push(resolve(dirname(file), spec.endsWith(".ts") ? spec : `${spec}.ts`));
    else out.push(spec);
  }
  return out;
}

function closure(entry: string): Set<string> {
  const seen = new Set<string>();
  const walk = (f: string) => {
    if (seen.has(f)) return;
    seen.add(f);
    if (f.endsWith(".ts")) for (const g of runtimeImports(f)) walk(g);
  };
  walk(entry);
  return seen;
}

const rel = (f: string) => f.startsWith(SRC) ? f.slice(SRC.length + 1) : f;

describe("the bridge split", () => {
  it("keeps comment mode, anchoring, areas, clips and capability members out of the eager bridge", () => {
    const eager = [...closure(resolve(SRC, "bridge.ts"))].map(rel);
    const lazy = ["anchor.ts", "area.ts", "clip.ts", "comment-mode.ts", "target.ts", "text-walk.ts", "sha256.ts", "modern-screenshot"];
    expect(eager.filter(f => lazy.includes(f) || f.startsWith("caps/") || (f.startsWith("parts/") && f !== "parts/types.ts"))).toEqual([]);
  });
  it("lets no part import the eager bridge's state", () => {
    for (const part of ["comment", "clip", "caps"]) {
      const files = [...closure(resolve(SRC, `parts/${part}.ts`))].map(rel);
      expect(files, part).not.toContain("comments-context.ts");
      expect(files, part).not.toContain("bridge.ts");
    }
  });
});
```

Run: `cd web && npx vitest run bridge/test/split.test.ts`
Expected: FAIL: the first test lists `anchor.ts`, `area.ts`, `clip.ts`, `comment-mode.ts`, `caps/…`, `modern-screenshot`, …; the second fails on the missing `parts/*.ts`.

- [ ] **Step 2: Separate what the parts need from the eager bridge**

1. `web/bridge/src/block.ts`: move `blockAncestor` and the `inlineLike` helper it uses from `clip.ts`, verbatim. `clip.ts` re-exports it (`export { blockAncestor } from "./block";`) so `test/clip.test.ts` keeps its import. In `area.ts`, change the import to `import { blockAncestor } from "./block";` so comment mode does not pull in `clip.ts`.
2. `web/bridge/src/comments-context.ts`:

```ts
import { INDEX_FILE } from "./protocol";

/** State the eager bridge shares with the `comments` capability's page-side
 * members: the frame's version and file, whether a custom-anchors
 * registration is live (the bridge's own comment mode then stands down), the
 * hook the bridge calls on scroll and resize, and the one it gives to hear
 * `live` change. Parts get this object as an argument and never import this
 * module: a part is a separate build with its own module instances. */
export type CommentsContext = {
  version: number;
  file: string;
  live: boolean;
  reflow: (() => void) | null;
  liveChanged: (() => void) | null;
};

export const commentsContext: CommentsContext = { version: 0, file: INDEX_FILE, live: false, reflow: null, liveChanged: null };
```

3. `web/bridge/src/caps/comments.ts`:
   - delete its own `commentsContext` declaration;
   - add `import type { CommentsContext } from "../comments-context";` and `import type { ClipPart } from "../parts/types";`;
   - export `type CapsEnv = { ctx: CommentsContext; clip: () => Promise<ClipPart> }`;
   - change the signature to `commentsLocals(rpc: Pick<Rpc, "call" | "on">, config: unknown, env: CapsEnv): Local`;
   - replace every `commentsContext.` with `env.ctx.`;
   - replace each `await renderTargetClip(x)` with `await (await env.clip()).renderTargetClip(x)`;
   - remove the `renderTargetClip` import;
   - make every helper outside `commentsLocals` that read `commentsContext` (for example `toAnchor`) take `ctx: CommentsContext` as a parameter, and pass `env.ctx` at each call.
4. `web/bridge/src/caps/index.ts`: `export function localsFor(name: CapabilityName, rpc: Rpc, config: unknown, env: CapsEnv): Local`, passing `env` to `commentsLocals`.
5. `web/bridge/src/use.ts`: change the options to `{ framed: boolean; rpc: Rpc; locals: (name: CapabilityName, rpc: Rpc, config: unknown) => Promise<Local> }`, remove the `./caps` import, and build the namespace with `buildNamespace(key, opts.rpc, await opts.locals(key, opts.rpc, grant.config))`. The existing `.catch(() => null)` turns a part that cannot load into a `null` namespace, the same as a refused capability.
6. `web/bridge/src/parts/comment.ts`:

```ts
// Comment mode, anchoring and areas: loaded right after the shell's welcome.
export { AnchorCache, buildElementAnchor, buildRangeAnchor, cssPath, resolveAnchor } from "../anchor";
export type { Resolved } from "../anchor";
export { areaBox, boxOf, buildAreaAnchor, containingElement, placeArea } from "../area";
export { blockAncestor } from "../block";
export { CommentMode } from "../comment-mode";
```

`web/bridge/src/parts/clip.ts`:

```ts
// Clip rendering and its screenshot library: loaded when comment mode turns on.
export { renderAreaClip, renderTargetClip } from "../clip";
```

`web/bridge/src/parts/caps.ts`:

```ts
// The capabilities' page-side members: loaded on the first claude.use().
export { localsFor } from "../caps";
export type { CapsEnv } from "../caps/comments";
```

`web/bridge/src/parts/types.ts`:

```ts
export type CommentPart = typeof import("./comment");
export type ClipPart = typeof import("./clip");
export type CapsPart = typeof import("./caps");
export type Parts = { comment(): Promise<CommentPart>; clip(): Promise<ClipPart>; caps(): Promise<CapsPart> };
```

7. `web/bridge/src/parts-url.ts`:

```ts
// The parts, imported by URL from beside the bridge script (/_clax/bridge/…).
// The build defines __CLAX_PARTS__ with their content-hashed names, so the
// bridge's own hash changes whenever a part does.
import type { CapsPart, ClipPart, CommentPart, Parts } from "./parts/types";

declare const __CLAX_PARTS__: { comment: string; clip: string; caps: string };

const once = <T>(f: () => Promise<T>) => { let p: Promise<T> | null = null; return () => (p ??= f()); };

export function loadParts(bridgeSrc: string): Parts {
  const base = new URL("./bridge/", bridgeSrc);
  const url = (name: keyof typeof __CLAX_PARTS__) => new URL(__CLAX_PARTS__[name], base).href;
  return {
    comment: once(() => import(/* @vite-ignore */ url("comment")) as Promise<CommentPart>),
    clip: once(() => import(/* @vite-ignore */ url("clip")) as Promise<ClipPart>),
    caps: once(() => import(/* @vite-ignore */ url("caps")) as Promise<CapsPart>),
  };
}
```

`web/bridge/src/parts-static.ts`:

```ts
// The parts from source, for unit tests (vitest aliases clax-bridge-parts here).
import type { CapsPart, ClipPart, CommentPart, Parts } from "./parts/types";

const loads: Promise<unknown>[] = [];
const once = <T>(f: () => Promise<T>) => { let p: Promise<T> | null = null; return () => { if (!p) { p = f(); loads.push(p); } return p; }; };

export function loadParts(_bridgeSrc: string): Parts {
  return {
    comment: once(() => import("./parts/comment") as Promise<CommentPart>),
    clip: once(() => import("./parts/clip") as Promise<ClipPart>),
    caps: once(() => import("./parts/caps") as Promise<CapsPart>),
  };
}

/** Resolves once every part requested so far has loaded and the work queued on it has run. */
export async function settle(): Promise<void> {
  await Promise.allSettled(loads);
  await new Promise(r => setTimeout(r, 0));
}
```

8. `web/bridge/src/protocol.ts`: add to `BridgeToShell`:

```ts
  /** A lazy part of the bridge could not load (the page's own CSP forbids it). */
  | { type: "clax:degraded"; part: "comment" | "clip" | "caps"; message: string }
```

Also add `"clax:degraded"` to `BRIDGE_TYPES`.

- [ ] **Step 3: Rewrite the eager bridge over the parts**

In `web/bridge/src/bridge.ts`, keep the header comment and add this paragraph to it: `Comment mode with anchoring and areas, clip rendering, and the capabilities' page-side members are lazy parts (parts/*.ts) imported from beside this script: comment mode right after the welcome, clips when comment mode turns on, capability members on the first claude.use(). A part that cannot load is reported to the shell once (clax:degraded).` Replace everything from the first `import` to the end with:

```ts
import { loadParts } from "clax-bridge-parts";
import type { Resolved } from "./anchor";
import { acceptFromShell, forwardedKey, shellOrigins } from "./channel";
import { commentsContext } from "./comments-context";
import { hashFor, helloFor, isFirstBridge, readMeta } from "./meta";
import { followInPlace, linkToHandOver } from "./nav";
import type { CommentPart } from "./parts/types";
import type { Anchor, AnchorResult, Box, BridgeToShell } from "./protocol";
import { Rpc } from "./rpc";
import { whenParsed } from "./parsed";
import { makeUse } from "./use";

type Mode = InstanceType<CommentPart["CommentMode"]>;
type Cache = InstanceType<CommentPart["AnchorCache"]>;
/** Comment mode once its part has loaded. */
type Live = { part: CommentPart; mode: Mode; cache(): Cache; reset(): void };

(() => {
  // One bridge per document: any bridge tag after the document's first one
  // (a copy the page carried in, or a second injection) stands down.
  const script = document.currentScript as HTMLScriptElement | null;
  if (!isFirstBridge(script)) return;
  const meta = readMeta(script);
  (window as any).__clax = meta;
  commentsContext.version = meta.version;
  commentsContext.file = meta.file;
  // Resolved now, before any page script could change the tag.
  const parts = loadParts(script?.src ?? location.href);

  // The shell's window as it is when the bridge loads: a page script that
  // later replaces `window.parent` can neither read nor alter what the
  // bridge posts, nor pose as the shell.
  const shellWin = window.parent;
  const framed = shellWin !== window;
  let shellOrigin: string | null = null;
  const post = (m: BridgeToShell, transfer: Transferable[] = []) =>
    shellWin.postMessage(m, shellOrigin ?? "*", transfer);
  const reported = new Set<string>();
  /** Reports, once per part, that it could not load. */
  const failed = (part: "comment" | "clip" | "caps") => (e: unknown) => {
    if (reported.has(part)) return;
    reported.add(part);
    console.warn(`clax: the ${part} part of the bridge could not load`, e);
    if (framed) post({ type: "clax:degraded", part, message: e instanceof Error ? e.message : String(e) });
  };
  const rpc = new Rpc(m => post(m));
  const use = makeUse({
    framed,
    rpc,
    locals: (name, r, config) => parts.caps().then(
      c => c.localsFor(name, r, config, { ctx: commentsContext, clip: () => parts.clip() }),
      e => { failed("caps")(e); throw e; },
    ),
  });

  try {
    Object.defineProperty(window, "claude", {
      value: Object.freeze({ use }),
      writable: false,
      configurable: false,
      enumerable: true,
    });
  } catch (e) {
    // A page that already defined a non-configurable window.claude wins.
    console.warn("clax: could not install window.claude", e);
  }

  if (!framed) return; // opened directly: there is no shell

  const origins = shellOrigins(location.href);
  const box = (t: Element | Range): Box => { const r = t.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; };

  /** Where a resolved anchor is now: an area's rectangle projected onto its element, else the range or element. */
  const placeOf = (l: Live, anchor: Anchor, r: Resolved): Box => {
    if (anchor.kind === "area" && anchor.area) return l.part.placeArea(anchor, r.element);
    return box(r.range ?? r.element);
  };

  // Anchors are resolved once per shell request; scroll and resize only
  // re-measure, unless the DOM under a resolved element changed since.
  let anchors: { id: string; anchor: Anchor; sameVersion?: boolean }[] = [];
  let latestResolve: unknown = null;
  // The thread the shell focuses; its area, if it is one, is outlined dashed.
  let focusId: string | null = null;
  let shellMode = false;
  let welcomed = false;

  const pick = async (anchor: Anchor, clip: () => Promise<ArrayBuffer>) => {
    const pickId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;
    post({ type: "clax:pick-start", pickId });
    let clipPng: ArrayBuffer | undefined;
    let clipError: string | undefined;
    try { clipPng = await clip(); } catch (e) { clipError = e instanceof Error ? e.message : String(e); }
    post({ type: "clax:pick", pickId, version: meta.version, anchor, clipPng, clipError }, clipPng ? [clipPng] : []);
  };
  const clips = () => parts.clip().catch(e => { failed("clip")(e); throw e; });

  let live: Promise<Live> | null = null;
  /** Runs `f` with comment mode once its part has loaded; calls run in the order made. */
  const withComment = (f: (l: Live) => void) => {
    live ??= parts.comment().then(part => {
      let resolutions: Cache | null = null;
      const mode: Mode = new part.CommentMode(document, {
        hover: t => post({ type: "clax:hover", selector: t ? part.cssPath(t instanceof Element ? t : part.blockAncestor(t.commonAncestorContainer, window)) : null, rect: t ? box(t) : null }),
        pickElement: el => { void pick(part.buildElementAnchor(document, el, meta.file), () => clips().then(c => c.renderTargetClip(el))).finally(() => mode.captured()); },
        pickRange: r => { void pick(part.buildRangeAnchor(document, r, meta.file), () => clips().then(c => c.renderTargetClip(r))).finally(() => mode.captured()); },
        // The clip starts at once, from the page as it is at release.
        pickArea: r => {
          const el = part.containingElement(document, r);
          const anchor = part.buildAreaAnchor(document, r, meta.file, el);
          const clip = clips().then(c => c.renderAreaClip(el, part.areaBox(anchor.area!, part.boxOf(el))));
          void pick(anchor, () => clip).finally(() => mode.captured());
        },
        cancel: () => { mode.set(false); post({ type: "clax:cancel" }); },
      });
      commentsContext.liveChanged = () => mode.set(shellMode && !commentsContext.live);
      return {
        part,
        mode,
        cache: () => resolutions ??= new part.AnchorCache(document, undefined, meta.file, () => reflow()),
        reset: () => resolutions?.reset(),
      };
    });
    live.then(f, failed("comment"));
  };

  const updateFocus = (l: Live, flash = false) => {
    const f = focusId === null || commentsContext.live ? undefined : anchors.find(a => a.id === focusId);
    const r = f?.anchor.kind === "area" ? l.cache().resolve(f.id, f.anchor, f.sameVersion === true) : null;
    l.mode.showFocus(f && r ? placeOf(l, f.anchor, r) : null, flash);
  };
  const resolveAll = (l: Live, requestId: string | null) => {
    const resolved = l.cache();
    const results: AnchorResult[] = anchors.map(({ id, anchor, sameVersion }) => {
      const r = resolved.resolve(id, anchor, sameVersion === true);
      return r ? { id, found: true, method: r.method, rect: placeOf(l, anchor, r) } : { id, found: false, method: null, rect: null };
    });
    post({ type: "clax:anchors", requestId, results });
    updateFocus(l);
  };
  let raf = 0;
  const reflow = () => {
    // While the page anchors threads itself (comments.customAnchors), it
    // re-reports its placements, and the shell's anchors are not resolved.
    commentsContext.reflow?.();
    if (commentsContext.live || !anchors.length || raf) return;
    raf = requestAnimationFrame(() => { raf = 0; withComment(l => resolveAll(l, null)); });
  };
  addEventListener("scroll", reflow, { passive: true, capture: true });
  addEventListener("resize", reflow);

  // Bubble phase on the window: the page's own handlers run first, and comment
  // mode's capture-phase handler stops a click before it gets here.
  addEventListener("click", e => {
    const link = linkToHandOver(e, { welcomed, pageUrl: location.href, file: meta.file });
    if (!link) return;
    e.preventDefault();
    if (link.kind === "page") post({ type: "clax:navigate", file: link.file, ...(link.hash ? { hash: link.hash } : {}) });
    else followInPlace(link.hash, location);
  });

  // The shell keeps its address bar's fragment in step with the page's.
  addEventListener("hashchange", () => { if (welcomed) post(hashFor(location.hash)); });

  addEventListener("message", e => {
    const m = acceptFromShell(e, shellWin, origins);
    if (!m) return;
    shellOrigin = e.origin;
    switch (m.type) {
      case "clax:welcome":
        welcomed = true;
        shellMode = m.mode === "comment";
        if (shellMode) void clips().catch(() => {});
        withComment(l => l.mode.set(shellMode && !commentsContext.live));
        rpc.connect();
        post(hashFor(location.hash));
        break;
      case "clax:use-result": case "clax:call-result": case "clax:event": rpc.accept(m); break;
      case "clax:comment-mode":
        shellMode = m.on;
        if (m.on) void clips().catch(() => {});
        withComment(l => l.mode.set(shellMode && !commentsContext.live));
        break;
      case "clax:resolve-anchors": {
        if (commentsContext.live) break;
        // Only the latest request's anchors take effect, once the page has parsed.
        latestResolve = m;
        whenParsed(document, () => withComment(l => {
          if (latestResolve !== m || commentsContext.live) return;
          anchors = m.anchors;
          l.reset();
          resolveAll(l, m.requestId);
        }));
        break;
      }
      case "clax:scroll-to": whenParsed(document, () => withComment(l => {
        if (commentsContext.live) return;
        const r = l.part.resolveAnchor(document, m.anchor, undefined, meta.file, undefined, m.sameVersion === true);
        if (!r) return;
        if (m.anchor.kind === "area" && m.anchor.area) {
          // The drawn area is centred, not its element (often far taller).
          const a = placeOf(l, m.anchor, r);
          scrollBy({ left: a.x + a.w / 2 - innerWidth / 2, top: a.y + a.h / 2 - innerHeight / 2, behavior: "smooth" });
          setTimeout(() => l.mode.showFocus(placeOf(l, m.anchor, r), true), 350);
        } else {
          r.element.scrollIntoView({ block: "center", behavior: "smooth" });
          setTimeout(() => l.mode.flash(r.range ?? r.element), 350);
        }
      })); break;
      // The focused thread's area is outlined once the page has parsed.
      case "clax:focus": focusId = typeof m.id === "string" ? m.id : null; whenParsed(document, () => withComment(l => updateFocus(l))); break;
      case "clax:key": {
        const k = forwardedKey(m);
        if (k) withComment(l => l.mode.key(k.key, k.down));
        break;
      }
    }
  });
  post(helloFor(meta));
})();
```

The `import type` from `./anchor` does not affect the split, because the split guard ignores type-only imports and TypeScript erases them.

- [ ] **Step 4: Build the parts and point the bridge at them**

`web/vite.bridge-parts.config.ts`:

```ts
import { defineConfig } from "vite";
// The bridge's lazy parts: ES modules with content-hashed names under
// dist/_clax/bridge/, sharing chunks between them. The eager bridge build
// reads this build's manifest for their names.
export default defineConfig({
  build: {
    outDir: "dist/_clax/bridge", emptyOutDir: true, manifest: true, minify: true, sourcemap: false, target: "es2022",
    lib: { entry: { comment: "bridge/src/parts/comment.ts", clip: "bridge/src/parts/clip.ts", caps: "bridge/src/parts/caps.ts" }, formats: ["es"] },
    // `just watch` (CLAX_DEV=1) keeps stable names, so the watching bridge build,
    // which reads the names once, never points at a deleted file.
    rollupOptions: { output: process.env.CLAX_DEV ? { entryFileNames: "[name].js", chunkFileNames: "shared-[name].js" } : { entryFileNames: "[name]-[hash].js", chunkFileNames: "shared-[hash].js" } },
  },
});
```

In `scripts/dev.sh`, export `CLAX_DEV=1` near the top. Directly before the `for cfg in bridge shell` loop, add `(cd web && npx vite build -c vite.bridge-parts.config.ts)`, so the parts and their manifest exist before the bridge watcher reads them. Then make the loop `for cfg in bridge-parts bridge shell`.

`web/vite.bridge.config.ts`:

```ts
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

type Entry = { file: string; name?: string; isEntry?: boolean };
const manifest = JSON.parse(readFileSync("dist/_clax/bridge/.vite/manifest.json", "utf8")) as Record<string, Entry>;
const parts = Object.fromEntries(Object.values(manifest).filter(e => e.isEntry && e.name).map(e => [e.name!, e.file]));
for (const name of ["comment", "clip", "caps"]) if (!parts[name]) throw new Error(`the parts build has no ${name} entry; run vite build -c vite.bridge-parts.config.ts first`);

export default defineConfig({
  define: { __CLAX_PARTS__: JSON.stringify(parts) },
  resolve: { alias: { "clax-bridge-parts": fileURLToPath(new URL("./bridge/src/parts-url.ts", import.meta.url)) } },
  build: {
    outDir: "dist/_clax", emptyOutDir: false,
    lib: { entry: "bridge/src/bridge.ts", name: "claxBridge", formats: ["iife"], fileName: () => "bridge.js" },
    minify: true, sourcemap: false,
  },
});
```

In `web/package.json`: `"build": "node scripts/clean-dist.mjs && vite build -c vite.bridge-parts.config.ts && vite build -c vite.bridge.config.ts && vite build -c vite.shell.config.ts"`.

`web/vitest.config.ts`: add `resolve: { alias: { "clax-bridge-parts": fileURLToPath(new URL("./bridge/src/parts-static.ts", import.meta.url)) } }` (importing `fileURLToPath` from `node:url`).

`web/tsconfig.json`: add `"baseUrl": "."` and `"paths": { "clax-bridge-parts": ["bridge/src/parts-url.ts"] }` to `compilerOptions`. `tsc` then type-checks the production loader. The static loader has the same exported types, so the tests type-check against them.

In `web/scripts/bundle-size.mjs`, nothing changes. `bridge` is still the gzip size of `_clax/bridge.js`, which is now the eager part only.

In `crates/clax-server/src/routes/shell.rs` `static_file`, before the generic response, add:

```rust
    // The bridge's lazy parts: content-hashed ES modules, imported by pages in
    // sandboxed frames (an opaque origin, so the import is a CORS request).
    if path.starts_with("bridge/") {
        return Ok((
            [
                (header::CONTENT_TYPE, ct),
                // A debug build's parts keep one name while `just watch` rebuilds them.
                (header::CACHE_CONTROL, if cfg!(debug_assertions) { REVALIDATE } else { IMMUTABLE }.to_string()),
                (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".to_string()),
            ],
            f.data.into_owned(),
        )
            .into_response());
    }
```

Add a test for it to `crates/clax-server/tests/shell_entries.rs`: write `_clax/bridge/comment-abc.js` into the fixture dist, then GET `/_clax/bridge/comment-abc.js` and assert `access-control-allow-origin: *`, `content-type: text/javascript`, and `cache-control` equal to `no-cache` under `cfg!(debug_assertions)` and `public, max-age=31536000, immutable` otherwise (the pattern of the existing `only_the_url_naming_the_served_bytes_is_immutable` test).

- [ ] **Step 5: Tell the viewer when a part could not load**

In `web/shell/src/failure.ts`:

```ts
/** What the shell says when a lazy part of the bridge could not load in the page. */
export const PART_FAILED: Record<"comment" | "clip" | "caps", string> = {
  comment: "Comment mode could not load in this page",
  clip: "Screenshots could not load in this page",
  caps: "This page's capabilities could not load",
};
```

In `ArtifactController.onMessage`, add:

```ts
      case "clax:degraded": {
        if (!this.gate.open || !(m.part in PART_FAILED)) break;
        const text = PART_FAILED[m.part];
        this.noticeFor(text)(`${text}: ${typeof m.message === "string" ? m.message.slice(0, 200) : "unknown error"}`);
        if (m.part === "comment") this.set({ commenting: false });
        break;
      }
```

`web/e2e/bridge-parts.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { type FrameMode, contentFrame, openArtifact, publish, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

for (const mode of ["subdomain", "sandbox"] as FrameMode[]) {
  test(`loads comment mode lazily and says so when the page's CSP blocks it (${mode})`, async ({ page }) => {
    const ok = await publish(d.base, d.token, "Parts", { "index.html": "<h1 id=h>Parts</h1>" });
    const frame = await openArtifact(page, d.base, ok.artifact.id, 1, mode);
    const scripts = await frame.evaluate(() => performance.getEntriesByType("resource").map(e => e.name).filter(n => n.includes("/_clax/")));
    expect(scripts.some(n => /\/_clax\/bridge\/comment-[^/]+\.js$/.test(n))).toBe(true);
    await page.getByRole("button", { name: "Comment" }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");

    const blocked = await publish(d.base, d.token, "Strict", { "index.html": `<meta http-equiv="Content-Security-Policy" content="script-src 'unsafe-inline'"><h1>Strict</h1>` });
    await openArtifact(page, d.base, blocked.artifact.id, 1, mode);
    await expect(page.getByRole("alert")).toContainText("Comment mode could not load in this page");
    await contentFrame(page, blocked.artifact.id, 1);
  });
}
```

Run: `cd web && npx vitest run bridge/test/split.test.ts`
Expected: PASS.

- [ ] **Step 6: Record the sizes before and after**

The eager bridge's size before this task is `bridgeBaseline` in `web/perf/bundle-budget.json` (Task 11).

Run: `cd web && npm run build && node scripts/bundle-size.mjs; echo "exit=$?"; ls -l dist/_clax/bridge/; gzip -9 -c dist/_clax/bridge.js | wc -c`
Expected: the gallery and artifact entries are unchanged. `bridge` is well under the old budget: at most 30% of `bridgeBaseline`, or stop and report the numbers. `exit=0`. Put both numbers (before: `bridgeBaseline`; after: the measured eager size, plus the three parts' gzip sizes from `for f in dist/_clax/bridge/*.js; do printf '%s %s\n' "$f" "$(gzip -9 -c "$f" | wc -c)"; done`) in the commit message.

Run: `cd web && node scripts/bundle-size.mjs --record && git diff perf/bundle-budget.json`
Expected: `bridge` fell to the new size plus 10%; `bridgeBaseline` is unchanged.

- [ ] **Step 7: Bring the bridge tests along**

These tests change only as follows; every assertion stays:
- `bridge/test/comments.test.ts`: import `commentsContext` from `../src/comments-context`, and call `commentsLocals(rpc, config, { ctx: commentsContext, clip: () => import("../src/parts/clip") })` wherever it called `commentsLocals(rpc, config)`.
- `bridge/test/bridge-framed.test.ts`: import `commentsContext` from `../src/comments-context`.
- `bridge/test/use.test.ts`: import `localsFor` from `../src/caps` and `commentsContext` from `../src/comments-context`, and pass `locals: (n, r, c) => Promise.resolve(localsFor(n, r, c as never, { ctx: commentsContext, clip: () => import("../src/parts/clip") }))` in each `makeUse({ … })`.
- `bridge/test/bridge.test.ts`, `bridge-area.test.ts`, `bridge-framed.test.ts`, and any other file under `web/bridge/test` that sends the bridge a `clax:welcome`, `clax:comment-mode`, `clax:resolve-anchors`, `clax:scroll-to`, `clax:focus` or `clax:key` and asserts right after: import `settle` from `../src/parts-static`, make the `it` (or `beforeAll`) callback `async`, and put `await settle();` between the send and the first assertion that depends on it. Comment mode now arrives one module load after the message, as in the browser.

Run: `cd web && npx vitest run bridge; echo "exit=$?"`
Expected: `exit=0`. If an assertion still fails after `await settle()`, the order of work changed. Compare that handler with the Step 3 code, where every comment-mode action runs through `withComment` in arrival order. Do not loosen the assertion.

- [ ] **Step 8: Run the e2e suite, then enforce the targets**

Run: `cd web && npm run e2e; echo "exit=$?"`
Expected: `exit=0` in both modes. The area, gesture, clip and comments-capability specs matter most here, because they use every lazy part.

Set `"enforceTargets": true` in `web/perf/budget.json`, then:
Run: `cd web && CLAX_PERF_RECORD=budget npm run perf; echo "exit=$?"; cat perf/results.json | python3 -c "import json,sys; r=json.load(sys.stdin); print({m: r[m]['median'] for m in r})"`
Expected: `exit=0`. Both modes meet link → first paint ≤ 50% and link → comment ready ≤ 70% of the Task 1 baseline, and the budgets fell. If a target is missed, do not commit. Report the medians against the baseline to the person.

- [ ] **Step 9: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 10: Commit**

```bash
git add web/bridge/test/split.test.ts $(git diff --name-only -- web/bridge/test)
git add web/bridge/src/block.ts web/bridge/src/comments-context.ts web/bridge/src/parts/comment.ts web/bridge/src/parts/clip.ts web/bridge/src/parts/caps.ts web/bridge/src/parts/types.ts web/bridge/src/parts-url.ts web/bridge/src/parts-static.ts web/bridge/src/bridge.ts web/bridge/src/clip.ts web/bridge/src/area.ts web/bridge/src/use.ts web/bridge/src/caps/index.ts web/bridge/src/caps/comments.ts web/bridge/src/protocol.ts web/vite.bridge-parts.config.ts web/vite.bridge.config.ts web/vitest.config.ts web/tsconfig.json web/package.json web/shell/src/failure.ts web/shell/src/view/artifact-controller.ts web/e2e/bridge-parts.spec.ts crates/clax-server/src/routes/shell.rs crates/clax-server/tests/shell_entries.rs web/perf/budget.json web/perf/bundle-budget.json scripts/dev.sh
BEFORE=$(node -p "require('./web/perf/bundle-budget.json').bridgeBaseline")
AFTER=$(gzip -9 -c web/dist/_clax/bridge.js | wc -c | tr -d ' ')
git commit --no-gpg-sign -m "Load the bridge's comment mode, clips and capability members lazily

The eager bridge keeps window.claude, the hello, the channel and link
handover; comment mode with anchors and areas loads after the welcome,
clip rendering when comment mode turns on, and capability members on the
first claude.use(), as content-hashed ES modules the eager bridge names.
A part the page's CSP blocks is reported once and the shell says so.
Time-to-usable targets are now enforced." -m "Eager bridge: ${BEFORE} -> ${AFTER} gzip bytes."
```
