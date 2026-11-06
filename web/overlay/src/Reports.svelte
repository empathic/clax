<script lang="ts">
  // "Your reports": the reports this reporter may see, newest first, each
  // with its state and reply count; the open one shows as `ReportView`.
  // `reports` is null until the first page arrived. Which report is open is
  // the overlay's (`expanded`), so it can open the one waiting on the reporter.
  import type { Report } from "../../sdk/src/types";
  import { repliesOf, stateLabel, stateTone, summaryOf } from "./model";
  import ReportView from "./ReportView.svelte";

  type Props = {
    reports: Report[] | null;
    error: string | null;
    expanded: string | null;
    onExpand(id: string | null): void;
    onReply(id: string, body: string): Promise<void>;
    onVerify(id: string): Promise<void>;
    onStillBroken(r: Report): void;
    onClose(): void;
    /** Only these reports (this page's), with "Show all" back to every one. */
    only?: string[] | null;
    onShowAll?(): void;
    /** The Full view's clip, when the host can fetch one; absent leaves it unshown. */
    onClipUrl?(id: string): Promise<string>;
  };
  let { reports: all, error, expanded, only = null, onShowAll, onExpand, onReply, onVerify, onStillBroken, onClose, onClipUrl }: Props = $props();
  const reports = $derived(all && only ? all.filter(r => only.includes(r.id)) : all);

  // Grouped by where each report stands, the one waiting on the reporter first, and newest first within a group (the
  // queue view's states: Fix proposed; Open and Claimed; Verified and Resolved).
  const GROUPS: { key: string; label: string; states: string[] }[] = [
    { key: "waiting", label: "Fix ready for you to check", states: ["fix_proposed"] },
    { key: "open", label: "Open", states: ["open", "claimed"] },
    { key: "closed", label: "Closed", states: ["verified", "resolved"] },
  ];
  const newestFirst = (a: Report, b: Report) => (b.created_at ?? "").localeCompare(a.created_at ?? "");
  const grouped = $derived.by(() => {
    if (!reports) return [];
    const known = new Set(GROUPS.flatMap(g => g.states));
    const out = GROUPS.map(g => ({ ...g, items: reports.filter(r => g.states.includes(r.state)).sort(newestFirst) }));
    const other = reports.filter(r => !known.has(r.state)).sort(newestFirst);
    if (other.length) out.push({ key: "other", label: "Other", states: [], items: other });
    return out.filter(g => g.items.length > 0);
  });

  // Any number of reports open at once (the overlay's `expanded` opens one, such as the report waiting on you); a row
  // toggles its own.
  let opened = $state<Set<string>>(new Set());
  $effect(() => { if (expanded && !opened.has(expanded)) opened = new Set([...opened, expanded]); });
  function toggle(id: string): void {
    const next = new Set(opened);
    if (next.has(id)) next.delete(id); else { next.add(id); onExpand(id); }
    opened = next;
  }

  /** The full view: the panel fills the window as a reading page (each report
   * a heading, with what the panel leaves out); Escape goes back to the panel. */
  let full = $state(false);
  let fullBtn = $state<HTMLButtonElement | null>(null);
  function setFull(on: boolean): void {
    full = on;
    queueMicrotask(() => fullBtn?.focus());
  }
  // The full view is the page while it is open: the wheel scrolls it, never the host page behind it (which would
  // otherwise take every scroll the reports don't need). The host's own overflow comes back as it was.
  $effect(() => {
    if (!full) return;
    const root = (fullBtn?.ownerDocument ?? document).documentElement;
    const was = root.style.overflow;
    root.style.overflow = "hidden";
    return () => { root.style.overflow = was; };
  });
  /** Escape leaves the full view; F turns it on and off, but not while typing (a reply, or a field of the page).
   * Heard wherever focus is while the panel is open, ahead of the page's own keys (a host may use F itself). */
  function keys(e: KeyboardEvent): void {
    if (e.defaultPrevented || e.isComposing || e.metaKey || e.ctrlKey || e.altKey) return;
    if (full && e.key === "Escape") { e.preventDefault(); e.stopPropagation(); setFull(false); return; }
    const at = e.composedPath()[0];
    if (at instanceof HTMLElement && (at.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(at.tagName))) return;
    if (e.key.toLowerCase() === "f") { e.preventDefault(); e.stopPropagation(); setFull(!full); }
  }
  let panelEl = $state<HTMLElement | null>(null);
  $effect(() => {
    const win = panelEl?.ownerDocument.defaultView;
    if (!win) return;
    win.addEventListener("keydown", keys, true);
    return () => win.removeEventListener("keydown", keys, true);
  });
</script>

<section class="panel" class:full aria-label="Your reports" bind:this={panelEl}>
  <header>
    <h2>{only ? "Yours on this page" : "Your reports"}</h2>
    <span class="tools">
      {#if only && onShowAll}<button type="button" class="ghost small" onclick={onShowAll}>Show all</button>{/if}
      <button type="button" class="ghost small" aria-pressed={full} aria-keyshortcuts={full ? "F Escape" : "F"} bind:this={fullBtn} onclick={() => setFull(!full)}>{full ? "Back to panel" : "Full view"}<kbd aria-hidden="true">F</kbd></button>
      <button type="button" class="ghost" onclick={onClose} aria-label="Close your reports">✕</button>
    </span>
  </header>
  {#if error}<p class="error small" role="alert">{error}</p>{/if}
  {#if reports === null}
    {#if !error}<p class="muted small empty">Loading…</p>{/if}
  {:else if reports.length === 0}
    <p class="muted small empty">No reports yet. Press Report and pick what on the page your report is about.</p>
  {:else}
    {#each grouped as g (g.key)}
      <h3 class="group">{g.label} <span class="muted">{g.items.length}</span></h3>
      <ul>
        {#each g.items as r (r.id)}
          {@const replies = repliesOf(r)}
          <li class:open={opened.has(r.id)}>
            {#snippet row()}
              <button type="button" class="row" aria-expanded={opened.has(r.id)} onclick={() => toggle(r.id)}>
                <span class="sum">{summaryOf(r)}</span>
                <span class="meta small"><span class="state" data-state={r.state} data-tone={stateTone(r.state)}>{stateLabel(r)}</span>{replies.length === 1 ? "1 reply" : `${replies.length} replies`}</span>
              </button>
            {/snippet}
            {#if full}<h4>{@render row()}</h4>{:else}{@render row()}{/if}
            {#if opened.has(r.id)}
              <ReportView report={r} {full} {onReply} {onVerify} {onStillBroken} {onClipUrl} />
            {/if}
          </li>
        {/each}
      </ul>
    {/each}
  {/if}
</section>

<style>
  /* Placed by the overlay's dock, above its notice and launcher. */
  .panel { width: min(380px, calc(100vw - 32px)); max-height: 560px; overflow: auto; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
  header { position: sticky; top: 0; z-index: 1; display: flex; align-items: center; justify-content: space-between; padding: 6px 6px 6px 14px; background: var(--raised); border-bottom: 1px solid var(--border); }
  h2 { margin: 0; font: 600 15px/1.3 var(--font); }
  .tools { display: flex; gap: 4px; align-items: center; }
  h4 { margin: 0; font: inherit; }
  .group { margin: 12px 16px 4px; font-size: 12px; font-weight: 600; color: var(--muted); }
  .group .muted { font-weight: 400; }
  /* The key, beside the label and out of its accessible name (aria-keyshortcuts says it), as in the mode bar. */
  kbd { margin-left: 6px; font: 500 10px/1 var(--font); padding: 1px 3px; border: 1px solid currentColor; border-radius: var(--radius-xs); opacity: .7; }
  /* The full view: the whole window, one readable column. */
  .panel.full { position: fixed; inset: 0; width: auto; max-height: none; overflow-y: auto; overscroll-behavior: contain; border: 0; border-radius: 0; box-shadow: none; z-index: 10; font-size: 15px; }
  .panel.full > header, .panel.full > ul, .panel.full > p { max-width: 760px; margin-inline: auto; }
  .panel.full > header { padding: 12px 8px 12px 16px; }
  .panel.full h2 { font-size: 20px; }
  .panel.full .row { padding: 14px 16px; }
  .panel.full .sum { font-size: 16px; font-weight: 600; }
  .empty { padding: 16px 14px; }
  .error { padding: 6px 14px 0; color: var(--danger); }
  ul { list-style: none; margin: 0; padding: 0; }
  li { border-bottom: 1px solid var(--border); }
  li:last-child { border-bottom: 0; }
  .row { display: flex; flex-direction: column; align-items: stretch; gap: 2px; width: 100%; min-height: 0; padding: 10px 14px; border: 0; border-radius: 0; background: none; text-align: left; white-space: normal; }
  .sum { overflow-wrap: anywhere; }
  .meta { display: flex; gap: 8px; align-items: center; color: var(--muted); font-weight: 400; }
  .state { padding: 0 6px; border-radius: 999px; background: var(--hover); color: var(--fg); font-weight: 500; }
  .state[data-state="claimed"] { background: var(--accent-tint); color: var(--agent-ink); }
  .state[data-state="fix_proposed"] { background: var(--you); color: var(--on-you); }
  .state[data-state="verified"] { background: var(--accent-tint); color: var(--agent-ink); }
  .state[data-state="resolved"] { background: none; border: 1px solid var(--border-strong); color: var(--muted); }
</style>
