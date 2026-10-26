# The `shell` Capability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a page that declares `{"shell": {}}` point at Clax's own controls (`highlight`), ask which are shown (`can`), and turn comment mode on or off from the viewer's click (`commentMode`), in the owner's and editors' views (spec item F).

**Architecture:** The daemon tells the shell whether the viewer may edit (`may_edit`, beside `may_fork`). The shell marks each control it can point at with `data-shell-control="<name>"`; a lazy capability chunk (`caps/shell.ts`) finds the first shown element for a name and rings it with a coach mark (`ui/coach-mark.ts`). The page side is a plain namespace (`CAPABILITY_METHODS.shell`); the contract gains `ClaxExtensions.Shell`. D's per-version approval is a seam, `approved(name, served)` in `caps/availability.ts`, which D wires.

**Tech Stack:** Rust (axum, rusqlite), Svelte 5 + TypeScript (vitest, Playwright), Bazel.

**Spec:** `docs/superpowers/specs/2026-10-26-quickstart-onboarding-design.md` §8 (F), with D4 and D8 (§2). Built in its own worktree, in parallel with D (§7), and independent of it.

## Decisions

- **Who gets `shell`:** the owner's shell (it holds the token) and a viewer whose `may_edit` is true (the rule `whoami`'s `can_edit` uses). Readers (Can view, Can comment, link guests) do not: the spec serves owner and editors, and a reader's tour has no Fork or Publish to point at. A world visitor is never framed by the shell, so never has it. A spectator has it (`commentMode` then rejects `unavailable`).
- **App mode (`?app=1`):** served. The app hides the shell's chrome (`app/app.css`), so every control reads as not shown (`can` and `highlight` resolve `false`), and `commentMode` drives the same mode the app's `enterCommentMode` does, which the app hears through its state. A page's tour degrades instead of failing `use()`. The app's inspect view rejects `commentMode` `unavailable`, as `enterCommentMode` does nothing there.
- **Controls and what each maps to:**

  | Name | Desktop (> 700 px) | Phone (≤ 700 px) |
  |---|---|---|
  | `comment` | top bar `button.comment` | the same (it stays in the bar) |
  | `threads` | top bar `button.threads` | the Threads button of `PhoneTabs` |
  | `versions` | the version button (`VersionMenu` `.vbtn`) | none (more menu): `false` |
  | `publish` | `Publish.svelte`'s `button.publish` or `button.linked` (owner of a local daemon) | none: `false` |
  | `fork` | top bar `button.fork` | none: `false` |
  | `send` | the first Send to agent in the threads sidebar (a card's `SendButton`, the unsent bar, the selection bar's Send), in document order | the same, while the Threads tab shows the sidebar |

  Clax never opens the more menu, a panel or a tab to reach a control: a control the viewer cannot see now is not shown.
- **Shown:** an element marked for that name that is connected, has a box (`getClientRects().length > 0`), passes `checkVisibility({opacityProperty: true, visibilityProperty: true})`, is not `disabled`, and has no `[inert]` or `[aria-hidden="true"]` ancestor. A disabled Comment (a live page, a spectator, Comments off, a draft shown) is not shown.
- **`can(control)`:** resolves whether `control` is shown now, by that rule; nothing is drawn. It answers the same as `highlight` would, without the mark.
- **Coach mark look:** a ring `position: fixed` around the control's box inflated by `COACH_PAD = 4` px, `border-radius: 10px`, `box-shadow: 0 0 0 2px var(--accent), 0 0 0 6px color-mix(in srgb, var(--accent) 30%, transparent)`, pulsing twice (none under `prefers-reduced-motion: reduce`), `pointer-events: none`, `aria-hidden="true"`, `z-index: 45` (above the menus at 30 and the connection notice at 40, below the prompt backdrop at 50). It follows the control on `resize` and the control's `ResizeObserver`; a `send` control is first scrolled to `block: "nearest"`.
- **Dismissal:** after `COACH_MS = 6_000` on the shell clock (`clock.ts` `after`), on any `pointerdown` in the shell window (capture phase; the control's own click passes through the ring and acts as usual), on Escape, when the control stops being shown at a reposition, when the next `highlight` draws, and when the frame's document leaves (`Handler.leave`, `reset`, `dispose`). One mark at a time in the window.
- **`highlight(control)`:** resolves `true` once the mark is drawn, `false` (nothing drawn) when the control is not shown. No gesture needed: a tour points on load and between steps, and the worst case is a ring the viewer sees.
- **`commentMode(on)` gesture tier:** the composer tier (`frameGesture`, judged at the call's arrival by `registry.ts`'s `composerAtArrival`): entering or leaving comment mode writes nothing and the viewer sees it, the same worst case as an opened composer. Resolves whether comment mode is on afterwards; leaving keeps the mode while the composer holds typed text (as `CommentsUi.exitMode`).
- **Rejection codes:** `invalid_argument` (a control name outside the six, or `on` not a boolean); `no_gesture` (`commentMode` without the viewer's recent input in the page); `unavailable` (`commentMode` where this view cannot comment: a live page, a spectator, Comments off, a draft shown, the app's inspect view); `capability_removed` for any other method (the host's existing default). Undeclared or not served: `use("shell")` resolves `null`, and a call rejects `not_granted` (the host's existing path).
- **The seam for D:** `approved(name: string, served: Served): boolean` in `web/shell/src/caps/availability.ts`, reading a new `Served.approved?: ReadonlySet<string>`; `undefined` (D not wired) approves every declared name. `isAvailable`'s `shell` case calls it. D fills `served.approved` with the names the owner approved for the shown version (in `CapabilityHost.handle`, as `served.device` is filled) and extends the call to the other capabilities. If D instead serves the approved declaration as `env.declared`, D deletes `approved` and its one call.
- **Declaration:** `shell` takes no options: `clax_core::capabilities::validate` refuses any key in it (`invalid_capabilities`), so options can be added deliberately later.
- **D8:** the namespace says only whether a named control is shown; it names no agent, harness, session or person.
- **E2E proof that a mark lands on the real control:** in Chromium, the mark's box encloses the control's box with each edge within `COACH_PAD + 1` px, `document.elementFromPoint` at the mark's centre is inside `[data-shell-control="<name>"]`, and a real click at that point toggles comment mode and removes the mark.

## Global Constraints

- Build and test only through Bazel recipes: `just test-affected` while working, `just check` before merge. Never `cargo build|test` or `pnpm install` in the worktree (AGENTS.md).
- No `unsafe`; no hard links; tests use injected clocks (`globalThis.claxTestClock.advance`), no sleeps or wall-clock budgets.
- No migration: F adds none.
- Doc comments and commit messages state the contract or change; write "ID" in prose.
- A capability added to the contract appears word-for-word in `docs/contract.md` "## Runtime capabilities" and all four `plugins/*/skills/clax/SKILL.md` (checked by `scripts/test-plugins.sh`), and `scripts/sync-app-prompt.sh` re-run.
- Before editing a file, `qualifier threads <path>` (`web/shell/src/ui/TopbarIsland.svelte` carries alternative `a648ffa6` on :76-78, Get in Clax on a phone; leave it as is). Record risks and deferred work as qualifier records, not TODOs.
- Independent of D: touch no approvals table, grant route or Permissions panel; the seam above is the only point of contact.
- After each task: append a line to the `shell-tour` idea's `log` in the quickstart artifact's db (`extensions/shell-tour`), with `work.commit`.

## Review Focus

- A page that calls `highlight` in a tight loop or on every animation frame: one ring in the window at a time, never a stack (F2 `a_second_mark_replaces_the_first`, F3 `a_hundred_highlights_leave_one_ring`).
- The control moving or vanishing under a mark: the window narrowed to phone width with `fork` ringed, the threads panel closed with `send` ringed: the ring follows a moved control and goes with a hidden one, never ringing empty space (F2 `a_mark_goes_when_its_control_stops_being_shown`, F4 `narrowing_to_a_phone_removes_a_ring_on_fork`).
- `commentMode` from page load or a timer (no click in the page), and `commentMode(false)` while the viewer has typed a comment: `no_gesture` with the mode unchanged; `false` leaves the mode on and the text in place (F3 tests, F4 `comment_mode_needs_a_click_in_the_page`).
- A viewer who is not owner or editor (Can comment, a link guest), and the page in the Clax app: `use("shell")` is `null` for the first; in app mode every control reads `false` and `commentMode` still works (F3 `shell_is_served_to_owner_and_editors_only`, `app_mode_shows_no_controls`).
- The frame navigating or reloading while a ring shows: the ring goes with the document (F3 `leaving_the_document_dismisses_the_mark`).

---

### Task 1 (F1): The daemon says who may edit, and `shell` takes no options

**Files:**
- Modify: `crates/clax-server/src/routes/artifacts.rs:1020-1035` (stamp `may_edit`; add `browser_edits` beside `fork_refusal`)
- Modify: `crates/clax-server/src/shell/boot.rs:336-343` (stamp `may_edit` in the bootstrap)
- Modify: `crates/clax-core/src/capabilities.rs:24-60` (`validate`: `shell` must be `{}`; doc comment names it)
- Modify: `web/shell/src/api.ts:29` (`Artifact.may_edit?: boolean`)
- Modify: `docs/contract.md` (~:1279, beside `may_resolve`: `GET /api/artifacts/<id>` and the bootstrap carry `may_edit`)
- Test: `crates/clax-core/src/capabilities.rs` (unit), `crates/clax-server/tests/api_app_bundles.rs` (beside `may_resolve_is_said_and_a_comments_switch_change_announced`, :933), `crates/clax-server/tests/shell_boot.rs` (:185-195)

**Interfaces:**
- Produces: `pub(crate) fn browser_edits(st: &Store, caller: &Caller, member: Option<&MemberRights>, hosted: bool, aid: &str) -> clax_core::Result<bool>`: `Caller::Owner | Caller::OwnerAgent` → `!hosted || MemberRights::owner(st)?.allows_at(st, (RightType::Artifacts, "update"), aid)?`; a caller with member rights → `allows_at((Artifacts, "update"), aid)`; anyone else `false` (the rule of `whoami`'s `can_edit`, `routes/site_api.rs:497-515`).
- Produces: `artifact.may_edit: bool` on `GET /api/artifacts/<id>` and the bootstrap's `artifact.artifact`.

- [ ] **Step 1: Write the failing unit tests** in `capabilities.rs`: add `json!({"shell": {}})` to `accepts_the_contract_declarations`; new `shell_takes_no_options`: `validate(&json!({"shell": {"controls": ["fork"]}}))` is `Err` with code `invalid_capabilities` and a message containing `capabilities.shell takes no options`.
- [ ] **Step 2: Write the failing integration tests.** `may_edit_is_said_to_owner_and_editors_only` (hosted `Site`: owner, Sam shared `editor`, Kim shared `commenter`): `GET /api/artifacts/<aid>` gives `may_edit` `true`, `true`, `false`. In `shell_boot.rs`, beside the `may_resolve` assertion: a local guest's bootstrap has `may_edit == false`, and the owner's (token cookie path the file already uses for the owner) `true`.
- [ ] **Step 3: Run** `just test-affected`; expect the three new tests to FAIL (`may_edit` is `null`; `{"shell": {"controls": …}}` accepted).
- [ ] **Step 4: Implement** the `validate` check, `browser_edits`, and both stamps (`boot.rs` passes `Caller::Anonymous` when the caller cannot be told, as its `may_resolve` does). Add `may_edit` to `api.ts` and the contract paragraph.
- [ ] **Step 5: Run** `just test-affected`; expect PASS for `clax-core` units and the `clax-server` integration shards.
- [ ] **Step 6: Commit**

```bash
git add crates/clax-core/src/capabilities.rs crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/shell/boot.rs crates/clax-server/tests/api_app_bundles.rs crates/clax-server/tests/shell_boot.rs web/shell/src/api.ts docs/contract.md
git commit -m "Tell the shell whether the viewer may edit an artifact, and take no options in a shell declaration"
```

### Task 2 (F2): Clax's controls are marked, found, and ringed

**Files:**
- Create: `web/shell/src/ui/shell-controls.ts`
- Create: `web/shell/src/ui/coach-mark.ts`
- Modify: `web/shell/src/ui/TopbarIsland.svelte:68-77` (`data-shell-control` on `button.comment`, `button.threads`, `button.fork`)
- Modify: `web/shell/src/ui/VersionMenu.svelte:23` (`data-shell-control="versions"` on `.vbtn`)
- Modify: `web/shell/src/ui/Publish.svelte:199,202` (`data-shell-control="publish"` on both buttons)
- Modify: `web/shell/src/ui/PhoneTabs.svelte:9` (`data-shell-control="threads"` on the Threads button)
- Modify: `web/shell/src/ui/SendButton.svelte` (its send button) and `web/shell/src/ui/Sidebar.svelte:252` (the unsent bar's button): `data-shell-control="send"`
- Test: `web/shell/src/ui/shell-controls.test.ts`, `web/shell/src/ui/coach-mark.test.ts`

**Interfaces:**
- Produces: `export const SHELL_CONTROLS = ["comment", "threads", "versions", "publish", "fork", "send"] as const; export type ShellControl = (typeof SHELL_CONTROLS)[number]; export const isShellControl = (v: unknown): v is ShellControl`.
- Produces: `export function isShown(el: Element): boolean` (the Decisions' rule) and `export function findControl(c: ShellControl, doc: Document = document, shown: (el: Element) => boolean = isShown): HTMLElement | null`: the first `[data-shell-control="<c>"]` in document order that `shown` accepts.
- Produces: `export const COACH_MS = 6_000; export const COACH_PAD = 4; export function showCoachMark(el: HTMLElement, shown: (el: Element) => boolean = isShown): () => void`: draws the ring (`div.coach-mark`, appended to `document.body`, its style injected once as `<style data-coach-mark>`), replacing any ring already drawn, and returns an idempotent dismiss.

- [ ] **Step 1: Write the failing tests** in `shell-controls.test.ts` (jsdom; `shown` stubbed by a set of elements, since jsdom lays nothing out):
  - `finds_the_first_shown_element_for_a_name`: two `[data-shell-control="threads"]` (top bar hidden, phone tab shown) → the phone tab.
  - `a_disabled_or_inert_control_is_not_shown`: `isShown` of a `disabled` button, and of a button inside `[inert]` and inside `[aria-hidden="true"]`, is `false` with `getClientRects` stubbed non-empty and `checkVisibility` stubbed `true`.
  - `no_marked_element_is_null`: `findControl("fork")` on an empty body → `null`.
  - `every_name_is_marked_somewhere`: render `TopbarIsland`, `PhoneTabs`, `VersionMenu` and a `SendButton` with test props (as their existing tests do) and assert `[data-shell-control=<name>]` exists for `comment`, `threads`, `versions`, `fork`, `send`; `publish` is asserted by grepping `Publish.svelte`'s source for `data-shell-control="publish"` twice (it needs a daemon to mount).
- [ ] **Step 2: Write the failing tests** in `coach-mark.test.ts` (clock moved with `globalThis.claxTestClock.advance`):
  - `a_coach_mark_rings_its_control_and_goes_after_six_seconds`: control rect `{x: 100, y: 10, width: 80, height: 32}` → one `.coach-mark` with `left: 96px; top: 6px; width: 88px; height: 40px`; after `advance(5_999)` it is there, after `advance(1)` gone.
  - `a_press_anywhere_in_the_window_dismisses_it`: `pointerdown` on `document.body` → gone; `keydown` Escape on a fresh mark → gone.
  - `a_second_mark_replaces_the_first`: two `showCoachMark` calls → one `.coach-mark`, ringing the second control.
  - `a_mark_goes_when_its_control_stops_being_shown`: `shown` turns `false`, `window` dispatches `resize` → gone.
  - `dismiss_is_idempotent`: calling the returned dismiss twice, and after the timeout, throws nothing and leaves no `.coach-mark` and no listener (a later `pointerdown` changes nothing).
- [ ] **Step 3: Run** `bazel test //web:unit`; expect FAIL (modules missing).
- [ ] **Step 4: Implement** `shell-controls.ts`, `coach-mark.ts` and the attributes. No change to what any control shows or does.
- [ ] **Step 5: Run** `bazel test //web:unit //web:typecheck //web:lint //web:bundle_size`; expect PASS (the two modules are imported by nothing in the entry yet).
- [ ] **Step 6: Commit**

```bash
git add web/shell/src/ui/shell-controls.ts web/shell/src/ui/shell-controls.test.ts web/shell/src/ui/coach-mark.ts web/shell/src/ui/coach-mark.test.ts web/shell/src/ui/TopbarIsland.svelte web/shell/src/ui/VersionMenu.svelte web/shell/src/ui/Publish.svelte web/shell/src/ui/PhoneTabs.svelte web/shell/src/ui/SendButton.svelte web/shell/src/ui/Sidebar.svelte
git commit -m "Mark the shell's controls a page may point at, and ring one with a coach mark"
```

### Task 3 (F3): The `shell` capability, its contract and its docs

**Files:**
- Create: `web/shell/src/caps/shell.ts`, `web/shell/src/caps/shell.test.ts`
- Modify: `web/shell/src/caps/availability.ts` (`Served.edits`, `Served.approved`, `approved`, `case "shell"`; header comment)
- Modify: `web/shell/src/caps/host.ts` (`CapEnv.mayEdit?: boolean`, `CapEnv.commentMode?(on: boolean): boolean | null`; `handle` sets `this.served.edits = env.mayEdit === true`)
- Modify: `web/shell/src/caps/registry.ts` (`shell: lazyHandler(() => import("./shell").then(m => m.shellHandler), composerAtArrival)`)
- Modify: `web/shell/src/view/artifact-controller.ts:~720` (env gets `mayEdit: data.artifact.may_edit === true`, `commentMode: on => this.pageCommenting(on)`), `:1656` (`mayComment` guard shared with `toggleComment`; new `pageCommenting`)
- Modify: `web/bridge/src/capabilities.ts` (`shell: ["highlight", "commentMode", "can"]`, commented as Clax's own, `clax-extensions.d.ts` `Shell`)
- Modify: `web/contract/clax-extensions.d.ts` (`namespace Shell` after `Net`)
- Modify: `docs/contract.md` "## Runtime capabilities" (a `shell` bullet after `net`) and a new "### Pointing at Clax's controls (`shell`)" after "### Fetching from the network (`net`)" (the Decisions' control table, dismissal, codes); the same bullet in `plugins/{clax,claude-code,clax-grok,pi}/skills/clax/SKILL.md`; then `scripts/sync-app-prompt.sh`
- Test: `web/shell/src/caps/shell.test.ts`, `web/shell/src/caps/host.test.ts`, `web/shell/src/caps/registry.test.ts`, `web/bridge/test/capabilities.test.ts`, `web/shell/src/view/artifact-controller.test.ts` (or the controller test file that covers `toggleComment`: `grep -l toggleComment web/shell/src`)

**Interfaces:**
- Consumes: F1 `Artifact.may_edit`; F2 `ShellControl`, `isShellControl`, `findControl`, `showCoachMark`.
- Produces: `export function approved(name: string, served: Served): boolean` and `Served = { …; edits?: boolean; approved?: ReadonlySet<string> }` (the seam D wires).
- Produces: `export type ShellDeps = { find(c: ShellControl): HTMLElement | null; mark(el: HTMLElement): () => void }`, `export function makeShellHandler(deps: ShellDeps): HandlerFactory`, `export const shellHandler: HandlerFactory` (deps `findControl(c, document)` and `showCoachMark`).
- Produces: `ArtifactController.mayComment(s?: ViewState): boolean` (`!live && !spectator && !commentsOff && showDraft === null`) and `ArtifactController.pageCommenting(on: boolean): boolean | null` (`null` when `!mayComment()` or `inspectQuery(location.search)`; on → `commenting: true`; off → leaves unless `composerText.trim()`; returns `commenting` afterwards).
- Produces (contract): 

```ts
namespace Shell {
  type Control = "comment" | "threads" | "versions" | "publish" | "fork" | "send";
  type ErrorCode = "invalid_argument" | "no_gesture" | "unavailable";
  interface Shell {
    highlight(control: Control): Promise<boolean>;
    commentMode(on: boolean): Promise<boolean>;
    can(control: Control): Promise<boolean>;
  }
}
```

- [ ] **Step 1: Write the failing handler tests** in `shell.test.ts` (fake `deps`: `find` from a map, `mark` recording calls and returning a dismiss spy; `env.commentMode` a spy):
  - `highlight_rings_a_shown_control_and_resolves_true`: `find("comment")` → el; `call("highlight", ["comment"])` → `true`, `mark` called with el.
  - `highlight_of_a_control_not_shown_draws_nothing`: `find` → `null`; resolves `false`, `mark` not called.
  - `can_says_whether_shown_and_draws_nothing`: `true`/`false` by `find`; `mark` never called.
  - `an_unknown_control_or_a_non_boolean_is_invalid_argument`: `highlight(["menu"])`, `can([])`, `commentMode(["yes"])` reject `{code: "invalid_argument"}`.
  - `comment_mode_needs_the_viewers_gesture`: gesture `{composer: false}` → rejects `no_gesture`, `env.commentMode` not called; `{composer: true}` → resolves what `env.commentMode(true)` returned.
  - `comment_mode_where_the_view_cannot_comment_is_unavailable`: `env.commentMode` returns `null` → rejects `unavailable`.
  - `leaving_the_document_dismisses_the_mark`: after a highlight, `leave()`, `reset()` and `dispose()` each call the dismiss.
  - `a_hundred_highlights_leave_one_ring`: with the real `showCoachMark` and a `find` returning one stubbed-visible button, 100 highlights → one `.coach-mark`.
  - `an_unknown_method_is_capability_removed`.
- [ ] **Step 2: Write the failing availability and registry tests.**
  - `host.test.ts` `shell_is_served_to_owner_and_editors_only`: declared `{shell: {}}`: `use` granted with `token: "tok"`; with `token: null, mayEdit: true` granted; with `token: null` and no `mayEdit` refused; undeclared refused.
  - `host.test.ts` `shell_waits_for_the_owners_approval_once_d_says`: `approved("shell", {})` true; `approved("shell", {approved: new Set(["db"])})` false; `new Set(["shell"])` true; `isAvailable("shell", {shell: {}}, true, {approved: new Set()})` false.
  - `registry.test.ts` `shells_comment_mode_is_judged_when_the_call_arrives`: as the file's existing lazy-gesture tests do: a call with `at.verdict = "ok"` made before the chunk's gate opens, the verdict then set to `"no_gesture"`, the gate opened → the handler got `{composer: true}`.
- [ ] **Step 3: Write the failing controller tests.** `page_comment_mode_follows_the_comment_buttons_guard`: on a normal view `pageCommenting(true)` → `true` and `state.commenting`; on a view with a spectator, with Comments off, and with `showDraft` set → `null`, mode unchanged. `page_comment_mode_keeps_a_typed_comment`: with `composerText = "half"`, `pageCommenting(false)` → `true`, still commenting. `toggleComment` keeps its behaviour (the file's existing tests pass unchanged).
- [ ] **Step 4: Write the contract.** Add `namespace Shell` to `clax-extensions.d.ts`, its doc comment saying: declared `{"shell": {}}`; served in the owner's and editors' views (`null` elsewhere); `highlight` rings the control 6 s or until the viewer presses anywhere in the Clax window, one ring at a time, `false` when the control is not shown (at phone width `versions`, `publish` and `fork` are in the more menu; `send` needs the threads shown); `can` answers the same without drawing; `commentMode` only from a click or key in the page; the error codes as in Decisions. Add `["shell", members(ext, "interface Shell {")]` to the `it.each` list in `web/bridge/test/capabilities.test.ts`.
- [ ] **Step 5: Run** `bazel test //web:unit`; expect FAIL in all the tests above (no `shell.ts`, no `case "shell"`, no `CAPABILITY_METHODS.shell`).
- [ ] **Step 6: Implement** `shell.ts`, the availability case and seam, the registry entry, the `CapEnv` fields, the host's `served.edits`, the controller's `mayComment` and `pageCommenting` (`toggleComment` keeps its own leave, which ignores typed text, and uses `mayComment` for its guard), and `CAPABILITY_METHODS.shell`. `localsFor` needs no case: every member is a plain shell call.
- [ ] **Step 7: Write the docs.** In `docs/contract.md` "## Runtime capabilities", after the `net` bullet, and identically in the four skills:

```markdown
- `shell` (Clax's own): `highlight(control)` rings one of Clax's controls,
  `"comment"`, `"threads"`, `"versions"`, `"publish"`, `"fork"` or
  `"send"`, for 6 s or until the person presses anywhere in the Clax
  window, and resolves `false` when that control is not shown to them now
  (on a phone, versions, publish and fork are in a menu); `can(control)`
  answers the same without drawing; `commentMode(on)`, from a click
  handler only, turns comment mode on or off. Declare `{"shell": {}}`.
  Only the owner and editors get it: elsewhere `use("shell")` resolves
  `null`. Types: `ClaxExtensions.Shell` in `clax-extensions.d.ts`.
```

  Then the detail section with the Decisions' control table and codes, and run `scripts/sync-app-prompt.sh`.
- [ ] **Step 8: Record** with qualifier (one `record --stdin` batch, dry-run first): an `alternative` on `web/shell/src/caps/availability.ts` (the `shell` case's lines), "Serve `shell` to readers (Can view, Can comment) for shared tours; not taken: spec §8 serves owner and editors", tagged `revisit:shared-tour`; a `waiver` on the same lines, "`can("send")` tells a page whether a Send to agent is shown, so whether an open unsent thread or a live agent exists; it names neither (D8)".
- [ ] **Step 9: Run** `bazel test //web:unit //web:typecheck //web:lint //web:bundle_size //apps/clax_app:prompt_sync` and `just plugin-test`; expect every target PASSED and `test-plugins.sh` to print no `FAIL` (its `same_section "Runtime capabilities"` passes).
- [ ] **Step 10: Commit**

```bash
git add web/shell/src/caps/shell.ts web/shell/src/caps/shell.test.ts web/shell/src/caps/availability.ts web/shell/src/caps/host.ts web/shell/src/caps/host.test.ts web/shell/src/caps/registry.ts web/shell/src/caps/registry.test.ts web/shell/src/view/artifact-controller.ts web/shell/src/view/*.test.ts web/bridge/src/capabilities.ts web/bridge/test/capabilities.test.ts web/contract/clax-extensions.d.ts docs/contract.md plugins/*/skills/clax/SKILL.md apps/clax_app/assets/prompt web/shell/src/caps/.qual
git commit -m "Add the shell capability: a page points at Clax's controls and turns comment mode on from a click"
```

### Task 4 (F4): Browser tests: the ring lands on the real control

**Files:**
- Create: `web/e2e/shell-capability.spec.ts` (Playwright project `rest`)

**Interfaces:**
- Consumes: F3's `shell` namespace; F2's `COACH_PAD = 4`, `.coach-mark`, `data-shell-control`; e2e helpers `publishWith`, `openArtifact`, `reach` (`fixtures.ts`), `advance`, `quietEval` (`time.ts`), `hostedDaemon`, `ownerOf`, `memberOf`, `rightsOn` (`hosted-helpers.ts`).

The test page (`TOUR`) declares `{shell: {}}` and has one button per call, `#hl-<name>`, `#can-<name>`, `#on`, `#off`, writing `JSON.stringify(result or {code})` into `#out`; on load it calls `shell.commentMode(true)` once and writes `{load: <code>}` into `#load`.

- [ ] **Step 1: Write the failing tests.**
  - `a_highlight_rings_the_real_comment_button`: desktop viewport 1280×800; `reach` then click `#hl-comment` → `#out` is `true`; the `.coach-mark` box encloses `button.comment`'s box, each edge within `COACH_PAD + 1` px; `quietEval` of `document.elementFromPoint(cx, cy).closest('[data-shell-control="comment"]') !== null` at the mark's centre is `true`; `page.mouse.click(cx, cy)` → `button.comment[aria-pressed="true"]` and no `.coach-mark`.
  - `a_ring_goes_after_six_seconds`: `#hl-threads` → ring on `button.threads`; `advance(page, 5_999)` → still there; `advance(page, 1)` → gone.
  - `every_desktop_control_the_owner_sees_is_ringable`: for `comment`, `threads`, `versions`, `fork`, `publish`: `#can-<name>` → `true`, and `#hl-<name>` rings the element `[data-shell-control=<name>]` that is visible (box check as above).
  - `on_a_phone_the_menu_controls_read_false_and_threads_is_the_tab`: viewport 390×800: `versions`, `publish`, `fork` → `can` `false`, `highlight` `false`, no `.coach-mark`; `threads` → ring encloses `.phone-tabs [data-shell-control="threads"]`.
  - `narrowing_to_a_phone_removes_a_ring_on_fork`: desktop, `#hl-fork` → ring; `page.setViewportSize({width: 390, height: 800})` → no `.coach-mark`.
  - `comment_mode_needs_a_click_in_the_page`: `#load` reads `{"load":"no_gesture"}` and `button.comment` is not pressed; `reach` + click `#on` → `true` and pressed; click `#off` → `false` and not pressed.
  - `a_commenter_gets_no_shell_and_an_editor_does`: hosted daemon; owner publishes `TOUR`; a member with `rightsOn(aid)` (Can comment) reads `use("shell") === null` (`#out` `"null"` from a load-time probe); the same page shared with a member at editor rights reads an object with `can,commentMode,highlight`.
- [ ] **Step 2: Run** `bazel test //web:e2e_rest`. F1 to F3 are in place, so these tests verify rather than drive: expect PASS, and read any FAIL as a gap jsdom hid.
- [ ] **Step 3: Fix** each such gap (ring geometry, the `ResizeObserver` follow, `checkVisibility` on the phone layout) in F2's or F3's files, with a unit test that pins it, and run Step 2 again.
- [ ] **Step 4: Run** `bazel test //web:e2e_rest //web:e2e_gesture //web:unit`; expect PASSED (the gesture project proves the ring, `pointer-events: none` and above the top bar, changed no gesture verdict).
- [ ] **Step 5: Run** `just check` once; expect every lane PASSED. A lane failing in code F did not touch is rerun alone and both results reported.
- [ ] **Step 6: Commit**

```bash
git add web/e2e/shell-capability.spec.ts
git commit -m "Test in Chromium that a page's highlight rings the real Clax control, on desktop and phone"
```
