# Clax's design tokens

Clax draws its overlay, and the shell, in one set of roles: `bg`, `card`,
`fg`, `muted`, `border`, `accent`, `focus`, `danger`, `you`, `agent` and the
rest. A design system adopts Clax by giving those roles its own values. This
directory is the contract for doing that.

| File | What it is |
| --- | --- |
| `clax.tokens.json` | The source: Clax's palette, and every role in every profile, as [W3C Design Tokens](https://www.designtokens.org/tr/drafts/format/) (DTCG). Its `$extensions["dev.empathic.clax"].contrast` is the contrast contract. |
| `dist/clax.css` | Every role as a `--clax-<role>` custom property, with the profile chosen for the page (below). Generated. |
| `dist/clax.theme.tokens.json` | Every profile's roles, resolved, in Clax's role names: a template for the SDK's `theme` option. Generated. |
| `dist/clax.theme.ts` | The same as a module (`claxTheme`), and the `ClaxRole` type. Generated. |
| `examples/` | The two worked examples below. |

`web/scripts/build-tokens.ts` writes `dist/` with
[Style Dictionary](https://styledictionary.com) (pinned in
`web/package.json`; a build tool only, nothing of it ships). Edit
`clax.tokens.json`, then run `node scripts/build-tokens.ts` in `web/`.
`sdk/src/build-tokens.test.ts` fails while `dist/` differs from a build.

## Profiles

Every profile defines every role.

| Profile | When |
| --- | --- |
| `theme.light` | By default, on `:root`, `[data-theme="light"]` and `.clax-light`. |
| `theme.dark` | `prefers-color-scheme: dark` on a root not marked light, and on `[data-theme="dark"]` and `.clax-dark`. |
| `theme.light-more`, `theme.dark-more` | `prefers-contrast: more`, over the light or dark profile chosen as above: muted text and hairlines stronger. |
| `theme.forced` | `forced-colors: active`: every role a CSS system colour (`Canvas`, `CanvasText`, `Highlight`, …), so the user's palette is drawn. |
| `base` | Every profile: type (`font`, `mono`, `text-*`, `leading`), space (`space-*`, `gutter`), radius (`radius*`) and motion (`duration`, `ease`). |
| `reduced-motion` | `prefers-reduced-motion: reduce`: `duration` is `0ms`. |

The contract asks WCAG 2.x AA of each text pair (4.5:1) and 3:1 of each
border and focus ring against its background, in light, dark and both
more-contrast profiles. Forced colours are checked as pairs of system
colours the platform makes contrast (`CanvasText` on `Canvas`,
`HighlightText` on `Highlight`, …).

## Adopting the tokens

There are three ways in. Each leaves the roles you don't set at Clax's
values.

### 1. Map your tokens onto `--clax-*` (CSS)

The overlay reads each role from `--clax-<role>` first, falling back to
Clax's value: its stylesheet holds `--bg: var(--clax-bg, #f7f7f5)`. Custom
properties inherit into the overlay's shadow root, so a page sets them on
any ancestor of the overlay, usually `:root`:

```css
:root {
  --clax-bg: var(--my-surface);
  --clax-fg: var(--my-text);
  --clax-accent: var(--my-brand);
}
```

A `var()` resolves on the element that declares it. If your own tokens
already switch with your `data-theme`, class or media queries, these follow
them, and one rule is enough.

**Worked example: Changeling.** `examples/changeling.css` maps Changeling's
tokens (empathic/changelist, `web/src/styles/tokens.css`) onto Clax's
roles. `sdk/src/adoption.test.ts` checks it against the contract with
Changeling's light and dark values. That check is what moved muted text to
Changeling's `--gutter`: its `--muted` is 4.4:1 on Clax's comment highlight,
below AA.

### 2. Pass a token file to the SDK (`theme`)

`overlay({ …, theme })` takes a DTCG file whose `theme.light`, `theme.dark`
and `base` groups use Clax's role names (web/sdk/README.md). Start from
`dist/clax.theme.tokens.json`, or alias the roles to your own palette.

**Worked example: a viewer's token file.**
`examples/fold-viewer.theme.tokens.json` aliases Clax's roles to the role
names of the fold viewer (`paper`, `ink`, `soft`, `line`, `accent`, `warn`).
Its colours are illustrative. It is tested the same way.

### 3. Load Clax's palette (`dist/clax.css`)

`dist/clax.css` sets every `--clax-*` property itself, for every profile. A
page loads it to draw the overlay in Clax's own palette while letting its
own switch choose light or dark: `data-theme="light"` or `"dark"`, or
`.clax-light` / `.clax-dark`, on the root or on any element. Within each
profile an alias stays a link (`--clax-accent: var(--clax-agent)`), so
setting `--clax-agent` after it moves the accent too.

## What a page cannot override

- **Forced colours.** Under `forced-colors: active` the overlay uses
  `theme.forced`, whatever the page sets. Its forced-colours block reads no
  `--clax-*` property, and a `theme` option sits inside
  `@media not all and (forced-colors: active)`.
- **Reduced motion.** Under `prefers-reduced-motion: reduce` the overlay's
  `--duration` is `0ms`, whatever `--clax-duration` says.
- **More contrast is yours once you map a role.** A `--clax-*` property you
  set wins under `prefers-contrast: more` too. If your design system has a
  more-contrast set, switch your tokens with the media query, as Clax's own
  CSS does.
