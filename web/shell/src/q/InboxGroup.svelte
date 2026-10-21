<svelte:options css="injected" />

<script lang="ts">
  // One page's group in the inbox (`inbox-groups.ts`): a disclosure
  // toggle named by the group's heading (the page's title, and how many
  // of its items are new: "Sales dashboard, 3 new"; its open state in
  // aria-expanded alone), the heading, the page's title (a link to it, while it
  // has one),
  // how many of its items are new ("3 new") and its newest item's age, and
  // **Mark read** while any is new; its body, hidden while folded, the rows
  // the section gives it and **Show more** while it has more. A click on the heading's bare space
  // toggles it too. Every string is text. Its styles travel with the
  // question module's lazy chunk (injected on mount).
  import type { Snippet } from "svelte";
  import { type Group, groupHref, groupTitle, shortAge } from "./inbox-groups";

  type Props = {
    group: Group;
    /** Its items counted new (unread and not shown as a card). */
    fresh: number;
    open: boolean;
    now?: Date;
    /** Its body's element ID. */
    id: string;
    onToggle(): void;
    /** Fetches more rows; absent when there are none. */
    onMore?: () => void;
    /** Marks the group's new items read; absent where it has none. */
    onRead?: () => void;
    children: Snippet;
  };
  const p: Props = $props();
  const title = $derived(groupTitle(p.group));
  const href = $derived(groupHref(p.group));
  // The heading's bare space toggles the group; its link and buttons keep their own clicks.
  const onHead = (e: MouseEvent) => { if (!(e.target as Element).closest("a, button")) p.onToggle(); };
</script>

<div class="igroup" data-group={p.group.key} data-open={p.open ? "" : undefined} data-fresh={p.fresh ? "" : undefined}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <!-- The toggle button is the keyboard's way; the bare space is a larger target for the pointer. -->
  <div class="ghead" onclick={onHead}>
    <button type="button" class="gtoggle" onclick={p.onToggle} aria-expanded={p.open} aria-controls={p.id} aria-labelledby={`${p.id}-t`}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 4l4 4-4 4" /></svg>
    </button>
    <h3 class="gtitle" id={`${p.id}-t`} aria-label={p.fresh ? `${title}, ${p.fresh.toLocaleString("en-US")} new` : undefined} data-other={p.group.key === "none" ? "" : undefined}>{#if href}<a {href}>{title}</a>{:else}{title}{/if}</h3>
    <span class="gmeta">{#if p.fresh}<span class="gnew" aria-hidden="true">{p.fresh.toLocaleString("en-US")} new</span><span aria-hidden="true">{" · "}</span>{/if}<time datetime={p.group.newestAt}>{shortAge(p.group.newestAt, p.now ?? new Date())}</time></span>
    {#if p.onRead}<button type="button" class="gread" onclick={p.onRead} aria-label={`Mark ${title} read`}>Mark read</button>{/if}
  </div>
  <div class="gbody" id={p.id} hidden={!p.open}>
    {@render p.children()}
    {#if p.onMore}<button type="button" class="gmore" onclick={p.onMore}>Show more</button>{/if}
  </div>
</div>

<style>
  :global {
    .igroup { min-width: 0; border-bottom: 1px solid var(--border); }
    .igroup .ghead { display: flex; align-items: center; gap: 6px; min-width: 0; min-height: 40px; padding: 4px 4px 4px 0; cursor: pointer; }
    .igroup .gtoggle { flex: none; width: 28px; min-height: 28px; padding: 0; justify-content: center; background: none; border-color: transparent; color: var(--muted); }
    .igroup .gtoggle:hover { background-color: var(--hover); border-color: transparent; }
    .igroup .gtoggle svg { width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; transition: transform var(--t); }
    .igroup[data-open] .gtoggle svg { transform: rotate(90deg); }
    .igroup .gtitle { flex: 0 1 auto; min-width: 0; margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font: 600 14px/1.4 var(--font); color: var(--fg); }
    .igroup .gtitle a { color: inherit; text-decoration: none; }
    .igroup .gtitle a:hover { text-decoration: underline; text-decoration-color: var(--border-strong); text-underline-offset: 2px; }
    .igroup .gtitle[data-other] { color: var(--muted); }
    .igroup:not([data-fresh]) .gtitle { font-weight: 500; color: var(--muted); }
    .igroup .gmeta { flex: 1 0 auto; color: var(--muted); font-size: 12.5px; white-space: nowrap; font-variant-numeric: tabular-nums; }
    .igroup .gnew { color: var(--agent-ink); font-weight: 600; }
    .igroup .gread { flex: none; min-height: 28px; padding: 2px 10px; font-size: 12.5px; color: var(--muted); background: none; border-color: transparent; }
    .igroup .gread:hover { color: var(--fg); background-color: var(--hover); border-color: transparent; }
    .igroup .gbody { padding: 0 0 8px 34px; min-width: 0; }
    .igroup .gbody .rows { margin: 0; padding: 0; }
    .igroup .gbody .irow:last-child { border-bottom: 0; }
    .igroup .gbody .dlist { margin: 4px 0 8px; }
    .igroup .gmore { margin-top: 6px; font-size: 12.5px; }
    @media (max-width: 700px) {
      .igroup .gbody { padding-left: 0; }
    }
    @media (pointer: coarse) { .igroup .gtoggle, .igroup .gread { min-height: 44px; } .igroup .gtoggle { width: 44px; } }
  }
</style>
