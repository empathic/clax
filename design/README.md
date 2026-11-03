# Clax's design tokens

`design/tokens.json` is the one source for Clax's colours, type, space, layout,
radii, elevation and motion. `scripts/sync-tokens.py` writes each surface's tokens
from it. `scripts/sync-tokens.py --check` (`//design:tokens_sync`) fails while a
written file differs from what the source writes, or while one of the rules below
is broken.

| Output | What it gets |
| --- | --- |
| `web/shell/src/theme.css`, between `tokens:begin` and `tokens:end` | the shell's colours (both tiers, minus the app-only roles in `shellOmit`), tags, shadows, fonts, `--t` and radii; a dark block sets only what dark changes |
| `web/overlay/src/tokens.css`, between the markers | the overlay's `:host`: exactly the `overlayHost` names |
| `web/shell/src/ui/LiveWriters.svelte`, between the markers | `--live-<n>` |
| `apps/clax_app/assets/kit/kit.css`, between the markers | the kit's public `--k-*` (see "The kit's API") |
| `apps/clax_app/home/tokens.css` | the home app's custom properties |
| `apps/clax_app/lib/ui/tokens.g.dart` | Flutter's palettes, `ClaxLive`, type, space, layout, radii, elevation and motion |

Edit `design/tokens.json`, then run `python3 scripts/sync-tokens.py`. Never edit
a generated region by hand.

## Tiers

- **Semantic** (`color`): each role has a value per theme (`{"light", "dark"}`),
  or is an alias to another role (a string). Components read only these roles.
- **Over the artifact** (`over`): the pins, the addressed pin, the pin an agent is
  on, the pin shadow and the frame. They sit on the artifact, whose colours Clax
  does not set, so each one has **one value for every theme**. The schema
  enforces this: an `over` entry is a string. The checker fails on an entry with a
  value per theme, and on a name that is in both tiers. The outputs set the tier
  in `:root` only, never in a dark block.
- **Tags** (`tags`) and **live colours** (`live`): palettes keyed by their CSS
  names (`--tag-<key>-<bg|fg|bd>`, `--live-<n>`). Their contrast pairs are
  implied: each tag's text on its tint, and each live colour on `--card` and `--bg`
  in its own theme. The light set is also checked on the frame, since the frame
  draws it in both themes. `LIVE_PALETTE` in `web/shell/src/view/live-layers.ts`
  must be `live.light`.

## Rules the checker enforces

- Every pair under `contrast`, and every implied pair, meets its minimum (WCAG
  2.x) in each theme it names. It checks with and without more contrast. A
  translucent colour is composited on its pair's background first.
- Every `*-ink` and `on-*` colour is the text of at least one pair. An ink is text,
  so it needs a ground. An alias counts through the colour it names.
- Every `color-mix()` in the kit, `home.css` and the map sits inside `@supports`.
- The kit's API only grows (below).

`web/overlay/src/tokens.test.ts` adds one more rule: every `var(--…)` the overlay
reads is declared on its `:host`. `web/shell/src/theme.test.ts` adds another: no
`.svelte` file under `web/` writes a hex colour outside a generated region.

## The overlay follows the system; the shell follows the page

The shell's tokens follow `data-theme` on `<html>` when the page sets it, and
otherwise the system's `prefers-color-scheme`. The overlay is the reporter that
the SDK and extension mount in a shadow root on someone else's page, and it
follows **only** `prefers-color-scheme`. That page's `data-theme` belongs to the
page, not to Clax, so the overlay does not read it.

The overlay's `:host` uses `all: initial`. That resets every property except
custom properties, which still inherit from the page. So `:host` declares every
custom property the overlay reads, and a page's own `--muted` never reaches it.

## The kit's API

The kit's `--k-*` names are a public API. Published pages read them, and every
published version is kept. `design/kit-api.json` is the ledger: every name ever
published, with the kit API version that added it.

- **Additive only.** A name in the ledger that the `kit` map no longer sets fails
  the check. To rename or retire one, keep it in `kitAliases`
  (`"--k-old": "--k-new"`). kit.css then sets it as `var(--k-new)`.
- **New names** go into the ledger at `version + 1`, and `version` moves with
  them. A name that the ledger lacks fails the check. So does a `version` that no
  name carries.
- **The stamp.** kit.css sets `--k-version` to the ledger's version.
- **Review keeps the ledger append-only.** A single commit could delete a name
  from both the map and the ledger. The checker cannot see that, so a reviewer
  must refuse any removal from `kit-api.json`.

## Seams kept, and why

- **`accent` is an alias of `agent`, and `accent-ink` of `agent-ink`.** Clax's
  accent *is* the agents' green, so each theme sets that green once.
- **In dark, `you-ink` equals `you` and `agent-ink` equals `agent`.** The ink
  roles are text and must meet 4.5:1 on every ground. The fill roles need not. In
  dark the fill already passes, so the two values coincide. They stay separate
  roles, so a later change to the fill cannot fail the text.
- **The shell's `--t` is motion's `t-fast` (120 ms), but the home app's is
  motion's `t` (200 ms).** Each name keeps what its surface always had. Renaming
  either would change motion on that surface.
- **`pink` is gone.** `selection-bg` is the colour. No surface read `--pink`
  except as `selection-bg`'s alias.
