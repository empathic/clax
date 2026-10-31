# The UI kit in the shell and the extension: plan

Status: proposed, not started. Needs web/shell's owner to agree before any
of it is built. It goes on a separate branch off main, not on
goal/clax-sdk-kit's line (whose commits from 3fe88f2d on wait for the
licence ruling).

## Why

Clax's UI kit (web/sdk/ui: Surface, Quote, Thread, Button, Pin, Menu,
Dialog) is styled only through the token file
(web/sdk/tokens/clax.tokens.json), and the overlay draws with it. The shell
and the extension still draw the same things their own way, with
hand-written rules in web/shell/src/theme.css. The extension's side panel
and composer reuse the shell's components (through @empathic/clax-client),
so moving the shell onto the kit themes both at once. The shell is
AGPL-3.0-only and the kit LGPL-2.1-only; AGPL code may import LGPL code.

## Where it stands (measured at goal/clax-sdk-kit 8b5dbf20)

- Raw `<button>`s: about 90 shell and extension files, 300 or so buttons.
- Pins: web/shell/src/ui/Pins.svelte (theme.css's `.thread-pin` rules and
  the changelog's `.thread-pin.addressed` and `[data-v]` rules).
- Dialogs (`.prompt-backdrop`, `.prompt`, `role="dialog"`): PromptDialog,
  ApprovalDialog, CloneDialog, NewArtifactDialog, ShareDialog, KeysSheet,
  hosted/LegalPrompt, people/InviteDialog.
- Menus (`role="menu"`): MoreMenu, SendMenu, VersionMenu, Who,
  PeoplePanel, PermissionsPanel, q/InboxPage.
- Budgets (gzip bytes): gallery 31232, artifact 70000, extComposer 28672,
  extPanel 66560 (extPanel measured 63571 at 8b5dbf20: 2989 of room).

## Phases

Each phase is one commit, with its tests and a bundle-size run before the
next starts. None changes what a test selects without saying so in its
commit.

1. **Pins.** Pins.svelte draws each pin with the kit's Pin. theme.css drops
   its `.thread-pin` rules and the changelog's `.thread-pin.addressed`,
   which Pin's `addressed` holds; `.thread-pin[data-v]::after` (the
   version tag beside a pin) moves to Pins.svelte. The literal colours in
   those rules (#fff, #2f6f2a, #1c1b19) become tokens. Tests that select
   `.thread-pin` (comments, artifact, thread-owner, view/artifact-controller)
   keep working if Pin takes a `class` that Pins passes as `thread-pin`.
   Budget: artifact and extPanel draw pins. Pin adds about 0.3 kB gzip and
   theme.css loses about 0.4 kB, so roughly flat.
2. **Dialogs.** The eight dialogs render the kit's Dialog (title, body,
   actions). theme.css drops `.prompt-backdrop`, `.prompt` and their
   children's rules. PromptDialog's focus trap and Escape handling
   (today in artifact-controller.ts) become Dialog's. Tests that select
   `.prompt` or `role=dialog` (host, new-artifact, artifact-controller)
   need `.prompt` kept as Dialog's caller class, or to select by role.
   Budget: dialogs load with their pages (some lazily). Dialog is about
   0.6 kB gzip once, shared; theme.css loses about 0.3 kB. The gallery
   (31232, the tightest) carries NewArtifactDialog, so measure it first.
3. **Menus.** MoreMenu, SendMenu and VersionMenu use the kit's Menu; Who,
   PeoplePanel, PermissionsPanel and InboxPage's menus next. Menu brings
   the WAI-ARIA keys (arrows wrap, Home, End, Escape), which some of these
   handle by hand today. send-menu.test.ts and sidebar.test.ts select by
   role and label, so they should hold. Budget: about 0.5 kB gzip once.
4. **Buttons.** The broadest: about 300 buttons. Button would replace the
   global `button`, `button.primary`, `button.ghost`, `button.icon` rules
   in theme.css. Many e2e specs select `button.primary`, which
   Button keeps (`btn primary`). Done component by component, with the
   page bundles measured after each group. If the per-button cost of a
   component (vs. a plain element under a global rule) shows in the
   budgets, buttons stay global-rule elements whose rules theme.css takes
   from the token file, and only composite buttons (pills, toggles) use
   the kit.

## What theme.css keeps

The page layout (gallery, top bar, sidebar box, stage, cards, tags,
banners, phone tabs) stays the shell's: the kit holds the shared controls,
not the shell's layout. The generated token regions stay at its top.

## Risks

- **Budgets.** gallery has the least room. Each phase is measured before
  the next, and a phase that cannot fit is cut back (as phase 4 says), not
  given a bigger budget without the owner.
- **Tests that select by class.** Each phase keeps the selected classes as
  caller classes, or changes the test in the same commit and says so.
- **Licence.** Shell (AGPL) to kit (LGPL) is allowed. The extension
  (GPL-2.0-only) importing the AGPL client is the question Bobby is ruling
  on; this plan does not change it.
