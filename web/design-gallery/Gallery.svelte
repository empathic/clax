<script lang="ts">
  // The design gallery: the generated tokens made visible. Each section is a
  // variant table of one primitive, drawn by a snippet per variant; the
  // contrast page reads design/tokens.json's pairs with the ratios the
  // checker computed (pairs.g.json), so it shows the proof rather than
  // redoing it. Light or dark is the page's data-theme (main.ts).
  import PAIRS from "./pairs.g.json";
  import { COLOURS } from "../shell/src/view/tags-model";
  import LiveWriters from "../shell/src/ui/LiveWriters.svelte";
  import OverlaySpecimen from "./OverlaySpecimen.svelte";
  import { WRITERS } from "./fixtures";

  type Theme = "light" | "dark";
  type Pair = { fg: string; bg: string; min: number; note: string; themes: Partial<Record<Theme, { fg: string; bg: string; page: string; ratio: number }>> };

  let { theme }: { theme: Theme | null } = $props();
  const shown: Theme = $derived(theme ?? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"));
  const pairs = $derived((PAIRS as Pair[]).filter(p => p.themes[shown]));

  const ROLES = ["bg", "card", "raised", "hover", "fg", "muted", "border", "border-hover", "border-strong", "agent", "agent-ink", "accent-hover", "on-accent", "accent-tint", "you", "you-ink", "on-you", "comment-hl", "selection-bg", "danger", "danger-tint", "primary-hover", "focus"];
  const OVER = ["pin", "on-pin", "pin-ring", "pin-addressed", "pin-addressed-ink", "pin-addressed-ring", "pin-onit", "on-pin-onit", "frame"];
  const BUTTONS = [
    { label: "Default", cls: "" },
    { label: "Primary", cls: "primary" },
    { label: "Ghost", cls: "ghost" },
  ];
</script>

{#snippet swatch(name: string)}
  <li class="sw-item"><span class="sw-chip" style:background={`var(--${name})`}></span><code>--{name}</code></li>
{/snippet}

{#snippet button(label: string, cls: string, disabled: boolean)}
  <button type="button" class={cls} {disabled}>{label}</button>
{/snippet}

{#snippet tag(c: string, pressed: boolean)}
  <button type="button" class="tag" data-c={c} aria-pressed={pressed}>{c}</button>
{/snippet}

{#snippet section(id: string, title: string, rule: string)}
  <h2 id={id}>{title}</h2>
  <p class="rule">{rule}</p>
{/snippet}

<main class="dg">
  <header class="dg-head">
    <h1>Clax design gallery</h1>
    <p class="muted">The tokens and primitives, from design/tokens.json, in the {shown} theme. <a href="?theme=light">Light</a> · <a href="?theme=dark">Dark</a></p>
  </header>

  {@render section("roles", "Colour roles", "Components read only these roles. The swatches follow the theme.")}
  <ul class="sw">{#each ROLES as r (r)}{@render swatch(r)}{/each}</ul>

  {@render section("over", "Over the artifact", "One value in every theme: these sit on the artifact, whose colours Clax does not set.")}
  <ul class="sw">{#each OVER as r (r)}{@render swatch(r)}{/each}</ul>

  {@render section("buttons", "Buttons", "Default, primary (solid ink) and ghost; each enabled and disabled.")}
  <table class="vt">
    <thead><tr><th scope="col">Variant</th><th scope="col">Enabled</th><th scope="col">Disabled</th></tr></thead>
    <tbody>
      {#each BUTTONS as b (b.label)}
        <tr><th scope="row">{b.label}</th><td>{@render button(b.label, b.cls, false)}</td><td>{@render button(b.label, b.cls, true)}</td></tr>
      {/each}
    </tbody>
  </table>

  {@render section("fields", "Fields", "A boundary that must be seen is --border-strong; focus is the agents' green.")}
  <div class="row">
    <label class="field">Name <input value="Mia" /></label>
    <label class="field">Theme <select><option>System</option><option>Light</option><option>Dark</option></select></label>
    <label class="field wide">Comment <textarea rows="2">The second paragraph should come first.</textarea></label>
  </div>

  {@render section("tags", "Tags", "A tint, its text and a border, by palette key. The name always shows; pressed is ringed and ticked.")}
  <table class="vt">
    <thead><tr><th scope="col">Key</th><th scope="col">Tag</th><th scope="col">Pressed</th></tr></thead>
    <tbody>
      {#each COLOURS as c (c)}
        <tr><th scope="row">{c}</th><td>{@render tag(c, false)}</td><td>{@render tag(c, true)}</td></tr>
      {/each}
    </tbody>
  </table>

  {@render section("marks", "Counts, keys and code", "The monospace is for what is literal.")}
  <div class="row">
    <span class="cnt">3</span>
    <span class="kc">C</span>
    <code>clax publish</code>
    <span class="chip rally">2 people here</span>
    <span class="error">Could not send.</span>
  </div>

  {@render section("pins", "Pins", "A thread's number over the artifact: open, addressed, and with the version it was addressed in.")}
  <div class="pin-stage">
    <span class="thread-pin" style="left: 16px; top: 14px;">1</span>
    <span class="thread-pin addressed" style="left: 64px; top: 14px;">2</span>
    <span class="thread-pin addressed" data-v="v3" style="left: 112px; top: 14px;">3</span>
  </div>

  {@render section("banners", "Banners and sheets", "Raised surfaces at elevation; a notice on the danger tint.")}
  <div class="stack">
    <div class="banner static"><span>A newer version is published.</span><button type="button" class="primary">Show it</button></div>
    <div class="banner notice static"><span>This page could not be loaded.</span><button type="button">Retry</button></div>
    <p class="gesture-hint static">Drag over the part you mean.</p>
    <div class="composer static">
      <p class="composer-quote">“the chart needs a legend”</p>
      <textarea rows="2" aria-label="Comment">Agreed.</textarea>
      <div class="actions"><button type="button" class="ghost">Cancel</button><button type="button" class="primary">Comment</button></div>
    </div>
  </div>

  {@render section("live", "People writing", "A live colour per person, always beside their name.")}
  <div class="live-box"><LiveWriters writers={WRITERS} /></div>

  {@render section("overlay", "The overlay, white label", "The reporter's mode bar and its reports panel in a shadow root on a host page that is not Clax's. The overlay follows the system's scheme, not this page's theme.")}
  <OverlaySpecimen />

  {@render section("contrast", "Contrast pairs", `Every pair design/tokens.json lists or implies, in the ${shown} theme, with the ratio scripts/sync-tokens.py computed (WCAG 2.x). ${pairs.length} pairs.`)}
  <table class="pairs">
    <thead><tr><th scope="col">Sample</th><th scope="col">Text on ground</th><th scope="col">Ratio</th><th scope="col">Minimum</th><th scope="col">For</th></tr></thead>
    <tbody>
      {#each pairs as p, i (i)}
        {@const t = p.themes[shown]!}
        <tr>
          <td><span class="sample" style:color={t.fg} style:background={t.bg}>Aa text</span></td>
          <td><code>--{p.fg}</code> on <code>--{p.bg}</code></td>
          <td class="num">{t.ratio.toFixed(2)}</td>
          <td class="num">{p.min}{t.ratio >= p.min ? " ✓" : " ✗"}</td>
          <td class="muted">{p.note}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</main>

<style>
  .dg { max-width: 1040px; margin: 0 auto; padding: 24px var(--gutter) 48px; }
  .dg-head h1 { margin: 0 0 4px; font-size: 22px; }
  .dg-head p { margin: 0 0 8px; }
  .dg-head a { text-decoration: underline; }
  h2 { margin: 32px 0 4px; font-size: 17px; padding-bottom: 6px; border-bottom: 1px solid var(--border); }
  .rule { margin: 6px 0 12px; font-size: 12.5px; color: var(--muted); }
  .sw { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(190px, 1fr)); gap: 8px; }
  .sw-item { display: flex; align-items: center; gap: 8px; font-size: 12px; }
  .sw-chip { width: 28px; height: 28px; flex: none; border-radius: var(--radius-xs); border: 1px solid var(--border-strong); }
  .vt, .pairs { border-collapse: collapse; font-size: 13px; }
  .vt th, .vt td, .pairs th, .pairs td { padding: 6px 12px 6px 0; text-align: left; vertical-align: middle; }
  .vt thead th, .pairs thead th { font-weight: 600; color: var(--muted); font-size: 12px; }
  .vt tbody th { font-weight: 500; }
  .pairs { width: 100%; }
  .pairs .num { font-variant-numeric: tabular-nums; white-space: nowrap; }
  .sample { display: inline-block; padding: 4px 10px; border: 1px solid var(--border); border-radius: var(--radius-xs); font-weight: 500; }
  .row { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; }
  .field { display: grid; gap: 4px; font-size: 12.5px; color: var(--muted); }
  .field.wide { flex: 1 1 280px; }
  .pin-stage { position: relative; height: 52px; background: var(--frame); border: 1px solid var(--border); border-radius: var(--radius-sm); }
  .stack { display: grid; gap: 12px; justify-items: start; }
  .static { position: static; transform: none; left: auto; right: auto; top: auto; bottom: auto; }
  .composer.static { width: min(360px, 100%); }
  .live-box { max-width: 392px; padding: 10px 14px; background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius); }
  @media (max-width: 700px) { .pairs td:last-child, .pairs th:last-child { display: none; } }
</style>
